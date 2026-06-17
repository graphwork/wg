//! Interactive multi-turn REPL using the native executor.
//!
//! `wg nex` drops the user into an agentic coding session powered by any
//! OpenAI-compatible model. Supports streaming, tool calling, and multi-turn
//! conversation.
//!
//! ## Stdout-is-protocol contract (handler invocations)
//!
//! When invoked as a handler — `wg nex --chat <ref>` (and also
//! autonomous task-agent runs spawned by the daemon) — stdout is part
//! of the protocol stream that parent supervisors parse line-by-line.
//! **Never write diagnostic text to stdout from this file or anything
//! it transitively calls.** Config-load chatter, deprecation warnings,
//! progress notes, and debug output all belong on stderr or in the
//! daemon log; the existing call sites use `eprintln!` exactly because
//! a single stray `println!` corrupts the json-line stream and crashes
//! the next-turn parse silently. The only legitimate stdout writers in
//! this file are gated by `eval_mode` (one-line JSON summary for
//! benchmark harnesses) and are documented inline. The regression lock
//! lives in `tests/integration_handler_stdout_pristine.rs`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::{Config, DispatchRole};
use crate::executor::native::agent::AgentLoop;
use crate::executor::native::provider::create_provider_ext_with_config;
use crate::executor::native::tools::ToolRegistry;
use crate::executor::native::tools::helper_routing::HelperRouting;
use crate::nex_cli::NexArgs;
use crate::nex_runtime::{NexRuntime, NexRuntimeMode, NexSessionLayout};

pub fn run_args(workgraph_dir: &Path, args: &NexArgs, display_name: &str) -> Result<()> {
    let runtime = if args.eval_mode
        || args.autonomous
        || std::env::var_os("WG_TASK_ID").is_some()
        || std::env::var_os("WG_AGENT_ID").is_some()
    {
        crate::nex_runtime::resolve_wg_autonomous(workgraph_dir, dirs::home_dir())
    } else {
        crate::nex_runtime::resolve_wg_integrated(workgraph_dir)
    };
    run_args_with_runtime(&runtime, args, display_name)
}

pub fn run_args_with_runtime(
    runtime: &NexRuntime,
    args: &NexArgs,
    display_name: &str,
) -> Result<()> {
    run_inner(
        runtime,
        display_name,
        args.model.as_deref(),
        args.endpoint.as_deref(),
        args.api_key.as_deref(),
        args.system_prompt.as_deref(),
        args.message.as_deref(),
        args.max_turns,
        args.chatty,
        args.verbose,
        args.read_only,
        args.yolo,
        args.resume.as_deref(),
        args.role.as_deref(),
        args.chat_id,
        args.chat_ref.as_deref(),
        args.autonomous,
        args.no_mcp,
        args.eval_mode,
        args.idle_timeout_secs,
        args.minimal_tools,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    workgraph_dir: &Path,
    model: Option<&str>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
    system_prompt: Option<&str>,
    message: Option<&str>,
    max_turns: usize,
    chatty: bool,
    verbose: bool,
    read_only: bool,
    yolo: bool,
    resume: Option<&str>,
    role: Option<&str>,
    chat_id: Option<u32>,
    chat_ref: Option<&str>,
    autonomous: bool,
    no_mcp: bool,
    eval_mode: bool,
    idle_timeout_secs: Option<u64>,
    minimal_tools: bool,
) -> Result<()> {
    let runtime = crate::nex_runtime::resolve_wg_integrated(workgraph_dir);
    run_inner(
        &runtime,
        "wg nex",
        model,
        endpoint,
        api_key,
        system_prompt,
        message,
        max_turns,
        chatty,
        verbose,
        read_only,
        yolo,
        resume,
        role,
        chat_id,
        chat_ref,
        autonomous,
        no_mcp,
        eval_mode,
        idle_timeout_secs,
        minimal_tools,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    runtime: &NexRuntime,
    display_name: &str,
    model: Option<&str>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
    system_prompt: Option<&str>,
    message: Option<&str>,
    max_turns: usize,
    chatty: bool,
    verbose: bool,
    read_only: bool,
    yolo: bool,
    resume: Option<&str>,
    role: Option<&str>,
    chat_id: Option<u32>,
    chat_ref: Option<&str>,
    autonomous: bool,
    no_mcp: bool,
    eval_mode: bool,
    idle_timeout_secs: Option<u64>,
    minimal_tools: bool,
) -> Result<()> {
    let diagnostic_prefix = format!("[{}]", display_name);
    let state_dir = runtime.state_root.as_path();

    // --eval-mode is a preset for benchmark-harness invocation:
    //   * implies --autonomous  (one-shot, EndTurn exits the loop)
    //   * implies --no-mcp      (deterministic tool surface)
    //   * no chat-file surface  (no inbox/outbox/.streaming pollution
    //                            in the repo being evaluated)
    //   * silent banner         (clean stderr for harness logs)
    //   * stdout JSON summary   (machine-readable harness output)
    // The flags are forced here rather than at CLI-parse time so the
    // CLI surface stays orthogonal — a caller could still pass
    // `--autonomous --eval-mode` redundantly without confusion.
    let autonomous = autonomous || eval_mode;
    // --minimal-tools implies --no-mcp (minimal surface excludes all MCP tools)
    let no_mcp = no_mcp || eval_mode || minimal_tools;

    // --yolo: disable the workspace write sandbox so write_file/edit_file
    // can touch paths outside the cwd subtree. Requested via the flag OR
    // the WG_NEX_YOLO env var (1/true/yes/on). nex has no interactive
    // approval gate to suppress — tools already run autonomously — so the
    // only safety boundary yolo relaxes is the cwd-confinement sandbox in
    // the file tools (bash is already unconfined).
    //
    // Conflict: --yolo + --read-only is contradictory. Read-only wins
    // (the conservative choice — a request to NOT modify state must never
    // be silently overridden by a request to modify it recklessly). We
    // warn and ignore yolo in that case.
    let yolo_requested = yolo || yolo_env_truthy();
    let yolo_active = yolo_requested && !read_only;
    if yolo_requested && read_only && !eval_mode {
        eprintln!(
            "\x1b[33m{} Warning: --yolo and --read-only are contradictory; read-only wins. \
             yolo mode is OFF.\x1b[0m",
            diagnostic_prefix
        );
    }
    // Normalize WG_NEX_YOLO to a definite 1/0 so the file tools (which
    // read this env var via `yolo_enabled()`) see exactly the effective
    // decision — including when read-only forces yolo OFF despite a
    // truthy env var inherited from a parent process. Mirrors the
    // WG_STREAM_IDLE_TIMEOUT_SECS env-relay pattern below.
    //
    // SAFETY: single-threaded CLI setup before any threads spawn; this is
    // process-wide config the agent loop / tools read shortly after.
    unsafe {
        std::env::set_var("WG_NEX_YOLO", if yolo_active { "1" } else { "0" });
    }

    // Set the idle timeout via env var if provided via flag (flag takes precedence over
    // existing env var). The agent loop reads WG_STREAM_IDLE_TIMEOUT_SECS; we set it
    // here so the flag wiring is transparent to downstream code.
    if let Some(timeout) = idle_timeout_secs {
        // SAFETY: We're in single-threaded CLI setup before spawning any threads, and
        // we're setting a process-wide config that the agent loop will read shortly after.
        // No concurrent access to env vars at this point.
        unsafe {
            std::env::set_var("WG_STREAM_IDLE_TIMEOUT_SECS", timeout.to_string());
        }
    } else if (uses_standalone_nex_env(runtime.mode)
        || matches!(runtime.mode, NexRuntimeMode::LegacyWgCompat))
        && let Ok(timeout) = std::env::var("NEX_STREAM_IDLE_TIMEOUT_SECS")
        && !timeout.trim().is_empty()
    {
        unsafe {
            std::env::set_var("WG_STREAM_IDLE_TIMEOUT_SECS", timeout);
        }
    }

    let config = match crate::nex_runtime::load_config(runtime) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Warning: {}, using defaults", e);
            Config::default()
        }
    };
    let config_val = crate::nex_runtime::load_toml_value(runtime).ok();

    let env_model = if uses_standalone_nex_env(runtime.mode) {
        std::env::var("NEX_MODEL")
            .ok()
            .or_else(|| std::env::var("WG_MODEL").ok())
    } else {
        std::env::var("WG_MODEL").ok()
    };

    let effective_model = model
        .map(String::from)
        .or(env_model)
        .unwrap_or_else(|| config.resolve_model_for_role(DispatchRole::TaskAgent).model);

    let endpoint_env = if uses_standalone_nex_env(runtime.mode) {
        std::env::var("NEX_ENDPOINT").ok()
    } else {
        std::env::var("WG_ENDPOINT")
            .ok()
            .or_else(|| std::env::var("WG_ENDPOINT_NAME").ok())
            .or_else(|| std::env::var("WG_ENDPOINT_URL").ok())
    };
    let endpoint_owned = endpoint.map(String::from).or(endpoint_env);
    let endpoint = endpoint_owned.as_deref();

    record_nex_invocation(&effective_model, endpoint, eval_mode, runtime.mode);

    let working_dir = std::env::current_dir().unwrap_or_default();

    let is_coordinator = role.is_some_and(|r| r.eq_ignore_ascii_case("coordinator"));

    // The tokio runtime is created here rather than later so MCP
    // server spawn/handshake can run inside it before we hand the
    // registry to `AgentLoop`.
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

    let mut registry = {
        let mut reg = ToolRegistry::default_all_with_config_and_routing(
            state_dir,
            &working_dir,
            &config.native_executor,
            HelperRouting::new(Some(&effective_model), None, endpoint, api_key),
        );
        if minimal_tools {
            // Minimal tool surface: keep only the canonical local-dev set.
            // Dramatically reduces prefill cost for small local models.
            reg.keep_only_tools(&[
                "read_file",
                "edit_file",
                "write_file",
                "bash",
                "grep",
                "glob",
                "todo_write",
            ]);
        }
        if read_only {
            reg.filter_read_only()
        } else {
            reg
        }
    };

    // MCP: spawn configured servers, discover their tools, register
    // each one into the registry. The returned `_mcp_manager` keeps
    // all server subprocesses alive for the lifetime of this nex
    // session (servers are killed when the manager is dropped).
    let _mcp_manager = if no_mcp || config.mcp.servers.is_empty() {
        None
    } else {
        let server_configs: Vec<crate::executor::native::mcp::McpServerConfig> = config
            .mcp
            .servers
            .iter()
            .map(|s| crate::executor::native::mcp::McpServerConfig {
                name: s.name.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                env: s.env.clone(),
                enabled: s.enabled,
            })
            .collect();
        rt.block_on(async {
            match crate::executor::native::mcp::manager::start_and_discover(server_configs).await {
                Ok((manager, tools)) => {
                    let count = tools.len();
                    for t in tools {
                        registry.register(Box::new(t));
                    }
                    if verbose || count > 0 {
                        eprintln!(
                            "\x1b[2m{} MCP: {} tools from {} server(s)\x1b[0m",
                            diagnostic_prefix,
                            count,
                            manager.server_count()
                        );
                    }
                    Some(manager)
                }
                Err(e) => {
                    eprintln!(
                        "\x1b[33m{} MCP startup failed: {} — continuing without MCP\x1b[0m",
                        diagnostic_prefix, e
                    );
                    None
                }
            }
        })
    };

    // Load role/skill content from the agency primitives directory.
    // "coordinator" is a special-case role handled below. Other role names are looked up by fuzzy match
    // against component names in .wg/agency/primitives/components/.
    let role_prompt_addendum = if let Some(role_name) = role {
        if is_coordinator {
            // Full coordinator prompt (~290 lines) — matches what the
            // service-spawned claude_handler injects via --system-prompt.
            // Falls back to a hardcoded prompt if the agency/
            // coordinator-prompt/ dir is missing.
            runtime
                .wg_dir
                .as_deref()
                .map(crate::service::coordinator_prompt::build_system_prompt)
        } else {
            if let Some(wg_dir) = runtime.wg_dir.as_deref() {
                match load_agency_role(wg_dir, role_name) {
                    Some(content) => {
                        eprintln!(
                            "\x1b[2m{} loaded role: {}\x1b[0m",
                            diagnostic_prefix, role_name
                        );
                        Some(content)
                    }
                    None => {
                        eprintln!(
                            "\x1b[33m{} role '{}' not found in agency primitives\x1b[0m",
                            diagnostic_prefix, role_name
                        );
                        None
                    }
                }
            } else {
                eprintln!(
                    "\x1b[33m{} role '{}' requires a WG project; ignoring in standalone mode\x1b[0m",
                    diagnostic_prefix, role_name
                );
                None
            }
        }
    } else {
        None
    };

    let now = chrono::Local::now();
    let default_system = build_default_system_prompt(&working_dir, now, minimal_tools);
    let system_with_role = if let Some(ref addendum) = role_prompt_addendum {
        format!("{}\n\n## Role\n\n{}", default_system, addendum)
    } else {
        default_system.clone()
    };
    let system = system_prompt.unwrap_or(&system_with_role);

    // Every nex session — CLI, coordinator, task-agent — lives under
    // `<wg-dir>/chat/<ref>/`. Pick the reference:
    //   1. `--chat <ref>`  — explicit, wins over everything else.
    //   2. `--chat-id N`   — legacy numeric id, same effect.
    //   3. `--resume`      — interactive picker (no arg) or pattern
    //                        match (with arg), resolves to an
    //                        existing session's alias.
    //   4. None of the above — fresh session with a new UUID.
    //
    // Bare `wg nex` (no flags) no longer auto-resumes a tty-
    // derived session. That was confusing (recycled ptys could
    // resurrect stranger conversations) and the failure mode
    // wasn't what users expected. `--resume` is now the explicit
    // opt-in.
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let session_ref: String = if let Some(r) = chat_ref {
        r.to_string()
    } else if let Some(n) = chat_id {
        if runtime.session_layout == NexSessionLayout::WgChat {
            let _ = crate::chat_sessions::register_coordinator_session(state_dir, n);
        }
        n.to_string()
    } else if let Some(pattern) = resume {
        // `--resume` with optional pattern. Empty pattern → picker.
        // Non-empty → substring match on alias/uuid/kind, pick the
        // most-recent matching session.
        match crate::nex_runtime::pick_resume_session(runtime, pattern) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("\x1b[33m{} --resume: {}\x1b[0m", diagnostic_prefix, e);
                eprintln!(
                    "\x1b[2m  Starting a fresh session instead. Use the session list command for this runtime to see what's available.\x1b[0m"
                );
                crate::nex_runtime::create_fresh_session(runtime)?
            }
        }
    } else {
        // Fresh session. Every bare `wg nex` invocation gets a new
        // UUID and a new journal.
        crate::nex_runtime::create_fresh_session(runtime)?
    };

    // Resolve chat_dir through the session registry so aliases
    // (`coordinator-0`, `0`) land on the SAME UUID dir as other
    // writers (TUI, external readers, etc.). Previously we hardcoded
    // the literal join, which created a split-brain when the alias
    // was registered — nex wrote to `chat/coordinator-N/` while the
    // TUI looked at `chat/<uuid>/` and couldn't see nex's lock file.
    let chat_dir = crate::nex_runtime::session_dir_for_ref(runtime, &session_ref);
    let _ = std::fs::create_dir_all(&chat_dir);
    let journal_path = chat_dir.join("conversation.jsonl");
    let output_log = chat_dir.join("trace.ndjson");

    // Acquire the per-session handler lock. This enforces
    // at-most-one live handler per session (see
    // docs/design/sessions-as-identity.md). The lock file lives at
    // <chat_dir>/.handler.pid; Drop removes it on clean exit; stale
    // (dead-PID) locks are auto-recovered. For eval-mode we skip
    // this — eval runs are short-lived and the benchmark harness
    // shouldn't leave lock files in the repo it's grading.
    //
    // Held across `rt.block_on` so the agent loop sees it alive for
    // its entire duration. Dropped at function return (any exit
    // path — normal, error, panic) releasing cleanly.
    let handler_kind = if eval_mode {
        crate::session_lock::HandlerKind::Adapter
    } else if autonomous && (chat_ref.is_some() || chat_id.is_some()) {
        crate::session_lock::HandlerKind::ChatNex
    } else if autonomous {
        crate::session_lock::HandlerKind::AutonomousNex
    } else if chat_ref.is_some() || chat_id.is_some() {
        crate::session_lock::HandlerKind::ChatNex
    } else {
        crate::session_lock::HandlerKind::InteractiveNex
    };
    let _session_lock = if eval_mode {
        None
    } else {
        match crate::session_lock::SessionLock::acquire(&chat_dir, handler_kind) {
            Ok(lock) => Some(lock),
            Err(e) => {
                eprintln!(
                    "\x1b[31m{} session {} is already owned by another handler: {}\x1b[0m",
                    diagnostic_prefix, session_ref, e
                );
                eprintln!(
                    "\x1b[2m  Takeover is intentional: send a message via `wg tui` or another client,\n  \
                     or signal the existing handler via `wg session release {}`.\x1b[0m",
                    session_ref
                );
                anyhow::bail!("session lock busy");
            }
        }
    };
    // Clear any stale release marker left by a prior run. If we were
    // signalled-to-release but exited before observing, the next
    // handler shouldn't see that marker and immediately quit.
    crate::session_lock::clear_release_marker(&chat_dir);

    // Resume is enabled iff the chosen session has a journal.
    // With the new semantics, this is always true for `--resume` /
    // `--chat <ref>` pointing at a real session, and always false
    // for fresh sessions. No magic auto-resume.
    let journal_exists = journal_path.exists();
    let resume_enabled = journal_exists;
    if resume_enabled {
        eprintln!(
            "\x1b[1;33m{} resuming session {}\x1b[0m",
            diagnostic_prefix, session_ref
        );
    }

    if verbose {
        eprintln!(
            "\x1b[2m{} session log → {}\x1b[0m",
            diagnostic_prefix,
            output_log.display()
        );
        eprintln!(
            "\x1b[2m{} journal    → {}\x1b[0m",
            diagnostic_prefix,
            journal_path.display()
        );
    }

    let client = create_provider_ext_with_config(
        state_dir,
        &config,
        config_val.as_ref(),
        &effective_model,
        None,
        endpoint,
        api_key,
    )?;

    let model_registry = crate::nex_runtime::load_model_registry(runtime);
    let supports_tools = model_registry.supports_tool_use(&effective_model);

    let mut agent = AgentLoop::with_tool_support(
        client,
        registry,
        system.to_string(),
        max_turns,
        output_log,
        supports_tools,
    )
    .with_nex_verbose(verbose)
    .with_nex_chatty(chatty || verbose)
    .with_nex_repl_mode(true)
    .with_journal(journal_path, format!("nex-{}", stamp))
    .with_working_dir(working_dir.clone())
    .with_workgraph_dir(state_dir.to_path_buf())
    .with_resume(resume_enabled);

    // Chat-file I/O surface. Enabled whenever the caller said "I'm
    // tethered to a chat dir" (via `--chat` or `--chat-id`) OR when
    // running autonomous (task-agent mode) — autonomous runs always
    // want their inbox/outbox on disk so someone can attach to them
    // later via `wg chat attach <ref>`.
    //
    // Plain interactive `wg nex` (no flags) does NOT mount the chat
    // surface — it uses stdin/stderr for the human's low-latency
    // typing path, with the journal still written to
    // `chat/<ref>/conversation.jsonl` for persistence + auto-resume.
    // Eval mode skips the chat surface even though it's autonomous:
    // the benchmarked repo shouldn't get inbox.jsonl/outbox.jsonl/
    // .streaming files written into its `.wg/chat/<alias>/`
    // directory (no attacher will ever read them, and some graders
    // diff the working tree). Explicit chat bindings still win.
    let mount_chat_surface = runtime.session_layout == NexSessionLayout::WgChat
        && (chat_ref.is_some() || chat_id.is_some() || (autonomous && !eval_mode));
    if mount_chat_surface {
        agent = agent.with_chat_ref(state_dir.to_path_buf(), session_ref.clone(), resume_enabled);
    }
    if autonomous {
        agent = agent.with_autonomous(true);
    }

    if let Some(entry) = config.registry_lookup(&effective_model) {
        agent = agent.with_registry_entry(entry);
    }

    // Always show the minimal banner — it names the model so the user
    // knows what they're talking to. Verbose-only details (warning
    // text, exit hint) are gated. Eval mode is the one exception:
    // the harness captures stderr as logs, we keep it clean.
    if !eval_mode {
        if read_only {
            eprintln!(
                "\x1b[1;32m{}\x1b[0m \x1b[33m[read-only]\x1b[0m — interactive session with \x1b[1m{}\x1b[0m",
                display_name, effective_model
            );
        } else if yolo_active {
            // Loud, hard-to-miss banner: yolo mode lifts the workspace
            // write sandbox, so the agent can modify files anywhere on
            // disk with no confirmation. Make sure the human sees it.
            eprintln!(
                "\x1b[1;41;97m  YOLO MODE  \x1b[0m \x1b[1;31m{}\x1b[0m — interactive session with \x1b[1m{}\x1b[0m",
                display_name, effective_model
            );
            eprintln!(
                "\x1b[1;31m⚠ All safety gating disabled: write_file/edit_file can write OUTSIDE the \
                 working directory and no action requires confirmation.\x1b[0m"
            );
        } else {
            eprintln!(
                "\x1b[1;32m{}\x1b[0m — interactive session with \x1b[1m{}\x1b[0m",
                display_name, effective_model
            );
        }
        if !supports_tools {
            eprintln!(
                "\x1b[33mWarning: model '{}' may not support tool use\x1b[0m",
                effective_model
            );
        }
        if verbose {
            eprintln!("Type /quit or Ctrl-D to exit.\n");
        } else {
            eprintln!();
        }
    }

    // Eval mode: suppress the stderr half of `tool_progress!` for
    // the duration of the run. Callback routing (if any scope
    // installs one) still works; only the process-wide stderr
    // broadcast is silenced. Non-eval callers pass `false` and the
    // scope is a no-op — backward-compatible.
    let result = rt.block_on(crate::executor::native::tools::progress::stderr_scope(
        eval_mode,
        agent.run_interactive(message),
    ))?;

    if verbose {
        eprintln!(
            "\n\x1b[2mSession: {} turns, {} input + {} output tokens\x1b[0m",
            result.turns, result.total_usage.input_tokens, result.total_usage.output_tokens,
        );
    }

    // Eval mode: emit a single-line JSON summary on stdout so the
    // benchmark harness has a parseable completion record. Stdout
    // is reserved for this one line; everything else (banner,
    // progress, errors) lives on stderr. Emitted BEFORE the abnormal-
    // exit bail below so graders see the full outcome even on
    // failures (status becomes "abnormal" + exit_reason names it).
    if eval_mode {
        let status = if result.terminated_cleanly() {
            "ok"
        } else {
            "abnormal"
        };
        println!(
            "{{\"status\":\"{}\",\"turns\":{},\"input_tokens\":{},\"output_tokens\":{},\"exit_reason\":{}}}",
            status,
            result.turns,
            result.total_usage.input_tokens,
            result.total_usage.output_tokens,
            serde_json::to_string(&result.exit_reason).unwrap_or_else(|_| "\"\"".to_string()),
        );
    }

    // When the loop exited abnormally (context_limit, max_turns, etc.),
    // propagate that as a non-zero process exit so any wrapper (e.g., the
    // autonomous agent runner that calls `complete_task` on exit 0) marks
    // the driving task as FAILED rather than DONE. Observed 2026-04-17 on
    // ulivo: a research task hit the context limit on turn 34, the loop
    // returned Ok(result), the wrapper saw exit 0 and marked the graph
    // task done — with no deliverable on disk and FLIP scoring 0.45. The
    // mis-status broke downstream assumptions.
    if !result.terminated_cleanly() {
        anyhow::bail!(
            "agent loop terminated abnormally (reason: {}). \
             {} turns, {} input + {} output tokens. \
             Session journal is preserved; inspect it to recover state.",
            result.exit_reason,
            result.turns,
            result.total_usage.input_tokens,
            result.total_usage.output_tokens,
        );
    }

    Ok(())
}

/// Record a `wg nex` invocation in launcher history. Done early in
/// `run()` (before the long-running agent loop) so even a Ctrl-C'd
/// session leaves a recallable entry for the TUI new-coordinator
/// dialog. Eval mode skips recording — benchmark harnesses don't
/// want to pollute the history with one-shot grader invocations.
fn record_nex_invocation(
    effective_model: &str,
    endpoint: Option<&str>,
    eval_mode: bool,
    mode: NexRuntimeMode,
) {
    if eval_mode || matches!(mode, NexRuntimeMode::Standalone | NexRuntimeMode::Eval) {
        return;
    }
    let _ = crate::launcher_history::record_use(&crate::launcher_history::HistoryEntry::new(
        "native",
        Some(effective_model),
        endpoint,
        "cli",
    ));
}

fn uses_standalone_nex_env(mode: NexRuntimeMode) -> bool {
    matches!(mode, NexRuntimeMode::Standalone | NexRuntimeMode::Eval)
}

/// Whether the `WG_NEX_YOLO` env var requests yolo mode. Truthy values
/// are `1` / `true` / `yes` / `on` (case-insensitive); anything else
/// (including unset, empty, `0`, `false`) is falsey. Mirrors the truthy
/// parsing used elsewhere for nex env flags.
fn yolo_env_truthy() -> bool {
    std::env::var("WG_NEX_YOLO").ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn build_default_system_prompt(
    working_dir: &Path,
    now: chrono::DateTime<chrono::Local>,
    minimal_tools: bool,
) -> String {
    let tool_summary = if minimal_tools {
        "You have a minimal local-development tool set for reading and writing files, \
         running shell commands, and searching local files. In this mode, web_search and \
         web_fetch are not available; use bash with curl or wget for HTTP requests."
    } else {
        "You have tools for reading and writing files, running shell commands, searching \
         and fetching from the web, and summarizing or delegating work."
    };

    format!(
        "You are an AI assistant in an interactive terminal session. {tool_summary}\n\
         \n\
         Working directory: {}\n\
         Current date: {} ({})\n\
         \n\
         Use bash to run `wg` CLI commands when you need WG task management.\n\
         \n\
         When asked to produce content that requires current real-world data \
         (weather, news, prices, dates beyond your training cutoff, schedules, laws, \
         company facts, etc.):\n\
         - If a web search or web fetch tool is available, use it.\n\
         - If bash is available, use curl or wget for known data endpoints; for weather, \
         `curl https://wttr.in/<location>` is often a useful first check.\n\
         - If you cannot fetch live data with the available tools, state that limitation \
         explicitly and ask the user to provide the data or confirm they want a code \
         skeleton or placeholder.\n\
         - Do not write code or prose that fabricates current data.",
        working_dir.display(),
        now.format("%Y-%m-%d %H:%M %Z"),
        now.format("%A"),
    )
}

/// Load an agency role/skill component by name. Scans all YAML files
/// in `.wg/agency/primitives/components/` for one whose `name`
/// field matches (case-insensitive substring match). Returns the
/// `content` field as a string, or None if no match found.
fn load_agency_role(workgraph_dir: &Path, role_name: &str) -> Option<String> {
    let components_dir = workgraph_dir.join("agency/primitives/components");
    let entries = std::fs::read_dir(&components_dir).ok()?;
    let needle = role_name.to_lowercase();

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        // Quick check before full YAML parse — skip files whose text
        // doesn't contain the needle at all.
        if !text.to_lowercase().contains(&needle) {
            continue;
        }
        // Parse the YAML and check the name field.
        let doc: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
        let name = doc.get("name")?.as_str()?;
        if name.to_lowercase().contains(&needle) {
            // Found it — return the content field.
            let content = doc.get("content")?;
            return match content {
                serde_yaml::Value::Tagged(tagged) => Some(tagged.value.as_str()?.to_string()),
                serde_yaml::Value::String(s) => Some(s.clone()),
                _ => content.as_str().map(String::from),
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn with_history_env<F: FnOnce(&Path)>(f: F) {
        let tmp = TempDir::new().unwrap();
        let history_path = tmp.path().join("launcher-history.jsonl");
        unsafe {
            std::env::set_var("WG_LAUNCHER_HISTORY_PATH", &history_path);
        }
        f(&history_path);
        unsafe {
            std::env::remove_var("WG_LAUNCHER_HISTORY_PATH");
        }
    }

    #[test]
    #[serial_test::serial(launcher_history_env)]
    fn test_cli_nex_records_to_launcher_history() {
        with_history_env(|history_path| {
            record_nex_invocation(
                "qwen3-coder",
                Some("https://lambda01.tail334fe6.ts.net:30000"),
                false,
                NexRuntimeMode::WgIntegrated,
            );
            let contents = fs::read_to_string(history_path).expect("history file should exist");
            assert!(
                contents.contains("\"executor\":\"native\""),
                "wg nex records as native executor: {}",
                contents
            );
            assert!(
                contents.contains("qwen3-coder"),
                "history contains the model: {}",
                contents
            );
            assert!(
                contents.contains("lambda01.tail334fe6.ts.net"),
                "history contains the endpoint: {}",
                contents
            );
            assert!(
                contents.contains("\"source\":\"cli\""),
                "wg nex source = cli: {}",
                contents
            );
        });
    }

    #[test]
    #[serial_test::serial(launcher_history_env)]
    fn test_cli_nex_eval_mode_skips_recording() {
        with_history_env(|history_path| {
            record_nex_invocation("qwen3-coder", None, true, NexRuntimeMode::Eval);
            assert!(
                !history_path.exists() || fs::read_to_string(history_path).unwrap().is_empty(),
                "eval mode should not write to history"
            );
        });
    }

    #[test]
    fn test_default_prompt_directs_current_data_to_fetch_before_code() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-04T12:00:00-05:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        let prompt = build_default_system_prompt(Path::new("/tmp/work"), now, false);

        assert!(prompt.contains("requires current real-world data"));
        assert!(prompt.contains("If a web search or web fetch tool is available, use it"));
        assert!(prompt.contains("curl https://wttr.in/<location>"));
        assert!(prompt.contains("Do not write code or prose that fabricates current data"));
        assert!(prompt.contains("Current date: 2026-05-04"));
    }

    #[test]
    fn test_minimal_prompt_names_bash_http_fallback_without_web_tools() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-04T12:00:00-05:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        let prompt = build_default_system_prompt(Path::new("/tmp/work"), now, true);

        assert!(prompt.contains("minimal local-development tool set"));
        assert!(prompt.contains("web_search and web_fetch are not available"));
        assert!(prompt.contains("use bash with curl or wget for HTTP requests"));
        assert!(prompt.contains("If you cannot fetch live data with the available tools"));
    }

    #[test]
    fn test_nex_default_tool_surface_has_web_fetch_and_bash() {
        let tmp = TempDir::new().unwrap();
        let registry = ToolRegistry::default_all(tmp.path(), tmp.path());
        let names: Vec<String> = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();

        assert!(names.contains(&"web_fetch".to_string()));
        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&"bash".to_string()));
    }

    #[test]
    fn test_nex_minimal_tool_surface_keeps_bash_without_web_fetch() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ToolRegistry::default_all(tmp.path(), tmp.path());
        registry.keep_only_tools(&[
            "read_file",
            "edit_file",
            "write_file",
            "bash",
            "grep",
            "glob",
            "todo_write",
        ]);
        let names: Vec<String> = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();

        assert!(names.contains(&"bash".to_string()));
        assert!(!names.contains(&"web_fetch".to_string()));
        assert!(!names.contains(&"web_search".to_string()));
    }
}
