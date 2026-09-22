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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use worksgood::chat_id;
use worksgood::dispatch::handler_for_model;
use worksgood::graph::{Status, WorkGraph};
use worksgood::pi_plugin::{self, CacheState, EnsureMode, PluginStatus, ResolvedPlugin, Source};

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
    /// Chat task carries the `archived` tag after lifecycle retirement.
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
    // Full task ID form — including the bare "chat-N" spelling used by pi
    // transcripts (`--session-id chat-N`), tmux session names, and the TUI.
    // Users reach for exactly this string; make it addressable everywhere.
    let dotted = if let Some(rest) = reference.strip_prefix("chat-") {
        rest.parse::<u32>().ok().map(|_| format!(".chat-{rest}"))
    } else {
        None
    };
    let parsed = dotted
        .as_deref()
        .and_then(chat_id::parse_chat_task_id)
        .or_else(|| chat_id::parse_chat_task_id(reference));
    if let Some(n) = parsed {
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
    // A bare, explicitly-attended Pi console is the one route-free LLM chat:
    // Pi owns login/model selection inside its own UI. Every other chat still
    // fails closed unless its invocation or repository config selects a route.
    // This exception is deliberately keyed to explicit `--exec pi` + no model;
    // it cannot weaken unattended worker/evaluator service validation.
    let attended_bare_pi = executor == Some("pi") && model.is_none();
    let selection = if attended_bare_pi {
        None
    } else {
        Some(worksgood::execution_selection::require(
            dir,
            model.map(|m| (m, false)),
            "wg chat create",
        )?)
    };
    // The explicit chat route is independently sufficient, just like an
    // explicit task.model. Requiring every unrelated worker/agency role to be
    // configured here made `wg chat create -m codex:<model>` fail in an
    // otherwise graph-only project even though no fallback was needed.
    // Service startup still validates its full execution plane before it can
    // supervise this chat.
    // Plain interactive Pi is a terminal-hosted vendor console, not the
    // hermetic wg pi-handler/plugin transport. Validate that exact executable
    // before IPC or graph/session mutation so a missing Pi is visible and
    // transactional. An explicit --exec wins over the model/profile handler.
    let selected_executor = executor.or_else(|| {
        selection
            .as_ref()
            .and_then(|selection| selection.system.as_ref())
            .map(|system| system.handler.as_str())
    });
    if command.is_none() {
        require_interactive_executor_binary(selected_executor)?;
    }
    // This identity belongs to the logical create, not to its socket attempt.
    // The daemon persists it on the graph row before replying, so a lost/late
    // response reconciles to the exact committed chat instead of duplicating.
    let request_id = format!("chat-create-{}", uuid::Uuid::now_v7());
    if service_is_running(dir) {
        run_create_via_ipc(
            dir,
            &request_id,
            name,
            model,
            executor,
            endpoint,
            command,
            json,
        )
    } else {
        run_create_direct(dir, name, model, executor, endpoint, command, json)
    }
}

#[cfg(unix)]
fn run_create_via_ipc(
    dir: &Path,
    request_id: &str,
    name: Option<&str>,
    model: Option<&str>,
    executor: Option<&str>,
    endpoint: Option<&str>,
    command: Option<&str>,
    json: bool,
) -> Result<()> {
    crate::commands::service::run_create_coordinator(
        dir, request_id, name, model, executor, endpoint, command, json,
    )
}

#[cfg(not(unix))]
fn run_create_via_ipc(
    _dir: &Path,
    _request_id: &str,
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
    state.advance_route_generation();
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

// ============================================================================
// Subcommand: fork
// ============================================================================

/// Atomically install one forked transcript file: write to a unique temp name
/// in the destination dir, then rename over the final name. A handler that
/// races the copy observes either nothing or the complete file — never a
/// partial transcript.
fn atomic_copy(src: &Path, dst: &Path) -> Result<()> {
    let bytes = std::fs::read(src).with_context(|| format!("read {:?}", src))?;
    let tmp = dst.with_file_name(format!(
        ".fork-tmp-{}-{}",
        std::process::id(),
        dst.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("transcript")
    ));
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {:?}", tmp))?;
    std::fs::rename(&tmp, dst).with_context(|| format!("rename {:?} -> {:?}", tmp, dst))
}

/// `wg chat fork <ref> [--name <name>]` — fork a Pi chat into a NEW,
/// independent chat that starts from the same conversation history.
///
/// Mechanics: allocate the next chat id through the exact create path, copy
/// the parent's pi transcript (`<ts>_chat-N.jsonl`, plus its wake cursor) into
/// the fork's `pi-sessions/` under the fork's `--session-id` name, then let
/// the supervisor spawn the fork. Pi's `--session-id chat-M` contract picks up
/// the pre-seeded transcript — the fork opens with the full history and
/// evolves independently; the parent is untouched (still live if it was).
///
/// This is the warm, pi-native fork: no `pi --fork` needed, the transcript is
/// just a file any pi can open. Daemon-up, the fork's first (empty) handler
/// spawn is stopped, the transcript is installed, and the handler is resumed —
/// a warm reboot over the seeded session — so the live fork provably runs on
/// the copied history. Daemon-down, the fork is created dormant with the
/// transcript already in place (nothing can race: there is no supervisor).
pub fn run_fork(dir: &Path, reference: &str, name: Option<&str>, json: bool) -> Result<()> {
    migrate_existing_chat_tasks(dir)?;
    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;
    let cid = resolve_chat_id(&graph, reference)
        .with_context(|| format!("No chat matching '{}'", reference))?;
    validate_chat_resumable(&graph, cid)?;

    // Forking is Pi-chat-only today: the transcript-copy mechanism is keyed to
    // pi's `--session-id` naming. A claude/codex chat fork needs its own
    // transcript story; refuse loudly rather than silently forking nothing.
    let (executor, model) = reconstruct_resume_metadata(dir, cid);
    let task_executor =
        chat_id::find_chat_task(&graph, cid).and_then(|task| task.executor_preset_name.clone());
    let effective_executor = executor
        .or(task_executor)
        .unwrap_or_else(|| "pi".to_string());
    if effective_executor != "pi" {
        anyhow::bail!(
            "Chat fork currently supports Pi chats only (chat {} runs {}). \
             Fork `wg session` journals instead: `wg session fork chat-{cid}`.",
            cid,
            effective_executor
        );
    }

    // The parent transcript must exist — forking a conversation that never
    // started is just `wg chat create` with extra steps.
    let source_ref = format!("chat-{cid}");
    worksgood::chat_sessions::prepare_pi_chat_session(dir, cid)
        .with_context(|| format!("prepare source chat storage for {source_ref}"))?;
    let source_dir = worksgood::chat::chat_dir_for_ref(dir, &source_ref);
    let source_session_dir = source_dir.join("pi-sessions");
    let source_transcript =
        worksgood::chat_sessions::newest_pi_transcript(&source_session_dir, cid).ok_or_else(
            || {
                anyhow::anyhow!(
                    "Chat {cid} has no pi transcript under {} — nothing to fork. \
                 Send the chat a message first, then retry.",
                    source_session_dir.display()
                )
            },
        )?;

    // Allocate the fork through the exact create path (IPC when the daemon is
    // up, direct graph commit when down). It inherits the parent's pinned
    // model; the executor is pi by the gate above.
    let fork_name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("fork of chat-{cid}"));
    let new_cid = crate::commands::service::create_chat_cid(
        dir,
        Some(&fork_name),
        model.as_deref(),
        Some("pi"),
        None,
        None,
    )?;
    let fork_ref = format!("chat-{new_cid}");

    // Register the fork's session storage NOW (idempotent) so the transcript
    // has a home regardless of spawn ordering.
    worksgood::chat_sessions::prepare_pi_chat_session(dir, new_cid)
        .with_context(|| format!("prepare fork storage for {fork_ref}"))?;
    let fork_dir = worksgood::chat::chat_dir_for_ref(dir, &fork_ref);
    let fork_session_dir = fork_dir.join("pi-sessions");

    let install_fork_transcript = || -> Result<()> {
        std::fs::create_dir_all(&fork_session_dir)
            .with_context(|| format!("create {:?}", fork_session_dir))?;
        let source_name = source_transcript
            .file_name()
            .and_then(|n| n.to_str())
            .context("source transcript filename")?
            .to_string();
        let fork_stem = source_name
            .strip_suffix(&format!("_chat-{cid}.jsonl"))
            .unwrap_or(&source_name)
            .to_string();
        let fork_transcript = fork_session_dir.join(format!("{fork_stem}_chat-{new_cid}.jsonl"));
        atomic_copy(&source_transcript, &fork_transcript)?;
        // Pi keys the session identity on the `id` INSIDE line 1 of the
        // transcript, not the filename. Rewrite only that header so
        // `--session-id chat-{new_cid}` adopts the copied history instead of
        // silently starting a fresh session (the fork bug).
        worksgood::chat_sessions::rekey_pi_session_header(&fork_transcript, &fork_ref)
            .with_context(|| format!("rekey fork transcript for {fork_ref}"))?;
        // Carry the wake cursor so wake events the parent already consumed do
        // not replay into the fork on its first turn.
        let source_cursor =
            source_transcript.with_file_name(format!("{}.wg-wake-cursor.json", source_name));
        if source_cursor.exists() {
            let fork_cursor = fork_transcript.with_file_name(format!(
                "{fork_stem}_chat-{new_cid}.jsonl.wg-wake-cursor.json"
            ));
            atomic_copy(&source_cursor, &fork_cursor)?;
        }
        // If the supervisor raced the seed and pi already created its own empty
        // transcript under the fork's session-id, retire the stray (and its
        // cursor) so `--session-id chat-{new_cid}` resolves to exactly the
        // forked history. Safe: install only runs while no legit fork turns
        // exist (a raced handler is stopped before re-install).
        let fork_suffix = format!("_chat-{new_cid}.jsonl");
        for entry in std::fs::read_dir(&fork_session_dir)
            .with_context(|| format!("read {:?}", fork_session_dir))?
            .flatten()
        {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(&fork_suffix) && path != fork_transcript {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(
                    path.with_file_name(format!("{name}.wg-wake-cursor.json")),
                );
            }
        }
        Ok(())
    };

    if service_is_running(dir) {
        // Deterministic sequencing over racing the supervisor's 5s poll: seed
        // the transcript first, repair the rare race (supervisor spawned an
        // empty handler mid-seed → stop it and re-assert the seed), then force
        // the spawn and wait for stable liveness. The live fork provably runs
        // the copied history.
        install_fork_transcript()?;
        if chat_handler_is_live(dir, new_cid) {
            crate::commands::service::stop_chat_quiet(dir, new_cid)?;
            install_fork_transcript()?;
        }
        request_chat_resume(dir, new_cid)?;
        if !wait_for_stable_chat_runtime_with(RESUME_LIVE_TIMEOUT, RESUME_LIVE_POLL, || {
            chat_handler_is_live(dir, new_cid)
        }) {
            // The fork may legitimately stay unspawned (idle rule, no
            // consumer). The seed is already in place; the supervisor brings
            // it up when a consumer attaches — not an error.
            install_fork_transcript()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "source_chat_id": cid,
                        "fork_chat_id": new_cid,
                        "fork_task_id": chat_id::format_chat_task_id(new_cid),
                        "forked": true,
                        "live": false,
                        "note": "transcript seeded; handler not yet live (no consumer attached)"
                    }))?
                );
            } else {
                println!(
                    "Forked chat {cid} → {new_cid} (task {}).",
                    chat_id::format_chat_task_id(new_cid)
                );
                println!(
                    "  history: {} → {}",
                    source_transcript.display(),
                    fork_session_dir.display()
                );
                println!(
                    "  transcript seeded; the handler is not yet live (no consumer) — it spawns on attach"
                );
            }
            return Ok(());
        }
    } else {
        // Daemon down: no supervisor, no race. Seed and leave dormant.
        install_fork_transcript()?;
    }

    let lines = vec![
        format!(
            "Forked chat {cid} → {new_cid} (task {}).",
            chat_id::format_chat_task_id(new_cid)
        ),
        format!(
            "  history: {} → {}",
            source_transcript.display(),
            fork_session_dir.display()
        ),
        format!(
            "  parent untouched and still independent; open the fork in the TUI or `wg chat attach {fork_ref}`"
        ),
    ];
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "source_chat_id": cid,
                "fork_chat_id": new_cid,
                "fork_task_id": chat_id::format_chat_task_id(new_cid),
                "forked": true,
                "live": true,
            }))?
        );
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
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

/// Schedule a handler (re)spawn through the supervisor using the chat's saved
/// executor/model metadata (`reconstruct_resume_metadata`), without waiting for
/// liveness. Shared by `wg chat resume` and `wg chat reload` so both use the
/// exact same proven respawn path. Errors when the daemon rejects the request.
pub(crate) fn request_chat_resume(dir: &Path, cid: u32) -> Result<()> {
    use crate::commands::service::ipc::IpcRequest;
    use crate::commands::service::send_request;
    let (executor, model) = reconstruct_resume_metadata(dir, cid);
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
        anyhow::bail!("{}", msg);
    }
    Ok(())
}

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

/// How long liveness must hold continuously before a (re)spawn counts as
/// settled. The daemon-supervised adapter holds the `.handler.pid` session
/// lock for a brief window even when the respawn is about to be REFUSED
/// (identity mismatch / budget exhausted) — a single live observation at that
/// moment is a false positive, and `wg chat resume` would report success for
/// a chat that dies a second later. Requiring liveness to hold for this long
/// closes that gap; a genuinely live pi handler holds it trivially.
const RESUME_LIVE_SETTLE: Duration = Duration::from_millis(500);

/// Like [`wait_for_chat_runtime_with`], but success requires liveness to hold
/// CONTINUOUSLY for [`RESUME_LIVE_SETTLE`] (clamped to the timeout — a caller
/// that explicitly asked for a 0s wait gets the old single-observation
/// behavior rather than a guaranteed timeout). A blip (adapter lock acquired
/// then released on refusal) resets the timer instead of passing.
fn wait_for_stable_chat_runtime_with(
    timeout: Duration,
    poll: Duration,
    mut is_live: impl FnMut() -> bool,
) -> bool {
    let settle = RESUME_LIVE_SETTLE.min(timeout);
    let deadline = Instant::now() + timeout;
    let mut live_since: Option<Instant> = None;
    loop {
        if is_live() {
            let since = *live_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= settle {
                return true;
            }
        } else {
            live_since = None;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
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
    // Verify the spawn-recorded binding before any resume/reattach. A sentinel
    // that names a different chat surface is a loud refusal, never a silent
    // reattach into the wrong conversation (`chat-identity-is`).
    if let Some(info) = worksgood::session_lock::read_tui_driver_sentinel(&chat_dir)
        .ok()
        .flatten()
        && let Some(bound) = info.chat_ref.as_deref()
        && bound != chat_ref.as_str()
    {
        anyhow::bail!(
            "WG-CHAT-IDENTITY-SESSION-MISMATCH: cannot resume chat {cid}: its .tui-driven sentinel is bound to {bound}, not {chat_ref}. \
             Refusing to reattach the wrong chat; run `wg chat reload {cid}` after resolving the conflicting driver."
        );
    }
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

    if let Err(e) = request_chat_resume(dir, cid) {
        let msg = format!("{e}");
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
    if !wait_for_stable_chat_runtime_with(RESUME_LIVE_TIMEOUT, RESUME_LIVE_POLL, || {
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

// ============================================================================
// Subcommand: reload
// ============================================================================

/// Bounded window for the respawned handler to prove liveness.
const RELOAD_LIVE_TIMEOUT: Duration = Duration::from_secs(10);
const RELOAD_POLL: Duration = Duration::from_millis(100);

/// Identity of one executable: path, mtime, size, and a content digest. `None`
/// fields mean the file could not be inspected (missing/unreadable), never a
/// fabricated value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct BinaryIdentity {
    pub path: String,
    pub mtime_unix: Option<u64>,
    pub size_bytes: Option<u64>,
    pub digest: Option<String>,
}

/// A handler's process identity, captured before/after a reload. Provenance is
/// explicit (`source`) because a daemon handler holds WG's `.handler.pid` lock
/// while a TUI-driven handler owns a persistent tmux pane and holds no lock.
/// The `pid`/`started_at` pair defeats PID reuse: the same PID with a
/// different start time is a different generation. `live == false` means no
/// handler currently owns the chat.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HandlerIdentity {
    pub source: String,
    pub pid: Option<u32>,
    /// Lock-file ISO timestamp (daemon handler) or `/proc` start time (pane).
    pub started_at: Option<String>,
    pub live: bool,
}

impl HandlerIdentity {
    fn none() -> Self {
        Self {
            source: "none".to_string(),
            pid: None,
            started_at: None,
            live: false,
        }
    }
}

/// The full observable identity of a chat + its runtime before or after a
/// reload. Captured for before/after comparison.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ChatReloadIdentity {
    pub chat_id: u32,
    pub executor: Option<String>,
    pub model: Option<String>,
    pub wg_binary: Option<BinaryIdentity>,
    pub pi_binary: Option<BinaryIdentity>,
    pub plugin_compat: String,
    pub plugin_source: String,
    pub plugin_entry: String,
    pub embed_digest: String,
    pub cache_digest: Option<String>,
    pub cache_state: String,
    pub session_file: Option<String>,
    pub session_message_count: Option<usize>,
    pub handler: HandlerIdentity,
}

/// The before→after delta `wg chat reload` prints.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ChatReloadDelta {
    pub wg_binary_changed: bool,
    pub pi_binary_changed: bool,
    pub plugin_digest_changed: bool,
    pub session_file_preserved: bool,
    pub message_count_before: Option<usize>,
    pub message_count_after: Option<usize>,
    /// True only when the handler's process identity actually changed (new pid
    /// or start time / new ownership source). A reload that leaves the old
    /// handler running is refused before this is ever printed.
    pub handler_replaced: bool,
}

/// A compact, serializable view of the plugin resolution the reload performed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PluginResolution {
    pub compat: String,
    pub source: String,
    pub entry: String,
    pub root: String,
}

/// The result of one successful chat reload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ChatReloadOutcome {
    pub chat_id: u32,
    pub plugin: PluginResolution,
    pub before: ChatReloadIdentity,
    pub after: ChatReloadIdentity,
    pub delta: ChatReloadDelta,
}

/// Runtime side effects of a reload, injectable so unit tests can drive the
/// respawn/liveness state machine with a fake handler + fake daemon.
pub(crate) trait ChatReloadRuntime {
    /// Re-materialize the embedded pi plugin (idempotent).
    fn ensure_plugin(&self) -> Result<ResolvedPlugin>;
    /// Current read-only plugin status (cache state / digests).
    fn plugin_status(&self) -> Result<PluginStatus>;
    /// Signal the live handler and ask the supervisor to respawn it against the
    /// same session (the exact `SetChatExecutor` path `wg chat resume` uses).
    /// For a TUI-driven chat this first takes ownership of the pane (see
    /// [`take_tui_chat_ownership`]) — the supervisor otherwise deliberately
    /// defers a respawn while a TUI tmux pane is live, so the signal alone
    /// would leave the old handler running.
    fn respawn_handler(&self, dir: &Path, cid: u32) -> Result<()>;
    /// The current handler's process identity. `HandlerIdentity::live == false`
    /// means no handler owns the chat right now.
    fn handler_identity(&self, dir: &Path, cid: u32) -> HandlerIdentity;
}

/// The production runtime: real `ensure-pi-plugin`, real daemon IPC, real lock
/// probe. Every method delegates to an existing primitive.
pub(crate) struct RealChatReloadRuntime;

impl ChatReloadRuntime for RealChatReloadRuntime {
    fn ensure_plugin(&self) -> Result<ResolvedPlugin> {
        pi_plugin::ensure_pi_plugin(EnsureMode::Hermetic)
            .context("ensure-pi-plugin (Hermetic) before chat reload")
    }

    fn plugin_status(&self) -> Result<PluginStatus> {
        Ok(pi_plugin::status())
    }

    fn respawn_handler(&self, dir: &Path, cid: u32) -> Result<()> {
        // TUI-driven chats are not addressable by the daemon's `.handler.pid`
        // signal and the supervisor refuses to spawn beside a live pane, so the
        // `SetChatExecutor` IPC below would be a no-op for them. Take ownership
        // first: stop the pane + clear the `.tui-driven` sentinel, then let the
        // supervisor respawn so the TUI re-attaches to a fresh pane on its next
        // view/refresh. See this function's doc for the live-TUI interaction.
        take_tui_chat_ownership(dir, cid);
        request_chat_resume(dir, cid)
    }

    fn handler_identity(&self, dir: &Path, cid: u32) -> HandlerIdentity {
        capture_handler_identity(dir, cid)
    }
}

/// Capture the chat's concrete handler identity. Prefers the daemon lock
/// (`.handler.pid`) because a supervised handler owns it; falls back to the
/// persistent TUI tmux pane PID for vendor panes that never take WG's lock.
/// Returns `HandlerIdentity::none()` when neither owner is live.
pub(crate) fn capture_handler_identity(dir: &Path, cid: u32) -> HandlerIdentity {
    let chat_ref = format!("chat-{cid}");
    let chat_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref);
    if let Ok(Some(holder)) = worksgood::session_lock::read_holder(&chat_dir)
        && holder.alive
    {
        return HandlerIdentity {
            source: "daemon-lock".to_string(),
            pid: Some(holder.pid),
            started_at: (!holder.started_at.is_empty()).then(|| holder.started_at.clone()),
            live: true,
        };
    }
    if let Some(pid) = worksgood::chat_id::chat_tmux_pane_pid(dir, cid) {
        return HandlerIdentity {
            source: "tui-tmux-pane".to_string(),
            pid: Some(pid),
            started_at: worksgood::session_lock::process_start_time(pid),
            live: true,
        };
    }
    HandlerIdentity::none()
}

/// Take ownership of a TUI-driven chat's handler so the daemon supervisor will
/// actually respawn it. Returns `true` when a TUI owner was present and torn
/// down.
///
/// Interaction with a live user-facing TUI: the pane process is killed and the
/// `.tui-driven` sentinel is cleared. A TUI currently attached to that pane
/// sees its PTY close; on the next view/refresh of the chat its
/// `maybe_auto_enable_chat_pty` path finds no live pane and spawns a fresh one
/// (which loads the freshly materialized plugin). This is a takeover, not a
/// cooperative handshake — `wg chat reload` is an explicit operator action and
/// the caller has already proven a live owner exists.
pub(crate) fn take_tui_chat_ownership(dir: &Path, cid: u32) -> bool {
    let chat_ref = format!("chat-{cid}");
    let chat_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref);
    let sentinel_live = worksgood::session_lock::active_tui_driver_pid(&chat_dir).is_some();
    let tmux_live = worksgood::chat_id::chat_tmux_session_is_live(dir, cid);
    if !sentinel_live && !tmux_live {
        return false;
    }
    if tmux_live {
        worksgood::chat_id::kill_chat_tmux_session_for_id(dir, cid);
    }
    worksgood::session_lock::clear_tui_driver_sentinel(&chat_dir);
    true
}

/// Content digest of an executable, BLAKE3 over the full bytes. Content (not
/// just mtime) is what lets the delta distinguish "a different binary was
/// installed under the same path" from "the same binary respawned".
fn binary_identity(path: &Path) -> Option<BinaryIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime_unix = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let digest = std::fs::read(path).ok().map(|bytes| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&bytes);
        format!("b3:{}", hasher.finalize().to_hex())
    });
    Some(BinaryIdentity {
        path: path.display().to_string(),
        mtime_unix,
        size_bytes: Some(metadata.len()),
        digest,
    })
}

/// Newest pi transcript for `chat-N` under `<chat_dir>/pi-sessions`, matching
/// pi-handler's `--session-id <chat_ref>` naming (`*_chat-N.jsonl`). `None`
/// means no transcript exists yet (a fresh session).
fn find_chat_session_file(dir: &Path, cid: u32) -> Option<PathBuf> {
    let chat_ref = format!("chat-{cid}");
    let session_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref).join("pi-sessions");
    worksgood::chat_sessions::newest_pi_transcript(&session_dir, cid)
}

/// Count JSONL turns in a pi transcript. Cheap: one read + line count.
fn count_jsonl_messages(path: &Path) -> Option<usize> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().filter(|line| !line.trim().is_empty()).count())
}

/// Capture the observable identity of a chat's runtime. Pure with respect to
/// `plugin`, the injected binary paths, and the pre-captured `handler`, so
/// tests can pin the delta without a live daemon or a real `pi` install.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_chat_reload_identity(
    dir: &Path,
    cid: u32,
    executor: Option<String>,
    model: Option<String>,
    wg_binary: Option<&Path>,
    pi_binary: Option<&Path>,
    plugin: &PluginStatus,
    handler: HandlerIdentity,
) -> ChatReloadIdentity {
    let session_file = find_chat_session_file(dir, cid);
    let session_message_count = session_file.as_deref().and_then(count_jsonl_messages);
    ChatReloadIdentity {
        chat_id: cid,
        executor,
        model,
        wg_binary: wg_binary.and_then(binary_identity),
        pi_binary: pi_binary.and_then(binary_identity),
        plugin_compat: plugin.compat.clone(),
        plugin_source: format!("{:?}", plugin.source),
        plugin_entry: plugin.dist_entry.display().to_string(),
        embed_digest: plugin.embed_digest.clone(),
        cache_digest: plugin.cache_digest.clone(),
        cache_state: format!("{:?}", plugin.cache_state),
        session_file: session_file.map(|p| p.display().to_string()),
        session_message_count,
        handler,
    }
}

/// Compare two captured identities. `session_file_preserved` requires the same
/// path on both sides; message count is informational.
pub(crate) fn compute_reload_delta(
    before: &ChatReloadIdentity,
    after: &ChatReloadIdentity,
) -> ChatReloadDelta {
    ChatReloadDelta {
        wg_binary_changed: before.wg_binary != after.wg_binary,
        pi_binary_changed: before.pi_binary != after.pi_binary,
        plugin_digest_changed: before.cache_digest != after.cache_digest,
        session_file_preserved: before.session_file.is_some()
            && before.session_file == after.session_file,
        message_count_before: before.session_message_count,
        message_count_after: after.session_message_count,
        handler_replaced: before.handler != after.handler,
    }
}

/// Fail-closed freshness gate applied AFTER `ensure-pi-plugin`. When this
/// binary ships the cache path (`Source::Cache`), the cache must be byte-for-
/// byte current with the binary's embed. A cache that is still drifted here is
/// stale-and-unrefreshable: refuse loudly rather than respawn `pi` against old
/// extension bytes.
pub(crate) fn plugin_cache_freshness_gate(status: &PluginStatus) -> Result<()> {
    if status.source == Source::Cache && status.cache_state != CacheState::Current {
        anyhow::bail!(
            "WG-CHAT-RELOAD-PLUGIN-STALE: pi plugin cache is {:?} after ensure-pi-plugin and cannot be refreshed; \
             embed={} cache={} compat={} entry={}. \
             Refusing to respawn against stale extension bytes. Run `wg pi-plugin install` with this binary, or check \
             for write permission on {}. The chat was left untouched and is resumable with `wg chat resume`.",
            status.cache_state,
            status.embed_digest,
            status.cache_digest.as_deref().unwrap_or("<none>"),
            status.compat,
            status.dist_entry.display(),
            status.cache_version_dir.display(),
        );
    }
    Ok(())
}

/// A reload must resume the same conversation. No transcript means the
/// session-preserving guarantee cannot be honored — refuse rather than
/// silently start a blank session.
pub(crate) fn session_file_gate(session_file: Option<&str>, cid: u32) -> Result<()> {
    if session_file.is_none() {
        anyhow::bail!(
            "WG-CHAT-RELOAD-SESSION-MISSING: chat {cid} has no pi session transcript under its pi-sessions/ dir. \
             A reload cannot preserve a conversation that has no session file. Send the chat a message first so pi \
             creates its transcript, then retry `wg chat reload {cid}`. The chat was left untouched."
        );
    }
    Ok(())
}

fn plugin_resolution(plugin: &ResolvedPlugin) -> PluginResolution {
    PluginResolution {
        compat: plugin.compat.clone(),
        source: format!("{:?}", plugin.source),
        entry: plugin.dist_entry.display().to_string(),
        root: plugin.root.display().to_string(),
    }
}

/// Outcome of waiting for a respawned handler whose identity differs from the
/// one captured before the respawn.
enum HandlerReplaceOutcome {
    /// A live handler with a NEW identity holds the chat.
    Replaced(HandlerIdentity),
    /// A live handler is present but is the SAME generation as before — the
    /// respawn did not actually replace it.
    StillOld(HandlerIdentity),
    /// No live handler appeared at all.
    NotLive,
}

/// Wait until a live handler whose identity DIFFERS from `before` holds the
/// chat, requiring that state to hold for [`RESUME_LIVE_SETTLE`] (clamped to
/// `timeout`). Returns the new identity on success. When the deadline passes,
/// distinguishes "a live handler is still the old one" (refuse as NOT REPLACED)
/// from "nothing is live" (refuse as NOT LIVE) so the caller can print the
/// right error. `before.live == false` (no prior handler) is satisfied by any
/// live identity.
fn wait_for_replaced_handler(
    timeout: Duration,
    poll: Duration,
    before: &HandlerIdentity,
    mut current: impl FnMut() -> HandlerIdentity,
) -> HandlerReplaceOutcome {
    let settle = RESUME_LIVE_SETTLE.min(timeout);
    let deadline = Instant::now() + timeout;
    let mut replaced_since: Option<Instant> = None;
    loop {
        let last = current();
        if last.live && &last != before {
            let since = *replaced_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= settle {
                return HandlerReplaceOutcome::Replaced(last);
            }
        } else {
            replaced_since = None;
        }
        if Instant::now() >= deadline {
            return if last.live {
                HandlerReplaceOutcome::StillOld(last)
            } else {
                HandlerReplaceOutcome::NotLive
            };
        }
        std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// One reload: capture → ensure → gate → respawn (signals + restarts the live
/// handler over the same session) → wait for a REPLACED handler → capture →
/// delta. Every refusal happens before the respawn when possible, so a refused
/// reload leaves the live handler running.
pub(crate) fn reload_one(
    dir: &Path,
    cid: u32,
    runtime: &dyn ChatReloadRuntime,
    wg_binary: Option<&Path>,
    pi_binary: Option<&Path>,
    live_timeout: Duration,
    poll: Duration,
) -> Result<ChatReloadOutcome> {
    let (executor, model) = reconstruct_resume_metadata(dir, cid);

    // BEFORE identity, captured against the cache as it exists right now (so a
    // stale cache shows up in the delta when ensure refreshes it), including
    // the concrete handler process the respawn is meant to replace.
    let before_status = runtime.plugin_status()?;
    let before_handler = runtime.handler_identity(dir, cid);
    let before = capture_chat_reload_identity(
        dir,
        cid,
        executor,
        model,
        wg_binary,
        pi_binary,
        &before_status,
        before_handler.clone(),
    );

    // Refuse BEFORE respawning the live handler: the chat must stay live when
    // the preconditions for a clean reload are not met.
    let ensured = runtime.ensure_plugin()?;
    let after_ensure_status = runtime.plugin_status()?;
    plugin_cache_freshness_gate(&after_ensure_status)?;
    session_file_gate(before.session_file.as_deref(), cid)?;

    // `respawn_handler` signals the live handler to exit (and, for a TUI-driven
    // chat, stops the pane + clears the sentinel) so the supervisor respawns
    // against the same session with the freshly materialized plugin. Success is
    // NOT proven by liveness alone: the pre-existing handler is itself live, so
    // we REQUIRE a live identity different from `before_handler`. Otherwise the
    // old process — and its old plugin — would keep serving the session.
    runtime.respawn_handler(dir, cid)?;
    let after_handler = match wait_for_replaced_handler(live_timeout, poll, &before_handler, || {
        runtime.handler_identity(dir, cid)
    }) {
        HandlerReplaceOutcome::Replaced(handler) => handler,
        HandlerReplaceOutcome::StillOld(handler) => anyhow::bail!(
            "WG-CHAT-RELOAD-NOT-REPLACED: handler not replaced for chat {cid} within {}s — the handler identity is unchanged (source={} pid={:?} started_at={:?}). \
             Supervisor accepted the respawn but the same generation is still serving the session; the chat is resumable with `wg chat resume {cid}`. \
             Inspect {}/service/daemon.log.",
            live_timeout.as_secs(),
            handler.source,
            handler.pid,
            handler.started_at,
            dir.display()
        ),
        HandlerReplaceOutcome::NotLive => anyhow::bail!(
            "WG-CHAT-RELOAD-NOT-LIVE: supervisor accepted the respawn for chat {cid}, but no live handler appeared within {}s. \
             The chat is stopped but resumable with `wg chat resume {cid}`; inspect {}/service/daemon.log for the spawn error.",
            live_timeout.as_secs(),
            dir.display()
        ),
    };

    let after_status = runtime.plugin_status()?;
    let after = capture_chat_reload_identity(
        dir,
        cid,
        before.executor.clone(),
        before.model.clone(),
        wg_binary,
        pi_binary,
        &after_status,
        after_handler,
    );
    let delta = compute_reload_delta(&before, &after);
    // Belt-and-braces: the wait above already guarantees this, but make the
    // final success path itself fail closed if the identities ever compare
    // equal (e.g. a future refactor bypasses the wait).
    if !delta.handler_replaced {
        anyhow::bail!(
            "WG-CHAT-RELOAD-NOT-REPLACED: handler not replaced for chat {cid}; the handler identity is unchanged after the reload, refusing to report success. \
             The chat is resumable with `wg chat resume {cid}`."
        );
    }

    Ok(ChatReloadOutcome {
        chat_id: cid,
        plugin: plugin_resolution(&ensured),
        before,
        after,
        delta,
    })
}

fn print_reload_outcome(outcome: &ChatReloadOutcome) {
    let b = &outcome.before;
    let a = &outcome.after;
    let d = &outcome.delta;
    println!("Reloaded chat {}.", outcome.chat_id);
    println!(
        "  plugin:   compat={} source={} entry={}",
        outcome.plugin.compat, outcome.plugin.source, outcome.plugin.entry
    );
    println!(
        "  before:   cache={} cache-digest={} embed-digest={}",
        b.cache_state,
        b.cache_digest.as_deref().unwrap_or("<none>"),
        b.embed_digest
    );
    println!(
        "  after:    cache={} cache-digest={} embed-digest={}",
        a.cache_state,
        a.cache_digest.as_deref().unwrap_or("<none>"),
        a.embed_digest
    );
    println!(
        "  handler:  {} -> {}",
        format_handler_identity(&b.handler),
        format_handler_identity(&a.handler)
    );
    println!(
        "  session:  {} ({} messages)",
        a.session_file.as_deref().unwrap_or("<none>"),
        a.session_message_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".to_string())
    );
    println!(
        "  delta:    wg-binary={} pi-binary={} plugin-digest={} handler={} session={}",
        changed_label(d.wg_binary_changed),
        changed_label(d.pi_binary_changed),
        changed_label(d.plugin_digest_changed),
        changed_label(d.handler_replaced),
        if d.session_file_preserved {
            "preserved"
        } else {
            "NOT PRESERVED"
        }
    );
    if d.message_count_before != d.message_count_after {
        println!(
            "            message count {} -> {}",
            d.message_count_before
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".to_string()),
            d.message_count_after
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".to_string())
        );
    }
}

fn changed_label(changed: bool) -> &'static str {
    if changed { "CHANGED" } else { "unchanged" }
}

fn format_handler_identity(handler: &HandlerIdentity) -> String {
    match handler.pid {
        Some(pid) => format!(
            "{}:pid={} started_at={}",
            handler.source,
            pid,
            handler.started_at.as_deref().unwrap_or("?")
        ),
        None => "none".to_string(),
    }
}

/// `wg chat reload <ref>` / `wg chat reload --all` — the one session-preserving
/// reload verb. Requires the service daemon (the supervisor owns the handler).
pub fn run_reload(dir: &Path, reference: Option<&str>, all: bool, json: bool) -> Result<()> {
    if !service_is_running(dir) {
        anyhow::bail!(
            "Cannot reload: service daemon is not running. Reload needs the supervisor (which lives in the \
             daemon) to stop and respawn the handler. Start it with 'wg service start'."
        );
    }

    let graph =
        worksgood::parser::load_graph(&graph_path(dir)).with_context(|| "Failed to load graph")?;

    let targets: Vec<u32> = if all {
        let mut ids: Vec<u32> = graph
            .tasks()
            .filter(|task| task.tags.iter().any(|tag| chat_id::is_chat_loop_tag(tag)))
            .filter(|task| {
                !task.status.is_terminal() && !task.tags.iter().any(|tag| tag == "archived")
            })
            .filter_map(|task| chat_id::parse_chat_task_id(&task.id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    } else {
        let reference = reference.context("chat reference required unless --all is passed")?;
        vec![
            resolve_chat_id(&graph, reference)
                .with_context(|| format!("No chat matching '{}'", reference))?,
        ]
    };

    if targets.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "reloaded": 0,
                    "results": [],
                }))?
            );
        } else {
            println!("No active chats to reload.");
        }
        return Ok(());
    }

    let wg_binary = std::env::current_exe().ok();
    let pi_binary = worksgood::executor_discovery::pi_route_availability().pi_binary;
    let runtime = RealChatReloadRuntime;

    let mut outcomes: Vec<ChatReloadOutcome> = Vec::new();
    let mut failures: Vec<(u32, String)> = Vec::new();

    for cid in targets {
        // Terminal/archived state is authoritative — never reload into it.
        if let Err(e) = validate_chat_resumable(&graph, cid) {
            failures.push((cid, format!("{e}")));
            continue;
        }
        match reload_one(
            dir,
            cid,
            &runtime,
            wg_binary.as_deref(),
            pi_binary.as_deref(),
            RELOAD_LIVE_TIMEOUT,
            RELOAD_POLL,
        ) {
            Ok(outcome) => {
                if !json {
                    print_reload_outcome(&outcome);
                }
                outcomes.push(outcome);
            }
            Err(e) => {
                let msg = format!("{e}");
                if !json {
                    eprintln!("\x1b[31m[wg chat reload]\x1b[0m chat {cid} FAILED: {msg}");
                }
                failures.push((cid, msg));
            }
        }
    }

    if json {
        let results: Vec<serde_json::Value> = outcomes
            .iter()
            .map(|o| {
                serde_json::json!({
                    "chat_id": o.chat_id,
                    "reloaded": true,
                    "plugin": o.plugin,
                    "before": o.before,
                    "after": o.after,
                    "delta": o.delta,
                })
            })
            .collect();
        let errors: Vec<serde_json::Value> = failures
            .iter()
            .map(|(cid, msg)| serde_json::json!({"chat_id": cid, "reloaded": false, "error": msg}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "reloaded": outcomes.len(),
                "failed": failures.len(),
                "results": results,
                "errors": errors,
            }))?
        );
    }

    if !failures.is_empty() {
        let named: Vec<String> = failures
            .iter()
            .map(|(cid, msg)| format!("chat {cid}: {msg}"))
            .collect();
        anyhow::bail!(
            "{} of {} chat(s) failed to reload. Refusals are loud and leave each chat resumable:\n{}",
            failures.len(),
            outcomes.len() + failures.len(),
            named.join("\n")
        );
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
            if !t.status.is_terminal() {
                let request = worksgood::lifecycle::TransitionRequest::new(
                    worksgood::lifecycle::TransitionKind::Abandoned,
                    worksgood::lifecycle::LifecycleActor::operator(worksgood::current_user()),
                    "chat_archived",
                    format!("archive-chat:{cid}:{}", t.lifecycle.generation),
                )
                .expecting(worksgood::lifecycle::FenceExpectation::current(t));
                if worksgood::lifecycle::apply_transition(t, request).is_err() {
                    return false;
                }
            }
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
            if !t.status.is_terminal() {
                let request = worksgood::lifecycle::TransitionRequest::new(
                    worksgood::lifecycle::TransitionKind::Abandoned,
                    worksgood::lifecycle::LifecycleActor::operator(worksgood::current_user()),
                    "chat_deleted",
                    format!("delete-chat:{cid}:{}", t.lifecycle.generation),
                )
                .expecting(worksgood::lifecycle::FenceExpectation::current(t));
                if worksgood::lifecycle::apply_transition(t, request).is_err() {
                    return false;
                }
            }
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
            worksgood::profile::named::starter_template("pi").unwrap(),
        )
        .unwrap();
        td
    }

    /// Seed a fake pi transcript for `chat-N`: a real session header line
    /// (which pi keys its identity on) followed by two entries whose message
    /// text deliberately mentions the parent's chat id. This pins the "touch
    /// only the header" contract — rekeying must not rewrite message bodies
    /// that happen to contain the old id.
    fn seed_pi_transcript(dir: &Path, cid: u32) -> (PathBuf, Vec<u8>) {
        worksgood::chat_sessions::prepare_pi_chat_session(dir, cid).unwrap();
        let chat_ref = format!("chat-{cid}");
        let session_dir = worksgood::chat::chat_dir_for_ref(dir, &chat_ref).join("pi-sessions");
        let transcript = session_dir.join(format!("2026-01-01T00-00-00-000Z_chat-{cid}.jsonl"));
        let bytes = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"chat-{cid}\",\"cwd\":\"/tmp\"}}\n\
             {{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"message\":{{\"role\":\"user\",\"content\":\"tell me about chat-{cid}\"}}}}\n\
             {{\"type\":\"message\",\"id\":\"m2\",\"parentId\":\"m1\",\"message\":{{\"role\":\"assistant\",\"content\":\"chat-{cid} is great\"}}}}\n"
        )
        .into_bytes();
        std::fs::write(&transcript, &bytes).unwrap();
        (transcript, bytes)
    }

    /// Daemon-down fork: transcript is copied under the fork's session-id,
    /// the parent transcript is untouched, and the fork task inherits the
    /// parent's pinned model.
    #[test]
    fn fork_daemon_down_copies_transcript_into_new_chat() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(
            dir,
            Some("origin"),
            Some("pi:lunaroute:ds-4.1"),
            Some("pi"),
            None,
            None,
            true,
        )
        .unwrap();
        let (source_transcript, bytes) = seed_pi_transcript(dir, 0);

        run_fork(dir, "chat-0", Some("my fork"), false).unwrap();

        // The fork transcript exists under the fork's own session-id.
        worksgood::chat_sessions::prepare_pi_chat_session(dir, 1).unwrap();
        let fork_session_dir = worksgood::chat::chat_dir_for_ref(dir, "chat-1").join("pi-sessions");
        let fork_transcript = worksgood::chat_sessions::newest_pi_transcript(&fork_session_dir, 1)
            .expect("fork must have a seeded transcript");
        assert!(
            fork_transcript
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_chat-1.jsonl")),
            "fork transcript must be named for the fork's --session-id: {fork_transcript:?}"
        );

        // The fork's transcript header must carry the FORK's session id, not
        // the parent's — this is what makes pi's `--session-id chat-1` adopt
        // the copied history instead of opening a fresh conversation.
        let fork_bytes = std::fs::read(&fork_transcript).unwrap();
        let fork_nl = fork_bytes
            .iter()
            .position(|b| *b == b'\n')
            .expect("fork transcript must be newline-delimited JSONL");
        let (fork_header, fork_rest) = (&fork_bytes[..fork_nl], &fork_bytes[fork_nl + 1..]);
        let fork_header: serde_json::Value = serde_json::from_slice(fork_header).unwrap();
        assert_eq!(fork_header["type"], "session");
        assert_eq!(fork_header["id"], "chat-1");

        // Only the header line changed: the entry tree / message bodies —
        // including text that literally names the parent id `chat-0` — are
        // byte-for-byte the parent's.
        let parent_bytes = std::fs::read(&source_transcript).unwrap();
        let parent_nl = parent_bytes
            .iter()
            .position(|b| *b == b'\n')
            .expect("parent transcript must be newline-delimited JSONL");
        let (parent_header, parent_rest) =
            (&parent_bytes[..parent_nl], &parent_bytes[parent_nl + 1..]);
        let parent_header: serde_json::Value = serde_json::from_slice(parent_header).unwrap();
        assert_eq!(parent_header["type"], "session");
        assert_eq!(parent_header["id"], "chat-0");
        assert_eq!(
            fork_rest, parent_rest,
            "rekey must not touch entry tree or message bodies"
        );
        assert!(
            std::str::from_utf8(parent_rest).unwrap().contains("chat-0"),
            "parent message text naming chat-0 must remain untouched"
        );

        // Parent untouched in full.
        assert_eq!(parent_bytes, bytes);

        // Fork task exists and inherited the parent's model.
        let graph = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let fork_task = chat_id::find_chat_task(&graph, 1).unwrap();
        assert_eq!(fork_task.model.as_deref(), Some("pi:lunaroute:ds-4.1"));
        assert!(fork_task.tags.iter().any(|t| chat_id::is_chat_loop_tag(t)));

        // No stray temp files left in the fork's session dir.
        let entries: Vec<_> = std::fs::read_dir(&fork_session_dir)
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1, "no fork-tmp residue");
    }

    /// Forking a chat with no transcript is a clear refusal, not a silent
    /// empty fork.
    #[test]
    fn fork_without_transcript_refuses_loudly() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("origin"), None, Some("pi"), None, None, true).unwrap();

        let err = run_fork(dir, "chat-0", None, false).unwrap_err();
        assert!(format!("{err:#}").contains("no pi transcript"));
    }

    /// Forking is pi-only today; a nex chat forks at the session-journal
    /// layer instead (`wg session fork`).
    #[test]
    fn fork_refuses_non_pi_executor() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        run_create_direct(dir, Some("legacy"), None, Some("nex"), None, None, true).unwrap();
        seed_pi_transcript(dir, 0);

        let err = run_fork(dir, "chat-0", None, false).unwrap_err();
        assert!(format!("{err:#}").contains("Pi chats only"));
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
        assert_eq!(chat_tasks[0].executor_preset_name.as_deref(), Some("pi"));
        assert!(!chat_tasks[0].command_argv.is_empty());
        assert_eq!(
            chat_tasks[0].working_dir.as_deref(),
            dir.parent().map(|path| path.to_string_lossy()).as_deref(),
            "attended chat cwd must be the repository root"
        );
        assert_eq!(chat_tasks[0].exec_mode.as_deref(), Some("full"));
        assert_eq!(chat_tasks[0].context_scope.as_deref(), Some("full"));
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
        assert_eq!(chat.exec_mode.as_deref(), Some("full"));
        assert_eq!(chat.context_scope.as_deref(), Some("full"));
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
    fn create_explicit_claude_chat_is_supported_without_pi_preflight() {
        let td = TempDir::new().unwrap();
        let dir = td.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(dir.join("graph.jsonl"), "").unwrap();
        std::fs::write(dir.join("config.toml"), "").unwrap();
        run_create(
            dir,
            Some("explicit-claude"),
            Some("claude:future/opaque:native-v12"),
            Some("claude"),
            None,
            None,
            true,
        )
        .unwrap();

        let graph = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = graph.get_task(".chat-0").expect("Claude chat task exists");
        assert_eq!(chat.executor_preset_name.as_deref(), Some("claude"));
        assert_eq!(
            chat.model.as_deref(),
            Some("claude:future/opaque:native-v12")
        );
        let state = crate::commands::service::CoordinatorState::load_for(dir, 0).unwrap();
        assert_eq!(state.executor_override.as_deref(), Some("claude"));
        assert_eq!(
            state.model_override.as_deref(),
            Some("claude:future/opaque:native-v12")
        );
    }

    #[test]
    fn create_explicit_codex_chat_is_supported_without_pi_preflight() {
        let td = TempDir::new().unwrap();
        let dir = td.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();
        std::fs::write(dir.join("graph.jsonl"), "").unwrap();
        std::fs::write(dir.join("config.toml"), "").unwrap();
        run_create(
            dir,
            Some("explicit-codex"),
            Some("codex:future/opaque:native-v11"),
            Some("codex"),
            None,
            None,
            true,
        )
        .unwrap();

        let graph = worksgood::parser::load_graph(&graph_path(dir)).unwrap();
        let chat = graph.get_task(".chat-0").expect("Codex chat task exists");
        assert_eq!(chat.executor_preset_name.as_deref(), Some("codex"));
        assert_eq!(
            chat.model.as_deref(),
            Some("codex:future/opaque:native-v11")
        );
        let state = crate::commands::service::CoordinatorState::load_for(dir, 0).unwrap();
        assert_eq!(state.executor_override.as_deref(), Some("codex"));
        assert_eq!(
            state.model_override.as_deref(),
            Some("codex:future/opaque:native-v11")
        );
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
        assert_eq!(chat.exec_mode.as_deref(), Some("full"));
        assert_eq!(chat.context_scope.as_deref(), Some("full"));
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

    // --- chat reload: fake handler/daemon -----------------------------------

    fn fake_plugin_status(
        source: Source,
        cache_state: CacheState,
        cache_digest: Option<&str>,
    ) -> PluginStatus {
        PluginStatus {
            compat: pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION.to_string(),
            source,
            dist_entry: PathBuf::from("/tmp/pi-worksgood/index.js"),
            cache_version_dir: PathBuf::from("/tmp/cache/worksgood-pi/0.3.0"),
            ready: cache_state == CacheState::Current,
            settings_path: PathBuf::from("/tmp/.pi/agent/settings.json"),
            console_wired: false,
            embed_digest: "b3:embed".to_string(),
            cache_digest: cache_digest.map(str::to_string),
            cache_state,
        }
    }

    fn fake_resolved_plugin() -> ResolvedPlugin {
        ResolvedPlugin {
            root: PathBuf::from("/tmp/cache/worksgood-pi/0.3.0"),
            dist_entry: PathBuf::from("/tmp/cache/worksgood-pi/0.3.0/pi-worksgood/index.js"),
            host_script: PathBuf::from("/tmp/cache/worksgood-pi/0.3.0/host/wg-pi-host.mjs"),
            compat: pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION.to_string(),
            source: Source::Cache,
            has_node_modules: false,
            legacy_settings_migrated: false,
            legacy_package_accepted: false,
            console_settings_changed: false,
        }
    }

    fn handler_ident(source: &str, pid: u32, started_at: &str) -> HandlerIdentity {
        HandlerIdentity {
            source: source.to_string(),
            pid: Some(pid),
            started_at: Some(started_at.to_string()),
            live: true,
        }
    }

    /// Injected fake handler + daemon: `statuses` are returned in call order
    /// (before-capture, after-ensure, after-respawn). `handler_identity`
    /// returns `before_handler` until `respawn_handler` runs, then
    /// `after_handler`. `respawn_calls` lets refusal paths prove they never
    /// stopped the live handler.
    struct FakeReloadRuntime {
        statuses: std::cell::RefCell<Vec<PluginStatus>>,
        before_handler: HandlerIdentity,
        after_handler: HandlerIdentity,
        respawned: std::cell::Cell<bool>,
        respawn_calls: std::cell::Cell<u32>,
    }

    impl FakeReloadRuntime {
        /// Convenience for the common case: a handler that IS replaced
        /// (pid 100 -> pid 200) and is live afterwards when requested.
        fn new(before: PluginStatus, after: PluginStatus, live_after_respawn: bool) -> Self {
            let after_handler = if live_after_respawn {
                handler_ident("daemon-lock", 200, "2026-01-01T00:00:01Z")
            } else {
                HandlerIdentity::none()
            };
            Self::with_handlers(
                before,
                after,
                handler_ident("daemon-lock", 100, "2026-01-01T00:00:00Z"),
                after_handler,
            )
        }

        fn with_handlers(
            before: PluginStatus,
            after: PluginStatus,
            before_handler: HandlerIdentity,
            after_handler: HandlerIdentity,
        ) -> Self {
            Self {
                statuses: std::cell::RefCell::new(vec![before, after.clone(), after]),
                before_handler,
                after_handler,
                respawned: std::cell::Cell::new(false),
                respawn_calls: std::cell::Cell::new(0),
            }
        }
    }

    impl ChatReloadRuntime for FakeReloadRuntime {
        fn ensure_plugin(&self) -> Result<ResolvedPlugin> {
            Ok(fake_resolved_plugin())
        }
        fn plugin_status(&self) -> Result<PluginStatus> {
            let mut statuses = self.statuses.borrow_mut();
            if statuses.len() > 1 {
                Ok(statuses.remove(0))
            } else {
                Ok(statuses[0].clone())
            }
        }
        fn respawn_handler(&self, _dir: &Path, _cid: u32) -> Result<()> {
            self.respawn_calls.set(self.respawn_calls.get() + 1);
            self.respawned.set(true);
            Ok(())
        }
        fn handler_identity(&self, _dir: &Path, _cid: u32) -> HandlerIdentity {
            if self.respawned.get() {
                self.after_handler.clone()
            } else {
                self.before_handler.clone()
            }
        }
    }

    /// Create `<dir>/chat/chat-N/pi-sessions/<ts>_chat-N.jsonl` with `lines`
    /// non-empty JSONL rows.
    fn write_session_file(dir: &Path, cid: u32, lines: usize) -> PathBuf {
        let session_dir = dir
            .join("chat")
            .join(format!("chat-{cid}"))
            .join("pi-sessions");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join(format!("2026-01-01T00-00-00-000Z_chat-{cid}.jsonl"));
        let body: String = (0..lines).map(|i| format!("{{\"turn\":{i}}}\n")).collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn reload_identity_capture_counts_session_turns_and_delta_tracks_changes() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let session = write_session_file(dir, 5, 3);

        // Distinct fake binaries so content digest changes are observable.
        let bin_a = dir.join("bin-a");
        let bin_b = dir.join("bin-b");
        std::fs::write(&bin_a, b"binary-a").unwrap();
        std::fs::write(&bin_b, b"binary-b").unwrap();
        let pi_a = dir.join("pi-a");
        let pi_b = dir.join("pi-b");
        std::fs::write(&pi_a, b"pi-a").unwrap();
        std::fs::write(&pi_b, b"pi-b").unwrap();

        let stale = fake_plugin_status(Source::Cache, CacheState::Drift, Some("b3:old"));
        let fresh = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let handler_a = handler_ident("daemon-lock", 100, "2026-01-01T00:00:00Z");
        let handler_b = handler_ident("daemon-lock", 200, "2026-01-01T00:00:01Z");

        let before = capture_chat_reload_identity(
            dir,
            5,
            Some("pi".into()),
            Some("pi:openrouter:x".into()),
            Some(&bin_a),
            Some(&pi_a),
            &stale,
            handler_a.clone(),
        );
        let after = capture_chat_reload_identity(
            dir,
            5,
            Some("pi".into()),
            Some("pi:openrouter:x".into()),
            Some(&bin_b),
            Some(&pi_b),
            &fresh,
            handler_b,
        );

        assert_eq!(
            before.session_file.as_deref(),
            Some(session.display().to_string().as_str())
        );
        assert_eq!(before.session_message_count, Some(3));
        assert_eq!(after.session_message_count, Some(3));
        assert!(before.cache_digest.is_some());
        assert_eq!(before.embed_digest, "b3:embed");

        let delta = compute_reload_delta(&before, &after);
        assert!(
            delta.wg_binary_changed,
            "different wg binary content => changed"
        );
        assert!(
            delta.pi_binary_changed,
            "different pi binary content => changed"
        );
        assert!(
            delta.plugin_digest_changed,
            "stale -> current cache digest => changed"
        );
        assert!(
            delta.session_file_preserved,
            "same transcript path => preserved"
        );
        assert!(delta.handler_replaced, "different handler pid => replaced");
        assert_eq!(delta.message_count_before, Some(3));
        assert_eq!(delta.message_count_after, Some(3));

        // Same bytes + same cache digest + same handler => everything unchanged.
        let same = capture_chat_reload_identity(
            dir,
            5,
            Some("pi".into()),
            Some("pi:openrouter:x".into()),
            Some(&bin_a),
            Some(&pi_a),
            &stale,
            handler_a,
        );
        let no_delta = compute_reload_delta(&before, &same);
        assert!(!no_delta.wg_binary_changed);
        assert!(!no_delta.pi_binary_changed);
        assert!(!no_delta.plugin_digest_changed);
        assert!(!no_delta.handler_replaced);
        assert!(no_delta.session_file_preserved);
    }

    #[test]
    fn reload_succeeds_and_shows_plugin_digest_change() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 2);
        let stale = fake_plugin_status(Source::Cache, CacheState::Drift, Some("b3:old"));
        let fresh = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let runtime = FakeReloadRuntime::new(stale, fresh, true);

        let outcome = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(200),
            Duration::from_millis(1),
        )
        .expect("reload should succeed with a live respawn");

        assert_eq!(outcome.delta.plugin_digest_changed, true);
        assert_eq!(outcome.delta.session_file_preserved, true);
        assert_eq!(outcome.delta.handler_replaced, true);
        assert_eq!(outcome.before.handler.pid, Some(100));
        assert_eq!(outcome.after.handler.pid, Some(200));
        assert_eq!(runtime.respawn_calls.get(), 1);
    }

    /// The regression this task exists for: a reload that leaves the SAME
    /// handler running must NOT report success. The old wait only required
    /// liveness, which the pre-existing handler satisfies instantly.
    #[test]
    fn reload_refuses_when_handler_identity_unchanged() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 1);
        let fresh = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let same = handler_ident("daemon-lock", 100, "2026-01-01T00:00:00Z");
        let runtime = FakeReloadRuntime::with_handlers(fresh.clone(), fresh, same.clone(), same);

        let err = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect_err("unchanged handler identity must be refused");
        let msg = format!("{err}");
        assert!(msg.contains("WG-CHAT-RELOAD-NOT-REPLACED"), "{msg}");
        assert!(msg.contains("handler identity is unchanged"), "{msg}");
        assert!(!msg.contains("Reloaded"), "no success line: {msg}");
        assert_eq!(runtime.respawn_calls.get(), 1);
        assert!(
            runtime.handler_identity(dir, 5).live,
            "the (unchanged) handler is still live and resumable"
        );
    }

    /// TUI-driven chats hold no `.handler.pid`, so the daemon deferral path
    /// cannot replace them. The owner must transition from a tmux pane to a
    /// daemon lock; the reload reports the source change.
    #[test]
    fn reload_replaces_tui_driven_handler_with_daemon_lock() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 1);
        let stale = fake_plugin_status(Source::Cache, CacheState::Drift, Some("b3:old"));
        let fresh = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let runtime = FakeReloadRuntime::with_handlers(
            stale,
            fresh,
            handler_ident("tui-tmux-pane", 4242, "999"),
            handler_ident("daemon-lock", 777, "2026-01-01T00:00:05Z"),
        );

        let outcome = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(200),
            Duration::from_millis(1),
        )
        .expect("TUI pane -> daemon lock is a replacement");

        assert!(outcome.delta.handler_replaced);
        assert_eq!(outcome.before.handler.source, "tui-tmux-pane");
        assert_eq!(outcome.before.handler.pid, Some(4242));
        assert_eq!(outcome.after.handler.source, "daemon-lock");
        assert_eq!(outcome.after.handler.pid, Some(777));
    }

    /// If the TUI takeover fails and the same pane keeps serving, the reload
    /// must refuse rather than call the pane "replaced".
    #[test]
    fn reload_refuses_when_tui_pane_not_replaced() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 1);
        let fresh = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let pane = handler_ident("tui-tmux-pane", 4242, "999");
        let runtime = FakeReloadRuntime::with_handlers(fresh.clone(), fresh, pane.clone(), pane);

        let err = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect_err("an unchanged TUI pane must be refused");
        assert!(
            format!("{err}").contains("WG-CHAT-RELOAD-NOT-REPLACED"),
            "{err}"
        );
    }

    #[test]
    fn reload_refuses_stale_unrefreshable_plugin_without_stopping() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 1);
        let stale = fake_plugin_status(Source::Cache, CacheState::Drift, Some("b3:old"));
        // ensure-pi-plugin cannot refresh it: still Drift afterwards.
        let runtime = FakeReloadRuntime::new(stale.clone(), stale, true);

        let err = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WG-CHAT-RELOAD-PLUGIN-STALE"), "{msg}");
        assert_eq!(
            runtime.respawn_calls.get(),
            0,
            "refusal must happen before respawn"
        );
        assert!(
            runtime.handler_identity(dir, 5).live,
            "live handler must survive refusal"
        );
    }

    #[test]
    fn reload_refuses_missing_session_file_without_stopping() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let fresh_a = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let fresh_b = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let runtime = FakeReloadRuntime::new(fresh_a, fresh_b, true);

        let err = reload_one(
            dir,
            7,
            &runtime,
            None,
            None,
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WG-CHAT-RELOAD-SESSION-MISSING"), "{msg}");
        assert_eq!(runtime.respawn_calls.get(), 0);
        assert!(
            runtime.handler_identity(dir, 7).live,
            "live handler must survive refusal"
        );
    }

    #[test]
    fn reload_refuses_when_respawn_never_becomes_live() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        write_session_file(dir, 5, 1);
        let fresh_a = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let fresh_b = fake_plugin_status(Source::Cache, CacheState::Current, Some("b3:embed"));
        let runtime = FakeReloadRuntime::new(fresh_a, fresh_b, false);

        let err = reload_one(
            dir,
            5,
            &runtime,
            None,
            None,
            Duration::from_millis(30),
            Duration::from_millis(1),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WG-CHAT-RELOAD-NOT-LIVE"), "{msg}");
        assert_eq!(runtime.respawn_calls.get(), 1);
        assert!(!runtime.handler_identity(dir, 5).live);
    }

    /// The TUI takeover primitive: a live `.tui-driven` sentinel is claimed
    /// (torn down) so the supervisor's respawn is no longer deferred.
    #[test]
    fn take_tui_chat_ownership_clears_sentinel() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let chat_ref = "chat-5";
        let chat_dir = worksgood::chat::chat_dir_for_ref(dir, chat_ref);
        std::fs::create_dir_all(&chat_dir).unwrap();

        // No TUI owner: no-op, reports false.
        assert!(!take_tui_chat_ownership(dir, 5));

        worksgood::session_lock::write_tui_driver_sentinel(&chat_dir, std::process::id()).unwrap();
        assert!(
            worksgood::session_lock::read_tui_driver_sentinel(&chat_dir)
                .unwrap()
                .is_some()
        );

        assert!(take_tui_chat_ownership(dir, 5));
        assert!(
            worksgood::session_lock::read_tui_driver_sentinel(&chat_dir)
                .unwrap()
                .is_none(),
            "sentinel must be cleared after takeover"
        );
    }

    /// `capture_handler_identity` reads the live daemon lock and pairs its PID
    /// with the lock's start timestamp.
    #[test]
    fn capture_handler_identity_reads_daemon_lock() {
        let td = mk_workgraph_dir();
        let dir = td.path();
        let chat_ref = "chat-9";
        let chat_dir = worksgood::chat::chat_dir_for_ref(dir, chat_ref);
        std::fs::create_dir_all(&chat_dir).unwrap();
        let _lock = worksgood::session_lock::SessionLock::acquire(
            &chat_dir,
            worksgood::session_lock::HandlerKind::ChatNex,
        )
        .unwrap();

        let identity = capture_handler_identity(dir, 9);
        assert_eq!(identity.source, "daemon-lock");
        assert_eq!(identity.pid, Some(std::process::id()));
        assert!(identity.live);
        assert!(identity.started_at.is_some());
    }

    #[test]
    fn plugin_cache_gate_skips_dev_and_env_override_sources() {
        let dev = fake_plugin_status(Source::Dev, CacheState::Drift, None);
        assert!(plugin_cache_freshness_gate(&dev).is_ok());
        let env = fake_plugin_status(Source::EnvOverride, CacheState::Missing, None);
        assert!(plugin_cache_freshness_gate(&env).is_ok());
        let cache = fake_plugin_status(Source::Cache, CacheState::Drift, Some("b3:old"));
        assert!(plugin_cache_freshness_gate(&cache).is_err());
    }
}
