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
//! | coordinator  | `chat/<ref>/inbox.jsonl`       | `chat/<ref>/streaming`         |
//! | `evaluate`   | eval prompt (one-shot)         | JSON record                    |
//!
//! The loop is identical. Stages A–G (cancel, inbox, microcompact,
//! L0 defense, idle watchdog, ...) all fire regardless of surface.
//! New features added to `AgentLoop` automatically benefit every role.
//!
//! Impl layout:
//!   * `ChatSurfaceState` (defined in `agent.rs` alongside the loop
//!     that uses it; it owns the inbox reader and the per-turn
//!     transcript buffer) is the one real implementation of this
//!     trait — used by the coordinator and task-agent paths.
//!   * Standalone `wg nex` (no `--chat`) deliberately does NOT use
//!     a surface. The agent loop reads from rustyline and streams
//!     to stderr inline (see `read_next_user_turn` in `agent.rs` and
//!     the on-stream / on-EndTurn handlers there). The trait was
//!     previously intended to cover that path too via a
//!     `TerminalSurface`, but it drifted into dead code — every
//!     terminal-UX change had to be made twice to keep both paths
//!     in sync. Now the inline path is canonical.
//!
//! See `docs/design/nex-as-coordinator.md` for the broader design.

use std::sync::Arc;

use async_trait::async_trait;

/// A pluggable input/output surface for an agent conversation.
///
/// Callers of `AgentLoop` pick a surface at startup; the loop then
/// uses it for all user interaction.
///
/// Thread model:
///   * Main-loop half (`next_user_input`, `on_turn_start`,
///     `on_turn_end`) is called sequentially from the single-owner
///     agent loop — `&mut self` is natural.
///   * Streaming half (`stream_sink`) returns an `Arc<dyn Fn(&str)>`
///     that the loop passes into the provider's streaming callback.
///     The sink uses interior mutability to accumulate per-turn
///     state (transcript buffer, streaming file) and is dropped at
///     turn end. One sink per turn; `stream_sink` is called after
///     `on_turn_start`.
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

    /// Called immediately after the loop receives a fresh user turn
    /// and before the first LLM call. The surface can reset its
    /// per-turn buffer, clear the streaming dotfile, etc. Default
    /// impl is a no-op for surfaces that keep no per-turn state.
    fn on_turn_start(&mut self, _request_id: Option<&str>) {}

    /// Called when the assistant finishes a turn (EndTurn stop
    /// reason, max-turns exit, cancel). Surface can flush its
    /// accumulated transcript to an outbox, clear the streaming
    /// dotfile, send a "done" marker to a remote viewer, etc.
    fn on_turn_end(&mut self);

    /// Produce a streaming sink for the current turn. Called once
    /// per LLM call within a turn; the sink is captured by the
    /// provider's streaming callback and invoked per text chunk.
    ///
    /// The sink uses interior mutability so the streaming closure
    /// (which is `Fn(String)`, not `FnMut`) can invoke it without
    /// needing a mutex at the call site. Implementations use
    /// `Arc<Mutex<...>>` internally for their transcript buffer.
    fn stream_sink(&self) -> Arc<dyn Fn(&str) + Send + Sync>;

    /// Called when a tool dispatch begins. Default impl is a no-op;
    /// ChatSurfaceState overrides this to render the opening of a
    /// tool "box" (┌─ Name ──── + `│ input` line) in the per-turn
    /// transcript.
    ///
    /// `input_summary` is a short one-line summary suitable for
    /// display (e.g. "pattern=foo" for grep, "$ ls" for bash). The
    /// full input JSON is also passed so surfaces that want it can
    /// render more detail.
    fn on_tool_start(&mut self, _name: &str, _input_summary: &str, _input: &serde_json::Value) {}

    /// Called as streaming tool output arrives (chunk by chunk,
    /// typically from `execute_batch_streaming`'s per-call callback).
    /// ChatSurfaceState overrides this to mirror the chunk into the
    /// transcript inside the current tool box (`│ ` prefix per line).
    fn on_tool_progress_chunk(&mut self, _chunk: &str) {}

    /// Return an Arc-friendly sink for tool-progress chunks. Used by
    /// the agent loop to capture the sink in an `Fn(String)` streaming
    /// callback that tokio tasks can invoke (where `&mut self` is not
    /// available). Default impl returns a no-op sink; ChatSurfaceState
    /// overrides to produce one that prefixes each line with `│ ` and
    /// mirrors to the per-session streaming file.
    fn tool_progress_sink(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        Arc::new(|_: &str| {})
    }

    /// Called when a tool call completes. `output` is the full
    /// content the model will see in the tool_result block; surfaces
    /// that render to a chat transcript use it to fill in the box.
    /// Default impl is a no-op.
    fn on_tool_end(&mut self, _name: &str, _output: &str, _is_error: bool, _duration_ms: u64) {}
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

/// Best-effort terminal width. Falls back to 80 when we can't
/// resolve it (non-Unix or no TTY).
#[allow(dead_code)]
fn terminal_cols() -> usize {
    #[cfg(unix)]
    {
        // SAFETY: writing to a zeroed winsize, passing its pointer
        // to ioctl(TIOCGWINSZ). The kernel fills cols/rows; we
        // only read the cols field.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return ws.ws_col as usize;
            }
        }
    }
    80
}
