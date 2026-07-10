//! Deliverable preflight parsing (guardrail G1).
//!
//! Parses a task's `## Deliverables` markdown block (with a `## Validation`
//! fallback for path-like lines) into a list of structured deliverables that
//! `wg done` checks before promoting a task to Done.
//!
//! The grammar is intentionally strict so research/review tasks (whose
//! `## Validation` is a rubric, not a file list) parse to an empty list and
//! are unaffected:
//!
//! - A `## Deliverables` header followed by a bullet list. Each bullet is
//!   either a filesystem path or a `registry:<file>:<id>` token.
//! - When no `## Deliverables` block is present, fall back to path-like
//!   bullet lines found under `## Validation`.
//!
//! This module is shared with the retry-mutation path (G3,
//! `build_previous_attempt_context`) so the directive block can reuse the
//! exact same parsed deliverable list — no duplication.

use std::path::Path;

/// A single parsed deliverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deliverable {
    /// A filesystem path (relative to the project root) that must exist and
    /// be non-empty.
    Path(String),
    /// A `registry:<file>:<id>` deliverable: `<id>` must appear in `<file>`.
    Registry { registry: String, id: String },
}

impl Deliverable {
    /// Render the deliverable back to its source form (for messages / G3
    /// directive blocks).
    pub fn as_source(&self) -> String {
        match self {
            Deliverable::Path(p) => p.clone(),
            Deliverable::Registry { registry, id } => {
                format!("registry:{}:{}", registry, id)
            }
        }
    }
}

/// Parse deliverables from a task's markdown body.
///
/// `description` is the task description (and any other markdown body that
/// may carry a `## Deliverables` / `## Validation` section). When a
/// `## Deliverables` block exists it is the single source of truth; otherwise
/// path-like bullet lines under `## Validation` are used as a fallback.
///
/// Returns an empty vec for tasks with no parseable deliverables (the
/// research/review no-regression path).
pub fn parse_deliverables(description: &str) -> Vec<Deliverable> {
    if let Some(block) = extract_section(description, "Deliverables") {
        let parsed = parse_bullets(&block);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    if let Some(block) = extract_section(description, "Validation") {
        let parsed = parse_bullets(&block);
        // The `## Validation` section is a rubric, not an explicit contract.
        // When the task's real work happens in a *different*, externally
        // managed worktree/repo (e.g. the description points at
        // `/tmp/wg-fix-lint-49` or another project checkout), a bare filename
        // mentioned in the rubric cannot be reliably resolved against *this*
        // task's own worktree — checking it there is a false positive. Drop
        // bare-filename fallbacks in that case; directory-qualified paths and
        // any explicit `## Deliverables` block are still honored.
        if references_external_worktree(description) {
            return parsed
                .into_iter()
                .filter(|d| !is_bare_filename(d))
                .collect();
        }
        return parsed;
    }
    Vec::new()
}

/// Negative-framing markers: when a bullet instructs that a file be
/// discarded, not committed, or is explicitly *not* a produced output, it is
/// not a required deliverable. Matched case-insensitively as substrings of the
/// bullet text.
const NEGATIVE_FRAMING: &[&str] = &[
    "discard",
    "do not commit",
    "don't commit",
    "do not create",
    "don't create",
    "do not stage",
    "do not add",
    "do not produce",
    "not a real change",
    "not a deliverable",
    "should not exist",
    "must not exist",
    "delete this",
    "remove this",
];

/// True when a bullet's text frames the mentioned file negatively (discard /
/// do-not-commit / not-a-real-change). Such bullets are skipped by
/// `parse_bullets` so a "discard foo.md" instruction is never mistaken for a
/// "produce foo.md" deliverable.
fn has_negative_framing(item: &str) -> bool {
    let lower = item.to_ascii_lowercase();
    NEGATIVE_FRAMING.iter().any(|m| lower.contains(m))
}

/// Strong signals that a task's real work lives in a *different*,
/// externally-managed worktree or repo checkout, so bare filenames in its
/// `## Validation` rubric should not be resolved against this task's own
/// worktree. Kept conservative to avoid dropping genuine local deliverables.
fn references_external_worktree(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    // WG scratch worktrees live under `/tmp/wg-...`; the other phrases are how
    // task authors describe an out-of-tree checkout.
    lower.contains("/tmp/wg-")
        || lower.contains("external worktree")
        || lower.contains("externally-managed")
        || lower.contains("externally managed")
        || lower.contains("different repo")
        || lower.contains("separate repo")
        || lower.contains("another repo")
}

/// True for a `Deliverable::Path` that is a bare filename (no directory
/// separator) — the ambiguous case when the referenced repo is external.
fn is_bare_filename(d: &Deliverable) -> bool {
    match d {
        Deliverable::Path(p) => !p.contains('/'),
        Deliverable::Registry { .. } => false,
    }
}

/// Extract the body of a `## <heading>` section (until the next `## ` header
/// or end of text). Heading match is case-insensitive and tolerates trailing
/// punctuation/whitespace.
fn extract_section(text: &str, heading: &str) -> Option<String> {
    let mut lines = text.lines();
    let mut in_section = false;
    let mut out = String::new();
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") {
            let h = trimmed.trim_start_matches('#').trim();
            // Strip trailing ':' or whitespace for tolerance.
            let h = h.trim_end_matches(':').trim();
            if in_section {
                // Next header — section over.
                break;
            }
            if h.eq_ignore_ascii_case(heading) {
                in_section = true;
                continue;
            }
        } else if in_section {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_section { Some(out) } else { None }
}

/// Parse bullet items (`- ` / `* ` / `+ `) in a section body into
/// deliverables. A bullet is a deliverable when it is a path-like token or a
/// `registry:<file>:<id>` token; prose bullets are ignored.
fn parse_bullets(section: &str) -> Vec<Deliverable> {
    let mut out = Vec::new();
    for raw in section.lines() {
        let trimmed = raw.trim();
        let Some(item) = strip_bullet(trimmed) else {
            continue;
        };
        // Negative framing: a bullet that says to discard / not commit / not
        // produce the mentioned file is NOT a required deliverable. Skip it so
        // "discard foo.md — do not commit it" is never treated as a required
        // output that must exist in the worktree.
        if has_negative_framing(item) {
            continue;
        }
        // Take the first whitespace-delimited token (the path / registry
        // token). Anything after is prose and ignored.
        let token = item.split_whitespace().next().unwrap_or("");
        if token.is_empty() {
            continue;
        }
        // Strip surrounding backticks if the author wrapped the path.
        let token = token.trim_matches('`');
        if let Some(d) = parse_registry(token) {
            out.push(d);
        } else if is_path_like(token) {
            out.push(Deliverable::Path(token.to_string()));
        }
        // else: prose bullet — ignore (strict grammar).
    }
    out
}

fn strip_bullet(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ") {
        Some(rest)
    } else if let Some(rest) = line.strip_prefix("* ") {
        Some(rest)
    } else if let Some(rest) = line.strip_prefix("+ ") {
        Some(rest)
    } else {
        None
    }
}

/// A `registry:<file>:<id>` token. `<file>` and `<id>` must be non-empty and
/// contain no whitespace or `:`.
fn parse_registry(token: &str) -> Option<Deliverable> {
    let rest = token.strip_prefix("registry:")?;
    let mut parts = rest.splitn(2, ':');
    let registry = parts.next()?.trim();
    let id = parts.next()?.trim();
    if registry.is_empty()
        || id.is_empty()
        || registry.contains(char::is_whitespace)
        || id.contains(char::is_whitespace)
        || registry.contains(':')
        || id.contains(':')
    {
        return None;
    }
    Some(Deliverable::Registry {
        registry: registry.to_string(),
        id: id.to_string(),
    })
}

/// A path-like token: non-empty, no whitespace, and contains either a `/`
/// (directory separator) or a `.` (file extension). This deliberately
/// excludes bare words like `latest` or `manifest` (no extension) and
/// slash-separated prose verbs like `Add/adjust` so review rubric bullets
/// don't get mis-parsed as paths.
fn is_path_like(token: &str) -> bool {
    !token.is_empty()
        && !token.contains(char::is_whitespace)
        && (token.contains('.') || (token.contains('/') && is_lowercase_path_token(token)))
        && !token.starts_with("registry:")
}

fn is_lowercase_path_token(token: &str) -> bool {
    !token.chars().any(|c| c.is_ascii_uppercase())
}

/// Outcome of checking a parsed deliverable list against the filesystem.
#[derive(Debug, Clone, Default)]
pub struct PreflightReport {
    /// Deliverables that are missing (absent/empty file, or absent registry
    /// id).
    pub missing: Vec<Deliverable>,
}

impl PreflightReport {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
    }

    /// A human-readable summary of the missing deliverables.
    pub fn missing_summary(&self) -> String {
        self.missing
            .iter()
            .map(|d| format!("- {}", d.as_source()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Run the preflight: check every deliverable against `project_root`.
///
/// - `Deliverable::Path(p)`: `project_root.join(p)` must exist and be
///   non-empty (a file with size > 0, or a non-empty directory).
/// - `Deliverable::Registry { registry, id }`: `project_root.join(registry)`
///   must be a readable text file containing `id`.
pub fn preflight(deliverables: &[Deliverable], project_root: &Path) -> PreflightReport {
    let mut missing = Vec::new();
    for d in deliverables {
        if !is_satisfied(d, project_root) {
            missing.push(d.clone());
        }
    }
    PreflightReport { missing }
}

fn is_satisfied(d: &Deliverable, project_root: &Path) -> bool {
    match d {
        Deliverable::Path(p) => {
            let path = project_root.join(p);
            // Existence + non-empty. A symlink is followed via metadata.
            match std::fs::metadata(&path) {
                Ok(md) => {
                    if md.is_file() {
                        md.len() > 0
                    } else if md.is_dir() {
                        // Non-empty directory: at least one entry.
                        std::fs::read_dir(&path)
                            .map(|mut it| it.next().is_some())
                            .unwrap_or(false)
                    } else {
                        false
                    }
                }
                Err(_) => false,
            }
        }
        Deliverable::Registry { registry, id } => {
            let path = project_root.join(registry);
            match std::fs::read_to_string(&path) {
                Ok(content) => content.contains(id),
                Err(_) => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parses_deliverables_block_paths_and_registry() {
        let desc = "## Description\nDo the thing.\n\n## Deliverables\n- latest.pt\n- seed/manifest.json\n- registry:registry.json:e97\n\n## Validation\n- rubric\n";
        let parsed = parse_deliverables(desc);
        assert_eq!(
            parsed,
            vec![
                Deliverable::Path("latest.pt".to_string()),
                Deliverable::Path("seed/manifest.json".to_string()),
                Deliverable::Registry {
                    registry: "registry.json".to_string(),
                    id: "e97".to_string(),
                },
            ]
        );
    }

    #[test]
    fn deliverables_block_wins_over_validation_fallback() {
        let desc = "## Deliverables\n- out.bin\n\n## Validation\n- score >= 0.7\n";
        let parsed = parse_deliverables(desc);
        assert_eq!(parsed, vec![Deliverable::Path("out.bin".to_string())]);
    }

    #[test]
    fn validation_fallback_picks_path_like_bullets_only() {
        // A rubric line ("score >= 0.7") and a path-like line.
        let desc = "## Validation\n- cargo test passes\n- produces latest.pt\n- score >= 0.7\n";
        let parsed = parse_deliverables(desc);
        // "produces latest.pt" — first token "produces" is not path-like, so
        // it's ignored; "latest.pt" is path-like but is the SECOND token, so
        // it is NOT picked up (strict: first token only). This documents the
        // strict grammar: authors must put the path first.
        assert!(parsed.is_empty(), "got {:?}", parsed);
    }

    #[test]
    fn validation_fallback_picks_clean_path_bullets() {
        let desc = "## Validation\n- latest.pt exists and is non-empty\n- seed/manifest.json lists the seed\n";
        let parsed = parse_deliverables(desc);
        assert_eq!(
            parsed,
            vec![
                Deliverable::Path("latest.pt".to_string()),
                Deliverable::Path("seed/manifest.json".to_string()),
            ]
        );
    }

    #[test]
    fn validation_fallback_ignores_slash_separated_prose_verbs() {
        let desc = "## Validation\n- Add/adjust Rust tests proving default GC skips dirty worktrees.\n- Add/adjust a smoke scenario for clean and dirty worktrees.\n";
        assert!(parse_deliverables(desc).is_empty());
    }

    #[test]
    fn no_deliverables_section_returns_empty() {
        let desc = "## Description\nJust research.\n\n## Validation\n- write a report\n";
        assert!(parse_deliverables(desc).is_empty());
    }

    #[test]
    fn backtick_wrapped_paths_parse() {
        let desc = "## Deliverables\n- `latest.pt`\n- `seed/manifest.json`\n";
        let parsed = parse_deliverables(desc);
        assert_eq!(
            parsed,
            vec![
                Deliverable::Path("latest.pt".to_string()),
                Deliverable::Path("seed/manifest.json".to_string()),
            ]
        );
    }

    #[test]
    fn registry_token_rejects_malformed() {
        assert!(parse_registry("registry:onlyone").is_none());
        assert!(parse_registry("registry::id").is_none()); // empty file
        assert!(parse_registry("registry:file:").is_none()); // empty id
        assert!(parse_registry("registry:file:with:colons").is_none());
        assert!(parse_registry("notregistry:file:id").is_none());
    }

    #[test]
    fn is_path_like_excludes_bare_words() {
        assert!(is_path_like("latest.pt"));
        assert!(is_path_like("seed/manifest.json"));
        assert!(!is_path_like("latest"));
        assert!(!is_path_like("a report"));
        assert!(!is_path_like("registry:x:y"));
    }

    #[test]
    fn preflight_passes_when_files_present() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("latest.pt"), b"checkpoint bytes").unwrap();
        fs::create_dir_all(root.join("seed")).unwrap();
        fs::write(root.join("seed/manifest.json"), b"{}").unwrap();
        fs::write(root.join("registry.json"), b"{\"e97\": true}").unwrap();

        let deliverables = vec![
            Deliverable::Path("latest.pt".to_string()),
            Deliverable::Path("seed/manifest.json".to_string()),
            Deliverable::Registry {
                registry: "registry.json".to_string(),
                id: "e97".to_string(),
            },
        ];
        let report = preflight(&deliverables, root);
        assert!(report.is_clean(), "missing: {:?}", report.missing);
    }

    #[test]
    fn preflight_reports_missing_and_empty_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // latest.pt absent; manifest.json present but empty; registry id absent.
        fs::create_dir_all(root.join("seed")).unwrap();
        fs::write(root.join("seed/manifest.json"), b"").unwrap();
        fs::write(root.join("registry.json"), b"{\"other\": true}").unwrap();

        let deliverables = vec![
            Deliverable::Path("latest.pt".to_string()),
            Deliverable::Path("seed/manifest.json".to_string()),
            Deliverable::Registry {
                registry: "registry.json".to_string(),
                id: "e97".to_string(),
            },
        ];
        let report = preflight(&deliverables, root);
        assert_eq!(report.missing.len(), 3);
        assert!(report.missing_summary().contains("latest.pt"));
        assert!(report.missing_summary().contains("seed/manifest.json"));
        assert!(
            report
                .missing_summary()
                .contains("registry:registry.json:e97")
        );
    }

    #[test]
    fn preflight_empty_deliverable_list_is_clean() {
        let dir = tempdir().unwrap();
        let report = preflight(&[], dir.path());
        assert!(report.is_clean());
    }

    #[test]
    fn discard_framing_bullet_is_not_a_deliverable() {
        // The finish-lint-pr49 regression: a bullet that instructs the file be
        // discarded must NOT be treated as a required deliverable.
        let desc = "## Validation\n- Discard PROMPT_CONSTRUCTION_ANALYSIS.md — it is a known macOS case-collision artifact, do not commit it.\n";
        assert!(
            parse_deliverables(desc).is_empty(),
            "got {:?}",
            parse_deliverables(desc)
        );
    }

    #[test]
    fn negative_framing_variants_all_skipped() {
        for line in [
            "- foo.md do not commit it",
            "- foo.md — discard this artifact",
            "- foo.md should not exist after the fix",
            "- foo.md is not a real change",
            "- foo.md do not create",
        ] {
            let desc = format!("## Deliverables\n{}\n", line);
            assert!(
                parse_deliverables(&desc).is_empty(),
                "expected no deliverable for {line:?}, got {:?}",
                parse_deliverables(&desc)
            );
        }
    }

    #[test]
    fn positive_deliverable_still_parsed_without_negative_framing() {
        // Guard against over-eager filtering: a normal produce-this bullet is
        // still a deliverable.
        let desc = "## Deliverables\n- out.bin produced by the run\n";
        assert_eq!(
            parse_deliverables(desc),
            vec![Deliverable::Path("out.bin".to_string())]
        );
    }

    #[test]
    fn external_worktree_drops_bare_filename_validation_fallback() {
        // When the description references an external worktree, a bare filename
        // in the ## Validation rubric cannot be resolved against this worktree.
        let desc = "## Description\nReal work happens in /tmp/wg-fix-lint-49.\n\n## Validation\n- report.md summarizes the run\n";
        assert!(
            parse_deliverables(desc).is_empty(),
            "got {:?}",
            parse_deliverables(desc)
        );
    }

    #[test]
    fn external_worktree_keeps_directory_qualified_paths() {
        // A path with a directory component is specific enough to keep even
        // when an external worktree is referenced.
        let desc = "## Description\nSee the different repo at /Users/x/other.\n\n## Validation\n- seed/manifest.json lists the seed\n";
        assert_eq!(
            parse_deliverables(desc),
            vec![Deliverable::Path("seed/manifest.json".to_string())]
        );
    }

    #[test]
    fn explicit_deliverables_block_survives_external_worktree_reference() {
        // The external-worktree relaxation only applies to the ## Validation
        // fallback. An explicit ## Deliverables contract is always honored.
        let desc = "## Description\nWork in /tmp/wg-foo.\n\n## Deliverables\n- out.bin\n";
        assert_eq!(
            parse_deliverables(desc),
            vec![Deliverable::Path("out.bin".to_string())]
        );
    }

    #[test]
    fn local_bare_filename_still_a_deliverable_without_external_reference() {
        // No external-worktree signal → bare filenames in ## Validation are
        // still deliverables (no regression).
        let desc = "## Validation\n- latest.pt exists and is non-empty\n";
        assert_eq!(
            parse_deliverables(desc),
            vec![Deliverable::Path("latest.pt".to_string())]
        );
    }

    #[test]
    fn as_source_roundtrips() {
        assert_eq!(
            Deliverable::Path("latest.pt".to_string()).as_source(),
            "latest.pt"
        );
        assert_eq!(
            Deliverable::Registry {
                registry: "registry.json".to_string(),
                id: "e97".to_string(),
            }
            .as_source(),
            "registry:registry.json:e97"
        );
    }
}
