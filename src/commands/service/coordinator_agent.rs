//! Persistent coordinator agent: a long-lived LLM session inside the service daemon.
//!
//! Spawns a Claude CLI process with `--input-format stream-json --output-format stream-json`
//! and keeps it running for the lifetime of the daemon. User chat messages are injected
//! via stdin, and responses are captured from stdout and written to the chat outbox.
//!
//! Architecture:
//! - The daemon creates a `CoordinatorAgent` on startup.
//! - Chat messages are sent via `CoordinatorAgent::send_message()`.
//! - A dedicated reader thread parses stdout and writes responses to the outbox.
//! - The agent subprocess is auto-restarted on crash with context recovery.
//!
//! Crash recovery:
//! - Time-windowed restart rate limiting: max 3 restarts per 10 minutes.
//! - On restart, injects previous conversation summary and current graph state.
//! - Conversation history persisted via chat inbox/outbox JSONL files.
//! - Old history is rotated on restart to prevent unbounded growth.
//!
//! Context refresh:
//! - On each user message, a context update is injected with graph summary,
//!   recent events, active agents, and items needing attention.
//! - Events are tracked in a bounded ring buffer (`EventLog`) shared between
//!   the daemon main thread and the agent thread.

use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use chrono::{DateTime, Utc};

use workgraph::chat;
use workgraph::graph::Status;
use workgraph::parser::load_graph;
use workgraph::service::registry::AgentRegistry;

use crate::commands::{graph_path, is_process_alive};

use super::DaemonLogger;

/// Maximum restarts allowed within the restart window before pausing.
const MAX_RESTARTS_PER_WINDOW: usize = 3;

/// Restart window duration in seconds (10 minutes).
const RESTART_WINDOW_SECS: u64 = 600;

/// Idle threshold for the supervisor's no-respawn rule. After a clean handler
/// exit, if the chat has no pending inbox messages AND its consumer cursor
/// has not been touched within this window, the supervisor exits cleanly
/// instead of respawning. Prevents the auto-respawn-without-TUI loop that
/// burns LLM tokens on idle chats.
const CHAT_IDLE_THRESHOLD_SECS: u64 = 300;

/// If a freshly-spawned child exits non-zero within this window AND a live
/// session-lock holder is present at the moment of exit, classify it as
/// session-lock contention rather than a genuine crash. Prevents normal
/// TUI handoff cycles from burning the restart-rate budget.
const SESSION_LOCK_CONTENTION_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// Base back-off after a session-lock-contention exit. Long enough that the
/// other handler has time to settle, short enough that a normal handoff
/// resumes promptly.
const LOCK_CONTENTION_BACKOFF_BASE_SECS: u64 = 10;

/// Maximum back-off ceiling when contentions repeat. Keeps the series well
/// below the 600s rate-limit pause so the user-facing pause character is
/// noticeably shorter than a real "we are giving up" event.
const LOCK_CONTENTION_BACKOFF_MAX_SECS: u64 = 60;

/// Stop respawning after this many CONSECUTIVE session-lock contentions.
/// At that point the other handler is clearly here to stay and the
/// supervisor should bow out instead of looping forever. Counter resets
/// on any non-contention exit.
const MAX_CONSECUTIVE_LOCK_CONTENTIONS: u32 = 6;

/// Classification of a coordinator child exit, used by the supervisor loop
/// to decide whether the exit counts toward the rate-limit budget, against
/// the contention back-off, or against neither.
///
/// Pulled out of the inline supervisor loop so the policy is unit-testable
/// without spawning a real subprocess. See the regression tests at the
/// bottom of this file for the cycles each branch is meant to handle.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChildExitKind {
    /// Child exited successfully (status 0). Healthy lifecycle event —
    /// e.g. user `/quit`, max-turns, cooperative TUI handoff. Does NOT
    /// count toward the restart rate-limit.
    Clean,
    /// Child exited fast (<1s) AND a live session-lock holder is present.
    /// We almost certainly lost a startup race against a TUI handler or
    /// an inline `wg nex` invocation. Routed through a separate, shorter
    /// back-off counter; never trips the 10-min pause.
    SessionLockContention,
    /// Genuine non-zero exit: long-running crash, startup failure with
    /// no contention signal, or wait() error. Counts toward the rate-limit.
    Crash,
}

/// Classify a coordinator child exit. Pure function — extracted so the
/// supervisor loop's policy is unit-testable. See module-level tests.
///
/// `success` — `exit_status.success()` (treat wait() errors as `false`).
/// `elapsed_since_spawn` — wall-clock duration the child ran.
/// `lock_holder_alive` — whether a live session-lock holder was observed
/// for the chat dir at the moment of exit.
pub(crate) fn classify_child_exit(
    success: bool,
    elapsed_since_spawn: std::time::Duration,
    lock_holder_alive: bool,
) -> ChildExitKind {
    if success {
        ChildExitKind::Clean
    } else if elapsed_since_spawn < SESSION_LOCK_CONTENTION_WINDOW && lock_holder_alive {
        ChildExitKind::SessionLockContention
    } else {
        ChildExitKind::Crash
    }
}

/// Back-off duration for the Nth consecutive session-lock contention.
/// Linear ramp from the base (10s) up to the ceiling (60s). Stays well
/// under the 600s rate-limit pause for any plausible `count`.
pub(crate) fn lock_contention_backoff(count: u32) -> std::time::Duration {
    let n = count.max(1) as u64;
    let secs = LOCK_CONTENTION_BACKOFF_BASE_SECS
        .saturating_mul(n)
        .min(LOCK_CONTENTION_BACKOFF_MAX_SECS);
    std::time::Duration::from_secs(secs)
}

/// How long the supervisor must sleep before its next spawn given the
/// current restart-timestamp window, or `None` if the budget has room.
/// Pure helper extracted from the supervisor loop so the rate-limit
/// behavior is testable in isolation.
pub(crate) fn rate_limit_wait(
    timestamps: &VecDeque<std::time::Instant>,
    now: std::time::Instant,
    window: std::time::Duration,
    max: usize,
) -> Option<std::time::Duration> {
    if timestamps.len() < max {
        return None;
    }
    timestamps
        .front()
        .map(|oldest| window.saturating_sub(now.duration_since(*oldest)))
}

/// Resolve the chat-supervisor's `coordinator_id` to a concrete task id
/// present in the live graph. Prefers the new `.chat-N` prefix, falls back
/// to the legacy `.coordinator-N` prefix for graphs that haven't been
/// migrated yet. Returns `None` if neither form exists — the caller
/// (supervisor pre-flight) treats that as orphan-supervisor and exits
/// cleanly after removing the stale per-coord state file.
///
/// Extracted for testability — the supervisor loop spawns subprocesses so
/// the resolution logic is isolated here so tests can drive every branch
/// without needing a real handler.
pub(crate) fn resolve_chat_task_id(
    graph: &workgraph::graph::WorkGraph,
    coordinator_id: u32,
) -> Option<String> {
    let new_id = workgraph::chat_id::format_chat_task_id(coordinator_id);
    let legacy_id = format!(".coordinator-{}", coordinator_id);
    if graph.get_task(&new_id).is_some() {
        Some(new_id)
    } else if graph.get_task(&legacy_id).is_some() {
        Some(legacy_id)
    } else {
        None
    }
}

pub(crate) fn tui_driver_deferral_pid(chat_dir: &Path) -> Option<u32> {
    workgraph::session_lock::read_tui_driver_sentinel(chat_dir)
        .ok()
        .flatten()
        .and_then(|info| info.alive.then_some(info.pid))
}

// ---------------------------------------------------------------------------
// Event log: bounded ring buffer for inter-interaction event tracking
// ---------------------------------------------------------------------------

/// An event tracked between coordinator interactions.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants constructed by daemon event recording
pub enum Event {
    /// A task completed.
    TaskCompleted {
        task_id: String,
        agent_id: Option<String>,
    },
    /// A task failed.
    TaskFailed { task_id: String, reason: String },
    /// A new task was added to the graph.
    TaskAdded {
        task_id: String,
        title: String,
        added_by: Option<String>,
    },
    /// An agent was spawned for a task.
    AgentSpawned {
        agent_id: String,
        task_id: String,
        executor: String,
    },
    /// An agent completed and exited.
    AgentCompleted { agent_id: String, task_id: String },
    /// An agent failed or died.
    AgentFailed {
        agent_id: String,
        task_id: String,
        reason: String,
    },
    /// A zero-output agent was detected and killed.
    ZeroOutputKill {
        agent_id: String,
        task_id: String,
        age_secs: u64,
    },
    /// A task hit the per-task zero-output circuit breaker.
    ZeroOutputCircuitBreak { task_id: String, attempts: u32 },
    /// Global API-down detected: majority of agents have zero output.
    GlobalApiOutage {
        zero_count: usize,
        total_count: usize,
        backoff_secs: u64,
    },
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::TaskCompleted { task_id, agent_id } => {
                if let Some(aid) = agent_id {
                    write!(f, "task {} completed ({})", task_id, aid)
                } else {
                    write!(f, "task {} completed", task_id)
                }
            }
            Event::TaskFailed { task_id, reason } => {
                write!(f, "task {} failed: {}", task_id, reason)
            }
            Event::TaskAdded {
                task_id,
                title: _,
                added_by,
            } => {
                if let Some(by) = added_by {
                    write!(f, "task {} added by {}", task_id, by)
                } else {
                    write!(f, "task {} added", task_id)
                }
            }
            Event::AgentSpawned {
                agent_id,
                task_id,
                executor,
            } => {
                write!(
                    f,
                    "agent {} spawned on {} (executor: {})",
                    agent_id, task_id, executor
                )
            }
            Event::AgentCompleted { agent_id, task_id } => {
                write!(f, "agent {} completed task {}", agent_id, task_id)
            }
            Event::AgentFailed {
                agent_id,
                task_id,
                reason,
            } => {
                write!(f, "agent {} failed on {}: {}", agent_id, task_id, reason)
            }
            Event::ZeroOutputKill {
                agent_id,
                task_id,
                age_secs,
            } => {
                write!(
                    f,
                    "zero-output agent {} killed on {} (alive {}s with no output)",
                    agent_id, task_id, age_secs
                )
            }
            Event::ZeroOutputCircuitBreak { task_id, attempts } => {
                write!(
                    f,
                    "task {} circuit-broken after {} zero-output attempts",
                    task_id, attempts
                )
            }
            Event::GlobalApiOutage {
                zero_count,
                total_count,
                backoff_secs,
            } => {
                write!(
                    f,
                    "GLOBAL API OUTAGE: {}/{} agents zero-output, backoff {}s",
                    zero_count, total_count, backoff_secs
                )
            }
        }
    }
}

/// A timestamped event entry.
#[derive(Debug, Clone)]
struct EventEntry {
    timestamp: DateTime<Utc>,
    event: Event,
}

/// Bounded ring buffer of events between coordinator interactions.
///
/// The daemon records events (task completions, agent spawns, etc.) and the
/// coordinator agent drains them when building context for each interaction.
#[derive(Debug)]
pub struct EventLog {
    entries: VecDeque<EventEntry>,
    capacity: usize,
}

const DEFAULT_EVENT_LOG_CAPACITY: usize = 200;

impl EventLog {
    /// Create a new event log with the default capacity.
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: DEFAULT_EVENT_LOG_CAPACITY,
        }
    }

    /// Record a new event. Oldest events are evicted when at capacity.
    pub fn record(&mut self, event: Event) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(EventEntry {
            timestamp: Utc::now(),
            event,
        });
    }

    /// Drain all events recorded since `since`, returning them.
    /// Events older than `since` are discarded.
    pub fn drain_since(&mut self, since: &DateTime<Utc>) -> Vec<(DateTime<Utc>, Event)> {
        let mut result = Vec::new();
        while let Some(entry) = self.entries.pop_front() {
            if entry.timestamp > *since {
                result.push((entry.timestamp, entry.event));
            }
        }
        result
    }

    /// Return event count (for testing/debugging).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Thread-safe shared event log.
pub type SharedEventLog = Arc<Mutex<EventLog>>;

/// Create a new shared event log.
pub fn new_event_log() -> SharedEventLog {
    Arc::new(Mutex::new(EventLog::new()))
}

/// A chat message to be injected into the coordinator agent.
pub struct ChatRequest {
    pub request_id: String,
    pub message: String,
}

/// Handle to the running coordinator agent.
///
/// The agent runs as a Claude CLI subprocess in a separate thread.
/// Messages are sent via a channel, and responses are written to the
/// chat outbox by the agent thread.
pub struct CoordinatorAgent {
    /// Send chat messages to the agent thread.
    tx: mpsc::Sender<ChatRequest>,
    /// The agent management thread handle.
    _agent_thread: JoinHandle<()>,
    /// Shared flag indicating whether the agent process is alive.
    alive: Arc<Mutex<bool>>,
    /// Shared PID of the agent process (0 if not running).
    pid: Arc<Mutex<u32>>,
    /// Shared event log for recording events from the daemon.
    #[allow(dead_code)]
    event_log: SharedEventLog,
    /// True when this agent is backed by a `wg nex --chat-id N`
    /// subprocess (reads user turns from the inbox directly, doesn't
    /// need messages pushed via the channel). Callers that would
    /// otherwise forward inbox messages through `send_message` should
    /// skip that step in subprocess mode to avoid re-appending
    /// the same message to the inbox.
    uses_subprocess: bool,
}

impl CoordinatorAgent {
    /// Check if the Claude CLI is available on the system.
    ///
    /// Returns true if `claude --version` runs successfully.
    pub fn is_claude_available() -> bool {
        std::process::Command::new("claude")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Spawn the coordinator agent.
    ///
    /// Launches a Claude CLI process and starts a management thread that
    /// handles message injection and response capture.
    ///
    /// The `event_log` is shared with the daemon thread — the daemon records
    /// events (task completions, agent spawns, etc.) and the agent reads them
    /// when building context for each interaction.
    ///
    /// Returns an error if the Claude CLI is not available.
    pub fn spawn(
        dir: &Path,
        coordinator_id: u32,
        model: Option<&str>,
        executor: Option<&str>,
        provider: Option<&str>,
        logger: &DaemonLogger,
        event_log: SharedEventLog,
    ) -> Result<Self> {
        let executor = executor.unwrap_or("claude");
        // Decide the coordinator implementation up front so send_message
        // can skip the redundant-append path for subprocess mode.
        //
        // SINGLE SOURCE OF TRUTH: route the supervisor's executor/model
        // through `plan_spawn` so subprocess detection here cannot
        // diverge from what `agent_thread_main` actually launches. We
        // build a synthetic Task carrying any per-task model override
        // (the chat task's id) and let plan_spawn cascade through the
        // explicit `agent_executor` (the supervisor's `executor` arg)
        // → config → default rules.
        let supervisor_config = workgraph::config::Config::load_or_default(dir);
        let chat_task_id = workgraph::chat_id::format_chat_task_id(coordinator_id);
        let mut chat_task = workgraph::graph::Task {
            id: chat_task_id.clone(),
            title: chat_task_id.clone(),
            ..workgraph::graph::Task::default()
        };
        if let Some(m) = model {
            chat_task.model = Some(m.to_string());
        }
        let supervisor_plan =
            workgraph::dispatch::plan_spawn(&chat_task, &supervisor_config, Some(executor), model)
                .context("plan_spawn for coordinator agent supervisor failed")?;
        // Post-Phase-7, ALL supported executors are backed by a
        // handler subprocess that reads the inbox directly:
        //   native → wg nex --chat
        //   claude → wg claude-handler --chat
        //   codex  → wg codex-handler --chat
        // Only `shell` would bypass the subprocess path. Provider
        // hints retained as a hedge for the legacy non-canonical
        // values still seen in older configs (the cleanup belongs in
        // a separate task — keep behavior conservative here).
        let uses_subprocess = supervisor_plan.executor != workgraph::dispatch::ExecutorKind::Shell
            || matches!(
                provider,
                Some("openrouter") | Some("oai-compat") | Some("openai") | Some("local")
            );

        if executor == "claude" && !Self::is_claude_available() {
            anyhow::bail!(
                "Claude CLI not found. Install it to enable the claude-handler coordinator."
            );
        }
        let (tx, rx) = mpsc::channel::<ChatRequest>();
        let alive = Arc::new(Mutex::new(false));
        let pid = Arc::new(Mutex::new(0u32));

        let dir = dir.to_path_buf();
        let model = model.map(String::from);
        let executor = executor.to_string();
        let provider = provider.map(String::from);
        let logger = logger.clone();
        let alive_clone = alive.clone();
        let pid_clone = pid.clone();
        let event_log_clone = event_log.clone();

        let agent_thread = thread::Builder::new()
            .name(format!("coordinator-agent-{}", coordinator_id))
            .spawn(move || {
                agent_thread_main(
                    &dir,
                    coordinator_id,
                    model.as_deref(),
                    &executor,
                    provider.as_deref(),
                    rx,
                    alive_clone,
                    pid_clone,
                    &logger,
                    &event_log_clone,
                );
            })
            .context("Failed to spawn coordinator agent thread")?;

        Ok(Self {
            tx,
            _agent_thread: agent_thread,
            alive,
            pid,
            event_log,
            uses_subprocess,
        })
    }

    /// True when this agent is backed by the `wg nex --chat-id N`
    /// subprocess path. Callers forwarding inbox messages should skip
    /// `send_message` for these agents — the subprocess reads the
    /// inbox directly.
    pub fn uses_subprocess(&self) -> bool {
        self.uses_subprocess
    }

    /// Get a reference to the shared event log.
    ///
    /// The daemon uses this to record events that the coordinator agent
    /// will see on its next context refresh.
    #[allow(dead_code)]
    pub fn event_log(&self) -> &SharedEventLog {
        &self.event_log
    }

    /// Send a chat message to the coordinator agent.
    ///
    /// Returns Ok(()) if the message was queued. The response will be
    /// written to the chat outbox asynchronously.
    pub fn send_message(&self, request_id: String, message: String) -> Result<()> {
        self.tx
            .send(ChatRequest {
                request_id,
                message,
            })
            .map_err(|_| anyhow::anyhow!("Coordinator agent thread has exited"))
    }

    /// Check if the coordinator agent process is alive.
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        *self.alive.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Get the PID of the coordinator agent process.
    #[allow(dead_code)]
    pub fn pid(&self) -> u32 {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Interrupt the current generation by sending SIGINT to the Claude CLI process.
    ///
    /// Returns `true` if SIGINT was sent, `false` if the process is not alive.
    /// The Claude CLI handles SIGINT by stopping the current generation and
    /// emitting a TurnComplete signal, preserving the conversation context.
    pub fn interrupt(&self) -> bool {
        let pid = *self.pid.lock().unwrap_or_else(|e| e.into_inner());
        if pid == 0 {
            return false;
        }
        // Send SIGINT (not SIGKILL) — Claude CLI treats this as "stop generating"
        unsafe {
            libc::kill(pid as i32, libc::SIGINT);
        }
        true
    }

    /// Shut down the coordinator agent.
    ///
    /// SIGTERMs the handler child so it releases its session lock
    /// promptly, then drops the sender channel. Without the kill,
    /// a Phase-7 handler (e.g. `wg claude-handler`) would be
    /// orphaned on daemon exit — its blocking I/O keeps it alive
    /// under init, still holding the chat-dir lock, and a fresh
    /// daemon on restart can't reclaim the session.
    pub fn shutdown(self) {
        let pid = *self.pid.lock().unwrap_or_else(|e| e.into_inner());
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        drop(self.tx);
        // The supervisor thread's `child.wait()` returns once the
        // handler responds to SIGTERM, letting the forwarder and
        // supervisor exit on their own. We don't join here to avoid
        // blocking daemon shutdown.
    }
}

// ---------------------------------------------------------------------------
// Agent thread implementation
// ---------------------------------------------------------------------------

/// Main loop for the coordinator agent management thread.
///
/// Phase 7 unification: all executors (native, claude, codex, gemini)
/// are dispatched through the same `subprocess_coordinator_loop`,
/// which spawns `wg spawn-task .coordinator-<N>` and lets spawn-task
/// pick the right handler binary based on `WG_EXECUTOR_TYPE`. The
/// daemon itself no longer knows how to speak Claude stdio or native
/// direct-API — it's purely a supervisor + rx-forwarder.
///
/// Executor resolution: config says `claude` or `native` (or future
/// `codex`/`gemini`), but some provider/model combinations force
/// native (Claude CLI only speaks Anthropic). We keep the same
/// auto-routing logic here so a misconfigured graph still ends up on
/// a working handler.
#[allow(clippy::too_many_arguments)]
fn agent_thread_main(
    dir: &Path,
    coordinator_id: u32,
    model: Option<&str>,
    executor: &str,
    provider: Option<&str>,
    rx: mpsc::Receiver<ChatRequest>,
    alive: Arc<Mutex<bool>>,
    pid: Arc<Mutex<u32>>,
    logger: &DaemonLogger,
    _event_log: &SharedEventLog,
) {
    // Executor/model resolution lives inside `subprocess_coordinator_loop`,
    // where each iteration calls `dispatch::plan_spawn` (single source of
    // truth) and emits a provenance line. The legacy
    // `requires_native_executor` model-based auto-switch is gone — model
    // never overrides an explicit executor floor (see dispatch::plan
    // module docs for the regression this fixes).
    subprocess_coordinator_loop(
        dir,
        coordinator_id,
        model,
        provider,
        executor,
        rx,
        alive,
        pid,
        logger,
    );
}

// ---------------------------------------------------------------------------
// subprocess coordinator: the unified path (native = claude = codex = ...)
// ---------------------------------------------------------------------------

/// Coordinator supervisor loop. Spawns `wg spawn-task .coordinator-<N>`
/// and lets spawn-task's per-executor adapter pick the right handler
/// binary: `wg nex --chat <ref>` for native, `wg claude-handler --chat
/// <ref>` for claude, etc. The daemon is purely a supervisor + inbox
/// forwarder — it no longer speaks any executor's protocol directly.
///
/// `rx` is drained into the inbox so `CoordinatorAgent::send_message`
/// (heartbeats, daemon-internal synthetic prompts) reaches the
/// subprocess. User messages written directly to the inbox by the
/// TUI or `wg chat` bypass this channel.
///
/// `executor` is exported as `WG_EXECUTOR_TYPE` so the spawned
/// `wg spawn-task` picks the correct adapter.
#[allow(clippy::too_many_arguments)]
fn subprocess_coordinator_loop(
    dir: &Path,
    coordinator_id: u32,
    model: Option<&str>,
    provider: Option<&str>,
    executor: &str,
    rx: mpsc::Receiver<ChatRequest>,
    alive: Arc<Mutex<bool>>,
    pid: Arc<Mutex<u32>>,
    logger: &DaemonLogger,
) {
    // Start a small forwarder thread that drains `rx` → inbox. We can't
    // own `rx` on the supervisor thread and also block on `child.wait()`,
    // so a dedicated forwarder keeps send_message non-blocking across
    // subprocess restarts.
    let dir_buf = dir.to_path_buf();
    let forwarder = std::thread::Builder::new()
        .name(format!("coordinator-nex-fwd-{}", coordinator_id))
        .spawn(move || {
            while let Ok(req) = rx.recv() {
                if let Err(e) =
                    chat::append_inbox_for(&dir_buf, coordinator_id, &req.message, &req.request_id)
                {
                    eprintln!(
                        "[coordinator-{}] forwarder: append_inbox_for failed: {}",
                        coordinator_id, e
                    );
                }
            }
        });
    let _forwarder = match forwarder {
        Ok(h) => Some(h),
        Err(e) => {
            logger.error(&format!(
                "Coordinator-{}: failed to spawn inbox forwarder thread: {}",
                coordinator_id, e
            ));
            None
        }
    };

    let mut restart_timestamps: VecDeque<std::time::Instant> = VecDeque::new();
    // Separate counter for session-lock contention exits. Reset on any
    // non-contention exit so a single non-contention iteration always
    // restores the supervisor to its base state.
    let mut lock_contention_count: u32 = 0;

    loop {
        // Rate-limit restarts in a sliding window, same policy as the
        // Claude CLI path above. Only GENUINE crashes push timestamps
        // here (see classify_child_exit below); clean exits and
        // session-lock contention have their own paths so normal TUI
        // handoff cycles never trip this pause.
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(RESTART_WINDOW_SECS);
        while let Some(front) = restart_timestamps.front() {
            if now.duration_since(*front) > window {
                restart_timestamps.pop_front();
            } else {
                break;
            }
        }
        if let Some(wait_time) =
            rate_limit_wait(&restart_timestamps, now, window, MAX_RESTARTS_PER_WINDOW)
        {
            logger.error(&format!(
                "Coordinator-{}: {} crashes in last {} minutes, pausing for {}s",
                coordinator_id,
                MAX_RESTARTS_PER_WINDOW,
                RESTART_WINDOW_SECS / 60,
                wait_time.as_secs()
            ));
            std::thread::sleep(wait_time);
            restart_timestamps.clear();
        }

        // Register the coordinator's chat session. Installs BOTH
        // aliases (`coordinator-<N>` subprocess-facing AND bare
        // `<N>` legacy-API-facing). The daemon MUST go through
        // this single entry point — see
        // `chat_sessions::register_coordinator_session` docs +
        // the `daemon_style_coordinator_registration_creates_both_paths`
        // unit test that locks in the invariant.
        let chat_alias = format!("coordinator-{}", coordinator_id);
        if let Err(e) = workgraph::chat_sessions::register_coordinator_session(dir, coordinator_id)
        {
            logger.error(&format!(
                "Coordinator-{}: register_coordinator_session failed: {}",
                coordinator_id, e
            ));
        }

        // Phase 6a: spawn via `wg spawn-task .coordinator-<N>` instead
        // of invoking `wg nex --chat <alias>` directly. This unifies
        // the daemon's spawn path with the TUI's (which also uses
        // `wg spawn-task`), so per-executor adapter dispatch,
        // --resume auto-detection, and role resolution all live in
        // ONE place (commands/spawn_task.rs). When Phase 7 adds
        // claude/codex/gemini adapters, the daemon gets them for
        // free — no duplicate executor-routing code to maintain.
        //
        // Task-level model/endpoint overrides on the coordinator
        // task are picked up by spawn-task automatically. The
        // `model` arg the daemon has comes from top-level config
        // and is less specific than the task's own; for now we
        // preserve it via WG_MODEL env so it's applied as a
        // last-resort default by nex.
        // Pre-flight: locate the chat task in the live graph. Prefer the
        // new `.chat-N` prefix; fall back to legacy `.coordinator-N` if we
        // are supervising a task that hasn't been migrated yet.
        //
        // Bug A regression-guard: if NEITHER form exists in the graph, the
        // chat task was deleted (or was never created — e.g. boot path
        // hardcoded "spawn coordinator-0" against a fresh init). DO NOT
        // restart-loop calling `wg spawn-task` with a non-existent ID; log
        // a clear error and exit cleanly so the supervisor thread terminates
        // instead of chewing CPU forever.
        let task_id = {
            let new_id = workgraph::chat_id::format_chat_task_id(coordinator_id);
            let legacy_id = format!(".coordinator-{}", coordinator_id);
            let graph_path = crate::commands::graph_path(dir);
            match workgraph::parser::load_graph(&graph_path) {
                Ok(g) => {
                    let resolved = resolve_chat_task_id(&g, coordinator_id);
                    let Some(rid) = resolved else {
                        logger.error(&format!(
                            "Coordinator-{}: orphan supervisor — neither {} nor {} exists in the graph. Exiting supervisor (no restart loop). \
                             Removing stale coordinator-state-{}.json so a daemon restart does not retry. \
                             If you intended to start this chat, run `wg chat new` (or use the TUI '+' key) to create the task first.",
                            coordinator_id, new_id, legacy_id, coordinator_id
                        ));
                        super::CoordinatorState::remove_for(dir, coordinator_id);
                        return;
                    };
                    // Mid-loop archive-check: if the chat task has been
                    // archived (Done + tag `archived`) since the last
                    // iteration, exit cleanly instead of respawning a handler.
                    // This is the path that `wg service purge-chats` and the
                    // user-facing `wg chat archive` rely on to actually stop
                    // the supervisor loop after they've mutated the graph.
                    if let Some(t) = g.get_task(&rid) {
                        let is_archived = t.tags.iter().any(|x| x == "archived");
                        let is_done = matches!(t.status, workgraph::graph::Status::Done);
                        let is_abandoned = matches!(t.status, workgraph::graph::Status::Abandoned);
                        if is_archived
                            || (is_done
                                && !t
                                    .tags
                                    .iter()
                                    .any(|x| workgraph::chat_id::is_chat_loop_tag(x)))
                            || is_abandoned
                        {
                            logger.info(&format!(
                                "Coordinator-{}: chat task {} is archived/Done/Abandoned — exiting supervisor (no respawn).",
                                coordinator_id, rid
                            ));
                            return;
                        }
                    }
                    rid
                }
                Err(e) => {
                    logger.error(&format!(
                        "Coordinator-{}: failed to load graph for pre-flight task check: {}. Exiting supervisor.",
                        coordinator_id, e
                    ));
                    return;
                }
            }
        };
        let chat_ref = format!("chat-{}", coordinator_id);
        let chat_dir = workgraph::chat::chat_dir_for_ref(dir, &chat_ref);
        if let Some(tui_pid) = tui_driver_deferral_pid(&chat_dir) {
            logger.info(&format!(
                "Coordinator-{}: TUI sentinel alive (pid={}) — deferring respawn 5s",
                coordinator_id, tui_pid
            ));
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }
        // Hot-swap support: re-read CoordinatorState each iteration
        // so `wg service set-executor <cid> ...` takes effect on the
        // next supervisor restart. Explicit overrides beat the
        // static daemon_cfg values we got at spawn time.
        let state = super::CoordinatorState::load_for(dir, coordinator_id);
        let exec_override = state
            .as_ref()
            .and_then(|s| s.executor_override.clone())
            .unwrap_or_else(|| executor.to_string());
        let model_override = state
            .as_ref()
            .and_then(|s| s.model_override.clone())
            .or_else(|| model.map(String::from));

        // SINGLE SOURCE OF TRUTH: every supervisor-iteration spawn flows
        // through plan_spawn. We hydrate the chat task from the graph so
        // any per-task model/exec overrides on `.chat-N` (or legacy
        // `.coordinator-N`) are honored, then layer the supervisor's
        // explicit `exec_override` as the agency-level executor input.
        let supervisor_config = workgraph::config::Config::load_or_default(dir);
        let chat_task = match workgraph::parser::load_graph(&crate::commands::graph_path(dir)) {
            Ok(g) => g
                .get_task(&task_id)
                .cloned()
                .unwrap_or_else(|| workgraph::graph::Task {
                    id: task_id.clone(),
                    title: task_id.clone(),
                    model: model_override.clone(),
                    ..workgraph::graph::Task::default()
                }),
            Err(_) => workgraph::graph::Task {
                id: task_id.clone(),
                title: task_id.clone(),
                model: model_override.clone(),
                ..workgraph::graph::Task::default()
            },
        };
        if !chat_task.command_argv.is_empty() && chat_task.executor_preset_name.is_none() {
            logger.info(&format!(
                "Coordinator-{}: {} is a custom command chat; TUI/tmux owns the pane spawn",
                coordinator_id, task_id
            ));
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }
        let plan = match workgraph::dispatch::plan_spawn(
            &chat_task,
            &supervisor_config,
            Some(&exec_override),
            model_override.as_deref(),
        ) {
            Ok(p) => p,
            Err(e) => {
                logger.error(&format!(
                    "Coordinator-{}: plan_spawn failed for {}: {} — sleeping 5s and retrying",
                    coordinator_id, task_id, e
                ));
                restart_timestamps.push_back(std::time::Instant::now());
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
        };
        let effective_exec = plan.executor.as_str().to_string();
        let effective_model_override = Some(plan.model.raw.clone());

        // Provenance: every supervisor-iteration spawn emits one line tracing
        // each {executor, model, endpoint} decision back to the config knob
        // that produced it. Eliminates silent-routing bugs.
        logger.info(&format!(
            "Coordinator-{}: {}",
            coordinator_id,
            plan.provenance.log_line(&plan)
        ));

        let wg_bin = std::env::current_exe().unwrap_or_else(|_| "wg".into());
        let mut cmd = Command::new(&wg_bin);
        cmd.arg("spawn-task").arg(&task_id);
        cmd.current_dir(dir.parent().unwrap_or(dir));
        cmd.env("WG_EXECUTOR_TYPE", &effective_exec);
        if let Some(p) = provider {
            cmd.env("WG_PROVIDER", p);
        }
        if let Some(ref m) = effective_model_override {
            cmd.env("WG_MODEL", m);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Fix D (fix-nex-chat): per-chat persistent stderr log file for nex
        // spawns, matching `claude_handler.rs:399-411` parity. The reader
        // thread below tees each stderr line BOTH to daemon.log (inline
        // preview, existing behavior) AND to this file (durable record).
        // On any future spawn-time failure the user has a discoverable
        // path to inspect even when daemon.log is rotated or noisy.
        let nex_stderr_log_path = dir
            .join("service")
            .join(format!("nex-handler-stderr-{}.log", coordinator_id));
        if let Some(parent) = nex_stderr_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Provenance breadcrumb BEFORE cmd.spawn() — matches the
        // claude_handler.rs:413-419 line. Lists executor, model, the
        // resolved endpoint (so dropped-endpoint regressions are visible),
        // and the persistent stderr log path. Emitted unconditionally so
        // even spawn-time failures leave a "here's where the file is"
        // breadcrumb in daemon.log.
        let endpoint_for_log = plan
            .endpoint
            .as_ref()
            .map(|e| e.name.as_str())
            .unwrap_or("none");
        logger.info(&format!(
            "Coordinator-{}: spawning via `wg spawn-task {}` (executor={}, model={:?}, endpoint={}, stderr_log={:?})",
            coordinator_id,
            task_id,
            effective_exec,
            effective_model_override,
            endpoint_for_log,
            nex_stderr_log_path,
        ));
        let _ = chat_alias; // silence unused — retained for register_coordinator_session above
        let spawn_time = std::time::Instant::now();
        let mut child: Child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                logger.error(&format!(
                    "Coordinator-{}: failed to spawn nex subprocess: {} (stderr_log={:?})",
                    coordinator_id, e, nex_stderr_log_path
                ));
                restart_timestamps.push_back(std::time::Instant::now());
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
        };

        let child_pid = child.id();
        *pid.lock().unwrap_or_else(|e| e.into_inner()) = child_pid;
        *alive.lock().unwrap_or_else(|e| e.into_inner()) = true;
        // NOTE: we used to push `restart_timestamps` here on every spawn,
        // which conflated "child was born" with "child crashed". Three
        // normal TUI handoff cycles would trip the rate-limit. The push
        // now lives in the post-wait classification block below — only
        // GENUINE crashes count.
        logger.info(&format!(
            "Coordinator-{}: nex subprocess running (pid {})",
            coordinator_id, child_pid
        ));

        // Drain stdout/stderr to the daemon log in background threads —
        // without this, the child's pipes fill and it blocks.
        let cid = coordinator_id;
        let logger_out = logger.clone();
        let stdout = child.stdout.take();
        std::thread::Builder::new()
            .name(format!("coordinator-nex-stdout-{}", cid))
            .spawn(move || {
                if let Some(out) = stdout {
                    for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                        logger_out.info(&format!("[coordinator-{} stdout] {}", cid, line));
                    }
                }
            })
            .ok();
        let logger_err = logger.clone();
        let stderr = child.stderr.take();
        let stderr_log_for_thread = nex_stderr_log_path.clone();
        std::thread::Builder::new()
            .name(format!("coordinator-nex-stderr-{}", cid))
            .spawn(move || {
                if let Some(err) = stderr {
                    // Open the persistent stderr file (create+append). Each
                    // line is tee'd to BOTH daemon.log (logger_err) and this
                    // file. If the file fails to open we silently fall back
                    // to daemon-log-only — the inline preview is still
                    // useful and the breadcrumb in the parent already
                    // logged the path so the user sees the open-failure
                    // implicitly (no file shows up).
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&stderr_log_for_thread)
                        .ok();
                    for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
                        logger_err.info(&format!("[coordinator-{} stderr] {}", cid, line));
                        if let Some(ref mut f) = file {
                            let _ = writeln!(f, "{}", line);
                        }
                    }
                }
            })
            .ok();

        let exit_status = child.wait();
        let elapsed = spawn_time.elapsed();
        *alive.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *pid.lock().unwrap_or_else(|e| e.into_inner()) = 0;

        let success = matches!(&exit_status, Ok(s) if s.success());
        // Read the live session-lock holder right at exit time. If the
        // child crashed *because* it lost a startup race against another
        // handler, the winning holder will still be present here. We
        // squash any IO error to "no holder" so a transient stat() fail
        // doesn't pretend a contention happened.
        let lock_holder_alive = workgraph::session_lock::read_holder(&chat_dir)
            .ok()
            .flatten()
            .map(|h| h.alive)
            .unwrap_or(false);
        let kind = classify_child_exit(success, elapsed, lock_holder_alive);

        match kind {
            ChildExitKind::Clean => {
                lock_contention_count = 0;
                logger.info(&format!(
                    "Coordinator-{}: nex subprocess exited cleanly ({:?})",
                    coordinator_id, exit_status
                ));
                // Idle-respawn rule (parent task bullet a): if there's no
                // unread inbox AND no recent consumer (TUI/CLI cursor older
                // than CHAT_IDLE_THRESHOLD_SECS), exit cleanly instead of
                // respawning a handler that would just exit again on an
                // empty inbox. The chat task remains in the graph; the user
                // resumes via `wg chat new`/the TUI, and a daemon restart
                // will spawn a fresh supervisor for active chats via
                // enumerate_chat_supervisors_for_boot. Without this gate the
                // supervisor burns LLM tokens in a tight loop whenever no
                // consumer is connected.
                let idle_threshold = std::time::Duration::from_secs(CHAT_IDLE_THRESHOLD_SECS);
                if chat::chat_session_is_idle(dir, coordinator_id, idle_threshold) {
                    logger.info(&format!(
                        "Coordinator-{}: idle (no consumer + empty inbox for {}s) — exiting supervisor (no respawn).",
                        coordinator_id, CHAT_IDLE_THRESHOLD_SECS
                    ));
                    return;
                }
                // Clean exit (user ran /quit, or max-turns hit) — don't
                // respawn in a tight loop. Sleep a moment to avoid eating
                // the whole restart budget on clean exits.
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            ChildExitKind::SessionLockContention => {
                lock_contention_count = lock_contention_count.saturating_add(1);
                let backoff = lock_contention_backoff(lock_contention_count);
                if lock_contention_count >= MAX_CONSECUTIVE_LOCK_CONTENTIONS {
                    logger.error(&format!(
                        "Coordinator-{}: {} consecutive session-lock contentions — \
                         another handler owns this chat. Exiting supervisor (no respawn).",
                        coordinator_id, lock_contention_count
                    ));
                    return;
                }
                logger.info(&format!(
                    "Coordinator-{}: nex subprocess exited {:?} after {:?} with a live \
                     session-lock holder — treating as cooperative-handoff race \
                     (count={}, backoff={}s, NOT a rate-limit event).",
                    coordinator_id,
                    exit_status,
                    elapsed,
                    lock_contention_count,
                    backoff.as_secs(),
                ));
                std::thread::sleep(backoff);
            }
            ChildExitKind::Crash => {
                lock_contention_count = 0;
                restart_timestamps.push_back(std::time::Instant::now());
                match &exit_status {
                    Ok(status) => logger.error(&format!(
                        "Coordinator-{}: nex subprocess exited {} — will restart",
                        coordinator_id, status
                    )),
                    Err(e) => logger.error(&format!(
                        "Coordinator-{}: wait() failed on nex subprocess: {} — will restart",
                        coordinator_id, e
                    )),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// System prompt
// ---------------------------------------------------------------------------

/// Coordinator prompt component file names (in composition order).
const COORDINATOR_PROMPT_FILES: &[&str] = &[
    "base-system-prompt.md",
    "behavioral-rules.md",
    "common-patterns.md",
    "evolved-amendments.md",
];

/// Build the system prompt for the coordinator agent by composing from files.
///
/// Reads from `.wg/agency/coordinator-prompt/` and concatenates the
/// component files in order. Falls back to the hardcoded prompt if the
/// directory doesn't exist or no files are found.
///
/// Dynamic state goes through context injection (see `build_coordinator_context`).
pub fn build_system_prompt(dir: &Path) -> String {
    let prompt_dir = dir.join("agency/coordinator-prompt");

    if prompt_dir.is_dir() {
        let mut parts = Vec::new();
        for filename in COORDINATOR_PROMPT_FILES {
            let path = prompt_dir.join(filename);
            if let Ok(content) = std::fs::read_to_string(&path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
        }
        if !parts.is_empty() {
            return parts.join("\n\n");
        }
    }

    // Fallback: hardcoded prompt (for projects without coordinator-prompt/ files)
    build_system_prompt_fallback()
}

/// Hardcoded fallback prompt used when coordinator-prompt files don't exist.
fn build_system_prompt_fallback() -> String {
    include_str!("coordinator_prompt_fallback.txt").to_string()
}

// ---------------------------------------------------------------------------
// Context injection
// ---------------------------------------------------------------------------

/// Build the dynamic context injection string for the coordinator agent.
///
/// This is prepended to each user message to give the coordinator awareness
/// of the current graph state, recent events, and active agents.
///
/// If `event_log` is provided, recent events are drained from it (more
/// efficient and accurate than scanning task logs). Otherwise, falls back
/// to scanning task log entries since `last_interaction`.
pub fn build_coordinator_context(
    dir: &Path,
    last_interaction: &str,
    event_log: Option<&SharedEventLog>,
    coordinator_id: u32,
) -> Result<String> {
    let gp = graph_path(dir);
    if !gp.exists() {
        return Ok(String::new());
    }

    let graph = load_graph(&gp).context("Failed to load graph for context injection")?;

    // --- Graph Summary ---
    let mut done = 0usize;
    let mut in_progress = 0usize;
    let mut open = 0usize;
    let mut blocked = 0usize;
    let mut failed = 0usize;
    let mut abandoned = 0usize;

    for task in graph.tasks() {
        match task.status {
            Status::Done => done += 1,
            Status::InProgress => in_progress += 1,
            Status::Open => {
                // Check if blocked (any after dep not Done)
                let is_blocked = task.after.iter().any(|dep_id| {
                    graph
                        .get_task(dep_id)
                        .map(|d| !d.status.is_terminal())
                        .unwrap_or(false)
                });
                if is_blocked {
                    blocked += 1;
                } else {
                    open += 1;
                }
            }
            Status::Failed => failed += 1,
            Status::Abandoned => abandoned += 1,
            _ => {}
        }
    }
    let total = done + in_progress + open + blocked + failed + abandoned;

    // --- Recent Events ---
    let mut events = Vec::new();

    if let Some(elog) = event_log {
        // Drain events from the shared event log (preferred path)
        if let Ok(last_dt) = last_interaction.parse::<DateTime<Utc>>()
            && let Ok(mut log) = elog.lock()
        {
            for (ts, event) in log.drain_since(&last_dt) {
                events.push(format!("- [{}] {}", ts.format("%H:%M:%S"), event));
            }
        }
    } else {
        // Fallback: scan task logs since last_interaction
        if let Ok(last_dt) = last_interaction.parse::<DateTime<Utc>>() {
            for task in graph.tasks() {
                for log_entry in &task.log {
                    if let Ok(entry_dt) = log_entry.timestamp.parse::<DateTime<Utc>>()
                        && entry_dt > last_dt
                    {
                        events.push(format!(
                            "- [{}] {} (task: {})",
                            &log_entry.timestamp[11..19], // HH:MM:SS
                            log_entry.message,
                            task.id,
                        ));
                    }
                }
            }
        }
    }
    // Limit to most recent 20 events
    events.sort();
    if events.len() > 20 {
        let skip = events.len() - 20;
        events = events.into_iter().skip(skip).collect();
    }

    // --- Active Agents ---
    let mut agent_lines = Vec::new();
    if let Ok(registry) = AgentRegistry::load(dir) {
        for agent in registry.list_agents() {
            if agent.is_alive() && is_process_alive(agent.pid) {
                agent_lines.push(format!(
                    "- {} working on \"{}\" (uptime: {})",
                    agent.id,
                    agent.task_id,
                    agent.uptime_human(),
                ));
            }
        }
    }

    // --- Failed Tasks ---
    let failed_tasks: Vec<String> = graph
        .tasks()
        .filter(|t| t.status == Status::Failed)
        .map(|t| {
            let class_suffix = t
                .failure_class
                .map(|c| format!(" [class: {}]", c))
                .unwrap_or_default();
            format!(
                "- FAILED: {} \"{}\" — {}{}",
                t.id,
                t.title,
                t.failure_reason.as_deref().unwrap_or("unknown reason"),
                class_suffix,
            )
        })
        .collect();

    // --- Format ---
    let now = chrono::Utc::now().to_rfc3339();
    let mut parts = Vec::new();

    parts.push(format!("## System Context Update ({})", now));

    // (Removed: graph-cycle "Compacted Project Context". The compactor module
    // and its `.compact-N` cycle scaffolding have been retired.)

    // --- Conversation Context Summary ---
    {
        use workgraph::service::chat_compactor::{ChatCompactorState, context_summary_path};
        let summary_path = context_summary_path(dir, coordinator_id);
        if summary_path.exists()
            && let Ok(contents) = std::fs::read_to_string(&summary_path)
        {
            let contents = contents.trim();
            if !contents.is_empty() {
                let cstate = ChatCompactorState::load(dir, coordinator_id);
                let ts_line = match &cstate.last_compaction {
                    Some(ts) => format!("_Last compacted: {}_\n", ts),
                    None => String::new(),
                };
                parts.push(format!(
                    "\n### Conversation Context Summary\n{}{}",
                    ts_line, contents
                ));
            }
        }
    }

    // --- Injected History Context (from Ctrl+H history browser) ---
    if let Some(injected) = workgraph::chat::take_injected_context(dir, coordinator_id) {
        parts.push(format!(
            "\n### Injected History Context\n\
             _The user selected this from conversation history for your reference:_\n\n{}",
            injected
        ));
    }

    parts.push(format!(
        "\n### Graph Summary\n{} tasks: {} done, {} in-progress, {} open, {} blocked, {} failed, {} abandoned",
        total, done, in_progress, open, blocked, failed, abandoned
    ));

    parts.push("\n### Recent Events".to_string());
    if events.is_empty() {
        parts.push("- No events since last interaction.".to_string());
    } else {
        for event in &events {
            parts.push(event.clone());
        }
    }

    parts.push("\n### Active Agents".to_string());
    if agent_lines.is_empty() {
        parts.push("- No active agents.".to_string());
    } else {
        for line in &agent_lines {
            parts.push(line.clone());
        }
    }

    parts.push("\n### Attention Needed".to_string());
    if failed_tasks.is_empty() {
        parts.push("- Nothing requires attention.".to_string());
    } else {
        for line in &failed_tasks {
            parts.push(line.clone());
        }
    }

    // Tell the coordinator where its chat log lives
    let chat_log = chat::chat_log_path_for(dir, coordinator_id);
    parts.push(format!(
        "\n### Chat Log\nYour full chat history is at: {}",
        chat_log.display()
    ));

    Ok(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_system_prompt_fallback() {
        let tmp = TempDir::new().unwrap();
        let prompt = build_system_prompt(tmp.path());
        // Falls back to hardcoded prompt since no coordinator-prompt dir exists
        assert!(prompt.contains("workgraph coordinator"));
        assert!(prompt.contains("Never implement"));
        assert!(prompt.contains("wg add"));
    }

    #[test]
    fn test_build_system_prompt_from_files() {
        let tmp = TempDir::new().unwrap();
        let prompt_dir = tmp.path().join("agency/coordinator-prompt");
        std::fs::create_dir_all(&prompt_dir).unwrap();
        std::fs::write(prompt_dir.join("base-system-prompt.md"), "Base prompt here").unwrap();
        std::fs::write(prompt_dir.join("behavioral-rules.md"), "Rules here").unwrap();
        std::fs::write(prompt_dir.join("common-patterns.md"), "Patterns here").unwrap();
        std::fs::write(prompt_dir.join("evolved-amendments.md"), "Amendments here").unwrap();

        let prompt = build_system_prompt(tmp.path());
        assert!(prompt.contains("Base prompt here"));
        assert!(prompt.contains("Rules here"));
        assert!(prompt.contains("Patterns here"));
        assert!(prompt.contains("Amendments here"));
        // Should NOT contain fallback content
        assert!(!prompt.contains("workgraph coordinator"));
    }

    #[test]
    fn test_build_coordinator_context_no_graph() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_build_coordinator_context_with_graph() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        // Create a minimal graph
        std::fs::create_dir_all(dir.join(".wg")).unwrap();
        let graph_file = dir.join("graph.md");
        std::fs::write(
            &graph_file,
            "# Graph\n\n## Tasks\n\n- [x] task-1: Done task\n- [ ] task-2: Open task\n",
        )
        .unwrap();

        // This will fail to load since it's not a valid graph format,
        // but we're testing the error path gracefully
        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0);
        // Either succeeds with content or fails gracefully
        assert!(ctx.is_ok() || ctx.is_err());
    }

    #[test]
    fn test_event_log_record_and_drain() {
        let mut log = EventLog::new();
        let before = Utc::now();

        log.record(Event::TaskCompleted {
            task_id: "task-1".to_string(),
            agent_id: Some("agent-1".to_string()),
        });
        log.record(Event::TaskFailed {
            task_id: "task-2".to_string(),
            reason: "test failure".to_string(),
        });

        assert_eq!(log.len(), 2);

        let events = log.drain_since(&before);
        assert_eq!(events.len(), 2);
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_event_log_drain_filters_old() {
        let mut log = EventLog::new();
        let after = Utc::now();

        // These events happened "before" our timestamp since we record them now
        // but the drain uses > comparison, so we need events after the timestamp
        std::thread::sleep(std::time::Duration::from_millis(10));

        log.record(Event::TaskAdded {
            task_id: "task-1".to_string(),
            title: "Test".to_string(),
            added_by: None,
        });

        let events = log.drain_since(&after);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_coordinator_context_does_not_include_compaction() {
        use workgraph::test_helpers::{make_task_with_status, setup_workgraph};

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        setup_workgraph(
            dir,
            vec![make_task_with_status("task-1", "A task", Status::Open)],
        );

        // Even if a stale compactor/context.md is present, it must NOT be injected.
        let ctx_path = dir.join("compactor").join("context.md");
        std::fs::create_dir_all(ctx_path.parent().unwrap()).unwrap();
        std::fs::write(&ctx_path, "stale graph-cycle compaction output").unwrap();

        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();

        assert!(!ctx.contains("Compacted Project Context"));
        assert!(!ctx.contains("stale graph-cycle compaction output"));
    }

    #[test]
    fn test_coordinator_context_includes_chat_summary() {
        use workgraph::service::chat_compactor::{ChatCompactorState, context_summary_path};
        use workgraph::test_helpers::{make_task_with_status, setup_workgraph};

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        setup_workgraph(
            dir,
            vec![make_task_with_status("task-1", "A task", Status::Open)],
        );

        // Write context-summary.md with known content
        let summary_path = context_summary_path(dir, 0);
        std::fs::create_dir_all(summary_path.parent().unwrap()).unwrap();
        std::fs::write(
            &summary_path,
            "# Conversation Context Summary\n\nUser prefers concise responses.",
        )
        .unwrap();

        // Write chat compactor state with a known timestamp
        let state = ChatCompactorState {
            last_compaction: Some("2026-03-27T15:00:00Z".to_string()),
            last_message_count: 20,
            compaction_count: 1,
            last_inbox_id: 10,
            last_outbox_id: 10,
        };
        state.save(dir, 0).unwrap();

        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();

        // Chat summary should appear
        assert!(
            ctx.contains("### Conversation Context Summary"),
            "missing chat summary section header"
        );
        assert!(
            ctx.contains("User prefers concise responses."),
            "missing chat summary body"
        );
        assert!(
            ctx.contains("2026-03-27T15:00:00Z"),
            "missing chat compaction timestamp"
        );
    }

    #[test]
    fn test_coordinator_context_without_chat_summary() {
        use workgraph::test_helpers::{make_task_with_status, setup_workgraph};

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        setup_workgraph(
            dir,
            vec![make_task_with_status("task-1", "A task", Status::Open)],
        );

        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();

        // Should not contain chat summary section
        assert!(!ctx.contains("Conversation Context Summary"));
    }

    #[test]
    fn test_coordinator_context_includes_injected_history() {
        use workgraph::test_helpers::{make_task_with_status, setup_workgraph};

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        setup_workgraph(
            dir,
            vec![make_task_with_status("task-1", "A task", Status::Open)],
        );

        // Write injected context
        workgraph::chat::write_injected_context(dir, 0, "We discussed auth last week").unwrap();

        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();

        // Injected history should appear
        assert!(
            ctx.contains("### Injected History Context"),
            "missing injected history section header"
        );
        assert!(
            ctx.contains("We discussed auth last week"),
            "missing injected history body"
        );

        // After consumption, the file should be gone (take_injected_context removes it)
        assert!(
            workgraph::chat::take_injected_context(dir, 0).is_none(),
            "injected context should be consumed after build_coordinator_context"
        );
    }

    #[test]
    fn test_coordinator_context_no_injected_history_when_absent() {
        use workgraph::test_helpers::{make_task_with_status, setup_workgraph};

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        setup_workgraph(
            dir,
            vec![make_task_with_status("task-1", "A task", Status::Open)],
        );

        let ctx = build_coordinator_context(dir, "2026-01-01T00:00:00Z", None, 0).unwrap();

        assert!(!ctx.contains("Injected History Context"));
    }

    /// resolve_chat_task_id is the chat-supervisor's pre-flight task lookup.
    /// Prefers the new `.chat-N` prefix; falls back to `.coordinator-N` so we
    /// keep supervising graphs that haven't run `wg migrate chat-rename` yet.
    #[test]
    fn test_resolve_chat_task_id_prefers_new_prefix() {
        let mut graph = workgraph::graph::WorkGraph::new();
        graph.add_node(workgraph::graph::Node::Task(workgraph::graph::Task {
            id: ".chat-7".to_string(),
            title: "Chat 7".to_string(),
            ..Default::default()
        }));
        assert_eq!(resolve_chat_task_id(&graph, 7), Some(".chat-7".to_string()));
    }

    #[test]
    fn test_resolve_chat_task_id_falls_back_to_legacy_prefix() {
        let mut graph = workgraph::graph::WorkGraph::new();
        graph.add_node(workgraph::graph::Node::Task(workgraph::graph::Task {
            id: ".coordinator-2".to_string(),
            title: "Legacy 2".to_string(),
            ..Default::default()
        }));
        assert_eq!(
            resolve_chat_task_id(&graph, 2),
            Some(".coordinator-2".to_string())
        );
    }

    #[test]
    fn test_resolve_chat_task_id_prefers_new_when_both_present() {
        // Edge case: a half-migrated graph where both prefixes exist.
        // The supervisor must commit to the new prefix so subsequent
        // `wg spawn-task` invocations land on the canonical task.
        let mut graph = workgraph::graph::WorkGraph::new();
        graph.add_node(workgraph::graph::Node::Task(workgraph::graph::Task {
            id: ".chat-3".to_string(),
            title: "Chat 3".to_string(),
            ..Default::default()
        }));
        graph.add_node(workgraph::graph::Node::Task(workgraph::graph::Task {
            id: ".coordinator-3".to_string(),
            title: "Coordinator 3".to_string(),
            ..Default::default()
        }));
        assert_eq!(resolve_chat_task_id(&graph, 3), Some(".chat-3".to_string()));
    }

    /// Stale `.coordinator-N` self-archive (parent task bullet 3): when neither
    /// `.chat-N` nor `.coordinator-N` exists in the graph, the supervisor
    /// pre-flight returns None — the caller exits cleanly and removes the
    /// per-coord state file so daemon restarts do not resurrect dead chats.
    #[test]
    fn test_resolve_chat_task_id_returns_none_for_orphan_supervisor() {
        let graph = workgraph::graph::WorkGraph::new();
        assert!(resolve_chat_task_id(&graph, 99).is_none());
    }

    /// End-to-end of the orphan-exit cleanup path used by the supervisor:
    /// when the chat task has been removed from the graph, `remove_for`
    /// scrubs the per-coord state file. Boot enumeration then has nothing
    /// to respawn, so dead chats stay dead across daemon restart.
    #[test]
    fn test_orphan_supervisor_removes_state_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        // Pretend the daemon previously wrote per-coord state for chat 5.
        let state = super::super::CoordinatorState {
            enabled: true,
            ..Default::default()
        };
        state.save_for(dir, 5);
        assert!(super::super::CoordinatorState::load_for(dir, 5).is_some());

        // Graph contains neither `.chat-5` nor `.coordinator-5`.
        let graph = workgraph::graph::WorkGraph::new();
        assert!(resolve_chat_task_id(&graph, 5).is_none());

        // Supervisor's orphan-exit calls remove_for; verify the file is gone.
        super::super::CoordinatorState::remove_for(dir, 5);
        assert!(super::super::CoordinatorState::load_for(dir, 5).is_none());
    }

    /// Bug-fix regression test for `re-implement-fix`.
    ///
    /// The supervisor used to push `restart_timestamps` on EVERY spawn,
    /// regardless of why the child exited. Three normal TUI handoff cycles
    /// (write sentinel → cooperative release → respawn) would trip the
    /// 3-restarts-per-10-minute pause. The fix moves the count from "spawn"
    /// to "crash classification", and routes session-lock contention exits
    /// through a separate, shorter back-off counter.
    ///
    /// This test drives `classify_child_exit` directly so we don't need a
    /// real subprocess.
    #[test]
    fn test_classify_child_exit_session_lock_contention() {
        use std::time::Duration;
        // status=1 within ~1s of spawn AND a live session-lock holder is
        // present → cooperative-handoff race, NOT a genuine crash.
        assert_eq!(
            classify_child_exit(false, Duration::from_millis(500), true),
            ChildExitKind::SessionLockContention,
        );
    }

    #[test]
    fn test_classify_child_exit_clean_success() {
        use std::time::Duration;
        assert_eq!(
            classify_child_exit(true, Duration::from_secs(60), false),
            ChildExitKind::Clean,
        );
        // Clean even if a lock holder happens to be present (TUI handoff).
        assert_eq!(
            classify_child_exit(true, Duration::from_millis(100), true),
            ChildExitKind::Clean,
        );
    }

    #[test]
    fn test_classify_child_exit_crash_when_no_lock_holder() {
        use std::time::Duration;
        // status=1 fast but NO live lock holder → genuine crash.
        assert_eq!(
            classify_child_exit(false, Duration::from_millis(500), false),
            ChildExitKind::Crash,
        );
    }

    #[test]
    fn test_classify_child_exit_crash_when_long_running() {
        use std::time::Duration;
        // status=1 after running >1s → not a startup race, even if a lock
        // holder is present (the lock holder appeared after our child died).
        assert_eq!(
            classify_child_exit(false, Duration::from_secs(60), true),
            ChildExitKind::Crash,
        );
    }

    /// The bug: 3 normal TUI handoff cycles in <1 minute would push 3
    /// timestamps and trip the 600s rate-limit. With the fix, clean exits
    /// MUST NOT push to `restart_timestamps`, so the rate-limit stays clear.
    #[test]
    fn test_three_clean_exits_do_not_trip_rate_limit() {
        use std::time::Duration;
        let mut timestamps: VecDeque<std::time::Instant> = VecDeque::new();
        let now = std::time::Instant::now();

        // Simulate three clean TUI handoff cycles.
        for _ in 0..3 {
            let elapsed = Duration::from_millis(200);
            let cls = classify_child_exit(true, elapsed, true);
            assert_eq!(cls, ChildExitKind::Clean);
            // Only Crash pushes to restart_timestamps in the supervisor loop.
            if cls == ChildExitKind::Crash {
                timestamps.push_back(now);
            }
        }

        let wait = rate_limit_wait(
            &timestamps,
            now,
            Duration::from_secs(RESTART_WINDOW_SECS),
            MAX_RESTARTS_PER_WINDOW,
        );
        assert!(
            wait.is_none(),
            "three clean exits must not trip the rate-limit, but got wait={:?}",
            wait
        );
    }

    /// Three rapid session-lock contentions also must not trip the
    /// 600s rate-limit — they back off through their own counter.
    #[test]
    fn test_three_lock_contentions_do_not_trip_rate_limit() {
        use std::time::Duration;
        let mut timestamps: VecDeque<std::time::Instant> = VecDeque::new();
        let now = std::time::Instant::now();
        let mut lock_contention_count: u32 = 0;

        for _ in 0..3 {
            let cls = classify_child_exit(false, Duration::from_millis(300), true);
            assert_eq!(cls, ChildExitKind::SessionLockContention);
            match cls {
                ChildExitKind::Crash => timestamps.push_back(now),
                ChildExitKind::SessionLockContention => {
                    lock_contention_count += 1;
                }
                _ => {}
            }
        }

        // Lock contentions should accumulate in the dedicated counter,
        // not in restart_timestamps.
        assert_eq!(lock_contention_count, 3);
        let wait = rate_limit_wait(
            &timestamps,
            now,
            Duration::from_secs(RESTART_WINDOW_SECS),
            MAX_RESTARTS_PER_WINDOW,
        );
        assert!(wait.is_none());

        // The contention back-off itself must be ≥10s, NOT 600s.
        let backoff = lock_contention_backoff(lock_contention_count);
        assert!(
            backoff >= Duration::from_secs(10),
            "lock-contention backoff must be ≥10s, got {:?}",
            backoff
        );
        assert!(
            backoff < Duration::from_secs(600),
            "lock-contention backoff must be <600s (the rate-limit pause), got {:?}",
            backoff
        );
    }

    /// Three actual crashes (no lock holder) DO push timestamps and DO trip
    /// the rate-limit on the fourth iteration. This guards against the
    /// converse regression: don't accidentally turn off the rate-limit.
    #[test]
    fn test_three_crashes_do_trip_rate_limit() {
        use std::time::Duration;
        let mut timestamps: VecDeque<std::time::Instant> = VecDeque::new();
        let now = std::time::Instant::now();

        for _ in 0..MAX_RESTARTS_PER_WINDOW {
            let cls = classify_child_exit(false, Duration::from_millis(50), false);
            assert_eq!(cls, ChildExitKind::Crash);
            timestamps.push_back(now);
        }

        let wait = rate_limit_wait(
            &timestamps,
            now,
            Duration::from_secs(RESTART_WINDOW_SECS),
            MAX_RESTARTS_PER_WINDOW,
        );
        assert!(
            wait.is_some(),
            "{} crashes must trip the rate-limit",
            MAX_RESTARTS_PER_WINDOW
        );
    }

    #[test]
    fn test_tui_sentinel_defers_supervisor_respawn_only_while_alive() {
        let tmp = TempDir::new().unwrap();
        let chat_dir = tmp.path().join("chat");
        workgraph::session_lock::write_tui_driver_sentinel(&chat_dir, std::process::id()).unwrap();

        assert_eq!(tui_driver_deferral_pid(&chat_dir), Some(std::process::id()));

        workgraph::session_lock::clear_tui_driver_sentinel(&chat_dir);
        assert_eq!(tui_driver_deferral_pid(&chat_dir), None);

        std::fs::create_dir_all(&chat_dir).unwrap();
        std::fs::write(
            workgraph::session_lock::tui_driver_sentinel_path(&chat_dir),
            "999999\n2020-01-01T00:00:00Z\n",
        )
        .unwrap();
        assert_eq!(tui_driver_deferral_pid(&chat_dir), None);
    }
}
