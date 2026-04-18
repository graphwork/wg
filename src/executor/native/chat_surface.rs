//! Chat-file I/O surface for a long-running `wg nex` session.
//!
//! When `wg nex --chat-id N` is set, nex stops reading from stdin and
//! writing to stderr, and instead:
//!
//! - **Reads** user turns from `<workgraph>/chat/N/inbox.jsonl` via
//!   `chat::read_inbox_since_for` — one `ChatMessage` per line, polled
//!   tail-style. A cursor file (`.nex-cursor`) persists the last-
//!   consumed message id so a restart resumes where we left off.
//! - **Writes** streaming token chunks to `<workgraph>/chat/N/.streaming`
//!   via `chat::write_streaming` — the canonical streaming dotfile
//!   the TUI already tails.
//! - **Appends** each finalized assistant turn to
//!   `<workgraph>/chat/N/outbox.jsonl` via `chat::append_outbox_for`,
//!   tagged with the originating `request_id` so the TUI can correlate.
//!
//! This module is a thin adapter over `crate::chat` — same paths,
//! same formats — so a nex agent with `--chat-id N` is indistinguishable
//! from the legacy `native_coordinator_loop` as far as the TUI is
//! concerned. That's the "make the coordinator be `wg nex`" plank.
//!
//! Journal location is also deterministic from the chat-id so
//! `wg nex --chat-id N --resume` restores from the right place:
//!   `<workgraph>/chat/N/conversation.jsonl`
//!   `<workgraph>/chat/N/session-summary.md`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};

/// One user turn, flattened from `crate::chat::ChatMessage` into what
/// the agent loop actually needs: the request_id (for correlating the
/// outbox response) and the message content.
#[derive(Debug, Clone)]
pub struct InboxEntry {
    pub request_id: String,
    pub message: String,
    /// Monotonic `ChatMessage.id` — stored back to the cursor file so
    /// a restart skips what we already consumed.
    pub id: u64,
}

/// Paths for one chat-tethered nex session. The inbox/outbox/streaming
/// trio is owned by `crate::chat` — we only name journal + cursor files
/// that chat.rs doesn't know about.
#[derive(Clone, Debug)]
pub struct ChatPaths {
    pub dir: PathBuf,
    pub journal: PathBuf,
    pub session_summary: PathBuf,
    pub cursor: PathBuf,
}

impl ChatPaths {
    pub fn for_chat_id(workgraph_dir: &Path, chat_id: u32) -> Self {
        let dir = workgraph_dir.join("chat").join(chat_id.to_string());
        Self {
            journal: dir.join("conversation.jsonl"),
            session_summary: dir.join("session-summary.md"),
            cursor: dir.join(".nex-cursor"),
            dir,
        }
    }

    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("create_dir_all {:?}", self.dir))
    }
}

/// Tail-style reader over the chat inbox. Delegates parsing to
/// `crate::chat` (so we pick up the ChatMessage format the TUI writes)
/// and persists an id-based cursor for crash-safe resume.
pub struct ChatInboxReader {
    workgraph_dir: PathBuf,
    chat_id: u32,
    paths: ChatPaths,
    cursor: Arc<Mutex<u64>>,
}

impl ChatInboxReader {
    pub fn new(workgraph_dir: PathBuf, chat_id: u32, paths: ChatPaths) -> Result<Self> {
        paths.ensure_dir()?;
        let cursor = load_cursor(&paths.cursor).unwrap_or(0);
        Ok(Self {
            workgraph_dir,
            chat_id,
            paths,
            cursor: Arc::new(Mutex::new(cursor)),
        })
    }

    /// Block until the next inbox entry beyond our cursor is available.
    /// Polls at `poll_interval`. Returns `None` only on persistent read
    /// errors; callers should treat that as shutdown.
    pub async fn next_entry(&self, poll_interval: Duration) -> Option<InboxEntry> {
        loop {
            match self.try_next_entry() {
                Ok(Some(entry)) => return Some(entry),
                Ok(None) => {
                    tokio::time::sleep(poll_interval).await;
                }
                Err(e) => {
                    eprintln!(
                        "\x1b[33m[chat-inbox] read error on chat {}: {} — retrying\x1b[0m",
                        self.chat_id, e
                    );
                    tokio::time::sleep(poll_interval * 2).await;
                }
            }
        }
    }

    /// Non-blocking read. Returns Ok(None) if no new entries, Ok(Some)
    /// if one was read (advancing the cursor), Err on I/O failure.
    pub fn try_next_entry(&self) -> Result<Option<InboxEntry>> {
        let cursor = *self.cursor.lock().unwrap();
        let new_msgs = crate::chat::read_inbox_since_for(&self.workgraph_dir, self.chat_id, cursor)
            .with_context(|| {
                format!(
                    "read_inbox_since_for(chat={}, cursor={})",
                    self.chat_id, cursor
                )
            })?;
        // Take the first user-role message (skip anything else).
        for msg in new_msgs {
            // Advance cursor past anything we look at, even if we skip it,
            // so we don't spin forever on a non-user entry.
            *self.cursor.lock().unwrap() = msg.id;
            save_cursor(&self.paths.cursor, msg.id);
            if msg.role != "user" {
                continue;
            }
            return Ok(Some(InboxEntry {
                request_id: msg.request_id,
                message: msg.content,
                id: msg.id,
            }));
        }
        Ok(None)
    }

    pub fn cursor(&self) -> u64 {
        *self.cursor.lock().unwrap()
    }
}

fn load_cursor(path: &Path) -> Option<u64> {
    let s = std::fs::read_to_string(path).ok()?;
    s.trim().parse::<u64>().ok()
}

fn save_cursor(path: &Path, cursor: u64) {
    let _ = std::fs::write(path, cursor.to_string());
}

/// Advance the cursor past all current inbox messages. Used at
/// fresh-session start (no `--resume`) so we don't re-process queued
/// messages meant for a previous session.
pub fn seek_inbox_to_end(workgraph_dir: &Path, chat_id: u32, paths: &ChatPaths) -> Result<u64> {
    let msgs = crate::chat::read_inbox_for(workgraph_dir, chat_id).unwrap_or_default();
    let last_id = msgs.iter().map(|m| m.id).max().unwrap_or(0);
    save_cursor(&paths.cursor, last_id);
    Ok(last_id)
}

/// Overwrite the streaming dotfile with the full accumulated text.
/// Thin pass-through to `chat::write_streaming`.
pub fn write_streaming(workgraph_dir: &Path, chat_id: u32, text: &str) -> Result<()> {
    crate::chat::write_streaming(workgraph_dir, chat_id, text)
}

/// Append a finalized coordinator response to the outbox.
/// Thin pass-through to `chat::append_outbox_for`.
pub fn append_outbox(
    workgraph_dir: &Path,
    chat_id: u32,
    text: &str,
    request_id: &str,
) -> Result<u64> {
    crate::chat::append_outbox_for(workgraph_dir, chat_id, text, request_id)
}

/// Clear the streaming dotfile (called between turns).
pub fn clear_streaming(workgraph_dir: &Path, chat_id: u32) {
    crate::chat::clear_streaming(workgraph_dir, chat_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn reader_sees_new_inbox_messages() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let paths = ChatPaths::for_chat_id(wg_dir, 7);
        paths.ensure_dir().unwrap();

        crate::chat::append_inbox_for(wg_dir, 7, "hello", "r1").unwrap();
        crate::chat::append_inbox_for(wg_dir, 7, "world", "r2").unwrap();

        let reader = ChatInboxReader::new(wg_dir.to_path_buf(), 7, paths).unwrap();
        let e1 = reader.try_next_entry().unwrap().unwrap();
        assert_eq!(e1.request_id, "r1");
        assert_eq!(e1.message, "hello");
        let e2 = reader.try_next_entry().unwrap().unwrap();
        assert_eq!(e2.request_id, "r2");
        assert_eq!(e2.message, "world");
        assert!(reader.try_next_entry().unwrap().is_none());
    }

    #[test]
    fn cursor_persists_across_reader_instances() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let paths = ChatPaths::for_chat_id(wg_dir, 7);
        paths.ensure_dir().unwrap();

        crate::chat::append_inbox_for(wg_dir, 7, "a", "r1").unwrap();
        crate::chat::append_inbox_for(wg_dir, 7, "b", "r2").unwrap();

        let r1 = ChatInboxReader::new(wg_dir.to_path_buf(), 7, paths.clone()).unwrap();
        let e = r1.try_next_entry().unwrap().unwrap();
        assert_eq!(e.message, "a");
        drop(r1);

        let r2 = ChatInboxReader::new(wg_dir.to_path_buf(), 7, paths).unwrap();
        let e = r2.try_next_entry().unwrap().unwrap();
        assert_eq!(e.message, "b", "cursor should have advanced past 'a'");
    }

    #[test]
    fn seek_to_end_skips_existing_messages() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let paths = ChatPaths::for_chat_id(wg_dir, 7);
        paths.ensure_dir().unwrap();

        crate::chat::append_inbox_for(wg_dir, 7, "old", "r1").unwrap();
        crate::chat::append_inbox_for(wg_dir, 7, "older", "r2").unwrap();

        seek_inbox_to_end(wg_dir, 7, &paths).unwrap();

        let reader = ChatInboxReader::new(wg_dir.to_path_buf(), 7, paths).unwrap();
        assert!(
            reader.try_next_entry().unwrap().is_none(),
            "new reader should see no existing messages after seek_to_end"
        );

        crate::chat::append_inbox_for(wg_dir, 7, "new", "r3").unwrap();
        let e = reader.try_next_entry().unwrap().unwrap();
        assert_eq!(e.request_id, "r3");
    }

    #[test]
    fn outbox_roundtrip_via_chat_rs() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        let paths = ChatPaths::for_chat_id(wg_dir, 7);
        paths.ensure_dir().unwrap();

        append_outbox(wg_dir, 7, "response text", "r1").unwrap();
        let msgs = crate::chat::read_outbox_since_for(wg_dir, 7, 0).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "response text");
        assert_eq!(msgs[0].request_id, "r1");
    }
}
