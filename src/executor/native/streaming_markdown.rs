//! Progressive (streaming) markdown rendering for the nex CLI.
//!
//! The model streams its answer as a sequence of content deltas. Plain
//! text can be echoed byte-for-byte as it arrives, but *styled* markdown
//! needs lookahead: you cannot bold `**France**` until the closing `**`
//! has been seen, and a fenced code block is not a code block until its
//! closing ```` ``` ```` lands. Buffering the whole turn and rendering at
//! the end would lose the live streaming feel.
//!
//! This module renders markdown **progressively** by splitting the stream
//! into two regions:
//!
//!   * **Committed blocks** — markdown blocks (paragraphs, headings,
//!     lists, fenced code, …) that are definitely complete because a
//!     blank line terminated them at top level. A committed block is
//!     rendered once and printed permanently; it never moves or redraws,
//!     so there is zero flicker on text the user has already read.
//!   * **The live tail** — the in-progress block after the last blank
//!     line. It is re-rendered and redrawn in place on every delta, so
//!     the current paragraph/list/heading updates token-by-token exactly
//!     like plain streaming text. Partial markdown (an unclosed
//!     `**bold`, a half-open code fence, an in-progress list) renders
//!     best-effort and is corrected the instant the closing token
//!     arrives.
//!
//! [`BlockSplitter`] is the pure, fence-aware state machine that performs
//! the split; it is lossless and *chunking-invariant* (the sequence of
//! committed blocks does not depend on how the byte stream was chopped
//! into deltas). [`CursorRenderer`] drives it for a VT100 terminal,
//! emitting the cursor-control + ANSI byte sequence for each delta.
//!
//! Rendering of a single block reuses [`crate::markdown::markdown_to_ansi`]
//! (the same renderer the TUI uses), so the streamed output is identical
//! to a one-shot render of the finished text. Non-TTY / piped /
//! `--eval-mode` consumers do not use this module at all — they keep the
//! raw plain stream, and [`markdown_to_plain`] is provided for callers
//! that want structured-but-uncolored text.

/// Splits a streaming markdown *source* into stable committed blocks and
/// an in-progress live tail.
///
/// A block becomes committable once a blank line terminates it at the top
/// level. Blank lines inside a fenced code block do **not** split it, so a
/// ```` ``` ```` block streams as one unit. The splitter is lossless:
/// every byte pushed is eventually emitted across the committed blocks and
/// the final [`finish`](BlockSplitter::finish) flush.
#[derive(Debug, Default)]
pub struct BlockSplitter {
    /// Uncommitted source: the current in-progress block, with any
    /// leading fully-blank separator lines already stripped. Always
    /// starts at a block boundary (fence closed).
    tail: String,
}

impl BlockSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The in-progress block that has not yet been committed. Rendering
    /// this gives the live preview of what the model is currently
    /// emitting.
    pub fn tail(&self) -> &str {
        &self.tail
    }

    /// Feed one streamed content delta. Returns the markdown sources of
    /// any blocks that just became complete, in order (often empty: a
    /// block only commits when a blank line closes it).
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.tail.push_str(delta);
        self.drain(false)
    }

    /// Flush the final in-progress block at end of stream. An unclosed
    /// fence or a block with no trailing blank line is emitted best-effort
    /// (rendered the same way a one-shot render would handle it).
    pub fn finish(&mut self) -> Vec<String> {
        self.drain(true)
    }

    /// Pop every complete block currently in `tail`. When `at_eof`, the
    /// trailing remainder is also emitted as a final block.
    fn drain(&mut self, at_eof: bool) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            strip_leading_blank_lines(&mut self.tail);
            match next_block_end(&self.tail) {
                Some(end) => {
                    let block = self.tail[..end].trim().to_string();
                    self.tail.replace_range(..end, "");
                    if !block.is_empty() {
                        out.push(block);
                    }
                }
                None => break,
            }
        }
        if at_eof {
            let block = self.tail.trim().to_string();
            self.tail.clear();
            if !block.is_empty() {
                out.push(block);
            }
        }
        out
    }
}

/// Remove leading lines that are entirely blank (empty or whitespace
/// only). These are paragraph separators with no following committed
/// block yet; dropping them keeps `tail` anchored at a real block start
/// without disturbing indented content (whose lines are non-blank).
fn strip_leading_blank_lines(s: &mut String) {
    let mut cut = 0usize;
    loop {
        let rest = &s[cut..];
        let Some(nl) = rest.find('\n') else { break };
        let line = &rest[..nl];
        if line.trim().is_empty() {
            cut += nl + 1;
        } else {
            break;
        }
    }
    if cut > 0 {
        s.replace_range(..cut, "");
    }
}

/// Byte index one past the end of the first complete block in `tail`
/// (including its terminating blank line), or `None` if no block has been
/// fully terminated yet. Fence-aware: a ```` ``` ````/`~~~` line toggles
/// code-fence state and blank lines inside the fence do not terminate the
/// block. A trailing line with no newline is treated as incomplete.
fn next_block_end(tail: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut started = false;
    let mut idx = 0usize;
    while idx < tail.len() {
        let rel_nl = tail[idx..].find('\n');
        let (line_end, has_nl) = match rel_nl {
            Some(p) => (idx + p + 1, true),
            None => (tail.len(), false),
        };
        if !has_nl {
            // Incomplete final line — no committable boundary here.
            return None;
        }
        let content = &tail[idx..line_end - 1];
        let trimmed = content.trim_start();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        let is_blank = content.trim().is_empty();
        if is_fence {
            in_fence = !in_fence;
            started = true;
        } else if is_blank && !in_fence {
            if started {
                return Some(line_end);
            }
            // Leading separator before the block started — skip it.
        } else {
            started = true;
        }
        idx = line_end;
    }
    None
}

/// Render one complete markdown block to ANSI, terminated by a single
/// newline. Thin wrapper over [`crate::markdown::markdown_to_ansi`] kept
/// here so the streaming path and any future callers share one
/// definition of "render a block".
pub fn render_block_ansi(block: &str, width: usize) -> String {
    let mut s = crate::markdown::markdown_to_ansi(block, width);
    while s.ends_with('\n') {
        s.pop();
    }
    s.push('\n');
    s
}

/// Render markdown to structured **plain text** (no ANSI escapes): the
/// layout of [`crate::markdown::markdown_to_ansi`] (bullets, heading text,
/// code-block bars, tables) with every color/style escape stripped.
/// Suitable as a no-ANSI fallback for callers that still want structure.
pub fn markdown_to_plain(md: &str, width: usize) -> String {
    strip_ansi(&crate::markdown::markdown_to_ansi(md, width))
}

/// Remove ANSI/VT100 escape sequences (CSI `\x1b[…<final>`), leaving only
/// the visible text.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Consume a CSI sequence: `[` then params/intermediates then a
            // final byte in 0x40..=0x7e. Non-CSI escapes are dropped along
            // with their single following byte.
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Number of terminal rows the (ANSI-containing) `text` occupies at column
/// width `width`, accounting for explicit `\n` and soft-wrapping of the
/// visible characters. Escape sequences do not consume columns.
pub fn visible_rows(text: &str, width: usize) -> usize {
    let visible = strip_ansi(text);
    if visible.is_empty() {
        return 0;
    }
    let w = width.max(1);
    let mut rows = 1usize;
    let mut col = 0usize;
    for ch in visible.chars() {
        if ch == '\n' {
            rows += 1;
            col = 0;
            continue;
        }
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cw == 0 {
            continue;
        }
        if col + cw > w {
            rows += 1;
            col = cw;
        } else {
            col += cw;
        }
    }
    rows
}

/// Progressive markdown renderer for a VT100 terminal.
///
/// Feed each streamed answer delta to [`push`](CursorRenderer::push) and
/// write the returned bytes to the terminal; call
/// [`finish`](CursorRenderer::finish) at end of stream. Committed blocks
/// are printed once and scroll away above the cursor; the live tail (the
/// current block) is erased and redrawn in place on each delta so it
/// updates token-by-token without disturbing committed text.
///
/// One renderer handles one turn; create a fresh one per turn so no state
/// leaks across turns.
#[derive(Debug)]
pub struct CursorRenderer {
    splitter: BlockSplitter,
    width: usize,
    /// Whether any block has been committed yet (controls the single
    /// blank-line separator inserted between blocks).
    committed_any: bool,
    /// Visible rows the currently-drawn live region occupies on screen, so
    /// the next redraw can move up and erase exactly that many rows.
    live_rows: usize,
    /// The live region bytes currently on screen, to skip redundant
    /// redraws when a delta does not change the rendered tail.
    live_drawn: String,
}

impl CursorRenderer {
    pub fn new(width: usize) -> Self {
        Self {
            splitter: BlockSplitter::new(),
            width: width.max(1),
            committed_any: false,
            live_rows: 0,
            live_drawn: String::new(),
        }
    }

    /// Feed one delta; returns the VT100 byte sequence to write (may be
    /// empty when nothing changed visibly).
    pub fn push(&mut self, delta: &str) -> String {
        let blocks = self.splitter.push(delta);
        let tail = self.splitter.tail().to_string();
        self.emit(&blocks, &tail, false)
    }

    /// Flush at end of stream: commit the final block and leave the cursor
    /// on a fresh line below the rendered output.
    pub fn finish(&mut self) -> String {
        let blocks = self.splitter.finish();
        self.emit(&blocks, "", true)
    }

    fn emit(&mut self, blocks: &[String], tail: &str, at_eof: bool) -> String {
        // The new live region we want on screen after this call.
        let live_region = if tail.trim().is_empty() {
            String::new()
        } else {
            let mut r = String::new();
            if self.committed_any || !blocks.is_empty() {
                r.push('\n');
            }
            let mut body = crate::markdown::markdown_to_ansi(tail, self.width);
            while body.ends_with('\n') {
                body.pop();
            }
            r.push_str(&body);
            r
        };

        // Nothing committed and the live region is unchanged → no-op. This
        // keeps redundant deltas (e.g. whitespace folded into an open tag)
        // from re-emitting identical escapes.
        if blocks.is_empty() && !at_eof && live_region == self.live_drawn {
            return String::new();
        }

        let mut out = String::new();
        // 1. Move to the start of the existing live region and erase it.
        if self.live_rows > 0 {
            if self.live_rows > 1 {
                out.push_str(&format!("\x1b[{}A", self.live_rows - 1));
            }
            out.push_str("\r\x1b[0J");
        }

        // 2. Commit newly-complete blocks permanently (each ends in '\n',
        //    so the cursor advances below them and they never redraw).
        for block in blocks {
            if self.committed_any {
                out.push('\n');
            }
            out.push_str(&render_block_ansi(block, self.width));
            self.committed_any = true;
        }

        // 3. Draw the new live region in place (no trailing newline, so the
        //    cursor rests at its end ready for the next erase).
        out.push_str(&live_region);
        self.live_rows = visible_rows(&live_region, self.width);
        self.live_drawn = live_region;

        // 4. At end of stream the cursor must end on a fresh line below the
        //    answer so following output (markers, the next prompt) starts
        //    cleanly.
        if at_eof && (self.committed_any || !self.live_drawn.is_empty()) {
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BlockSplitter ──

    /// Drive a splitter over `deltas` and return (committed blocks, final
    /// tail flushed by finish()).
    fn run(deltas: &[&str]) -> Vec<String> {
        let mut s = BlockSplitter::new();
        let mut blocks = Vec::new();
        for d in deltas {
            blocks.extend(s.push(d));
        }
        blocks.extend(s.finish());
        blocks
    }

    #[test]
    fn single_paragraph_commits_at_eof() {
        assert_eq!(run(&["hello ", "world"]), vec!["hello world".to_string()]);
    }

    #[test]
    fn blank_line_separates_two_blocks() {
        let blocks = run(&["para one\n\npara two\n"]);
        assert_eq!(blocks, vec!["para one".to_string(), "para two".to_string()]);
    }

    #[test]
    fn block_commits_before_eof_when_blank_line_seen() {
        // The first block must commit as soon as the blank line arrives —
        // not be held until finish(). This is the progressive property.
        let mut s = BlockSplitter::new();
        let early = s.push("# Title\n\n");
        assert_eq!(early, vec!["# Title".to_string()]);
        // The next block is still in the tail, uncommitted.
        let more = s.push("body text");
        assert!(more.is_empty());
        assert_eq!(s.tail().trim(), "body text");
        assert_eq!(s.finish(), vec!["body text".to_string()]);
    }

    #[test]
    fn fence_with_blank_lines_stays_one_block() {
        // Blank lines INSIDE a code fence must not split the block.
        let blocks = run(&["```rust\nfn a() {}\n\n\nfn b() {}\n```\n\nafter\n"]);
        assert_eq!(blocks.len(), 2, "fence + paragraph, got {:?}", blocks);
        assert!(blocks[0].starts_with("```rust"));
        assert!(blocks[0].contains("fn a()"));
        assert!(blocks[0].contains("fn b()"));
        assert!(blocks[0].trim_end().ends_with("```"));
        assert_eq!(blocks[1], "after");
    }

    #[test]
    fn unclosed_fence_flushed_at_eof() {
        let mut s = BlockSplitter::new();
        // Half-open fence with a blank line inside: must NOT commit early.
        assert!(s.push("```\ncode line\n\n").is_empty());
        assert!(s.push("more code\n").is_empty());
        let final_blocks = s.finish();
        assert_eq!(final_blocks.len(), 1);
        assert!(final_blocks[0].starts_with("```"));
        assert!(final_blocks[0].contains("more code"));
    }

    #[test]
    fn fence_split_marker_across_deltas() {
        // The opening fence marker arrives fragmented; still recognized.
        let blocks = run(&["``", "`\ncode\n", "```", "\n\ndone\n"]);
        assert_eq!(blocks.len(), 2, "got {:?}", blocks);
        assert!(blocks[0].contains("code"));
        assert_eq!(blocks[1], "done");
    }

    #[test]
    fn chunking_invariant_byte_by_byte_equals_whole() {
        let doc = "# Heading\n\nA paragraph with **bold** and `code`.\n\n\
                   - item one\n- item two\n\n\
                   ```python\nprint('hi')\n\nprint('bye')\n```\n\n\
                   Final words.\n";
        let whole = run(&[doc]);
        let by_byte: Vec<String> = {
            let mut s = BlockSplitter::new();
            let mut b = Vec::new();
            for ch in doc.chars() {
                let mut buf = [0u8; 4];
                b.extend(s.push(ch.encode_utf8(&mut buf)));
            }
            b.extend(s.finish());
            b
        };
        assert_eq!(whole, by_byte, "block sequence must not depend on chunking");
        assert_eq!(whole.len(), 5, "5 blocks expected, got {:?}", whole);
    }

    #[test]
    fn chunking_invariant_random_splits() {
        let doc = "Intro line.\n\n## Sub\n\ntext `x` here\n\n1. one\n2. two\n\nbye\n";
        let reference = run(&[doc]);
        for step in [2usize, 3, 5, 7, 11] {
            let mut s = BlockSplitter::new();
            let mut b = Vec::new();
            let bytes: Vec<char> = doc.chars().collect();
            let mut i = 0;
            while i < bytes.len() {
                let end = (i + step).min(bytes.len());
                let chunk: String = bytes[i..end].iter().collect();
                b.extend(s.push(&chunk));
                i = end;
            }
            b.extend(s.finish());
            assert_eq!(b, reference, "split step {step} changed the block sequence");
        }
    }

    #[test]
    fn losslessness_all_text_preserved_in_order() {
        let doc = "alpha beta\n\ngamma\n\n- delta\n- epsilon\n";
        let blocks = run(&[doc]);
        let joined = blocks.join("\n");
        for word in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            assert!(joined.contains(word), "lost {word:?} in {joined:?}");
        }
    }

    // ── strip_ansi / visible_rows / markdown_to_plain ──

    #[test]
    fn strip_ansi_removes_escapes_keeps_text() {
        assert_eq!(strip_ansi("\x1b[1mbold\x1b[0m"), "bold");
        assert_eq!(strip_ansi("\x1b[38;5;75mX\x1b[0mY"), "XY");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn visible_rows_ignores_escapes() {
        // Two visible chars with color codes, wide width → one row.
        assert_eq!(visible_rows("\x1b[1mab\x1b[0m", 80), 1);
        // A leading newline (separator) plus content → two rows.
        assert_eq!(visible_rows("\nabc", 80), 2);
        assert_eq!(visible_rows("", 80), 0);
    }

    #[test]
    fn markdown_to_plain_has_no_ansi_but_keeps_structure() {
        let out = markdown_to_plain("# Title\n\n- one\n- two\n", 40);
        assert!(!out.contains('\x1b'), "plain output must have no escapes");
        assert!(out.contains("Title"));
        assert!(out.contains('•'));
        assert!(out.contains("one") && out.contains("two"));
    }

    // ── CursorRenderer: a virtual terminal to assert no-garble. ──

    /// A minimal VT100 model supporting exactly the controls
    /// `CursorRenderer` emits: printable text, `\n`, `\r`, `ESC[<n>A`
    /// (cursor up), `ESC[0J` (erase to end of screen). SGR color codes are
    /// ignored. Tests use a wide width so no soft-wrap occurs.
    #[derive(Default)]
    struct VTerm {
        rows: Vec<Vec<char>>,
        cur_row: usize,
        cur_col: usize,
    }

    impl VTerm {
        fn ensure_row(&mut self, r: usize) {
            while self.rows.len() <= r {
                self.rows.push(Vec::new());
            }
        }

        fn feed(&mut self, bytes: &str) {
            let mut chars = bytes.chars().peekable();
            while let Some(ch) = chars.next() {
                match ch {
                    '\x1b' => {
                        if chars.peek() == Some(&'[') {
                            chars.next();
                            let mut params = String::new();
                            let mut final_byte = '\0';
                            for c in chars.by_ref() {
                                if ('\u{40}'..='\u{7e}').contains(&c) {
                                    final_byte = c;
                                    break;
                                }
                                params.push(c);
                            }
                            match final_byte {
                                'A' => {
                                    let n: usize = params.parse().unwrap_or(1);
                                    self.cur_row = self.cur_row.saturating_sub(n);
                                }
                                'J' => {
                                    // 0J (default): erase from cursor to end.
                                    self.ensure_row(self.cur_row);
                                    self.rows[self.cur_row].truncate(self.cur_col);
                                    self.rows.truncate(self.cur_row + 1);
                                }
                                _ => {}
                            }
                        }
                    }
                    '\r' => self.cur_col = 0,
                    '\n' => {
                        self.cur_row += 1;
                        self.cur_col = 0;
                        self.ensure_row(self.cur_row);
                    }
                    _ => {
                        self.ensure_row(self.cur_row);
                        let row = &mut self.rows[self.cur_row];
                        while row.len() <= self.cur_col {
                            row.push(' ');
                        }
                        row[self.cur_col] = ch;
                        self.cur_col += 1;
                    }
                }
            }
        }

        fn screen(&self) -> String {
            let mut lines: Vec<String> = self
                .rows
                .iter()
                .map(|r| r.iter().collect::<String>().trim_end().to_string())
                .collect();
            while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                lines.pop();
            }
            lines.join("\n")
        }
    }

    fn drive_cursor(deltas: &[&str], width: usize) -> String {
        let mut r = CursorRenderer::new(width);
        let mut vt = VTerm::default();
        for d in deltas {
            vt.feed(&r.push(d));
        }
        vt.feed(&r.finish());
        vt.screen()
    }

    fn expected_screen(doc: &str, width: usize) -> String {
        let ansi = crate::markdown::markdown_to_ansi(doc, width);
        let plain = strip_ansi(&ansi);
        plain
            .lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }

    #[test]
    fn cursor_final_screen_matches_oneshot_render() {
        // Wide width avoids soft-wrap so the VTerm stays simple.
        let w = 200;
        let doc = "# Heading\n\nA paragraph with **bold** text.\n\n\
                   - first\n- second\n\nDone.\n";
        // Stream one char at a time — the worst case for partial markdown.
        let deltas: Vec<String> = doc.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = deltas.iter().map(|s| s.as_str()).collect();
        let got = drive_cursor(&refs, w);
        let want = expected_screen(doc, w);
        assert_eq!(got, want, "streamed screen must equal one-shot render");
    }

    #[test]
    fn cursor_final_screen_matches_oneshot_with_code_fence() {
        let w = 200;
        let doc = "Here is code:\n\n```rust\nfn main() {}\n```\n\nAnd after.\n";
        let deltas: Vec<String> = doc.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = deltas.iter().map(|s| s.as_str()).collect();
        let got = drive_cursor(&refs, w);
        let want = expected_screen(doc, w);
        assert_eq!(got, want);
    }

    #[test]
    fn cursor_commits_progressively_not_buffered() {
        // After a block is closed by a blank line mid-stream, its rendered
        // (styled) bytes must already have been emitted — proving we do not
        // buffer the whole response until the end.
        let mut r = CursorRenderer::new(200);
        let mut emitted = String::new();
        emitted.push_str(&r.push("# Title\n\n"));
        emitted.push_str(&r.push("second paragraph still going"));
        // The heading was committed: its styled text is in the output even
        // though the stream (and finish) has not completed.
        assert!(
            strip_ansi(&emitted).contains("Title"),
            "heading should be committed mid-stream, got {:?}",
            strip_ansi(&emitted)
        );
        assert!(
            emitted.contains("\x1b["),
            "committed output should carry ANSI styling"
        );
    }

    #[test]
    fn cursor_partial_bold_does_not_garble_final() {
        // An unclosed `**bold` mid-stream must not corrupt the final
        // render once the closing `**` arrives.
        let w = 200;
        let deltas = ["The ", "**bo", "ld** ", "word.\n"];
        let got = drive_cursor(&deltas, w);
        let want = expected_screen("The **bold** word.\n", w);
        assert_eq!(got, want);
        assert!(got.contains("bold"));
        assert!(!got.contains('*'), "asterisks should be consumed: {got:?}");
    }

    #[test]
    fn cursor_empty_stream_is_noop() {
        let mut r = CursorRenderer::new(80);
        assert!(r.push("").is_empty());
        assert_eq!(r.finish(), "");
    }

    #[test]
    fn cursor_plain_text_streams_without_control_until_needed() {
        // A single growing paragraph: first delta has no prior live region
        // to erase, so it emits no cursor-up.
        let mut r = CursorRenderer::new(200);
        let first = r.push("hello");
        assert!(!first.contains("\x1b[1A"), "no cursor-up on first draw");
        assert!(strip_ansi(&first).contains("hello"));
    }
}
