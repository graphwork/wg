//! L0 current-turn defense: reject-and-explain for oversized tool_use
//! arguments, plus save-to-buffer so no work is lost.
//!
//! Motivation: on 2026-04-17 a research task on ulivo hit the context
//! limit on turn 34 while issuing a `write_file` call with a ~20KB
//! `content` argument. The single outgoing request (conversation
//! history + the oversized tool_use) exceeded the 32k window. The
//! agent loop's historical compaction (microcompact / summarize-
//! history / file re-injection) can't help here — it protects
//! historical blocks, not the current turn's tool_use args the model
//! just authored.
//!
//! The defense:
//!
//! 1. Detect a tool_use block whose serialized `input` exceeds a
//!    model-window-scaled threshold.
//! 2. Save the full `input` JSON to a pending buffer file so nothing
//!    is lost.
//! 3. Rewrite the tool_use in-place with a compact placeholder
//!    (`{"_compacted": {"bytes": N, "saved_to": "path/..."}}`).
//! 4. Synthesize an `is_error` tool_result for that id with a
//!    human-readable explanation pointing at the buffer path and
//!    suggesting chunked retry (`append_file`, `bash cat >>`, etc).
//!
//! After L0 fires, the next outgoing request sees:
//! - Compact tool_use (maybe ~150 bytes instead of 20 KB)
//! - Short error tool_result telling the model what happened
//!
//! The model's next turn can read the buffer if it needs the original
//! content back, or simply try a chunked approach. Work preserved,
//! context bounded, no silent data loss.
//!
//! This is the first recursive layer — the explanation in the tool_result
//! is itself small and self-describing; we never put the 20KB back into
//! context, only a pointer. Higher layers (microcompact, summarize-
//! history) handle everything else.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::client::ContentBlock;

/// Default maximum bytes allowed for a single tool_use's serialized
/// input before L0 intervenes. 8192 bytes (~2k tokens at 4 chars/tok)
/// fits comfortably in the current-turn overhead budget of a 32k
/// window model. Models with larger windows can raise this, but the
/// fundamental pathology — model generates 20KB of content inline —
/// is a prompt-engineering / tool-design issue regardless of window
/// size, so the threshold is conservative.
pub const DEFAULT_MAX_TOOL_USE_INPUT_BYTES: usize = 8192;

/// Monotonic counter for pending-buffer filenames.
static PENDING_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Subdirectory under `<workgraph>/nex-sessions/` where oversized
/// tool_use inputs get stashed.
pub const PENDING_DIR: &str = "pending-tool-use-buffers";

/// Describes one tool_use rejected by L0. Caller synthesizes a
/// matching tool_result from this record.
pub struct Rejection {
    pub tool_use_id: String,
    pub tool_name: String,
    pub original_bytes: usize,
    pub buffer_path: PathBuf,
}

impl Rejection {
    /// Build the human-readable explanation body that goes into the
    /// synthesized tool_result. Kept small — this lands in context
    /// and must not itself bust the window.
    pub fn explain(&self) -> String {
        format!(
            "[L0 defense] Your `{tool}` call's arguments were {bytes} bytes — too \
             large to fit in this model's context window without forcing a compaction. \
             The full original arguments have been saved to:\n  \
             {buf}\n\
             \n\
             To proceed:\n\
             - `read_file` that buffer to see what you tried to send, if you need it\n\
             - Split the work into chunks: first call writes a shorter piece, then \
               `append_file` (or `bash cat >> <path> <<'EOF' ...`) for each subsequent chunk\n\
             - Or compress/summarize the intended content inline before retrying\n\
             \n\
             The oversized tool_use in your message history has been replaced with a \
             compact placeholder pointing at this buffer, so your next request will fit.",
            tool = self.tool_name,
            bytes = self.original_bytes,
            buf = self.buffer_path.display(),
        )
    }
}

/// Walk a message's `content`, identify ToolUse blocks whose input
/// exceeds `max_input_bytes`, save each oversized input to a pending
/// buffer file under `<workgraph>/nex-sessions/<PENDING_DIR>/`, and
/// rewrite the ToolUse in place with a compact placeholder.
///
/// Returns one `Rejection` per oversized tool_use found. Caller
/// should synthesize a matching tool_result for each, to be pushed
/// to messages in place of (or alongside) actually executing the tool.
///
/// Safe to call on any message; if no ToolUse blocks are present or
/// none exceed the threshold, returns an empty vec and leaves the
/// message unchanged.
///
/// On buffer-write failure, returns the Rejection with an error path
/// string in `buffer_path` — caller should still surface it (better
/// to fail loud than to silently ship oversized content).
pub fn compact_oversized_tool_uses(
    message: &mut super::client::Message,
    workgraph_dir: &Path,
    max_input_bytes: usize,
) -> Vec<Rejection> {
    let mut rejections = Vec::new();

    for block in message.content.iter_mut() {
        if let ContentBlock::ToolUse { id, name, input } = block {
            let serialized = match serde_json::to_string(input) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if serialized.len() <= max_input_bytes {
                continue;
            }

            let n = PENDING_COUNTER.fetch_add(1, Ordering::SeqCst);
            let slug = slug_from_name(name);
            let filename = format!("{:05}-{}-{}.json", n, slug, id);
            let dir = workgraph_dir.join("nex-sessions").join(PENDING_DIR);
            let buffer_path = match std::fs::create_dir_all(&dir) {
                Ok(()) => dir.join(filename),
                Err(e) => {
                    eprintln!(
                        "\x1b[33m[l0-defense] create_dir_all {} failed: {} — \
                         proceeding with placeholder but buffer NOT saved\x1b[0m",
                        dir.display(),
                        e
                    );
                    PathBuf::from(format!("<buffer-save-failed: {}>", e))
                }
            };

            if buffer_path.is_absolute()
                && let Err(e) = std::fs::write(&buffer_path, &serialized)
            {
                eprintln!(
                    "\x1b[33m[l0-defense] write to {} failed: {}\x1b[0m",
                    buffer_path.display(),
                    e
                );
            }

            let original_bytes = serialized.len();
            let placeholder = serde_json::json!({
                "_compacted_by_l0_defense": {
                    "original_bytes": original_bytes,
                    "saved_to": buffer_path.display().to_string(),
                    "note": "Your tool_use args were oversized; see matching tool_result for how to retry."
                }
            });

            eprintln!(
                "\x1b[33m[l0-defense] {} tool_use args {} B > {} B cap — saved to {}\x1b[0m",
                name,
                original_bytes,
                max_input_bytes,
                buffer_path.display()
            );

            rejections.push(Rejection {
                tool_use_id: id.clone(),
                tool_name: name.clone(),
                original_bytes,
                buffer_path: buffer_path.clone(),
            });

            *input = placeholder;
        }
    }

    rejections
}

/// Filesystem-safe slug for a tool name. The tool-name space is
/// small (snake_case alphanumeric), but we clean defensively.
fn slug_from_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "tool".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::native::client::{Message, Role};
    use serde_json::json;
    use tempfile::tempdir;

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }
    }

    #[test]
    fn small_tool_use_not_touched() {
        let dir = tempdir().unwrap();
        let mut msg = Message {
            role: Role::Assistant,
            content: vec![tool_use("t1", "read_file", json!({"path": "/tmp/x"}))],
        };
        let rejections = compact_oversized_tool_uses(&mut msg, dir.path(), 8192);
        assert!(rejections.is_empty());
        // Input unchanged
        if let ContentBlock::ToolUse { input, .. } = &msg.content[0] {
            assert_eq!(input.get("path").unwrap().as_str().unwrap(), "/tmp/x");
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn oversized_tool_use_gets_saved_and_compacted() {
        let dir = tempdir().unwrap();
        let big = "x".repeat(20_000);
        let mut msg = Message {
            role: Role::Assistant,
            content: vec![tool_use(
                "t1",
                "write_file",
                json!({"path": "/tmp/out.md", "content": big.clone()}),
            )],
        };
        let rejections = compact_oversized_tool_uses(&mut msg, dir.path(), 8192);
        assert_eq!(rejections.len(), 1);
        let r = &rejections[0];
        assert_eq!(r.tool_name, "write_file");
        assert_eq!(r.tool_use_id, "t1");
        assert!(r.original_bytes > 20_000);
        assert!(r.buffer_path.exists(), "buffer should be saved to disk");

        // Buffer content should be the original serialized input
        let saved = std::fs::read_to_string(&r.buffer_path).unwrap();
        assert!(saved.contains(&big), "buffer should contain the full 20k content");

        // tool_use input should now be the placeholder
        if let ContentBlock::ToolUse { input, .. } = &msg.content[0] {
            assert!(input.get("_compacted_by_l0_defense").is_some());
            // And its serialized size should be small
            let s = serde_json::to_string(input).unwrap();
            assert!(s.len() < 500, "placeholder should be small, got {}", s.len());
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn explain_contains_recovery_path() {
        let r = Rejection {
            tool_use_id: "t1".to_string(),
            tool_name: "write_file".to_string(),
            original_bytes: 20_480,
            buffer_path: PathBuf::from("/wg/nex-sessions/pending-tool-use-buffers/00001-write_file-t1.json"),
        };
        let msg = r.explain();
        assert!(msg.contains("20480"));
        assert!(msg.contains("write_file"));
        assert!(msg.contains("append_file"));
        assert!(msg.contains("buffer"));
    }

    #[test]
    fn non_tool_use_blocks_ignored() {
        let dir = tempdir().unwrap();
        let mut msg = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "x".repeat(50_000),
            }],
        };
        let rejections = compact_oversized_tool_uses(&mut msg, dir.path(), 8192);
        // L0 defense targets tool_use inputs specifically; text blocks are
        // handled by the historical-compaction microcompact path, not here.
        assert!(rejections.is_empty());
        // Text block is untouched.
        if let ContentBlock::Text { text } = &msg.content[0] {
            assert_eq!(text.len(), 50_000);
        }
    }

    #[test]
    fn multiple_oversized_tool_uses_each_get_their_own_buffer() {
        let dir = tempdir().unwrap();
        let mut msg = Message {
            role: Role::Assistant,
            content: vec![
                tool_use("t1", "write_file", json!({"content": "a".repeat(15_000)})),
                tool_use("t2", "bash", json!({"command": "b".repeat(15_000)})),
            ],
        };
        let rejections = compact_oversized_tool_uses(&mut msg, dir.path(), 8192);
        assert_eq!(rejections.len(), 2);
        assert_ne!(rejections[0].buffer_path, rejections[1].buffer_path);
        assert!(rejections[0].buffer_path.exists());
        assert!(rejections[1].buffer_path.exists());
    }
}
