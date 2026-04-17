//! MRU tracker for files the agent has touched during a session.
//!
//! Purpose: when the history-summary compaction fires, we want the
//! model to wake up with a *refreshed view of its working environment*
//! alongside the (lossy) summary of what happened. The summary carries
//! narrative; the re-injected file contents carry state. Both are
//! needed — either alone leaves the agent blind.
//!
//! Populated from tool inputs post-execution (see `agent.rs` —
//! the run loop extracts `path` from the input JSON of file-touching
//! tools after each batch). Consulted on compaction to re-read the
//! most-recently-touched files and splice them into the message vec
//! right after the summary block.
//!
//! # Why a separate module (not a field on ToolOutput)
//!
//! Adding a `touched_paths` field to `ToolOutput` would ripple through
//! ~40 tool call sites, most of which don't touch files. Extracting
//! paths from input JSON at the agent loop keeps the concern scoped
//! to the one place that cares (the compaction path).
//!
//! # MRU ordering
//!
//! The same file touched multiple times keeps a single entry at the
//! front of the queue. We track distinct paths, most-recent-first,
//! capped at `MAX_TRACKED` entries.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Maximum number of distinct paths tracked. Anything older falls off
/// the back of the queue. Set high enough to cover a single session's
/// working set without unbounded growth.
pub const MAX_TRACKED: usize = 32;

/// How many files re-inject at most on a compaction event.
pub const REINJECT_MAX_FILES: usize = 5;

/// Per-file content cap when re-injecting.
pub const REINJECT_PER_FILE_BYTES: usize = 5_000;

/// Total content cap across all re-injected files.
pub const REINJECT_TOTAL_BYTES: usize = 25_000;

/// Tools whose `input.path` argument should be tracked as a touched
/// file. Kept narrow to avoid false positives — we only want content
/// the agent actually loaded or wrote, not paths mentioned in passing.
pub const FILE_TOUCHING_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "reader",
    "summarize",
];

#[derive(Debug, Default, Clone)]
pub struct TouchedFiles {
    /// MRU queue. Front = most recent.
    paths: VecDeque<PathBuf>,
}

impl TouchedFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a path as touched. If already present, promotes to front
    /// of the queue. No-op on empty paths.
    pub fn mark(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return;
        }
        self.paths.retain(|p| p != &path);
        self.paths.push_front(path);
        while self.paths.len() > MAX_TRACKED {
            self.paths.pop_back();
        }
    }

    /// Inspect the tool call (name + input JSON) and, if it's a
    /// file-touching tool with a `path` string argument, mark the
    /// path as touched. Silently ignores tools not in the allow-list
    /// and inputs without a usable `path`.
    pub fn observe(&mut self, tool_name: &str, input: &serde_json::Value) {
        if !FILE_TOUCHING_TOOLS.iter().any(|t| *t == tool_name) {
            return;
        }
        // `reader` and `summarize` accept either `path` or `source`.
        // Check both — the tracker shouldn't care which tool schema
        // we're looking at.
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("source").and_then(|v| v.as_str()));
        if let Some(p) = path {
            self.mark(p);
        }
    }

    /// Take up to `n` most-recent paths. Returns them newest-first.
    pub fn top(&self, n: usize) -> Vec<&Path> {
        self.paths.iter().take(n).map(|p| p.as_path()).collect()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Re-read the top `REINJECT_MAX_FILES` touched files from disk, cap
/// each to `REINJECT_PER_FILE_BYTES`, cap the total to
/// `REINJECT_TOTAL_BYTES`, and format as a single markdown-style
/// document suitable for splicing into a user-role Text block right
/// after a history summary.
///
/// Files that no longer exist or can't be read are quietly skipped —
/// that's fresher-is-correct behavior. A file deleted since the last
/// touch is genuinely gone, and the summary should carry that fact.
///
/// Returns `None` when nothing was readable (empty tracker, all files
/// missing, etc.) so callers can skip emitting a wrapper block.
pub fn reinject_files_markdown(tracker: &TouchedFiles) -> Option<String> {
    if tracker.is_empty() {
        return None;
    }

    let mut out = String::new();
    let mut total = 0usize;
    let mut included = 0usize;

    out.push_str(
        "[Post-compaction working state — most-recently-touched files re-read fresh:]\n\n",
    );

    for path in tracker.top(REINJECT_MAX_FILES) {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let truncated = if content.len() > REINJECT_PER_FILE_BYTES {
            let boundary = content
                .char_indices()
                .nth(REINJECT_PER_FILE_BYTES)
                .map(|(i, _)| i)
                .unwrap_or(REINJECT_PER_FILE_BYTES);
            format!(
                "{}\n\n[truncated at {} bytes; file is {} bytes total]",
                &content[..boundary],
                REINJECT_PER_FILE_BYTES,
                content.len()
            )
        } else {
            content
        };

        // Global budget: bail before adding the block that would push
        // us over. The first file is always included regardless of
        // size so the agent gets at least one fresh file view.
        if included > 0 && total + truncated.len() > REINJECT_TOTAL_BYTES {
            break;
        }

        out.push_str(&format!("## `{}`\n\n", path.display()));
        out.push_str("```\n");
        out.push_str(&truncated);
        if !truncated.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
        total += truncated.len();
        included += 1;
    }

    if included == 0 { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn mark_puts_new_path_at_front() {
        let mut t = TouchedFiles::new();
        t.mark("a.txt");
        t.mark("b.txt");
        t.mark("c.txt");
        let top: Vec<_> = t.top(3).into_iter().map(|p| p.to_str().unwrap().to_string()).collect();
        assert_eq!(top, vec!["c.txt", "b.txt", "a.txt"]);
    }

    #[test]
    fn mark_promotes_existing_path() {
        let mut t = TouchedFiles::new();
        t.mark("a.txt");
        t.mark("b.txt");
        t.mark("a.txt"); // re-touch
        let top: Vec<_> = t.top(3).into_iter().map(|p| p.to_str().unwrap().to_string()).collect();
        assert_eq!(top, vec!["a.txt", "b.txt"]);
        assert_eq!(t.len(), 2, "re-touch should not duplicate");
    }

    #[test]
    fn mark_caps_at_max_tracked() {
        let mut t = TouchedFiles::new();
        for i in 0..MAX_TRACKED + 10 {
            t.mark(format!("file{}.txt", i));
        }
        assert_eq!(t.len(), MAX_TRACKED);
    }

    #[test]
    fn observe_extracts_path_from_read_file() {
        let mut t = TouchedFiles::new();
        let input = serde_json::json!({"path": "src/main.rs"});
        t.observe("read_file", &input);
        assert_eq!(t.len(), 1);
        assert_eq!(t.top(1)[0], Path::new("src/main.rs"));
    }

    #[test]
    fn observe_ignores_non_file_tools() {
        let mut t = TouchedFiles::new();
        let input = serde_json::json!({"path": "whatever"});
        t.observe("bash", &input);
        t.observe("web_search", &input);
        assert!(t.is_empty());
    }

    #[test]
    fn observe_extracts_source_from_summarize() {
        let mut t = TouchedFiles::new();
        let input = serde_json::json!({"source": "notes.md"});
        t.observe("summarize", &input);
        assert_eq!(t.top(1)[0], Path::new("notes.md"));
    }

    #[test]
    fn observe_silently_skips_missing_path() {
        let mut t = TouchedFiles::new();
        let input = serde_json::json!({"other": "thing"});
        t.observe("read_file", &input);
        assert!(t.is_empty());
    }

    #[test]
    fn reinject_empty_tracker_returns_none() {
        let t = TouchedFiles::new();
        assert!(reinject_files_markdown(&t).is_none());
    }

    #[test]
    fn reinject_includes_existing_files_skips_missing() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("real.txt");
        let mut f = std::fs::File::create(&existing).unwrap();
        writeln!(f, "hello world").unwrap();

        let mut t = TouchedFiles::new();
        t.mark(dir.path().join("ghost.txt"));
        t.mark(&existing);

        let md = reinject_files_markdown(&t).expect("should have content");
        assert!(md.contains("real.txt"));
        assert!(md.contains("hello world"));
        assert!(!md.contains("ghost.txt"));
    }

    #[test]
    fn reinject_caps_per_file_size() {
        let dir = tempdir().unwrap();
        let big = dir.path().join("big.txt");
        let body = "a".repeat(REINJECT_PER_FILE_BYTES + 5_000);
        std::fs::write(&big, &body).unwrap();

        let mut t = TouchedFiles::new();
        t.mark(&big);

        let md = reinject_files_markdown(&t).unwrap();
        assert!(md.contains("truncated at"));
        assert!(md.len() < REINJECT_PER_FILE_BYTES + 2_000);
    }

    #[test]
    fn reinject_respects_total_budget() {
        let dir = tempdir().unwrap();
        let body = "z".repeat(REINJECT_PER_FILE_BYTES);
        let mut paths = vec![];
        for i in 0..REINJECT_MAX_FILES + 3 {
            let p = dir.path().join(format!("f{}.txt", i));
            std::fs::write(&p, &body).unwrap();
            paths.push(p);
        }
        let mut t = TouchedFiles::new();
        for p in &paths {
            t.mark(p);
        }

        let md = reinject_files_markdown(&t).unwrap();
        // First file is always included even if huge — hence the modest
        // slack. Total budget applies to subsequent files only.
        assert!(md.len() <= REINJECT_TOTAL_BYTES + REINJECT_PER_FILE_BYTES + 2_000);
    }
}
