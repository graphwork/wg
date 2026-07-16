//! `wg spawn-task` — the single entry point that turns a task-id
//! into a live handler process.
//!
//! See `docs/design/sessions-as-identity.md` for the full model.
//! This command:
//!   1. Looks up the task in the graph
//!   2. Resolves its executor type, chat session, and role
//!   3. Dispatches to the right handler command via a per-executor
//!      adapter
//!   4. `exec()`s into the child so stdio passes through cleanly —
//!      the PTY embedding in `wg tui` just spawns `wg spawn-task`
//!      and gets the handler's output as its own.
//!
//! Adapters live inline here (one match arm per live-session executor).
//! Native execs into `wg nex`; Claude execs into `wg claude-handler`
//! (the standalone Claude CLI ↔ chat/*.jsonl bridge). Worker-only
//! external executors such as OpenCode, Aider, Goose, Qwen, Cline,
//! Crush, and Amplifier are rejected here with a clear error because
//! they belong on the task-agent worker path, not the live chat path.
//!
//! ## Stdout-is-protocol contract
//!
//! After dispatch, this command `exec()`s into the chosen handler so
//! the child inherits our stdio. That means anything we (or any
//! transitively-called code, including `Config::load_*`) write to
//! stdout BEFORE the exec becomes part of the handler's protocol
//! stream and corrupts the chat json-line conversation. The only
//! legitimate stdout writer in this file is the `--dry-run` preview
//! line which exits before any handler is spawned. All other
//! diagnostics use `eprintln!` / the logger.

use std::path::Path;

use anyhow::{Context, Result, anyhow};

use worksgood::dispatch::ExecutorKind;
use worksgood::graph::Task;

/// Dispatch table for what handler to run for a task. Parsed from
/// the task's executor hint (config override) or defaults to native.
#[derive(Clone, Debug)]
pub enum HandlerSpec {
    Native {
        chat_ref: String,
        role: Option<String>,
        resume: bool,
        model: Option<String>,
        endpoint: Option<String>,
    },
    Claude {
        chat_ref: String,
        model: Option<String>,
    },
    Codex {
        chat_ref: String,
        model: Option<String>,
    },
    OpenCode {
        chat_ref: String,
        model: Option<String>,
    },
    Pi {
        chat_ref: String,
        model: Option<String>,
    },
    Gemini {
        chat_ref: String,
    },
}

impl HandlerSpec {
    /// Render the command line we'd exec, for preview / dry-run.
    pub fn command_preview(&self) -> String {
        match self {
            Self::Native {
                chat_ref,
                role,
                resume,
                model,
                endpoint,
            } => {
                let mut s = format!("wg nex --chat {}", chat_ref);
                if *resume {
                    s.push_str(" --resume");
                }
                if let Some(r) = role {
                    s.push_str(&format!(" --role {}", r));
                }
                if let Some(m) = model {
                    s.push_str(&format!(" -m {}", m));
                }
                if let Some(e) = endpoint {
                    s.push_str(&format!(" -e {}", e));
                }
                s
            }
            Self::Claude { chat_ref, model } => {
                let mut s = format!("wg claude-handler --chat {}", chat_ref);
                if let Some(m) = model {
                    s.push_str(&format!(" -m {}", m));
                }
                s
            }
            Self::Codex { chat_ref, model } => {
                let mut s = format!("wg codex-handler --chat {}", chat_ref);
                if let Some(m) = model {
                    s.push_str(&format!(" -m {}", m));
                }
                s
            }
            Self::OpenCode { chat_ref, model } => {
                let mut s = format!("wg opencode-handler --chat {}", chat_ref);
                if let Some(m) = model {
                    s.push_str(&format!(" -m {}", m));
                }
                s
            }
            Self::Pi { chat_ref, model } => {
                let mut parts = vec![
                    "pi".to_string(),
                    "--session-id".to_string(),
                    chat_ref.to_string(),
                    "--session-dir".to_string(),
                    format!("chat/{}/pi-sessions", chat_ref),
                ];
                if let Some(marg) = crate::commands::pi_handler::pi_model_arg(model.as_deref()) {
                    parts.splice(
                        1..1,
                        [
                            "--provider".to_string(),
                            marg.provider,
                            "--model".to_string(),
                            marg.model,
                        ],
                    );
                }
                parts.join(" ")
            }
            Self::Gemini { chat_ref } => format!("gemini [TODO: adapter for session={}]", chat_ref),
        }
    }
}

/// The entry point called from `main.rs` for `Commands::SpawnTask`.
pub fn run(
    workgraph_dir: &Path,
    task_id: &str,
    role_override: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let graph_path = workgraph_dir.join("graph.jsonl");
    // A missing graph.jsonl is NOT a fatal error for spawn-task:
    // the daemon needs to spawn coordinator-0 on startup before any
    // tasks exist (and before the graph file has even been created
    // on first run). We treat "no graph file" the same as "empty
    // graph" and fall through to the synthesized-task branch. Any
    // OTHER load error (malformed JSONL, permissions, etc.) still
    // bails.
    let graph = if graph_path.exists() {
        worksgood::parser::load_graph(&graph_path)
            .with_context(|| format!("load graph at {:?}", graph_path))?
    } else {
        worksgood::graph::WorkGraph::new()
    };
    let found = graph.tasks().find(|t| t.id == task_id).cloned();
    let task = match found {
        Some(t) => t,
        None if is_coordinator_id(task_id) => {
            // Coordinator sessions can exist without a graph task —
            // the daemon auto-spawns coordinator-0 at startup before
            // any `CreateCoordinator` IPC fires, and older flows
            // drove `wg nex --chat coordinator-N` without a graph
            // entry at all. Synthesize a minimal task so handler
            // resolution still works.
            Task {
                id: task_id.to_string(),
                title: task_id.to_string(),
                ..Default::default()
            }
        }
        None => return Err(anyhow!("no such task: {}", task_id)),
    };

    let spec = resolve_handler(workgraph_dir, &task, role_override)?;

    if dry_run {
        println!("{}", spec.command_preview());
        return Ok(());
    }

    worksgood::execution_selection::require(
        workgraph_dir,
        task.model.as_deref().map(|m| (m, true)),
        "wg spawn-task",
    )?;
    dispatch(&spec, workgraph_dir)
}

/// Figure out what kind of handler to spawn for this task, given
/// config + task-specific overrides.
///
/// All `{executor, model, endpoint}` decisions are delegated to
/// [`worksgood::dispatch::plan_spawn`] — the single source of truth for
/// spawn-time resolution. This function only sources `WG_EXECUTOR_TYPE`
/// (the per-coordinator env hint set by the daemon) and converts the
/// resulting `SpawnPlan` into a `HandlerSpec` for the local exec adapter.
pub fn resolve_handler(
    workgraph_dir: &Path,
    task: &Task,
    role_override: Option<&str>,
) -> Result<HandlerSpec> {
    let config = worksgood::config::Config::load_or_default(workgraph_dir);

    // chat_ref convention: task id IS the chat alias, until Phase 5
    // migration swaps to `.chat-<uuid>`. Exceptions: `.chat-N` and
    // `.coordinator-N` task ids map to the registered `chat-N` /
    // `coordinator-N` aliases (see `register_coordinator_session`,
    // which installs both). Without these strips, the handler would
    // resolve `.chat-N` / `.coordinator-N` literally, fall back to a
    // fresh `chat/.chat-N/` dir that no IPC writer touches, and the
    // chat's inbox would appear empty (split-brain with the UUID dir
    // the registered alias resolves to).
    let chat_ref = if let Some(n) = task.id.strip_prefix(".chat-") {
        format!("chat-{}", n)
    } else if let Some(n) = task.id.strip_prefix(".coordinator-") {
        format!("coordinator-{}", n)
    } else {
        task.id.clone()
    };

    // Role: coordinator tasks get `--role coordinator`. Caller
    // override wins. `.compact-*`, `.assign-*`, etc. inherit no
    // special role — they're just task-agent runs.
    let role = role_override.map(|s| s.to_string()).or_else(|| {
        if task.id.starts_with(".coordinator-") {
            Some("coordinator".to_string())
        } else {
            None
        }
    });

    // Single source of truth: ALL executor/model/endpoint decisions flow
    // through `plan_spawn`. We source TWO env vars set by the parent
    // (typically the daemon supervisor):
    //   - `WG_EXECUTOR_TYPE` — agency-derived executor for THIS chat/task,
    //     so a codex chat in the same graph as a claude one routes
    //     correctly even if the global `[dispatcher].executor` differs.
    //   - `WG_MODEL` — agency-derived model for THIS chat/task, fed as
    //     `default_model`. Without this, the per-chat model the daemon
    //     resolved (from `CoordinatorState.model_override` etc.) silently
    //     falls back to `[dispatcher].model` here — the chat-launched-with
    //     bug where "create chat with codex:gpt-5" ran codex with
    //     `-m claude:opus` because spawn-task only honored the executor
    //     half of the per-chat plan.
    let env_executor = std::env::var("WG_EXECUTOR_TYPE").ok();
    let env_model = std::env::var("WG_MODEL").ok();
    let plan = worksgood::dispatch::plan_spawn(
        task,
        &config,
        env_executor.as_deref(),
        env_model.as_deref(),
    )?;

    // Provenance: every spawn emits one line tracing each decision back to
    // the config knob that produced it. Eliminates silent-routing bugs.
    eprintln!(
        "[spawn_task] {}: {}",
        task.id,
        plan.provenance.log_line(&plan)
    );

    // Resume if the session journal exists on disk — same rule
    // `wg nex` uses internally. Route through the registry so
    // aliases (`coordinator-0`, `0`) resolve to the UUID dir.
    let chat_dir = worksgood::chat::chat_dir_for_ref(workgraph_dir, &chat_ref);
    let journal_exists = chat_dir.join("conversation.jsonl").exists();

    let plain_pi_chat = task
        .tags
        .iter()
        .any(|tag| worksgood::chat_id::is_chat_loop_tag(tag))
        && task.executor_preset_name.as_deref() == Some("pi")
        && task.model.as_deref().is_none_or(|m| m.trim().is_empty());
    let model = if plain_pi_chat && plan.executor == ExecutorKind::Pi {
        None
    } else {
        Some(plan.model.raw.clone())
    };
    let endpoint = plan.endpoint.as_ref().map(|e| e.name.clone());

    Ok(match plan.executor {
        ExecutorKind::Native => HandlerSpec::Native {
            chat_ref,
            role,
            resume: journal_exists,
            model,
            endpoint,
        },
        ExecutorKind::Claude => HandlerSpec::Claude { chat_ref, model },
        ExecutorKind::Codex => HandlerSpec::Codex { chat_ref, model },
        ExecutorKind::OpenCode => HandlerSpec::OpenCode { chat_ref, model },
        ExecutorKind::Aider
        | ExecutorKind::Goose
        | ExecutorKind::Qwen
        | ExecutorKind::Cline
        | ExecutorKind::Crush
        | ExecutorKind::Amplifier => {
            return Err(worker_only_executor_error(plan.executor));
        }
        ExecutorKind::Octomind | ExecutorKind::Dexto => {
            // Chat-capable external CLIs wired only into the TUI live-chat PTY
            // path (which owns chat_dir + PTY sizing), not the daemon
            // spawn-task handler path (prototype-octomind-dexto-chat).
            return Err(anyhow!(
                "executor '{}' currently runs only via the TUI live-chat PTY path \
                 (open a chat from the TUI [+] menu and choose it); it has no \
                 spawn-task / daemon handler yet",
                plan.executor.as_str()
            ));
        }
        ExecutorKind::Pi => HandlerSpec::Pi { chat_ref, model },
        ExecutorKind::RemoteRunner => {
            return Err(anyhow!(
                "remote-runner executor is driven by the WG-Exec providers plane \
                 (`wg provider …`), not the local spawn-task handler path: a \
                 `Placement::Provider(wgid:)` spawn is placed/granted/run/accepted over \
                 the execution wire (src/providers/), where the two scoped UCANs + the \
                 epoch-fenced lease live"
            ));
        }
        ExecutorKind::Shell => {
            return Err(anyhow!(
                "shell executor is not supported by spawn-task; \
                 task.exec runs through the dispatcher's shell-spawn path, \
                 not the handler-exec path"
            ));
        }
    })
}

fn worker_only_executor_error(executor: ExecutorKind) -> anyhow::Error {
    anyhow!(
        "executor '{}' is worker-only and cannot run through spawn-task/live chat; \
         use it for task-agent workers via the dispatcher or `wg spawn`, or choose \
         a live chat executor such as claude, codex, opencode, or native/nex",
        executor.as_str()
    )
}

fn is_coordinator_id(task_id: &str) -> bool {
    task_id
        .strip_prefix(".coordinator-")
        .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

/// Exec into the handler process. This REPLACES the current process
/// (via `execvp`) on Unix so stdio passes through cleanly — the PTY
/// parent sees the handler's bytes directly.
fn dispatch(spec: &HandlerSpec, workgraph_dir: &Path) -> Result<()> {
    match spec {
        HandlerSpec::Native {
            chat_ref,
            role,
            resume,
            model,
            endpoint,
        } => dispatch_native(
            chat_ref,
            role.as_deref(),
            *resume,
            model.as_deref(),
            endpoint.as_deref(),
            workgraph_dir,
        ),
        HandlerSpec::Claude { chat_ref, model } => {
            dispatch_claude(chat_ref, model.as_deref(), workgraph_dir)
        }
        HandlerSpec::Codex { chat_ref, model } => {
            dispatch_codex(chat_ref, model.as_deref(), workgraph_dir)
        }
        HandlerSpec::OpenCode { chat_ref, model } => {
            dispatch_opencode(chat_ref, model.as_deref(), workgraph_dir)
        }
        HandlerSpec::Pi { chat_ref, model } => {
            dispatch_pi(chat_ref, model.as_deref(), workgraph_dir)
        }
        HandlerSpec::Gemini { .. } => Err(anyhow!(
            "gemini adapter not yet implemented (Phase 7). Use --executor native for now."
        )),
    }
}

fn dispatch_codex(chat_ref: &str, model: Option<&str>, workgraph_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let self_exe =
            std::env::current_exe().context("resolve current exe for spawn-task dispatch")?;
        let mut cmd = std::process::Command::new(&self_exe);
        cmd.arg("codex-handler").arg("--chat").arg(chat_ref);
        cmd.env("WG_DIR", workgraph_dir);
        if let Some(m) = model {
            cmd.arg("-m").arg(m);
        }
        let err = cmd.exec();
        Err(anyhow!("exec wg codex-handler failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        let _ = (chat_ref, model, workgraph_dir);
        Err(anyhow!(
            "spawn-task dispatch not yet supported on this platform"
        ))
    }
}

fn dispatch_opencode(chat_ref: &str, model: Option<&str>, workgraph_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let self_exe =
            std::env::current_exe().context("resolve current exe for spawn-task dispatch")?;
        let mut cmd = std::process::Command::new(&self_exe);
        cmd.arg("opencode-handler").arg("--chat").arg(chat_ref);
        cmd.env("WG_DIR", workgraph_dir);
        if let Some(m) = model {
            cmd.arg("-m").arg(m);
        }
        let err = cmd.exec();
        Err(anyhow!("exec wg opencode-handler failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        let _ = (chat_ref, model, workgraph_dir);
        Err(anyhow!(
            "spawn-task dispatch not yet supported on this platform"
        ))
    }
}

fn dispatch_pi(chat_ref: &str, model: Option<&str>, workgraph_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let chat_dir = worksgood::chat::chat_dir_for_ref(workgraph_dir, chat_ref);
        let session_dir = chat_dir.join("pi-sessions");
        std::fs::create_dir_all(&session_dir)
            .with_context(|| format!("create pi session dir {:?}", session_dir))?;
        let mut cmd = std::process::Command::new("pi");
        if let Some(marg) = crate::commands::pi_handler::pi_model_arg(model) {
            cmd.arg("--provider")
                .arg(marg.provider)
                .arg("--model")
                .arg(marg.model);
        }
        cmd.arg("--session-id")
            .arg(chat_ref)
            .arg("--session-dir")
            .arg(&session_dir);
        cmd.env("WG_DIR", workgraph_dir);
        let err = cmd.exec();
        Err(anyhow!("exec pi failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        let _ = (chat_ref, model, workgraph_dir);
        Err(anyhow!(
            "spawn-task dispatch not yet supported on this platform"
        ))
    }
}

fn dispatch_claude(chat_ref: &str, model: Option<&str>, workgraph_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let self_exe =
            std::env::current_exe().context("resolve current exe for spawn-task dispatch")?;
        let mut cmd = std::process::Command::new(&self_exe);
        cmd.arg("claude-handler").arg("--chat").arg(chat_ref);
        cmd.env("WG_DIR", workgraph_dir);
        // Coordinator role is implicit for `coordinator-*` refs; pass
        // explicit role if the caller set one via role_override.
        if let Some(m) = model {
            cmd.arg("-m").arg(m);
        }
        let err = cmd.exec();
        Err(anyhow!("exec wg claude-handler failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        let _ = (chat_ref, model, workgraph_dir);
        Err(anyhow!(
            "spawn-task dispatch not yet supported on this platform"
        ))
    }
}

fn dispatch_native(
    chat_ref: &str,
    role: Option<&str>,
    resume: bool,
    model: Option<&str>,
    endpoint: Option<&str>,
    workgraph_dir: &Path,
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let self_exe =
            std::env::current_exe().context("resolve current exe for spawn-task dispatch")?;
        let mut cmd = std::process::Command::new(&self_exe);
        cmd.arg("nex").arg("--chat").arg(chat_ref);
        cmd.env("WG_DIR", workgraph_dir);
        if resume {
            cmd.arg("--resume");
        }
        if let Some(r) = role {
            cmd.arg("--role").arg(r);
        }
        if let Some(m) = model {
            cmd.arg("-m").arg(m);
        }
        if let Some(e) = endpoint {
            cmd.arg("-e").arg(e);
        }
        // Clean handoff — exec replaces us, child inherits stdio.
        let err = cmd.exec();
        // exec() only returns on error.
        Err(anyhow!("exec wg nex failed: {}", err))
    }
    #[cfg(not(unix))]
    {
        // Fallback on non-Unix: spawn + wait + propagate exit code.
        let _ = (chat_ref, role, resume, model, endpoint, workgraph_dir);
        Err(anyhow!(
            "spawn-task dispatch not yet supported on this platform"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn mktask(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: id.to_string(),
            ..Default::default()
        }
    }

    /// Save and restore WG_EXECUTOR_TYPE + WG_MODEL + WG_GLOBAL_DIR across a
    /// test body. `set_exec` / `set_model` configure the env for the duration
    /// of `f`. Env restoration runs even on panic via Drop, so failed
    /// assertions don't leak into other tests.
    ///
    /// `WG_GLOBAL_DIR` is repointed at a fresh empty tempdir for the duration
    /// of `f`. `resolve_handler` calls `Config::load_or_default`, which merges
    /// the machine-global `~/.wg/config.toml` and consults `~/.wg/active-profile`;
    /// on a developer machine that has, say, an active `opencode` profile, the
    /// global `[dispatcher].model = "opencode:openrouter/…"` executor-qualified
    /// route leaks in and overrides the `WG_EXECUTOR_TYPE=native` hint these
    /// tests pin — routing to OpenCode and failing the `expected Native handler`
    /// assertions. Pointing `WG_GLOBAL_DIR` at an empty dir makes
    /// `Config::global_dir()` (the single chokepoint for global config +
    /// active-profile) resolve to nothing, so these tests depend only on the
    /// per-test tempdir + env, never the global profile. We override
    /// `WG_GLOBAL_DIR` rather than `HOME` so sibling tests that shell out to
    /// `git` are unaffected.
    fn with_env<R>(set_exec: Option<&str>, set_model: Option<&str>, f: impl FnOnce() -> R) -> R {
        struct EnvGuard {
            saved_exec: Option<String>,
            saved_model: Option<String>,
            saved_global_dir: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                unsafe {
                    match self.saved_exec.take() {
                        Some(v) => std::env::set_var("WG_EXECUTOR_TYPE", v),
                        None => std::env::remove_var("WG_EXECUTOR_TYPE"),
                    }
                    match self.saved_model.take() {
                        Some(v) => std::env::set_var("WG_MODEL", v),
                        None => std::env::remove_var("WG_MODEL"),
                    }
                    match self.saved_global_dir.take() {
                        Some(v) => std::env::set_var("WG_GLOBAL_DIR", v),
                        None => std::env::remove_var("WG_GLOBAL_DIR"),
                    }
                }
            }
        }
        // Keep the tempdir alive for the whole body; dropped after `_guard`
        // restores the env (drop order is reverse of declaration).
        let global = tempfile::tempdir().unwrap();
        let _guard = EnvGuard {
            saved_exec: std::env::var("WG_EXECUTOR_TYPE").ok(),
            saved_model: std::env::var("WG_MODEL").ok(),
            saved_global_dir: std::env::var("WG_GLOBAL_DIR").ok(),
        };
        unsafe {
            std::env::set_var("WG_GLOBAL_DIR", global.path());
            match set_exec {
                Some(v) => std::env::set_var("WG_EXECUTOR_TYPE", v),
                None => std::env::remove_var("WG_EXECUTOR_TYPE"),
            }
            match set_model {
                Some(v) => std::env::set_var("WG_MODEL", v),
                None => std::env::remove_var("WG_MODEL"),
            }
        }
        f()
    }

    /// Write a project-local `config.toml` into `dir` pinning a native
    /// (`nex:`) model. Without it, `Config::load_or_default` falls back to
    /// the built-in `[agent].model = "claude:opus"` default; the model-compat
    /// override in `plan_spawn` then reroutes the pinned
    /// `WG_EXECUTOR_TYPE=native` hint to the claude handler (logging
    /// `native ... cannot run model claude:opus ... routing to claude`),
    /// failing the `expected Native handler` assertions. Combined with
    /// `with_env`'s empty `WG_GLOBAL_DIR`, handler resolution then depends
    /// only on this per-test temp config — never on the machine's global
    /// config or active profile.
    fn pin_native_config(dir: &Path) {
        std::fs::write(
            dir.join("config.toml"),
            b"[agent]\nmodel = \"nex:qwen3-coder\"\n",
        )
        .unwrap();
    }

    // These tests pin WG_EXECUTOR_TYPE=native because role/resume are
    // Native-handler-specific concepts; the dispatcher default (Claude)
    // would route to a Claude handler with no role/resume fields. We also
    // scrub WG_MODEL because the agent harness running cargo test sets it
    // to the agent's resolved model, which would otherwise leak in via
    // dispatch::plan_spawn's `default_model` arg.
    #[test]
    #[serial]
    fn coordinator_task_gets_coordinator_role() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".wg")).unwrap();
            pin_native_config(dir.path());
            let task = mktask(".coordinator-0");
            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            match spec {
                HandlerSpec::Native { role, .. } => {
                    assert_eq!(role, Some("coordinator".to_string()));
                }
                _ => panic!("expected Native handler"),
            }
        });
    }

    #[test]
    #[serial]
    fn non_coordinator_task_gets_no_role() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            pin_native_config(dir.path());
            let task = mktask("my-task");
            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            match spec {
                HandlerSpec::Native { role, .. } => {
                    assert!(role.is_none(), "regular task should not have a role");
                }
                _ => panic!("expected Native handler"),
            }
        });
    }

    #[test]
    #[serial]
    fn role_override_wins() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            pin_native_config(dir.path());
            let task = mktask(".coordinator-0");
            let spec = resolve_handler(dir.path(), &task, Some("evaluator")).unwrap();
            match spec {
                HandlerSpec::Native { role, .. } => {
                    assert_eq!(role, Some("evaluator".to_string()));
                }
                _ => panic!("expected Native handler"),
            }
        });
    }

    #[test]
    #[serial]
    fn resume_true_when_journal_exists() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            pin_native_config(dir.path());
            let task = mktask("have-journal");
            let chat = dir.path().join("chat").join(&task.id);
            std::fs::create_dir_all(&chat).unwrap();
            std::fs::write(chat.join("conversation.jsonl"), b"").unwrap();
            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            match spec {
                HandlerSpec::Native { resume, .. } => assert!(resume),
                _ => panic!(),
            }
        });
    }

    #[test]
    #[serial]
    fn resume_false_when_fresh() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            pin_native_config(dir.path());
            let task = mktask("fresh-task");
            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            match spec {
                HandlerSpec::Native { resume, .. } => assert!(!resume),
                _ => panic!(),
            }
        });
    }

    /// Composition glue regression: `.chat-N` task ids must resolve to
    /// the registered `chat-N` alias, not the literal `.chat-N` directory.
    /// Without this, `wg spawn-task .chat-N` → `wg nex --chat .chat-N`
    /// → `chat_dir_for_ref(".chat-N")` falls back to `chat/.chat-N/`,
    /// which no IPC writer touches — split-brain with the UUID dir the
    /// registered alias resolves to. Surfaced by integrate-nex-chat-end-to-end.
    #[test]
    #[serial]
    fn dot_chat_id_strips_leading_dot_for_chat_ref() {
        with_env(Some("native"), None, || {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".wg")).unwrap();
            pin_native_config(dir.path());
            let task = mktask(".chat-7");
            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            match spec {
                HandlerSpec::Native { chat_ref, .. } => {
                    assert_eq!(
                        chat_ref, "chat-7",
                        ".chat-N task id must map to the registered `chat-N` alias"
                    );
                }
                _ => panic!("expected Native handler"),
            }
        });
    }

    #[test]
    fn command_preview_has_chat_flag() {
        let spec = HandlerSpec::Native {
            chat_ref: "foo".into(),
            role: Some("coordinator".into()),
            resume: true,
            model: None,
            endpoint: None,
        };
        let p = spec.command_preview();
        assert!(p.contains("--chat foo"));
        assert!(p.contains("--resume"));
        assert!(p.contains("--role coordinator"));
    }

    #[test]
    #[serial]
    fn spawn_task_passes_model_to_claude_handler() {
        let saved = std::env::var("WG_EXECUTOR_TYPE").ok();
        unsafe { std::env::set_var("WG_EXECUTOR_TYPE", "claude") };
        let dir = tempfile::tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("config.toml").parent().unwrap()).unwrap();

        let mut task = mktask("test-task");
        task.model = Some("claude:opus".to_string());
        let spec = resolve_handler(wg_dir, &task, None).unwrap();

        if let Some(v) = saved {
            unsafe { std::env::set_var("WG_EXECUTOR_TYPE", v) };
        } else {
            unsafe { std::env::remove_var("WG_EXECUTOR_TYPE") };
        }

        let preview = spec.command_preview();
        match spec {
            HandlerSpec::Claude { model, .. } => {
                assert_eq!(
                    model,
                    Some("claude:opus".to_string()),
                    "task.model should pass through to HandlerSpec"
                );
            }
            _ => panic!("expected Claude handler"),
        }
        assert!(
            preview.contains("-m claude:opus"),
            "dry-run should include --model flag: {}",
            preview
        );
    }

    #[test]
    #[serial]
    fn spawn_task_falls_back_to_config_model() {
        with_env(Some("claude"), None, || {
            let dir = tempfile::tempdir().unwrap();
            let wg_dir = dir.path();
            std::fs::write(
                wg_dir.join("config.toml"),
                b"[coordinator]\nmodel = \"claude:opus\"\n",
            )
            .unwrap();

            let task = mktask(".coordinator-0");
            assert!(task.model.is_none(), "synthesized task has no model");
            let config = worksgood::config::Config::load_or_default(wg_dir);
            let expected_model = config
                .coordinator
                .model
                .clone()
                .unwrap_or_else(|| config.agent.model.clone());
            let expected_executor = worksgood::dispatch::handler_for_model(&expected_model);
            let spec = resolve_handler(wg_dir, &task, None).unwrap();

            let preview = spec.command_preview();
            let (actual_executor, actual_model) = match spec {
                HandlerSpec::Claude { model, .. } => ("claude", model),
                HandlerSpec::Codex { model, .. } => ("codex", model),
                HandlerSpec::OpenCode { model, .. } => ("opencode", model),
                HandlerSpec::Pi { model, .. } => ("pi", model),
                HandlerSpec::Native { model, .. } => ("native", model),
                HandlerSpec::Gemini { .. } => ("gemini", None),
            };
            assert_eq!(
                actual_executor,
                expected_executor.as_str(),
                "should use the handler implied by the effective config model"
            );
            assert_eq!(
                actual_model,
                Some(expected_model.clone()),
                "should fall back to the effective config model when task.model is None"
            );
            assert!(
                preview.contains(&format!("-m {}", expected_model)),
                "dry-run should include config model: {}",
                preview
            );
        });
    }

    #[test]
    #[serial]
    fn plain_pi_chat_does_not_inherit_config_model() {
        with_env(Some("pi"), Some("pi:lunaroute:glm-5.2-nvfp4"), || {
            let dir = tempfile::tempdir().unwrap();
            let mut task = mktask(".chat-0");
            task.tags = vec![worksgood::chat_id::CHAT_LOOP_TAG.to_string()];
            task.executor_preset_name = Some("pi".to_string());

            let spec = resolve_handler(dir.path(), &task, None).unwrap();
            let preview = spec.command_preview();
            match spec {
                HandlerSpec::Pi { model, chat_ref } => {
                    assert_eq!(chat_ref, "chat-0");
                    assert_eq!(
                        model, None,
                        "plain Pi chat should not inherit WG_MODEL/config route"
                    );
                }
                other => panic!("expected Pi handler, got {}", other.command_preview()),
            }
            assert!(
                preview.starts_with("pi "),
                "plain Pi chat should launch the Pi CLI directly: {}",
                preview
            );
            assert!(!preview.contains("--mode rpc"), "{}", preview);
            assert!(!preview.contains("--provider"), "{}", preview);
            assert!(!preview.contains("--model"), "{}", preview);
        });
    }

    #[test]
    #[serial]
    fn explicit_pi_chat_model_is_preserved() {
        with_env(
            Some("pi"),
            Some("pi:openrouter:anthropic/claude-haiku-4-5"),
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut task = mktask(".chat-1");
                task.tags = vec![worksgood::chat_id::CHAT_LOOP_TAG.to_string()];
                task.executor_preset_name = Some("pi".to_string());
                task.model = Some("pi:lunaroute:glm-5.2-nvfp4".to_string());

                let spec = resolve_handler(dir.path(), &task, None).unwrap();
                let preview = spec.command_preview();
                match spec {
                    HandlerSpec::Pi { model, chat_ref } => {
                        assert_eq!(chat_ref, "chat-1");
                        assert_eq!(model.as_deref(), Some("lunaroute:glm-5.2-nvfp4"));
                    }
                    other => panic!("expected Pi handler, got {}", other.command_preview()),
                }
                assert!(
                    preview.contains("--provider lunaroute --model glm-5.2-nvfp4"),
                    "{}",
                    preview
                );
            },
        );
    }

    #[test]
    #[serial]
    fn user_pinned_dated_id_passes_through_unchanged() {
        let saved = std::env::var("WG_EXECUTOR_TYPE").ok();
        unsafe { std::env::set_var("WG_EXECUTOR_TYPE", "claude") };
        let dir = tempfile::tempdir().unwrap();

        let mut task = mktask("pinned-task");
        task.model = Some("claude:claude-opus-4-6".to_string());
        let spec = resolve_handler(dir.path(), &task, None).unwrap();

        if let Some(v) = saved {
            unsafe { std::env::set_var("WG_EXECUTOR_TYPE", v) };
        } else {
            unsafe { std::env::remove_var("WG_EXECUTOR_TYPE") };
        }

        match spec {
            HandlerSpec::Claude { model, .. } => {
                assert_eq!(
                    model,
                    Some("claude:claude-opus-4-6".to_string()),
                    "user-pinned dated ID should pass through unchanged"
                );
            }
            _ => panic!("expected Claude handler"),
        }
    }

    #[test]
    #[serial]
    fn task_model_wins_over_config_model() {
        let saved = std::env::var("WG_EXECUTOR_TYPE").ok();
        unsafe { std::env::set_var("WG_EXECUTOR_TYPE", "claude") };
        let dir = tempfile::tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::write(
            wg_dir.join("config.toml"),
            b"[coordinator]\nmodel = \"claude:sonnet\"\n",
        )
        .unwrap();

        let mut task = mktask("override-task");
        task.model = Some("claude:opus".to_string());
        let spec = resolve_handler(wg_dir, &task, None).unwrap();

        if let Some(v) = saved {
            unsafe { std::env::set_var("WG_EXECUTOR_TYPE", v) };
        } else {
            unsafe { std::env::remove_var("WG_EXECUTOR_TYPE") };
        }

        match spec {
            HandlerSpec::Claude { model, .. } => {
                assert_eq!(
                    model,
                    Some("claude:opus".to_string()),
                    "task.model should win over config.coordinator.model"
                );
            }
            _ => panic!("expected Claude handler"),
        }
    }

    /// Regression test for chat-launched-with: when the daemon supervisor
    /// spawns `wg spawn-task .chat-N`, it sets BOTH `WG_EXECUTOR_TYPE` and
    /// `WG_MODEL` env vars carrying the per-chat overrides resolved from
    /// `CoordinatorState`. spawn-task must honor BOTH — previously only
    /// WG_EXECUTOR_TYPE was read, so the chat would dispatch to the right
    /// executor binary but with the wrong model (the global
    /// `[dispatcher].model` fallback, which is `claude:opus` in most
    /// installs). The user-visible symptom: "I asked for codex and got
    /// claude" — the codex-handler received `-m claude:opus` and either
    /// errored out or fell through.
    #[test]
    #[serial]
    fn spawn_task_propagates_wg_model_env_var() {
        let saved_exec = std::env::var("WG_EXECUTOR_TYPE").ok();
        let saved_model = std::env::var("WG_MODEL").ok();
        unsafe {
            std::env::set_var("WG_EXECUTOR_TYPE", "codex");
            std::env::set_var("WG_MODEL", "codex:gpt-5");
        }
        let dir = tempfile::tempdir().unwrap();
        let wg_dir = dir.path();
        // Set a config.coordinator.model that differs from the env var so
        // we can tell which one won.
        std::fs::write(
            wg_dir.join("config.toml"),
            b"[coordinator]\nmodel = \"claude:opus\"\n",
        )
        .unwrap();

        // Synthesized chat task — no task.model field, mirroring what
        // create_chat_in_graph writes today.
        let task = mktask(".chat-7");
        assert!(task.model.is_none(), "chat tasks have no task.model today");
        let spec = resolve_handler(wg_dir, &task, None).unwrap();

        // Restore env before assertions.
        unsafe {
            if let Some(v) = saved_exec {
                std::env::set_var("WG_EXECUTOR_TYPE", v);
            } else {
                std::env::remove_var("WG_EXECUTOR_TYPE");
            }
            if let Some(v) = saved_model {
                std::env::set_var("WG_MODEL", v);
            } else {
                std::env::remove_var("WG_MODEL");
            }
        }

        match spec {
            HandlerSpec::Codex { model, .. } => {
                assert_eq!(
                    model,
                    Some("codex:gpt-5".to_string()),
                    "WG_MODEL env var must take precedence over \
                     config.coordinator.model. Got: {:?}",
                    model,
                );
            }
            other => panic!(
                "expected Codex handler with codex:gpt-5 model, got {:?}",
                match other {
                    HandlerSpec::Claude { model, .. } => format!("Claude {{ model: {:?} }}", model),
                    HandlerSpec::Native { model, .. } => format!("Native {{ model: {:?} }}", model),
                    HandlerSpec::Codex { model, .. } => format!("Codex {{ model: {:?} }}", model),
                    HandlerSpec::OpenCode { model, .. } => {
                        format!("OpenCode {{ model: {:?} }}", model)
                    }
                    HandlerSpec::Pi { model, .. } => format!("Pi {{ model: {:?} }}", model),
                    HandlerSpec::Gemini { .. } => "Gemini".to_string(),
                }
            ),
        }
    }

    /// Fix C end-to-end (fix-nex-chat / diagnose-wg-nex root cause #2):
    /// `wg spawn-task --dry-run` for a task with `task.endpoint` set to an
    /// inline http(s)://URL must emit `wg nex --chat ... -e <url>` on
    /// stdout. Before Fix C, the URL was silently dropped and the dry-run
    /// emitted no `-e` flag — meaning even when the supervisor DID spawn,
    /// nex would talk to the global default endpoint (or fall through
    /// to provider heuristics) instead of the user's chosen URL.
    ///
    /// This is the exact reproduction scenario from `diagnose-wg-nex`:
    ///   `WG_EXECUTOR_TYPE=native WG_MODEL=qwen3-coder \
    ///    wg spawn-task --dry-run .chat-32`
    #[test]
    #[serial]
    fn dry_run_includes_inline_url_endpoint_from_task() {
        with_env(Some("native"), Some("nex:qwen3-coder"), || {
            let dir = tempfile::tempdir().unwrap();
            let wg_dir = dir.path();

            let mut task = mktask(".chat-32");
            task.tags = vec![worksgood::chat_id::CHAT_LOOP_TAG.to_string()];
            task.model = Some("nex:qwen3-coder".to_string());
            task.endpoint = Some("https://lambda01.tail334fe6.ts.net:30000".to_string());

            let spec = resolve_handler(wg_dir, &task, None).unwrap();
            let preview = spec.command_preview();

            match &spec {
                HandlerSpec::Native {
                    endpoint, model, ..
                } => {
                    assert_eq!(
                        endpoint.as_deref(),
                        Some("https://lambda01.tail334fe6.ts.net:30000"),
                        "Native handler MUST carry task.endpoint URL — got endpoint={:?}, model={:?}, preview={}",
                        endpoint,
                        model,
                        preview
                    );
                }
                other => panic!(
                    "expected Native handler with inline URL endpoint, got: {}",
                    other.command_preview()
                ),
            }

            assert!(
                preview.contains("-e https://lambda01.tail334fe6.ts.net:30000"),
                "dry-run preview MUST include -e <url>, got: {}",
                preview
            );
        });
    }

    #[test]
    #[serial]
    fn external_worker_executors_get_worker_only_spawn_task_error() {
        // OpenCode is intentionally excluded: it now ships a live chat handler
        // (`wg opencode-handler --chat`), so it maps to a HandlerSpec rather
        // than the worker-only error (see
        // `opencode_executor_maps_to_opencode_handler`). The remaining
        // external CLIs stay worker-only.
        for executor in ["aider", "goose", "qwen", "cline", "crush", "amplifier"] {
            with_env(Some(executor), Some("claude:opus"), || {
                let dir = tempfile::tempdir().unwrap();
                let task = mktask(".chat-0");

                let err = resolve_handler(dir.path(), &task, None)
                    .expect_err("external worker executor must not map to a live handler")
                    .to_string();

                assert!(
                    err.contains(executor),
                    "error should name executor '{}': {}",
                    executor,
                    err
                );
                assert!(
                    err.contains("worker-only"),
                    "error should explain worker-only boundary: {}",
                    err
                );
                assert!(
                    err.contains("spawn-task/live chat"),
                    "error should name the rejected live path: {}",
                    err
                );
            });
        }
    }

    /// Goal #5 (fix-opencode-build): opencode is chat-capable. With
    /// `WG_EXECUTOR_TYPE=opencode` and an opencode model, a `.chat-*` task must
    /// resolve to the OpenCode handler whose preview names `wg opencode-handler
    /// --chat` and carries the model explicitly — NOT the worker-only error.
    #[test]
    #[serial]
    fn opencode_executor_maps_to_opencode_handler() {
        with_env(
            Some("opencode"),
            Some("opencode:openrouter/stepfun/step-3.7-flash"),
            || {
                let dir = tempfile::tempdir().unwrap();
                let task = mktask(".chat-0");

                let spec = resolve_handler(dir.path(), &task, None)
                    .expect("opencode must map to a live handler, not the worker-only error");

                match &spec {
                    HandlerSpec::OpenCode { model, .. } => {
                        // The inner model is normalized to the openrouter spec.
                        assert_eq!(model.as_deref(), Some("openrouter:stepfun/step-3.7-flash"));
                    }
                    other => panic!(
                        "expected OpenCode handler, got: {}",
                        other.command_preview()
                    ),
                }

                let preview = spec.command_preview();
                assert!(
                    preview.contains("wg opencode-handler --chat"),
                    "preview must dispatch to the opencode handler: {}",
                    preview
                );
                assert!(
                    preview.contains("openrouter:stepfun/step-3.7-flash"),
                    "preview must pass the resolved model explicitly: {}",
                    preview
                );
            },
        );
    }
}
