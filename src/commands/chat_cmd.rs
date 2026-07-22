//! `wg chat <subcommand>` — chat as a first-class graph entity.
//!
//! Decouples chat persistence from service runtime: chats are graph tasks
//! (`.chat-N`) that survive daemon restart. The supervisor in the running
//! daemon spawns a handler subprocess for each active chat task.
//!
//! Design constraints:
//! - `wg chat create`, `send`, `list`, `show` MUST work when the service
//!   daemon is down — they operate directly on `.wg/graph.jsonl`
//!   and `.wg/chat/<uuid>/`.
//! - `wg chat resume` and `wg chat stop` require the daemon (the handler
//!   process is owned by the supervisor); they error clearly when down.
//! - When the daemon IS running, `create` / `delete` / `archive` go
//!   through IPC so the supervisor immediately reflects the change.
//!
//! See task wg-chat-as for the full spec.
//!
//! Backward compat: `wg service create-chat` etc. still parse, but emit
//! a deprecation warning and route here.

use anyhow::{Context, Result};
use std::path::Path;

use worksgood::chat_id;
use worksgood::dispatch::handler_for_model;
use worksgood::graph::{Status, WorkGraph};

use crate::commands::graph_path;
use crate::commands::is_process_alive;

/// Liveness category for `wg chat list` / `show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRuntimeStatus {
    /// Chat task has a concrete live runtime owner: a daemon handler lock or
    /// a persistent TUI-owned tmux pane. The historical label remains
    /// `supervised` for output compatibility.
    Supervised,
    /// Chat task exists in graph; service daemon is NOT running.
    /// Inbox messages will be queued until the daemon is started.
    Dormant,
    /// Chat task is Status::Done with the `archived` tag.
    Archived,
    /// Chat task is Status::Abandoned.
    Deleted,
    /// Chat task exists, daemon is up, but the supervisor has no
    /// active handler entry (e.g. after `wg chat stop`).
    Stopped,
}

impl ChatRuntimeStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Dormant => "dormant",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
            Self::Stopped => "stopped",
        }
    }
}

/// Service-running detection — checks ServiceState file + PID liveness.
/// Returns true when the daemon socket should be reachable.
pub fn service_is_running(dir: &Path) -> bool {
    use crate::commands::service::ServiceState;
    match ServiceState::load(dir) {
        Ok(Some(state)) => is_process_alive(state.pid),
        _ => false,
    }
}

/// Resolve a chat reference (numeric ID, `.chat-N`, `.coordinator-N`,
/// or alias name like "testbot") to the numeric chat agent ID.
pub fn resolve_chat_id(graph: &WorkGraph, reference: &str) -> Option<u32> {
    // Numeric form ("0", "7")
    if let Ok(n) = reference.parse::<u32>() {
        if chat_id::find_chat_task(graph, n).is_some() {
            return Some(n);
        }
        return Some(n); // tolerate ID-without-task (still try downstream ops)
    }
    // Full task ID form
    if let Some(n) = chat_id::parse_chat_task_id(reference) {
        return Some(n);
    }
    // Name-based: scan chat tasks for a matching title suffix.
    // Title format from create_chat_in_graph is "Chat: <name>" or "Chat <id>".
    let want = reference.to_ascii_lowercase();
    for task in graph.tasks() {
        if !task.tags.iter().any(|t| chat_id::is_chat_loop_tag(t)) {
            continue;
        }
        let title_lower = task.title.to_ascii_lowercase();
        // Match "chat: <name>" exactly on the suffix
        let matches_suffix = title_lower
            .strip_prefix("chat: ")
            .map(|rest| rest == want)
            .unwrap_or(false);
        if matches_suffix && let Some(id) = chat_id::parse_chat_task_id(&task.id) {
            return Some(id);
        }
    }
    None
}

/// Categorize a chat task's runtime status given current daemon state.
fn classify_chat_task(
    task: &worksgood::graph::Task,
    daemon_running: bool,
    supervised_ids: &[u32],
) -> ChatRuntimeStatus {
    if matches!(task.status, Status::Abandoned) {
        return ChatRuntimeStatus::Deleted;
    }
    if task.tags.iter().any(|t| t == "archived") {
        return ChatRuntimeStatus::Archived;
    }
    let id = match chat_id::parse_chat_task_id(&task.id) {
        Some(n) => n,
        None => return ChatRuntimeStatus::Dormant,
    };
    if !daemon_running {
        return ChatRuntimeStatus::Dormant;
    }
    if supervised_ids.contains(&id) {
        ChatRuntimeStatus::Supervised
    } else {
        ChatRuntimeStatus::Stopped
    }
}

/// Query the running daemon for its supervised chat IDs (if reachable).
/// Returns empty Vec on failure or when daemon is down.
/// True when a live handler currently holds the chat's session lock.
///
/// `wg chat show`/`list` derive runtime status from the daemon's
/// supervised-coordinator list, but a handler can be alive (holding the
/// lock, serving the inbox) before/without appearing in that list — e.g.
/// a TUI-driven pane or a just-(re)spawned handler. Consulting the lock
/// directly keeps `wg chat show` honest: if there's a live handler, the
/// chat is running, not "stopped". Acceptance criterion for
/// fix-nex-chat23-eof-resume: show/status must agree on a live handler.
fn chat_handler_is_live(dir: &Path, cid: u32) -> bool {
    worksgood::chat::chat_runtime_is_live(dir, cid)
}

/// Promote a dormant/stopped classification when a concrete runtime owner is
/// live. A TUI-owned tmux pane remains live even when the daemon is down and
/// vendor panes do not hold `.handler.pid`, so daemon state alone cannot be
/// the liveness authority. Archived/deleted chats remain terminal.
fn refine_status_with_runtime(status: ChatRuntimeStatus, runtime_live: bool) -> ChatRuntimeStatus {
    if matches!(
        status,
        ChatRuntimeStatus::Stopped | ChatRuntimeStatus::Dormant
    ) && runtime_live
    {
        ChatRuntimeStatus::Supervised
    } else {
        status
    }
}

fn refine_status_with_live_handler(
    status: ChatRuntimeStatus,
    dir: &Path,
    cid: u32,
) -> ChatRuntimeStatus {
    refine_status_with_runtime(status, chat_handler_is_live(dir, cid))
}

fn supervised_chat_ids(dir: &Path) -> Vec<u32> {
    if !service_is_running(dir) {
        return Vec::new();
    }
    use crate::commands::service::ipc::IpcRequest;
    use crate::commands::service::send_request;
    let resp = match send_request(dir, &IpcRequest::ListChats) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let data = match &resp.data {
        Some(d) => d,
        None => return Vec::new(),
    };
    let arr = match data.get("coordinators").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter(|value| {
            value
                .get("runtime_live")
                .and_then(|flag| flag.as_bool())
                .unwrap_or(false)
        })
        .filter_map(|v| v.get("coordinator_id").and_then(|x| x.as_u64()))
        .map(|n| n as u32)
        .collect()
}

fn migrate_existing_chat_tasks(dir: &Path) -> Result<()> {
    let path = graph_path(dir);
    worksgood::parser::modify_graph(&path, |graph| {
        let mut changed = false;
        let ids: Vec<String> = graph
            .tasks()
            .filter(|t| t.tags.iter().any(|tag| chat_id::is_chat_loop_tag(tag)))
            .map(|t| t.id.clone())
            .collect();
        for task_id in ids {
            let Some(cid) = chat_id::parse_chat_task_id(&task_id) else {
                continue;
            };
            let coord_state = crate::commands::service::CoordinatorState::load_for(dir, cid);
            if let Some(task) = graph.get_task_mut(&task_id) {
                let task_model = task.model.clone();
                let task_endpoint = task.endpoint.clone();
                let executor = coord_state
                    .as_ref()
                    .and_then(|s| s.executor_override.as_deref());
                let model = coord_state
                    .as_ref()
                    .and_then(|s| s.model_override.as_deref())
                    .or(task_model.as_deref());
                let endpoint = coord_state
                    .as_ref()
                    .and_then(|s| s.endpoint_override.as_deref())
                    .or(task_endpoint.as_deref());
                changed |= worksgood::chat_command::migrate_chat_task_metadata(
                    task, dir, executor, model, endpoint,
                );
            }
        }
        changed
    })
    .with_context(|| "Failed to migrate chat task metadata")?;
    Ok(())
}

// ============================================================================
// Subcommand: create
// ============================================================================

fn validate_interactive_executor_binary(
    executor: Option<&str>,
    binary_path: Option<&Path>,
) -> Result<()> {
    if executor == Some("pi") && binary_path.is_none() {
        anyhow::bail!(
            "interactive Pi executable `pi` was not found on PATH; no chat was created and no fallback executor was attempted"
        );
    }
    Ok(())
}

fn require_interactive_executor_binary(executor: Option<&str>) -> Result<()> {
    let binary_path = if executor == Some("pi") {
        worksgood::executor_discovery::discover()
            .into_iter()
            .find(|info| info.name == "pi" && info.available)
            .and_then(|info| info.binary_path)
    } else {
        None
    };
    validate_interactive_executor_binary(executor, binary_path.as_deref())
}

/// `wg chat create` — create a new chat agent entity in the graph.
///
/// When the service is running, talks to it via IPC (so the supervisor
/// can immediately spawn the handler). When it's down, writes the graph
/// task directly — the supervisor picks it up on next service start.
/// Both paths produce identical on-disk state.
pub fn run_create(
    dir: &Path,
    name: Option<&str>,
    model: Option<&str>,
    executor: Option<&str>,
    endpoint: Option<&str>,
    command: Option<&str>,
    json: bool,
) -> Result<()> {
    // A chat is an LLM-backed entity. Refuse before IPC or direct graph
    // mutation unless its invocation or config explicitly selects a route.
    let selection =
        worksgood::execution_selection::require(dir, model.map(|m| (m, false)), "wg chat create")?;
    // Plain interactive Pi is a terminal-hosted vendor console, not the
    // hermetic wg pi-handler/plugin transport. Validate that exact executable
    // before IPC or graph/session mutation so a missing Pi is visible and
    // transactional. An explicit --exec wins over the model/profile handler.
    let selected_executor = executor.or_else(|| {
        selection
            .system
            .as_ref()
            .map(|system| system.handler.as_str())
    });
    if command.is_none() {
        require_interactive_executor_binary(selected_executor)?;
    }
    if service_is_running(dir) {
        run_create_via_ipc(dir, name, model, executor, endpoint, command, json)
    } else {
        run_create_direct(dir, name, model, executor, endpoint, command, json)
    }
}

#[cfg(unix)]
fn run_create_via_ipc(
    dir: &Path,
    name: Option<&str>,
    model: Option<&str>,
    executor: Option<&str>,
    endpoint: Option<&str>,
    command: Option<&str>,
    json: bool,
) -> Result<()> {
    crate::commands::service::run_create_coordinator(
        dir, name, model, executor, endpoint, command, json,
    )
}

#[cfg(not(unix))]
fn run_create_via_ipc(
    _dir: &Path,
    _name: Option<&str>,
    _model: Option<&str>,
    _executor: Option<&str>,
    _endpoint: Option<&str>,
    _command: Option<&str>,
    _json: bool,
) -> Result<()> {
    anyhow::bail!("Service IPC is only supported on Unix systems")
}

fn run_create_direct(
    dir: &Path,
    name: Option<&str>,
    model: Option<&str>,
    executor: Option<&str>,
    endpoint: Option<&str>,
    command: Option<&str>,
    json: bool,
) -> Result<()> {
    let next_id = crate::commands::service::ipc::create_chat_in_graph(
        dir, name, model, executor, endpoint, command,
    )?;
    let task_id = chat_id::format_chat_task_id(next_id);
    if json {
        let v = serde_json::json!({
            "chat_id": next_id,
            "coordinator_id": next_id,
            "task_id": task_id,
            "name": name,
            "service": "down",
            "status": "dormant",
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "Created chat {} (task {}). Service is not running — chat is dormant.",
            next_id, task_id
        );
        println!(
            "Start the service ('wg service start') and the supervisor will spawn the handler."
        );
    }
    Ok(())
}

// ============================================================================
// Subcommand: list / ls
// ============================================================================

/// `wg chat list` — show all chat entities with truthful status.
pub fn run_list(dir: &Path, json: bool) -> Result<()> {
    migrate_existing_chat_tasks(dir)?;
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;

    let daemon_running = service_is_running(dir);
    let supervised = supervised_chat_ids(dir);

    let mut rows = Vec::new();
    for task in graph.tasks() {
        if !task.tags.iter().any(|t| chat_id::is_chat_loop_tag(t)) {
            continue;
        }
        let cid = match chat_id::parse_chat_task_id(&task.id) {
            Some(n) => n,
            None => continue,
        };
        let status = classify_chat_task(task, daemon_running, &supervised);
        let status = refine_status_with_live_handler(status, dir, cid);
        rows.push((cid, task, status));
    }
    rows.sort_by_key(|(cid, _, _)| *cid);

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|(cid, t, s)| {
                serde_json::json!({
                    "chat_id": cid,
                    "task_id": t.id,
                    "title": t.title,
                    "status": s.label(),
                    "task_status": format!("{:?}", t.status),
                    "service_running": daemon_running,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"chats": arr}))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("No chats. Create one with 'wg chat create --name <NAME>'.");
        return Ok(());
    }

    println!("{:<6}  {:<14}  {:<24}  {}", "ID", "STATUS", "TASK", "TITLE");
    for (cid, t, s) in rows {
        let suffix = if matches!(s, ChatRuntimeStatus::Dormant) && !daemon_running {
            " — service stopped"
        } else {
            ""
        };
        println!(
            "{:<6}  {:<14}  {:<24}  {}{}",
            cid,
            s.label(),
            t.id,
            t.title,
            suffix
        );
    }
    Ok(())
}

// ============================================================================
// Subcommand: show
// ============================================================================

/// `wg chat show` — detailed view of a single chat entity.
pub fn run_show(dir: &Path, reference: &str, json: bool) -> Result<()> {
    migrate_existing_chat_tasks(dir)?;
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;

    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;
    let task = chat_id::find_chat_task(&graph, cid)
        .with_context(|| format!("Chat task for id {} not found in graph", cid))?;

    let daemon_running = service_is_running(dir);
    let supervised = supervised_chat_ids(dir);
    let status = classify_chat_task(task, daemon_running, &supervised);
    let status = refine_status_with_live_handler(status, dir, cid);

    // Per-chat overrides from CoordinatorState.
    let coord_state = crate::commands::service::CoordinatorState::load_for(dir, cid);
    let exec_override = coord_state
        .as_ref()
        .and_then(|s| s.executor_override.clone());
    let model_override = coord_state.as_ref().and_then(|s| s.model_override.clone());

    // Live handler (session-lock holder), if any. Reported so `wg chat
    // show` agrees with `wg session status` — both must reflect a live
    // handler after launch/resume. Resolve via the dot-less session ref
    // (the handler's actual lock dir), not the `.chat-N` task id.
    let chat_ref = chat_id::format_chat_session_ref(cid);
    let chat_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref);
    let handler = worksgood::session_lock::read_holder(&chat_dir)
        .ok()
        .flatten()
        .filter(|info| info.alive);
    let tmux_session = chat_id::chat_tmux_session_for_id(dir, cid);
    let tmux_live = chat_id::chat_tmux_session_is_live(dir, cid);
    let runtime_chat_dir = worksgood::chat_runtime::runtime_chat_dir(dir, &chat_ref)
        .unwrap_or_else(|_| chat_dir.clone());
    let runtime_ledger = worksgood::chat_runtime::read_ledger(&runtime_chat_dir);
    let last_runtime = runtime_ledger.last_specific_event();
    let last_runtime_reason = runtime_ledger.last_specific_reason();
    let last_recovery = runtime_ledger.last_decision();

    if json {
        let v = serde_json::json!({
            "chat_id": cid,
            "task_id": task.id,
            "title": task.title,
            "task_status": format!("{:?}", task.status),
            "runtime_status": status.label(),
            "service_running": daemon_running,
            "executor": exec_override,
            "model": model_override,
            "handler": handler.as_ref().map(|info| serde_json::json!({
                "pid": info.pid,
                "kind": info.kind.map(|k| k.label()),
                "started_at": info.started_at,
            })),
            "tmux": {
                "session": tmux_session,
                "live": tmux_live,
            },
            "runtime": {
                "ledger": worksgood::chat_runtime::ledger_path(&runtime_chat_dir),
                "malformed_records": runtime_ledger.malformed_records,
                "last_reason": last_runtime_reason,
                "last_event": last_runtime,
                "last_recovery": last_recovery,
            },
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    println!("Chat {}", cid);
    println!("  task     : {}", task.id);
    println!("  title    : {}", task.title);
    println!("  status   : {}", status.label());
    println!("  task     : {:?}", task.status);
    if let Some(e) = exec_override {
        println!("  executor : {}", e);
    }
    if let Some(m) = model_override {
        println!("  model    : {}", m);
    }
    println!(
        "  service  : {}",
        if daemon_running { "running" } else { "stopped" }
    );
    match &handler {
        Some(info) => println!(
            "  handler  : live pid={} kind={}",
            info.pid,
            info.kind.map(|k| k.label()).unwrap_or("unknown")
        ),
        None if tmux_live => println!("  handler  : live tmux={tmux_session}"),
        None => println!("  handler  : none"),
    }
    println!(
        "  runtime  : {}",
        last_runtime_reason
            .as_deref()
            .unwrap_or("no durable exit recorded")
    );
    if let Some(event) = last_runtime {
        println!("  observed : {} UTC ({:?})", event.at, event.source);
        if let Some(path) = event.stderr_path.as_deref() {
            println!("  stderr   : {}", path);
        }
    }
    if let Some(decision) = last_recovery
        && let Some(value) = decision.decision
    {
        println!(
            "  recovery : {:?} attempt {}",
            value,
            decision.attempt.unwrap_or(0)
        );
    }
    Ok(())
}

// ============================================================================
// Subcommand: send
// ============================================================================

/// `wg chat send <ref> <msg>` — append a message to the chat's inbox.
///
/// Works with the daemon up OR down: `inbox.jsonl` is the source of
/// truth. When the daemon is up, the supervisor's handler will pick
/// the message up via the standard chat loop. When down, the message
/// queues until the daemon (re)starts.
pub fn run_send(dir: &Path, reference: &str, message: &str, json: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;

    // Make sure the chat dir exists (chat::append_inbox_for creates parent
    // dirs, but we want a stable filesystem location for non-running chats).
    let request_id = format!("wg-chat-send-{}", chrono::Utc::now().timestamp_millis());
    let inbox_id = worksgood::chat::append_inbox_for(dir, cid, message, &request_id)
        .with_context(|| format!("Failed to append to chat {} inbox", cid))?;

    let running = service_is_running(dir);
    if json {
        let v = serde_json::json!({
            "chat_id": cid,
            "inbox_id": inbox_id,
            "request_id": request_id,
            "service_running": running,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "Appended message #{} to chat {} inbox.{}",
            inbox_id,
            cid,
            if running {
                ""
            } else {
                " Service is not running — message will be processed when daemon starts."
            }
        );
    }
    Ok(())
}

// ============================================================================
// Subcommand: model
// ============================================================================

/// Convert pi's native `provider:model-id` event identity into WG's
/// handler-first route. An already-handler-qualified `pi:...` route is kept.
fn pi_model_writeback_spec(spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.starts_with("pi:") {
        trimmed.to_string()
    } else {
        format!("pi:{trimmed}")
    }
}

/// Persist an override only when it actually changed. This keeps duplicate Pi
/// notifications idempotent and avoids needless state-file rewrites.
fn persist_chat_model_override(dir: &Path, cid: u32, executor: &str, model: &str) -> Result<bool> {
    let mut state = crate::commands::service::CoordinatorState::load_or_default_for(dir, cid);
    if state.executor_override.as_deref() == Some(executor)
        && state.model_override.as_deref() == Some(model)
    {
        return Ok(false);
    }
    state.executor_override = Some(executor.to_string());
    state.model_override = Some(model.to_string());
    state.save_for(dir, cid);
    Ok(true)
}

/// `wg chat model <ref> <spec>` — persist a per-chat model override.
///
/// The plugin passes `--warm-pi-writeback` after Pi has already changed the
/// model in-process. That path must never signal/respawn the live Pi process;
/// it records executor=pi plus a handler-first `pi:<provider>:<model>` route for
/// the next resume. Ordinary CLI use retains the existing cold SetChatExecutor
/// behavior when the service is running.
pub fn run_model(
    dir: &Path,
    reference: &str,
    spec: &str,
    warm_pi_writeback: bool,
    json: bool,
) -> Result<()> {
    if spec.trim().is_empty() {
        anyhow::bail!("model spec must not be empty");
    }
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{reference}'"))?;
    let task = chat_id::find_chat_task(&graph, cid)
        .with_context(|| format!("No graph chat task for canonical id .chat-{cid}"))?;
    if !task.tags.iter().any(|tag| chat_id::is_chat_loop_tag(tag)) {
        anyhow::bail!("task '{}' is not a WG chat", task.id);
    }

    let (executor, model) = if warm_pi_writeback {
        ("pi".to_string(), pi_model_writeback_spec(spec))
    } else {
        let model = spec.trim().to_string();
        (handler_for_model(&model).as_str().to_string(), model)
    };

    let changed = if !warm_pi_writeback && service_is_running(dir) {
        crate::commands::service::run_set_coordinator_executor(
            dir,
            cid,
            Some(&executor),
            Some(&model),
            json,
        )?;
        true
    } else {
        persist_chat_model_override(dir, cid, &executor, &model)?
    };

    if warm_pi_writeback || !service_is_running(dir) {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "chat_id": cid,
                    "task_id": task.id,
                    "executor": executor,
                    "model": model,
                    "warm": warm_pi_writeback,
                    "changed": changed,
                }))?
            );
        } else if changed {
            println!("Chat {} model override recorded: {}", task.id, model);
        } else {
            println!("Chat {} already uses {}; no change", task.id, model);
        }
    }
    Ok(())
}

// ============================================================================
// Subcommand: stop / resume / archive / delete
// ============================================================================

/// `wg chat stop` — SIGTERM the live handler (chat entity stays in graph).
/// Requires the daemon (the supervisor owns the handler).
pub fn run_stop(dir: &Path, reference: &str, json: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;
    if !service_is_running(dir) {
        anyhow::bail!(
            "Cannot stop chat {}: service daemon is not running. \
             The handler is supervised by the daemon — without it there is no \
             handler to stop. Start the daemon ('wg service start') first.",
            cid
        );
    }
    crate::commands::service::run_stop_coordinator(dir, cid, json)
}

/// Reconstruct the `(executor, model)` a chat should resume with from
/// its saved metadata, mirroring the precedence `wg chat show` uses:
///
///   * executor: per-chat `CoordinatorState.executor_override`, falling
///     back to the chat task's `executor_preset_name` (e.g. `nex`).
///   * model:    per-chat `CoordinatorState.model_override`, falling
///     back to the chat task's own `task.model`.
///
/// Either field may be `None` if nothing was ever recorded; callers
/// must ensure at least one is `Some` before sending the swap IPC.
/// Deriving the executor from the preset guarantees a non-empty result
/// even for a chat created with `--exec nex` and no explicit model, so
/// `wg chat resume <id>` never falls through to the hidden-flags error.
/// Returning the saved values means resume reproduces the exact
/// (executor, model) the chat last ran with — no hidden flags.
pub(crate) fn reconstruct_resume_metadata(
    dir: &Path,
    cid: u32,
) -> (Option<String>, Option<String>) {
    let coord_state = crate::commands::service::CoordinatorState::load_for(dir, cid);
    let mut executor = coord_state
        .as_ref()
        .and_then(|s| s.executor_override.clone());
    let mut model = coord_state.as_ref().and_then(|s| s.model_override.clone());

    // Fall back to the chat task's own model/preset when no per-chat
    // override is recorded (the common case for TUI-created chats, whose
    // model lives on the `.chat-N` task).
    if (model.is_none() || executor.is_none())
        && let Ok(graph) = worksgood::parser::load_graph(&graph_path(dir))
        && let Some(task) = chat_id::find_chat_task(&graph, cid)
    {
        if model.is_none() {
            model = task.model.clone();
        }
        if executor.is_none() {
            executor = task.executor_preset_name.clone();
        }
    }
    (executor, model)
}

fn validate_chat_resumable(graph: &WorkGraph, cid: u32) -> Result<()> {
    let task = chat_id::find_chat_task(graph, cid)
        .with_context(|| format!("Chat task for id {cid} not found in graph"))?;
    if task.status.is_terminal() || task.tags.iter().any(|tag| tag == "archived") {
        anyhow::bail!(
            "Cannot resume chat {cid}: authoritative task {} is terminal ({}){}",
            task.id,
            task.status,
            if task.tags.iter().any(|tag| tag == "archived") {
                " and archived"
            } else {
                ""
            }
        );
    }
    Ok(())
}

fn resume_runtime_proof_is_valid(graph: &WorkGraph, cid: u32, runtime_live: bool) -> bool {
    runtime_live && validate_chat_resumable(graph, cid).is_ok()
}

const RESUME_LIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const RESUME_LIVE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

fn wait_for_chat_runtime_with(
    timeout: std::time::Duration,
    poll: std::time::Duration,
    mut is_live: impl FnMut() -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_live() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll.min(deadline.saturating_duration_since(std::time::Instant::now())));
    }
}

/// `wg chat resume` — ask the supervisor to (re)spawn the handler and wait for
/// concrete liveness. An accepted IPC is scheduling acknowledgement, not user
/// success: this command returns success only after a handler lock or the
/// persistent TUI tmux owner becomes live within a bounded interval.
pub fn run_resume(dir: &Path, reference: &str, json: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;
    // Terminal graph state is authoritative. Reject it before consulting the
    // daemon, clearing sentinels, reconstructing route metadata, or accepting
    // any tmux process as runtime evidence.
    validate_chat_resumable(&graph, cid)?;
    if !service_is_running(dir) {
        anyhow::bail!(
            "Cannot resume chat {}: service daemon is not running. \
             Resume requires the supervisor (which lives in the daemon) to spawn \
             the handler. Start the daemon ('wg service start') and the supervisor \
             will pick up this chat automatically.",
            cid
        );
    }
    let chat_ref = format!("chat-{}", cid);
    let chat_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref);
    if worksgood::session_lock::read_tui_driver_sentinel(&chat_dir)
        .ok()
        .flatten()
        .is_some()
        && worksgood::session_lock::active_tui_driver_pid(&chat_dir).is_none()
        && !json
    {
        eprintln!(
            "\x1b[2m[wg chat]\x1b[0m cleared stale TUI sentinel for chat {} before resume",
            cid
        );
    }
    // Resume re-spawns the handler using the chat's *saved* executor /
    // model metadata (per-chat CoordinatorState override, falling back to
    // the chat task's own model). The respawn path is the executor-swap
    // IPC (`SetChatExecutor`), which requires at least one of
    // executor/model — passing `None, None` made `wg chat resume` fail
    // with "at least one of --executor or --model must be provided" even
    // though the metadata was on disk. We reconstruct it here so the user
    // never has to supply the (non-existent) flags. See
    // `reconstruct_resume_metadata`.
    // Re-read immediately before scheduling: archive/abandon may have raced
    // the earlier reference resolution while we inspected runtime metadata.
    let scheduling_graph = worksgood::parser::load_graph(&graph_path(dir))
        .with_context(|| "Failed to reload graph before scheduling chat resume")?;
    validate_chat_resumable(&scheduling_graph, cid)?;

    let (executor, model) = reconstruct_resume_metadata(dir, cid);
    use crate::commands::service::ipc::IpcRequest;
    use crate::commands::service::send_request;
    let resp = send_request(
        dir,
        &IpcRequest::SetChatExecutor {
            chat_id: cid,
            executor,
            model,
        },
    )?;
    if !resp.ok {
        let msg = resp.error.unwrap_or_else(|| "Unknown error".to_string());
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"error": msg}))?
            );
        } else {
            eprintln!("Error: {}", msg);
        }
        anyhow::bail!("{}", msg);
    }
    if !wait_for_chat_runtime_with(RESUME_LIVE_TIMEOUT, RESUME_LIVE_POLL, || {
        chat_handler_is_live(dir, cid)
    }) {
        let msg = format!(
            "Supervisor accepted resume for chat {cid}, but no live handler or TUI tmux session appeared within {}s. Inspect {}/service/daemon.log and retry after fixing the recorded spawn error.",
            RESUME_LIVE_TIMEOUT.as_secs(),
            dir.display()
        );
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "chat_id": cid,
                    "resumed": false,
                    "runtime_status": "stopped",
                    "error": msg,
                }))?
            );
        }
        anyhow::bail!(msg);
    }

    // Runtime proof is not sufficient by itself: a stale tmux session can
    // outlive an archive/abandon racing the supervisor acknowledgement. Re-read
    // the graph before reporting success and require both facts together.
    let proof_graph = worksgood::parser::load_graph(&graph_path(dir))
        .with_context(|| "Failed to reload graph while validating chat resume")?;
    if !resume_runtime_proof_is_valid(&proof_graph, cid, chat_handler_is_live(dir, cid)) {
        validate_chat_resumable(&proof_graph, cid)?;
        anyhow::bail!("Cannot resume chat {cid}: runtime ownership proof disappeared");
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "chat_id": cid,
                "resumed": true,
                "runtime_status": "supervised",
            }))?
        );
    } else {
        println!("Resumed chat {} — runtime is live.", cid);
    }
    Ok(())
}

/// `wg chat archive` — mark Done + tag 'archived'. Reversible-ish (archived
/// chats can still be inspected; their dirs are moved to .archive/).
pub fn run_archive(dir: &Path, reference: &str, json: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;
    let result = if service_is_running(dir) {
        crate::commands::service::run_archive_coordinator(dir, cid, json)
    } else {
        archive_chat_direct(dir, cid, json)
    };
    // Tear down the tmux chat session so we don't accumulate orphan
    // wg-chat-* sessions. Best-effort — the archive itself succeeded
    // (or failed) before this runs.
    chat_id::kill_chat_tmux_session_for_id(dir, cid);
    result
}

fn archive_chat_direct(dir: &Path, cid: u32, json: bool) -> Result<()> {
    let graph_p = graph_path(dir);
    let task_id = chat_id::format_chat_task_id(cid);
    let legacy_id = format!(".coordinator-{}", cid);
    worksgood::parser::modify_graph(&graph_p, |g| {
        let resolved = if g.get_task(&task_id).is_some() {
            task_id.clone()
        } else if g.get_task(&legacy_id).is_some() {
            legacy_id.clone()
        } else {
            return false;
        };
        if let Some(t) = g.get_task_mut(&resolved) {
            t.status = Status::Done;
            if !t.tags.iter().any(|x| x == "archived") {
                t.tags.push("archived".to_string());
            }
            t.log.push(worksgood::graph::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                actor: Some("wg-chat-archive".to_string()),
                user: Some(worksgood::current_user()),
                message: format!("Chat {} archived (service down)", cid),
            });
        }
        true
    })?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "chat_id": cid,
                "archived": true,
                "service": "down",
            }))?
        );
    } else {
        println!("Archived chat {} (service was not running).", cid);
    }
    Ok(())
}

/// `wg chat delete` — abandon the graph task and remove the chat dir.
pub fn run_delete(dir: &Path, reference: &str, yes: bool, json: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;

    if !yes && !json {
        eprint!(
            "Delete chat {} (graph task abandoned, chat dir preserved)? [y/N] ",
            cid
        );
        std::io::Write::flush(&mut std::io::stderr()).ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let result = if service_is_running(dir) {
        crate::commands::service::run_delete_coordinator(dir, cid, json)
    } else {
        delete_chat_direct(dir, cid, json)
    };
    // Tear down the tmux chat session if any — see run_archive.
    chat_id::kill_chat_tmux_session_for_id(dir, cid);
    result
}

fn delete_chat_direct(dir: &Path, cid: u32, json: bool) -> Result<()> {
    let graph_p = graph_path(dir);
    let task_id = chat_id::format_chat_task_id(cid);
    let legacy_id = format!(".coordinator-{}", cid);
    worksgood::parser::modify_graph(&graph_p, |g| {
        let resolved = if g.get_task(&task_id).is_some() {
            task_id.clone()
        } else if g.get_task(&legacy_id).is_some() {
            legacy_id.clone()
        } else {
            return false;
        };
        if let Some(t) = g.get_task_mut(&resolved) {
            t.status = Status::Abandoned;
            t.log.push(worksgood::graph::LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                actor: Some("wg-chat-delete".to_string()),
                user: Some(worksgood::current_user()),
                message: format!("Chat {} deleted (service down)", cid),
            });
        }
        true
    })?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "chat_id": cid,
                "deleted": true,
                "service": "down",
            }))?
        );
    } else {
        println!("Deleted chat {} (service was not running).", cid);
    }
    Ok(())
}

// ============================================================================
// Subcommand: attach
// ============================================================================

/// `wg chat attach` — open an interactive view of the chat session.
///
/// Preferred path: when a tmux session exists for this chat (TUI was
/// run with chat-persistence wrappers), `exec tmux attach -t <session>`
/// hands the user the live vendor CLI directly — including history and
/// in-flight tool calls. This is the strongest reattach UX and works
/// from any terminal (no TUI required).
///
/// Fallbacks (in order):
///   1. TUI mode via `chat::run_interactive` when on a TTY + service is
///      up. Talks to daemon over IPC.
///   2. Read-only outbox stream (CLI mode). Use `wg chat send` to
///      enqueue messages.
pub fn run_attach(dir: &Path, reference: &str, force_cli: bool) -> Result<()> {
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout());

    // Try the tmux fast-path first when on a TTY: if the wg-chat-* tmux
    // session for this chat is alive, attach to it. This is what the
    // user actually wants for "drop me back into my chat" — no
    // outbox-tail, no IPC roundtrip. Skip when --cli forced or when not
    // on a TTY (tmux attach into a pipe would hang).
    if !force_cli
        && is_tty
        && let Some(session) = chat_tmux_session_for_dir(dir, cid)
        && tmux_session_alive(&session)
    {
        eprintln!("Attaching to tmux session: {}", session);
        let status = std::process::Command::new("tmux")
            .args(["attach", "-d", "-t", &session])
            .status()
            .with_context(|| "Failed to invoke tmux attach")?;
        if status.success() {
            return Ok(());
        }
        eprintln!(
            "tmux attach exited with status {:?}; falling back to other modes.",
            status.code()
        );
    }

    if !force_cli && is_tty {
        // Interactive REPL via existing chat::run_interactive (talks to
        // daemon over IPC for live responses).
        if !service_is_running(dir) {
            eprintln!(
                "Note: service daemon is not running. Falling back to read-only \
                 stream view; use 'wg chat send' to enqueue messages."
            );
            return read_only_attach(dir, cid);
        }
        crate::commands::chat::run_interactive(dir, None, cid)
    } else {
        read_only_attach(dir, cid)
    }
}

fn chat_tmux_session_for_dir(dir: &Path, cid: u32) -> Option<String> {
    Some(worksgood::chat_id::prepare_chat_tmux_session_for_id(
        dir, cid,
    ))
}

fn tmux_session_alive(name: &str) -> bool {
    std::process::Command::new("tmux")
        .args(["has-session", "-t", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn read_only_attach(dir: &Path, cid: u32) -> Result<()> {
    // Reuse the existing session-attach implementation, addressing the
    // chat by its `.chat-N` task id (chat_dir_for_ref handles both
    // legacy and new naming + alias resolution).
    let session_ref = chat_id::format_chat_task_id(cid);
    crate::commands::chat_session::run(
        dir,
        crate::cli::SessionCommands::Attach {
            session: session_ref,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mk_workgraph_dir() -> TempDir {
        let td = TempDir::new().unwrap();
        let dir = td.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(dir.join("graph.jsonl"), "").unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[dispatcher]\nmodel = \"claude:opus\"\n",
        )
        .unwrap();
        td
    }

    #[test]
    fn missing_pi_preflight_names_actual_executable_and_is_executor_scoped() {
        let error = validate_interactive_executor_binary(Some("pi"), None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("`pi`"), "{message}");
        assert!(message.contains("no chat was created"), "{message}");
        assert!(message.contains("no fallback executor"), "{message}");

        validate_interactive_executor_binary(Some("pi"), Some(Path::new("/tmp/pi"))).unwrap();
        validate_interactive_executor_binary(Some("claude"), None).unwrap();
        validate_interactive_executor_binary(None, None).unwrap();
    }

    #[test]
    fn create_chat_works_when_service_down() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        // service_is_running is false (no service/state.json) — exercise
        // the direct path:
        assert!(!service_is_running(dir));
        run_create_direct(dir, Some("alpha"), None, None, None, None, true).unwrap();

        // Graph contains a .chat-N task
        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat_tasks: Vec<_> = g
            .tasks()
            .filter(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .collect();
        assert_eq!(chat_tasks.len(), 1, "Should have created one chat task");
        assert!(chat_tasks[0].id.starts_with(".chat-"));
        assert_eq!(
            chat_tasks[0].executor_preset_name.as_deref(),
            Some("claude")
        );
        assert!(!chat_tasks[0].command_argv.is_empty());
        assert!(chat_tasks[0].working_dir.is_some());
    }

    #[test]
    fn create_custom_command_chat_stores_command_metadata() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("shell"), None, None, None, Some("bash"), true).unwrap();

        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g
            .tasks()
            .find(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .expect("chat task exists");
        assert_eq!(chat.executor_preset_name, None);
        assert_eq!(
            chat.command_argv,
            vec!["bash".to_string(), "-lc".to_string(), "bash".to_string()]
        );
        assert!(chat.working_dir.as_deref().is_some_and(|d| !d.is_empty()));
    }

    #[test]
    fn create_plain_pi_chat_stores_pi_with_no_model() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("plain-pi"), None, Some("pi"), None, None, true).unwrap();

        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g
            .tasks()
            .find(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .expect("chat task exists");
        assert_eq!(chat.executor_preset_name.as_deref(), Some("pi"));
        assert_eq!(chat.model, None);
        assert_eq!(chat.endpoint, None);
        assert_eq!(chat.command_argv, vec!["pi".to_string()]);
    }

    #[test]
    fn create_explicit_pi_chat_preserves_model() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(
            dir,
            Some("explicit-pi"),
            Some("pi:lunaroute:glm-5.2-nvfp4"),
            Some("pi"),
            None,
            None,
            true,
        )
        .unwrap();

        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g
            .tasks()
            .find(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .expect("chat task exists");
        assert_eq!(chat.executor_preset_name.as_deref(), Some("pi"));
        assert_eq!(chat.model.as_deref(), Some("pi:lunaroute:glm-5.2-nvfp4"));
        assert!(
            chat.command_argv
                .windows(2)
                .any(|w| w[0] == "--model" && w[1] == "pi:lunaroute:glm-5.2-nvfp4"),
            "{:?}",
            chat.command_argv
        );
    }

    #[test]
    fn migrate_legacy_preset_chat_writes_command_metadata() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let mut graph = worksgood::graph::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: ".chat-0".to_string(),
            title: "Chat 0".to_string(),
            status: worksgood::graph::Status::InProgress,
            tags: vec![chat_id::CHAT_LOOP_TAG.to_string()],
            model: Some("nex:qwen3-coder".to_string()),
            endpoint: Some("http://127.0.0.1:8088".to_string()),
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path(dir)).unwrap();

        migrate_existing_chat_tasks(dir).unwrap();

        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g.get_task(".chat-0").unwrap();
        assert_eq!(chat.executor_preset_name.as_deref(), Some("nex"));
        assert_eq!(chat.command_argv[0], "wg");
        assert!(chat.command_argv.contains(&"nex".to_string()));
        assert!(chat.working_dir.as_deref().is_some_and(|d| !d.is_empty()));
    }

    #[test]
    fn send_to_dormant_chat_appends_inbox() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("bot"), None, None, None, None, true).unwrap();

        // Find the chat id we just created
        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g
            .tasks()
            .find(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .expect("chat task exists");
        let cid = chat_id::parse_chat_task_id(&chat.id).unwrap();

        // Send
        run_send(dir, &cid.to_string(), "hi from test", true).unwrap();

        // Inbox file exists and has one message
        let inbox = worksgood::chat::chat_dir_for_ref(dir, &cid.to_string()).join("inbox.jsonl");
        let contents = std::fs::read_to_string(&inbox).expect("inbox file written");
        assert!(
            contents.contains("hi from test"),
            "inbox.jsonl should contain the message: {}",
            contents
        );
    }

    #[test]
    fn warm_pi_model_writeback_targets_exact_chat_and_is_idempotent() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(
            dir,
            Some("pi-chat"),
            Some("pi:openrouter:qwen/old"),
            Some("pi"),
            None,
            None,
            true,
        )
        .unwrap();

        run_model(dir, ".chat-0", "openrouter:qwen/qwen3.6-flash", true, true).unwrap();
        let state = crate::commands::service::CoordinatorState::load_for(dir, 0).unwrap();
        assert_eq!(state.executor_override.as_deref(), Some("pi"));
        assert_eq!(
            state.model_override.as_deref(),
            Some("pi:openrouter:qwen/qwen3.6-flash")
        );

        // A duplicate notification is a successful no-op, not a second write.
        assert!(
            !persist_chat_model_override(dir, 0, "pi", "pi:openrouter:qwen/qwen3.6-flash").unwrap()
        );
        assert!(
            crate::commands::service::CoordinatorState::load_for(dir, 1).is_none(),
            "write-back must not leak into any other chat"
        );
    }

    #[test]
    fn warm_pi_model_writeback_rejects_nonexistent_canonical_chat() {
        let td = mk_workgraph_dir();
        let err = run_model(
            td.path(),
            ".chat-41",
            "llamacpp:llama-3.3-local",
            true,
            true,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("No graph chat task"));
    }

    #[test]
    fn resume_errors_clearly_when_service_down() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("c"), None, None, None, None, true).unwrap();
        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = g
            .tasks()
            .find(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
            .unwrap();
        let cid = chat_id::parse_chat_task_id(&chat.id).unwrap();

        let err = run_resume(dir, &cid.to_string(), true).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("daemon is not running") || msg.contains("not running"),
            "Resume error should explain service is down: {}",
            msg
        );
    }

    #[test]
    fn resume_metadata_falls_back_to_chat_task_model() {
        // No CoordinatorState on disk — the model must be reconstructed
        // from the chat task itself (the common TUI-created-chat case).
        let td = mk_workgraph_dir();
        let dir = td.path();
        let mut graph = worksgood::graph::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: ".chat-23".to_string(),
            title: "Chat 23".to_string(),
            status: worksgood::graph::Status::InProgress,
            tags: vec![chat_id::CHAT_LOOP_TAG.to_string()],
            model: Some("openrouter:minimax/minimax-m3".to_string()),
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path(dir)).unwrap();

        let (executor, model) = reconstruct_resume_metadata(dir, 23);
        assert_eq!(executor, None);
        assert_eq!(model.as_deref(), Some("openrouter:minimax/minimax-m3"));
        // The reconstructed pair is non-empty, so the SetChatExecutor IPC
        // will NOT hit the "at least one of --executor or --model" error.
        assert!(
            executor.is_some() || model.is_some(),
            "resume must supply saved metadata so the swap IPC is accepted"
        );
    }

    #[test]
    fn resume_metadata_derives_executor_from_preset_when_no_model() {
        // A chat created with `--exec nex` and no explicit model: the model
        // is absent everywhere, but the executor preset is recorded on the
        // task. Resume must still yield a non-empty pair so the swap IPC is
        // accepted (otherwise it falls through to the hidden-flags error).
        let td = mk_workgraph_dir();
        let dir = td.path();
        let mut graph = worksgood::graph::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: ".chat-3".to_string(),
            title: "Chat 3".to_string(),
            status: worksgood::graph::Status::InProgress,
            tags: vec![chat_id::CHAT_LOOP_TAG.to_string()],
            executor_preset_name: Some("nex".to_string()),
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path(dir)).unwrap();

        let (executor, model) = reconstruct_resume_metadata(dir, 3);
        assert_eq!(executor.as_deref(), Some("nex"));
        assert_eq!(model, None);
        assert!(
            executor.is_some() || model.is_some(),
            "resume must supply at least the executor preset"
        );
    }

    #[test]
    fn resume_metadata_prefers_coordinator_state_overrides() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let mut graph = worksgood::graph::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: ".chat-7".to_string(),
            title: "Chat 7".to_string(),
            status: worksgood::graph::Status::InProgress,
            tags: vec![chat_id::CHAT_LOOP_TAG.to_string()],
            model: Some("nex:qwen3-coder".to_string()),
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path(dir)).unwrap();

        // A per-chat hot-swap was persisted to CoordinatorState — it wins.
        let mut state = crate::commands::service::CoordinatorState::load_or_default_for(dir, 7);
        state.executor_override = Some("native".to_string());
        state.model_override = Some("openrouter:anthropic/claude-opus-4-7".to_string());
        state.save_for(dir, 7);

        let (executor, model) = reconstruct_resume_metadata(dir, 7);
        assert_eq!(executor.as_deref(), Some("native"));
        assert_eq!(
            model.as_deref(),
            Some("openrouter:anthropic/claude-opus-4-7")
        );
    }

    #[test]
    fn list_truthful_status_when_service_down() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("alpha"), None, None, None, None, true).unwrap();
        run_create_direct(dir, Some("beta"), None, None, None, None, true).unwrap();

        // Build the in-memory representation list_truthfully would emit.
        let g = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        for task in g
            .tasks()
            .filter(|t| t.tags.iter().any(|x| chat_id::is_chat_loop_tag(x)))
        {
            let status = classify_chat_task(task, false, &[]);
            assert_eq!(
                status,
                ChatRuntimeStatus::Dormant,
                "Daemon down — every chat should be Dormant"
            );
        }
    }

    #[test]
    fn terminal_chat_tasks_cannot_be_resumed_even_with_runtime_proof() {
        for (status, archived) in [
            (Status::Done, false),
            (Status::Done, true),
            (Status::Abandoned, false),
            (Status::Failed, false),
        ] {
            let mut graph = WorkGraph::new();
            let mut tags = vec![chat_id::CHAT_LOOP_TAG.to_string()];
            if archived {
                tags.push("archived".to_string());
            }
            graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
                id: ".chat-9".to_string(),
                status,
                tags,
                ..Default::default()
            }));

            assert!(
                validate_chat_resumable(&graph, 9).is_err(),
                "terminal status {status:?} must reject resume"
            );
            assert!(
                !resume_runtime_proof_is_valid(&graph, 9, true),
                "stale tmux proof must not revive {status:?}"
            );
        }
    }

    #[test]
    fn resume_wait_never_turns_scheduling_ack_into_false_success() {
        let mut probes = 0;
        let live = wait_for_chat_runtime_with(
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
            || {
                probes += 1;
                false
            },
        );
        assert!(!live);
        assert!(probes >= 1);
    }

    #[test]
    fn resume_wait_observes_delayed_runtime_within_bound() {
        let mut probes = 0;
        let live = wait_for_chat_runtime_with(
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(1),
            || {
                probes += 1;
                probes >= 3
            },
        );
        assert!(live);
    }

    #[test]
    fn tui_owned_runtime_promotes_stopped_and_dormant_to_supervised() {
        assert_eq!(
            refine_status_with_runtime(ChatRuntimeStatus::Stopped, true),
            ChatRuntimeStatus::Supervised
        );
        assert_eq!(
            refine_status_with_runtime(ChatRuntimeStatus::Dormant, true),
            ChatRuntimeStatus::Supervised
        );
        assert_eq!(
            refine_status_with_runtime(ChatRuntimeStatus::Stopped, false),
            ChatRuntimeStatus::Stopped
        );
    }

    #[test]
    fn classify_archived_and_deleted() {
        let mut t = worksgood::graph::Task::default();
        t.id = ".chat-1".to_string();
        t.tags = vec![chat_id::CHAT_LOOP_TAG.to_string(), "archived".to_string()];
        t.status = Status::Done;
        assert_eq!(
            classify_chat_task(&t, true, &[1]),
            ChatRuntimeStatus::Archived
        );

        let mut t2 = worksgood::graph::Task::default();
        t2.id = ".chat-2".to_string();
        t2.tags = vec![chat_id::CHAT_LOOP_TAG.to_string()];
        t2.status = Status::Abandoned;
        assert_eq!(
            classify_chat_task(&t2, true, &[2]),
            ChatRuntimeStatus::Deleted
        );

        let mut t3 = worksgood::graph::Task::default();
        t3.id = ".chat-3".to_string();
        t3.tags = vec![chat_id::CHAT_LOOP_TAG.to_string()];
        t3.status = Status::InProgress;
        // Daemon up, supervised
        assert_eq!(
            classify_chat_task(&t3, true, &[3]),
            ChatRuntimeStatus::Supervised
        );
        // Daemon up but not supervised → stopped
        assert_eq!(
            classify_chat_task(&t3, true, &[]),
            ChatRuntimeStatus::Stopped
        );
        // Daemon down → dormant
        assert_eq!(
            classify_chat_task(&t3, false, &[]),
            ChatRuntimeStatus::Dormant
        );
    }
}
