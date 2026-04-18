//! Conversation surface: the plug point where `AgentLoop` meets the
//! outside world for user input and streaming output.
//!
//! The point of this trait is to let a single agent codepath (`nex`
//! = task-agent = coordinator = evaluate) serve every role workgraph
//! has. Each role differs only in WHERE user input comes from and
//! WHERE streaming output goes.
//!
//! | Role         | Input source                   | Output sink                    |
//! |--------------|--------------------------------|--------------------------------|
//! | `wg nex`     | rustyline on stdin             | stderr                         |
//! | task agent   | task description + inbox poll  | stream.ndjson + stderr         |
//! | coordinator  | `mpsc::Receiver<ChatRequest>`  | `chat/<id>/streaming`          |
//! | `evaluate`   | eval prompt (one-shot)         | JSON record                    |
//!
//! The loop is identical. Stages A–G (cancel, inbox, microcompact,
//! L0 defense, idle watchdog, ...) all fire regardless of surface.
//! New features added to `AgentLoop` automatically benefit every role.
//!
//! Today's state: only two impls exist (`TerminalSurface` below, and
//! the implicit autonomous path that goes through state injection).
//! Coordinator migration to `ChatFileSurface` is the next step.
//!
//! See `docs/design/nex-as-coordinator.md` for the broader design.

use async_trait::async_trait;

/// A pluggable input/output surface for an agent conversation.
///
/// Callers of `AgentLoop` pick a surface at startup; the loop then
/// uses it for all user interaction.
#[async_trait]
pub trait ConversationSurface: Send {
    /// Block until the next user message is available. Returns `None`
    /// when the surface has closed (EOF on stdin, shutdown on a chat
    /// channel, etc.) — that tells the loop to exit cleanly.
    ///
    /// Surfaces that multiplex with other signals (Ctrl-C, an
    /// `AgentInbox`) should return promptly when those fire so the
    /// loop can handle them at the next turn boundary.
    async fn next_user_input(&mut self) -> Option<UserTurn>;

    /// Called when the agent emits a streaming text chunk. Surface
    /// decides where it goes — stderr for a terminal, a file for the
    /// coordinator's chat UI, a network socket for a remote viewer.
    fn write_stream_chunk(&mut self, text: &str);

    /// Called when the assistant finishes a turn. Surface can flush
    /// the buffer, finalize the streaming file, send a "done" marker
    /// to a remote viewer, etc.
    fn on_turn_end(&mut self);

    /// Called when a tool starts / ends — the surface can render
    /// progress lines, write per-call artifacts, etc. Default impl
    /// is a no-op so surfaces that don't care don't have to override.
    fn on_tool_start(&mut self, _name: &str, _input_summary: &str) {}

    /// Called when a tool finishes.
    fn on_tool_end(&mut self, _name: &str, _is_error: bool, _duration_ms: u64) {}
}

/// One user turn as delivered by a surface. Carries the text plus an
/// optional reference id for correlation (coordinator chat threads
/// track per-request ids so the outbox message can reference the
/// right request).
#[derive(Debug, Clone)]
pub struct UserTurn {
    pub text: String,
    pub request_id: Option<String>,
}

impl UserTurn {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            request_id: None,
        }
    }

    pub fn with_request_id(text: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            request_id: Some(request_id.into()),
        }
    }
}

/// `ConversationSurface` impl that reads from a chat-request channel
/// and writes streaming output to the per-coordinator chat files.
/// This is what makes the native coordinator "just nex with a
/// different surface" — everything else in the conversation loop is
/// shared with `wg nex`.
///
/// Skeleton only: the channel drain is threaded so `next_user_input`
/// is actually async, but the full hook into `AgentLoop` (replacing
/// rustyline in `run_interactive`) is a separate step. Committed
/// alongside the trait so the next change has a concrete target.
pub struct ChatFileSurface {
    pub workgraph_dir: std::path::PathBuf,
    pub coordinator_id: u32,
    pub rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<ChatTurn>>,
    /// Accumulated streaming buffer for the current turn — on each
    /// chunk we append and rewrite the streaming file so the TUI can
    /// tail it without seeing partial UTF-8.
    streaming_buf: std::sync::Mutex<String>,
    /// The request_id of the in-flight turn, used to tag the outbox
    /// message when `on_turn_end` fires.
    current_request_id: std::sync::Mutex<Option<String>>,
}

/// Data-only shape mirroring `service::coordinator_agent::ChatRequest`
/// but defined here so the trait module doesn't depend on the service
/// layer. Callers adapt.
#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub request_id: String,
    pub message: String,
}

impl ChatFileSurface {
    pub fn new(
        workgraph_dir: std::path::PathBuf,
        coordinator_id: u32,
        rx: tokio::sync::mpsc::Receiver<ChatTurn>,
    ) -> Self {
        Self {
            workgraph_dir,
            coordinator_id,
            rx: tokio::sync::Mutex::new(rx),
            streaming_buf: std::sync::Mutex::new(String::new()),
            current_request_id: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl ConversationSurface for ChatFileSurface {
    async fn next_user_input(&mut self) -> Option<UserTurn> {
        let mut rx = self.rx.lock().await;
        let turn = rx.recv().await?;
        if let Ok(mut cur) = self.current_request_id.lock() {
            *cur = Some(turn.request_id.clone());
        }
        Some(UserTurn::with_request_id(turn.message, turn.request_id))
    }

    fn write_stream_chunk(&mut self, text: &str) {
        if let Ok(mut buf) = self.streaming_buf.lock() {
            buf.push_str(text);
            let _ = crate::chat::write_streaming(&self.workgraph_dir, self.coordinator_id, &buf);
        }
    }

    fn on_turn_end(&mut self) {
        // Flush the accumulated streaming buffer into the outbox as
        // the final assistant message for this request, then clear
        // the streaming file so the TUI moves on.
        let (buf, request_id) = {
            let b = self
                .streaming_buf
                .lock()
                .map(|s| s.clone())
                .unwrap_or_default();
            let r = self.current_request_id.lock().ok().and_then(|r| r.clone());
            (b, r)
        };
        if let Some(rid) = request_id
            && !buf.is_empty()
        {
            let _ = crate::chat::append_outbox_for(
                &self.workgraph_dir,
                self.coordinator_id,
                &buf,
                &rid,
            );
        }
        if let Ok(mut s) = self.streaming_buf.lock() {
            s.clear();
        }
        if let Ok(mut r) = self.current_request_id.lock() {
            *r = None;
        }
        crate::chat::clear_streaming(&self.workgraph_dir, self.coordinator_id);
    }
}
