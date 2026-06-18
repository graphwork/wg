//! Fuzzy / robust string matching for the `edit_file` tool.
//!
//! ## Why this exists
//!
//! A strict EXACT-MATCH `edit_file` is the single biggest cause of
//! full-file-rewrite thrash on local / weaker models: the model reads a
//! file, tries a targeted `old_string` edit, gets the indentation or a
//! trailing space slightly wrong, the edit fails with a bare "not found",
//! and the model gives up and rewrites the whole file with `write_file`
//! instead (re-introducing bugs, burning tokens and latency). This was
//! observed directly while watching qwen3 build a TUI — `main.rs` rewritten
//! 4× (2.3KB → 17KB → 19.5KB → …) rather than patched.
//!
//! ## What this does
//!
//! [`fuzzy_replace`] runs a strictness cascade, strict → loose, and accepts
//! the first level that yields exactly one match:
//!
//! 1. **Exact substring** — byte-for-byte (preserves within-line edits and
//!    the historical semantics).
//! 2. **Line-based, line-ending tolerant** — `\n` ≡ `\r\n`, and a final line
//!    with/without a trailing newline.
//! 3. **+ trailing-whitespace tolerant** — ignore trailing spaces/tabs per
//!    line.
//! 4. **+ indentation tolerant** — ignore *leading* whitespace per line; the
//!    replacement is re-indented to the file's actual indentation so the edit
//!    lands correctly formatted.
//! 5. **+ internal-whitespace collapse** — OPT-IN only (the caller's
//!    `normalize_whitespace` flag). Off by default because collapsing
//!    interior whitespace can match semantically different code.
//!
//! On no match at any level it returns a **near-miss diagnostic** that shows
//! the closest candidate block in the file side-by-side with the requested
//! `old_string`, so the model can correct the edit instead of falling back to
//! a rewrite.
//!
//! ## Design note — why fuzzy str-replace, not apply_patch / search-replace blocks
//!
//! See `docs/research/nex-fuzzy-edit-design.md` for the full rationale. In
//! short: `edit_file` is already the canonical, audited tool surface; making
//! its matching tolerant is strictly higher-leverage and lower-risk than
//! introducing a second diff/patch wire format (apply_patch V4A / Aider
//! search-replace), and the indentation re-anchoring here gives most of what
//! Aider's fuzzy-context blocks provide without a new tool.

/// Strictness levels, tried in order from strict to loose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchLevel {
    /// Byte-for-byte substring match.
    Exact,
    /// Line-based; `\n` ≡ `\r\n` and trailing-newline-at-EOF tolerant.
    LineEndings,
    /// Additionally ignore trailing whitespace per line.
    TrailingWs,
    /// Additionally ignore leading whitespace (indentation) per line.
    Indentation,
    /// Additionally collapse runs of interior whitespace (opt-in).
    Collapse,
}

impl MatchLevel {
    /// Human-readable label for diagnostics / success notes.
    pub fn label(self) -> &'static str {
        match self {
            MatchLevel::Exact => "exact",
            MatchLevel::LineEndings => "line-ending-insensitive",
            MatchLevel::TrailingWs => "trailing-whitespace-insensitive",
            MatchLevel::Indentation => "indentation-insensitive",
            MatchLevel::Collapse => "whitespace-insensitive",
        }
    }
}

/// Outcome of [`fuzzy_replace`].
#[derive(Debug)]
pub enum FuzzyOutcome {
    /// Exactly one match. `new_content` is the full updated file body.
    Unique {
        new_content: String,
        level: MatchLevel,
    },
    /// `old_string` matched in more than one place at the first level that
    /// matched at all. The caller should ask the model for more context.
    Ambiguous { count: usize, level: MatchLevel },
    /// No match at any enabled level. `diagnostic` is a ready-to-return,
    /// model-facing message including the closest candidate.
    NoMatch { diagnostic: String },
}

/// Replace `old` with `new` in `content` using the strictness cascade.
///
/// `allow_collapse` enables the most aggressive interior-whitespace-collapse
/// level (the caller's opt-in `normalize_whitespace` flag).
pub fn fuzzy_replace(content: &str, old: &str, new: &str, allow_collapse: bool) -> FuzzyOutcome {
    if old.is_empty() {
        return FuzzyOutcome::NoMatch {
            diagnostic: "old_string is empty — provide the text to replace.".to_string(),
        };
    }

    // ── Level 1: exact substring (fast path, within-line capable) ──────────
    let exact_count = content.matches(old).count();
    if exact_count == 1 {
        let start = content.find(old).expect("counted one match");
        let mut new_content = String::with_capacity(content.len() + new.len());
        new_content.push_str(&content[..start]);
        new_content.push_str(new);
        new_content.push_str(&content[start + old.len()..]);
        return FuzzyOutcome::Unique {
            new_content,
            level: MatchLevel::Exact,
        };
    }
    if exact_count > 1 {
        return FuzzyOutcome::Ambiguous {
            count: exact_count,
            level: MatchLevel::Exact,
        };
    }

    // ── Levels 2-5: line-based cascade ─────────────────────────────────────
    let file_lines = split_inclusive_lines(content);
    let old_lines = split_inclusive_lines(old);
    let n = old_lines.len();

    // A single within-line fragment that didn't exact-match cannot be matched
    // by whole-line comparison; fall straight to diagnostics.
    if n == 0 {
        return FuzzyOutcome::NoMatch {
            diagnostic: format!("old_string not found. {}", no_match_hint()),
        };
    }

    let file_bodies: Vec<&str> = file_lines.iter().map(|l| line_body(l)).collect();
    let old_bodies: Vec<&str> = old_lines.iter().map(|l| line_body(l)).collect();

    let mut levels = vec![
        MatchLevel::LineEndings,
        MatchLevel::TrailingWs,
        MatchLevel::Indentation,
    ];
    if allow_collapse {
        levels.push(MatchLevel::Collapse);
    }

    for level in levels {
        let starts = find_windows(&file_bodies, &old_bodies, level);
        match starts.len() {
            0 => continue,
            1 => {
                let new_content = build_replacement(
                    &file_lines,
                    &file_bodies,
                    &old_bodies,
                    starts[0],
                    n,
                    new,
                    level,
                );
                return FuzzyOutcome::Unique { new_content, level };
            }
            count => return FuzzyOutcome::Ambiguous { count, level },
        }
    }

    // ── No match anywhere: build a near-miss diagnostic ────────────────────
    FuzzyOutcome::NoMatch {
        diagnostic: near_miss_diagnostic(&file_bodies, &old_bodies),
    }
}

/// Split `s` into lines, each element INCLUDING its trailing terminator
/// (`\n` or `\r\n`). A final line without a terminator is included as-is.
fn split_inclusive_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split_inclusive('\n').collect()
}

/// The body of a line: the text with any trailing `\n` / `\r\n` removed.
fn line_body(line: &str) -> &str {
    let l = line.strip_suffix('\n').unwrap_or(line);
    l.strip_suffix('\r').unwrap_or(l)
}

/// Leading whitespace (spaces / tabs) prefix of a string.
fn leading_ws(s: &str) -> &str {
    let end = s.find(|c: char| c != ' ' && c != '\t').unwrap_or(s.len());
    &s[..end]
}

/// Normalize a line body for comparison at the given strictness level.
fn norm(body: &str, level: MatchLevel) -> String {
    match level {
        MatchLevel::Exact | MatchLevel::LineEndings => body.to_string(),
        MatchLevel::TrailingWs => body.trim_end().to_string(),
        MatchLevel::Indentation => body.trim().to_string(),
        MatchLevel::Collapse => collapse_ws(body.trim()),
    }
}

/// Collapse every run of interior whitespace to a single space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Find every start index in `file_bodies` where a window of `old_bodies.len()`
/// lines matches `old_bodies` under `level` normalization.
fn find_windows(file_bodies: &[&str], old_bodies: &[&str], level: MatchLevel) -> Vec<usize> {
    let n = old_bodies.len();
    let mut starts = Vec::new();
    if n == 0 || n > file_bodies.len() {
        return starts;
    }
    let old_norm: Vec<String> = old_bodies.iter().map(|b| norm(b, level)).collect();
    for start in 0..=(file_bodies.len() - n) {
        if (0..n).all(|k| norm(file_bodies[start + k], level) == old_norm[k]) {
            starts.push(start);
        }
    }
    starts
}

/// Build the full updated file content by replacing the matched line window
/// `[start, start+n)` with `new`, preserving the file's newline style and
/// re-indenting `new` to the file's actual indentation when the match ignored
/// leading whitespace.
fn build_replacement(
    file_lines: &[&str],
    file_bodies: &[&str],
    old_bodies: &[&str],
    start: usize,
    n: usize,
    new: &str,
    level: MatchLevel,
) -> String {
    let prefix: String = file_lines[..start].concat();
    let suffix: String = file_lines[start + n..].concat();

    // Newline style + whether the matched block ended with a terminator.
    let term = if file_lines[start].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let last_has_terminator = file_lines[start + n - 1].ends_with('\n');

    // Replacement lines (terminators stripped; rejoined with `term`).
    let mut new_bodies: Vec<String> = split_inclusive_lines(new)
        .iter()
        .map(|l| line_body(l).to_string())
        .collect();
    // `new` non-empty but with no interior newline still yields one body;
    // a truly empty `new` yields none → the block is deleted.
    if new.is_empty() {
        new_bodies.clear();
    }

    // Re-indent only when the match ignored leading whitespace.
    if level >= MatchLevel::Indentation && !new_bodies.is_empty() {
        let old_base = old_bodies
            .iter()
            .find(|b| !b.trim().is_empty())
            .map(|b| leading_ws(b))
            .unwrap_or("");
        let file_base = file_bodies[start..start + n]
            .iter()
            .find(|b| !b.trim().is_empty())
            .map(|b| leading_ws(b))
            .unwrap_or("");
        if old_base != file_base {
            for body in new_bodies.iter_mut() {
                if body.trim().is_empty() {
                    continue; // leave blank lines untouched
                }
                if !old_base.is_empty() && body.starts_with(old_base) {
                    *body = format!("{}{}", file_base, &body[old_base.len()..]);
                } else if old_base.is_empty() {
                    *body = format!("{}{}", file_base, body);
                }
            }
        }
    }

    let mut middle = new_bodies.join(term);
    if last_has_terminator && !new_bodies.is_empty() {
        middle.push_str(term);
    }

    let mut out = String::with_capacity(prefix.len() + middle.len() + suffix.len());
    out.push_str(&prefix);
    out.push_str(&middle);
    out.push_str(&suffix);
    out
}

/// Generic hint appended to no-match diagnostics.
fn no_match_hint() -> &'static str {
    "Re-read the file and copy the target text exactly (whitespace and \
     indentation are matched leniently, so small formatting differences are \
     fine), then retry edit_file. Do NOT rewrite the whole file with \
     write_file — make a targeted edit."
}

/// Build a near-miss diagnostic: find the closest line window and show it
/// side-by-side with the requested `old_string`.
fn near_miss_diagnostic(file_bodies: &[&str], old_bodies: &[&str]) -> String {
    let n = old_bodies.len();
    let mut msg = String::new();
    msg.push_str(
        "old_string not found (tried exact, line-ending, trailing-whitespace, and \
                  indentation-insensitive matching).\n",
    );

    if let Some((start, matched)) = best_window(file_bodies, old_bodies) {
        let span_end = (start + n).min(file_bodies.len());
        msg.push_str(&format!(
            "Closest candidate at lines {}-{} ({}/{} lines align). Compare:\n\n",
            start + 1,
            span_end,
            matched,
            n
        ));
        // Cap the rendered block so huge edits don't flood the model.
        let cap = 30usize;
        let shown = n.min(cap);
        for k in 0..shown {
            let want = old_bodies[k];
            let got = file_bodies.get(start + k).copied().unwrap_or("");
            let mark = if want.trim() == got.trim() {
                " "
            } else {
                "✗"
            };
            msg.push_str(&format!(
                "  {} your old_string: {}\n",
                mark,
                truncate_line(want)
            ));
            msg.push_str(&format!(
                "  {} file (line {:>4}): {}\n",
                mark,
                start + k + 1,
                truncate_line(got)
            ));
        }
        if n > cap {
            msg.push_str(&format!("  … ({} more lines omitted)\n", n - cap));
        }
        msg.push('\n');
    } else {
        msg.push_str("No similar block was found in the file.\n\n");
    }

    msg.push_str(no_match_hint());
    msg
}

/// Truncate a single line for diagnostic display.
fn truncate_line(s: &str) -> String {
    const MAX: usize = 120;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(MAX);
        format!("{}…", &s[..end])
    }
}

/// Find the window (of length = old_bodies.len()) in the file with the most
/// trim-equal lines. Returns (start_index, matched_line_count). Ties broken by
/// the earliest position. Returns None if the file has no lines.
fn best_window(file_bodies: &[&str], old_bodies: &[&str]) -> Option<(usize, usize)> {
    let n = old_bodies.len();
    if file_bodies.is_empty() || n == 0 {
        return None;
    }
    // If old is longer than the file, anchor at 0 and score what overlaps.
    let last_start = file_bodies.len().saturating_sub(n);
    let mut best: Option<(usize, usize, usize)> = None; // (start, matched, prefix_score)
    for start in 0..=last_start {
        let mut matched = 0usize;
        let mut prefix_score = 0usize;
        for k in 0..n {
            let want = old_bodies[k].trim();
            let got = file_bodies.get(start + k).map(|s| s.trim()).unwrap_or("");
            if want == got {
                matched += 1;
                prefix_score += want.len();
            } else {
                prefix_score += common_prefix_len(want, got);
            }
        }
        let better = match best {
            None => true,
            Some((_, bm, bp)) => matched > bm || (matched == bm && prefix_score > bp),
        };
        if better {
            best = Some((start, matched, prefix_score));
        }
    }
    best.map(|(s, m, _)| (s, m))
}

/// Length (in bytes) of the common leading prefix of two strings.
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(content: &str, old: &str, new: &str) -> (String, MatchLevel) {
        match fuzzy_replace(content, old, new, false) {
            FuzzyOutcome::Unique { new_content, level } => (new_content, level),
            other => panic!("expected unique match, got {:?}", other),
        }
    }

    // ── exact ──────────────────────────────────────────────────────────────

    #[test]
    fn exact_substring_within_line() {
        let (out, level) = unique("let x = 1;", "x = 1", "y = 2");
        assert_eq!(out, "let y = 2;");
        assert_eq!(level, MatchLevel::Exact);
    }

    #[test]
    fn exact_multiline_block() {
        let (out, _) = unique("a\nb\nc\n", "a\nb", "A\nB");
        assert_eq!(out, "A\nB\nc\n");
    }

    // ── trailing whitespace ────────────────────────────────────────────────

    #[test]
    fn tolerates_trailing_whitespace_in_file() {
        // File line has trailing spaces; old_string does not.
        let (out, level) = unique("code   \nnext", "code\nnext", "CODE\nNEXT");
        assert_eq!(out, "CODE\nNEXT");
        assert_eq!(level, MatchLevel::TrailingWs);
    }

    #[test]
    fn tolerates_trailing_whitespace_in_old_string() {
        // old_string has a trailing space the file lacks.
        let (out, _) = unique("hello world", "hello world ", "hi there");
        assert_eq!(out, "hi there");
    }

    // ── line endings ───────────────────────────────────────────────────────

    #[test]
    fn tolerates_crlf_vs_lf() {
        let (out, level) = unique("line1\r\nline2\r\nline3\r\n", "line1\nline2", "A\nB");
        assert_eq!(out, "A\r\nB\r\nline3\r\n");
        assert_eq!(level, MatchLevel::LineEndings);
    }

    #[test]
    fn tolerates_missing_trailing_newline() {
        let (out, _) = unique("line1\n", "line1", "LINE1");
        assert_eq!(out, "LINE1\n");
    }

    // ── indentation ────────────────────────────────────────────────────────

    #[test]
    fn tolerates_indentation_and_reindents_replacement() {
        // File indents the block with 2 spaces; the model supplied it with 4.
        // (Fewer-space indentation would be an exact substring of more, so the
        // file must be LESS indented than old_string to actually exercise the
        // indentation level rather than the exact fast path.)
        let content = "fn f() {\n  let x = 1;\n}\n";
        let (out, level) = unique(content, "    let x = 1;", "    let x = 42;");
        // Replacement re-indented to the file's 2-space indentation.
        assert_eq!(out, "fn f() {\n  let x = 42;\n}\n");
        assert_eq!(level, MatchLevel::Indentation);
    }

    #[test]
    fn tolerates_tab_vs_space_indentation() {
        let content = "function() {\n\tindent\n}";
        let (out, level) = unique(content, "    indent", "        NEW");
        assert!(out.contains("NEW"), "got: {:?}", out);
        assert_eq!(level, MatchLevel::Indentation);
    }

    #[test]
    fn reindents_multiline_block_preserving_relative_indent() {
        let content = "class C:\n    def m(self):\n        pass\n";
        // Model supplies the body at top level (no indent); file has 8 spaces.
        let (out, _) = unique(content, "        pass", "        x = 1\n        return x");
        assert_eq!(
            out,
            "class C:\n    def m(self):\n        x = 1\n        return x\n"
        );
    }

    // ── collapse is opt-in ─────────────────────────────────────────────────

    #[test]
    fn interior_whitespace_not_collapsed_by_default() {
        // "hello world" (one space) must NOT match "hello   world" (three).
        match fuzzy_replace("hello   world", "hello world", "x", false) {
            FuzzyOutcome::NoMatch { .. } => {}
            other => panic!("expected NoMatch without collapse, got {:?}", other),
        }
    }

    #[test]
    fn interior_whitespace_collapsed_when_opted_in() {
        match fuzzy_replace("hello   world", "hello world", "hi", true) {
            FuzzyOutcome::Unique { new_content, level } => {
                assert_eq!(new_content, "hi");
                assert_eq!(level, MatchLevel::Collapse);
            }
            other => panic!("expected collapse match, got {:?}", other),
        }
    }

    // ── ambiguity ──────────────────────────────────────────────────────────

    #[test]
    fn exact_multiple_matches_is_ambiguous() {
        match fuzzy_replace("foo foo foo", "foo", "bar", false) {
            FuzzyOutcome::Ambiguous { count, level } => {
                assert_eq!(count, 3);
                assert_eq!(level, MatchLevel::Exact);
            }
            other => panic!("expected ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn fuzzy_multiple_matches_is_ambiguous() {
        // old_string "  foo" is not an exact substring (file uses a tab and a
        // single space), but two lines match under indentation tolerance.
        let content = "\tfoo\nbar\n foo\n";
        match fuzzy_replace(content, "  foo", "FOO", false) {
            FuzzyOutcome::Ambiguous { count, level } => {
                assert_eq!(count, 2);
                assert_eq!(level, MatchLevel::Indentation);
            }
            other => panic!("expected ambiguous fuzzy, got {:?}", other),
        }
    }

    // ── near-miss diagnostics ──────────────────────────────────────────────

    #[test]
    fn no_match_returns_near_miss_with_candidate() {
        let content = "fn main() {\n    let total = compute();\n    println!(\"{total}\");\n}\n";
        let diag = match fuzzy_replace(
            content,
            "let totals = compute();",
            "let total = compute();",
            false,
        ) {
            FuzzyOutcome::NoMatch { diagnostic } => diagnostic,
            other => panic!("expected NoMatch, got {:?}", other),
        };
        assert!(diag.contains("not found"), "diag: {}", diag);
        assert!(diag.contains("Closest candidate"), "diag: {}", diag);
        // Points at the real line and shows the file's actual text.
        assert!(diag.contains("compute()"), "diag: {}", diag);
        // Steers away from full rewrites.
        assert!(diag.contains("write_file"), "diag: {}", diag);
    }

    #[test]
    fn no_match_empty_old_string() {
        match fuzzy_replace("anything", "", "x", false) {
            FuzzyOutcome::NoMatch { diagnostic } => assert!(diagnostic.contains("empty")),
            other => panic!("expected NoMatch, got {:?}", other),
        }
    }

    // ── deletion ───────────────────────────────────────────────────────────

    #[test]
    fn empty_replacement_deletes_matched_block() {
        // Indentation mismatch forces the line-based path, which deletes the
        // whole matched line (terminator included) rather than leaving a blank.
        let (out, level) = unique("a\n  remove me\nb\n", "    remove me", "");
        assert_eq!(out, "a\nb\n");
        assert_eq!(level, MatchLevel::Indentation);
    }

    #[test]
    fn exact_empty_replacement_deletes_substring_only() {
        // When old_string is an exact substring, only that substring is
        // removed (historical behavior) — include the newline for a full line.
        let (out, level) = unique("a\nremove me\nb\n", "remove me", "");
        assert_eq!(out, "a\n\nb\n");
        assert_eq!(level, MatchLevel::Exact);
    }

    // ── unicode safety ─────────────────────────────────────────────────────

    #[test]
    fn unicode_lines_match_and_truncation_is_char_safe() {
        let (out, _) = unique("héllo\nwörld\n", "wörld", "wörld!");
        assert_eq!(out, "héllo\nwörld!\n");
    }
}
