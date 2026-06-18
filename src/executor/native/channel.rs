//! Tool output channeling.
//!
//! Large tool outputs (bash dumps, file reads, grep results) are written to
//! disk and replaced in the message vec with a bounded preview and a handle
//! string that tells the agent where to look.
//!
//! The handle string includes a short preview plus explicit bash hints
//! (`cat`, `head`, `tail`, `sed`, `grep`) so the agent can retrieve any
//! slice of the full output on demand. This makes channeled content
//! **retrievable**, which is the property that distinguishes L1 from L0:
//! compacted content is lost, channeled content is paged.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Fallback threshold when no model context window is available.
///
/// This is intentionally much larger than the old ~4 KiB behavior. A 32k-token
/// model can usually afford a single ~32 KiB command result (roughly 8k tokens
/// before tokenizer overhead) while context-pressure checks and compaction still
/// protect the full conversation. Context-aware callers should prefer
/// [`threshold_for_context_window`].
pub const DEFAULT_CHANNEL_THRESHOLD_BYTES: usize = 32 * 1024;

/// Maximum inline budget for one tool result, even on very large-context models.
///
/// Rendering and log surfaces become hard to scan well before model context is
/// exhausted, so we cap one inline tool result at 128 KiB and store the rest as
/// an artifact with first/last previews.
pub const MAX_CHANNEL_THRESHOLD_BYTES: usize = 128 * 1024;

/// Tools whose output should NEVER be channeled — their whole job is
/// to bring structured data INTO the model's context, so replacing
/// that data with a handle defeats the point. `web_search` enforces
/// its own size cap (MAX_RESULTS results) so it fits comfortably in
/// a turn and channeling is redundant. `web_fetch` self-manages its
/// output differently — it writes fetched pages to a file artifact
/// and returns metadata + preview, so it never passes a huge body
/// to the channeler in the first place.
///
/// We learned this the hard way: qwen3-coder-30b was hallucinating
/// restaurant names from real-looking URLs because it had never
/// actually seen the web_search output — the 8 KB of results had
/// been channeled to disk and the model only had a 400-char preview
/// to work with. The model grounded on what it could see (restaurant
/// name fragments) and confabulated plausible variants for the rest.
const NEVER_CHANNEL_TOOLS: &[&str] = &["web_search"];

/// Number of chars from each edge included in artifact handles.
pub const DEFAULT_EDGE_PREVIEW_CHARS: usize = 2 * 1024;

/// Derive an inline output budget from the model context window.
///
/// Policy: spend up to about 8% of the context window on a single command
/// result, using the same rough 4 chars/token estimate as fallback context
/// accounting. This keeps useful command output inline on 32k+ models, avoids
/// the old premature 4 KiB stop, and still leaves room for system/tool overhead,
/// the user request, prior turns, and the next answer. The budget is bounded to
/// 32-128 KiB for terminal readability and predictable rendering costs.
pub fn threshold_for_context_window(context_window_tokens: usize) -> usize {
    let estimated_bytes = context_window_tokens.saturating_mul(4).saturating_mul(8) / 100;
    estimated_bytes.clamp(DEFAULT_CHANNEL_THRESHOLD_BYTES, MAX_CHANNEL_THRESHOLD_BYTES)
}

/// Routes oversized tool outputs to disk and returns a compact handle.
pub struct ToolOutputChanneler {
    /// Directory where channeled outputs are written (typically
    /// `<agent_dir>/tool-outputs/`).
    dir: PathBuf,
    /// Monotonic counter for output filenames.
    counter: AtomicUsize,
    /// Outputs ≤ this size pass through unchanged.
    threshold_bytes: usize,
    /// Chars from each edge to include in the handle string.
    edge_preview_chars: usize,
}

impl ToolOutputChanneler {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_threshold(dir, DEFAULT_CHANNEL_THRESHOLD_BYTES)
    }

    pub fn with_threshold(dir: PathBuf, threshold_bytes: usize) -> Self {
        Self {
            dir,
            counter: AtomicUsize::new(0),
            threshold_bytes,
            edge_preview_chars: DEFAULT_EDGE_PREVIEW_CHARS,
        }
    }

    pub fn for_context_window(dir: PathBuf, context_window_tokens: usize) -> Self {
        Self::with_threshold(dir, threshold_for_context_window(context_window_tokens))
    }

    /// If `content` exceeds the threshold, write it to disk and return a
    /// handle string pointing to the file. Otherwise return `content`
    /// unchanged.
    ///
    /// Tools in `NEVER_CHANNEL_TOOLS` always pass through regardless
    /// of size — their outputs are the whole point of the call and
    /// truncating them to a 400-char preview destroys the value.
    ///
    /// On any I/O failure, returns the original content rather than
    /// silently losing it — channeling is best-effort, never a blocker.
    pub fn maybe_channel(&self, tool_name: &str, content: &str) -> String {
        self.maybe_channel_with_input(tool_name, None, content)
    }

    pub fn maybe_channel_with_input(
        &self,
        tool_name: &str,
        input: Option<&serde_json::Value>,
        content: &str,
    ) -> String {
        if NEVER_CHANNEL_TOOLS.contains(&tool_name) {
            return content.to_string();
        }
        if content.len() <= self.threshold_bytes {
            return content.to_string();
        }

        if tool_name == "bash" {
            if let Some(path) = artifact_path_from_bash_input(input) {
                return render_artifact_read_preview(
                    &path,
                    content,
                    self.edge_preview_chars,
                    self.threshold_bytes,
                );
            }
        }

        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let filename = format!("{:05}.log", n);
        let path = self.dir.join(&filename);

        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            eprintln!(
                "[channel] Failed to create {}: {} — passing output through unchanneled",
                self.dir.display(),
                e
            );
            return content.to_string();
        }
        if let Err(e) = std::fs::write(&path, content) {
            eprintln!(
                "[channel] Failed to write {}: {} — passing output through unchanneled",
                path.display(),
                e
            );
            return content.to_string();
        }

        // Prefer canonical/absolute path in the handle for agent clarity.
        let display_path = std::fs::canonicalize(&path)
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();

        render_channeled_handle(
            tool_name,
            content,
            &display_path,
            self.edge_preview_chars,
            self.threshold_bytes,
        )
    }
}

fn render_channeled_handle(
    tool_name: &str,
    content: &str,
    display_path: &str,
    edge_preview_chars: usize,
    threshold_bytes: usize,
) -> String {
    let preview = edge_preview(content, edge_preview_chars);
    let line_count = content.lines().count();

    format!(
        "[CHANNELED OUTPUT — {bytes} bytes, {lines} lines from tool '{tool}' saved to {path}]\n\
         Inline policy: output exceeded the per-call preview budget ({threshold} bytes), \
         derived from model context budget and terminal usability.\n\
         Preview (first/last bounded slices):\n\
         --- BEGIN FIRST SLICE ---\n\
         {head}\n\
         --- END FIRST SLICE ---\n\
         [... {omitted} bytes omitted; full output is preserved on disk ...]\n\
         --- BEGIN LAST SLICE ---\n\
         {tail}\n\
         --- END LAST SLICE ---\n\
         Inspect safely with:\n\
         - `head -n 80 {path}`\n\
         - `tail -n 80 {path}`\n\
         - `sed -n '100,200p' {path}`\n\
         - `grep -n 'PATTERN' {path}`\n\
         - `wc -l {path}`\n\
         Note: `cat {path}` returns a bounded non-recursive preview in Nex; use head/tail/sed/grep for slices.",
        bytes = content.len(),
        lines = line_count,
        tool = tool_name,
        path = display_path,
        threshold = threshold_bytes,
        head = preview.head,
        tail = preview.tail,
        omitted = preview.omitted_bytes,
    )
}

fn render_artifact_read_preview(
    path: &str,
    content: &str,
    edge_preview_chars: usize,
    threshold_bytes: usize,
) -> String {
    let preview = edge_preview(content, edge_preview_chars);
    format!(
        "[TOOL OUTPUT ARTIFACT PREVIEW — {bytes} bytes, {lines} lines from {path}]\n\
         This is a bounded preview of an existing routed output artifact. Nex did not \
         create another artifact for this read, avoiding recursive output routing.\n\
         Preview budget: {threshold} bytes before artifact routing.\n\
         --- BEGIN FIRST SLICE ---\n\
         {head}\n\
         --- END FIRST SLICE ---\n\
         [... {omitted} bytes omitted from the middle ...]\n\
         --- BEGIN LAST SLICE ---\n\
         {tail}\n\
         --- END LAST SLICE ---\n\
         Continue with `head -n 80 {path}`, `tail -n 80 {path}`, \
         `sed -n '100,200p' {path}`, or `grep -n 'PATTERN' {path}`.",
        bytes = content.len(),
        lines = content.lines().count(),
        path = path,
        threshold = threshold_bytes,
        head = preview.head,
        tail = preview.tail,
        omitted = preview.omitted_bytes,
    )
}

struct EdgePreview<'a> {
    head: &'a str,
    tail: &'a str,
    omitted_bytes: usize,
}

fn edge_preview(content: &str, edge_preview_chars: usize) -> EdgePreview<'_> {
    let head_end = content.floor_char_boundary(edge_preview_chars);
    let raw_tail_start = content.len().saturating_sub(edge_preview_chars);
    let tail_start = content.floor_char_boundary(raw_tail_start).max(head_end);
    let head = &content[..head_end];
    let tail = &content[tail_start..];
    EdgePreview {
        head,
        tail,
        omitted_bytes: content.len().saturating_sub(head.len() + tail.len()),
    }
}

fn artifact_path_from_bash_input(input: Option<&serde_json::Value>) -> Option<String> {
    let command = input?.get("command")?.as_str()?.trim();
    if !command.contains("/tool-outputs/") || !command.starts_with("cat ") {
        return None;
    }
    let path = command
        .strip_prefix("cat ")?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if path.contains("/tool-outputs/") {
        Some(path.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_small_output_passes_through() {
        let tmp = TempDir::new().unwrap();
        let channeler = ToolOutputChanneler::with_threshold(tmp.path().to_path_buf(), 2048);
        let out = channeler.maybe_channel("bash", "hello world");
        assert_eq!(out, "hello world");
        // No file should have been written
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn test_large_output_is_channeled() {
        let tmp = TempDir::new().unwrap();
        let channeler = ToolOutputChanneler::with_threshold(tmp.path().to_path_buf(), 100);
        let content: String = "a\n".repeat(20_000);
        let handle = channeler.maybe_channel("read_file", &content);

        // Handle is much smaller than original
        assert!(handle.len() < content.len() / 2);
        // Handle mentions the size
        assert!(handle.contains(&format!("{} bytes", content.len())));
        assert!(handle.contains("lines"));
        // Handle mentions the tool name
        assert!(handle.contains("read_file"));
        // Handle includes bash hints
        assert!(handle.contains("head -n"));
        assert!(handle.contains("grep"));

        // The file should exist and contain the full original content
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let file_content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert_eq!(file_content, content);
    }

    #[test]
    fn test_counter_increments_across_calls() {
        let tmp = TempDir::new().unwrap();
        let channeler = ToolOutputChanneler::with_threshold(tmp.path().to_path_buf(), 10);
        let _ = channeler.maybe_channel("bash", &"x".repeat(100));
        let _ = channeler.maybe_channel("bash", &"y".repeat(100));
        let _ = channeler.maybe_channel("bash", &"z".repeat(100));

        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert_eq!(entries.len(), 3);
        // Filenames are monotonically numbered
        let mut sorted = entries.clone();
        sorted.sort();
        assert_eq!(sorted[0], "00000.log");
        assert_eq!(sorted[1], "00001.log");
        assert_eq!(sorted[2], "00002.log");
    }

    #[test]
    fn test_preview_included_in_handle() {
        let tmp = TempDir::new().unwrap();
        let channeler = ToolOutputChanneler::with_threshold(tmp.path().to_path_buf(), 10);
        let content = format!("PREFIX_{}", "x".repeat(2000));
        let handle = channeler.maybe_channel("grep", &content);
        // The preview should include the prefix
        assert!(handle.contains("PREFIX_"));
    }

    #[test]
    fn test_default_threshold_spends_more_than_4kb() {
        let tmp = TempDir::new().unwrap();
        let channeler = ToolOutputChanneler::new(tmp.path().to_path_buf());
        assert!(DEFAULT_CHANNEL_THRESHOLD_BYTES > 4 * 1024);
        let large = "a".repeat(4096);
        assert_eq!(channeler.maybe_channel("bash", &large), large);
    }

    #[test]
    fn test_budget_decision_at_boundary() {
        // The inline-vs-file decision is `content.len() <= threshold`.
        // Exercise both sides of the boundary at a fixed threshold.
        let tmp = TempDir::new().unwrap();
        let threshold = 1000;
        let channeler = ToolOutputChanneler::with_threshold(tmp.path().to_path_buf(), threshold);

        // Exactly at the threshold: delivered inline, untouched, no file written.
        let at = "a".repeat(threshold);
        assert_eq!(channeler.maybe_channel("bash", &at), at);
        assert!(
            std::fs::read_dir(tmp.path()).unwrap().next().is_none(),
            "at-threshold output must not be channeled to disk"
        );

        // One byte over: channeled to a file with explicit parse guidance.
        let over = "b".repeat(threshold + 1);
        let handle = channeler.maybe_channel("bash", &over);
        assert_ne!(handle, over, "over-threshold output must be channeled");
        assert!(handle.contains("CHANNELED OUTPUT"));
        assert!(handle.contains("head -n"));
        assert!(handle.contains("sed -n"));
        assert!(handle.contains("grep -n"));
        // Full content preserved on disk.
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(std::fs::read_to_string(entries[0].path()).unwrap(), over);
    }

    #[test]
    fn test_budget_decision_tracks_context_window() {
        // End-to-end: a small-context model channels a 40 KiB output, while a
        // large-context model delivers the same output inline — proving the
        // threshold is derived from the resolved context window.
        let tmp = TempDir::new().unwrap();
        let small = ToolOutputChanneler::for_context_window(
            tmp.path().join("small"),
            8_192, // llama.cpp `-c 8192`: budget clamps to 32 KiB floor
        );
        let large = ToolOutputChanneler::for_context_window(
            tmp.path().join("large"),
            1_000_000, // budget clamps to 128 KiB ceiling
        );
        let payload = "x".repeat(40 * 1024);

        let small_out = small.maybe_channel("bash", &payload);
        assert!(
            small_out.contains("CHANNELED OUTPUT"),
            "40 KiB should exceed the 32 KiB budget of an 8k-context model"
        );
        assert_eq!(
            large.maybe_channel("bash", &payload),
            payload,
            "40 KiB fits inline under a large-context model's 128 KiB budget"
        );
    }

    #[test]
    fn test_threshold_scales_with_context_window() {
        assert_eq!(threshold_for_context_window(32_768), 32 * 1024);
        assert_eq!(threshold_for_context_window(200_000), 64_000);
        assert_eq!(threshold_for_context_window(1_000_000), 128 * 1024);
    }

    #[test]
    fn test_cat_of_artifact_is_non_recursive_preview() {
        let tmp = TempDir::new().unwrap();
        let tool_dir = tmp.path().join("tool-outputs");
        let channeler = ToolOutputChanneler::with_threshold(tool_dir.clone(), 100);
        let artifact = tool_dir.join("00000.log");
        let display = artifact.display().to_string();
        let content = format!("HEAD\n{}\nTAIL\n", "x".repeat(5000));
        let input = serde_json::json!({ "command": format!("cat {}", display) });

        let preview = channeler.maybe_channel_with_input("bash", Some(&input), &content);

        assert!(preview.contains("TOOL OUTPUT ARTIFACT PREVIEW"));
        assert!(preview.contains("Nex did not create another artifact"));
        assert!(preview.contains("HEAD"));
        assert!(preview.contains("TAIL"));
        assert!(preview.contains("sed -n"));
        assert!(
            std::fs::read_dir(&tool_dir).is_err(),
            "recursive artifact read should not create a new tool-output file"
        );
    }
}
