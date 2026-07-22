//! Spawn execution — claims a task, assembles prompt, launches executor process,
//! and registers the agent.

use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use worksgood::agency;
use worksgood::config::{CapBehavior, Config, EndpointConfig, ReasoningLevel};
use worksgood::dispatch::plan_spawn;
use worksgood::graph::{LogEntry, Node, Status, Task, is_system_task};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::service::executor::{ExecutorRegistry, PromptTemplate, TemplateVars, build_prompt};
use worksgood::service::registry::AgentRegistry;

use super::context::{
    build_previous_attempt_context, build_scope_context, build_task_context, discover_test_files,
    format_test_discovery_context, resolve_task_exec_mode, resolve_task_scope,
};
use super::worktree;
use super::{
    SpawnResult, agent_output_dir, graph_path, parse_timeout_secs, prompt_file_command,
    sanitize_bash_path, shell_escape, strip_verbatim_prefix,
};

/// Internal shared implementation for spawning an agent.
/// Both `run()` (CLI) and `spawn_agent()` (coordinator) delegate here.
pub(crate) fn spawn_agent_inner(
    dir: &Path,
    task_id: &str,
    executor_name: &str,
    timeout: Option<&str>,
    model: Option<&str>,
    spawned_by: &str,
) -> Result<SpawnResult> {
    spawn_agent_inner_with_reasoning(
        dir,
        task_id,
        executor_name,
        timeout,
        model,
        None,
        spawned_by,
    )
}

/// Internal shared implementation for spawning an agent with structured reasoning.
pub(crate) fn spawn_agent_inner_with_reasoning(
    dir: &Path,
    task_id: &str,
    executor_name: &str,
    timeout: Option<&str>,
    model: Option<&str>,
    reasoning: Option<&str>,
    spawned_by: &str,
) -> Result<SpawnResult> {
    let graph_path = graph_path(dir);

    if !graph_path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Load the graph and get task info
    let graph = load_graph(&graph_path).context("Failed to load graph")?;

    let task = graph.get_task_or_err(task_id)?;

    // Selection preflight must happen before plan resolution, worktree creation,
    // registry writes, or the atomic claim. Shell tasks remain graph-only.
    #[cfg(not(test))]
    if executor_name != "shell" && resolve_task_exec_mode(task, dir) != "shell" {
        let explicit = model
            .map(|m| (m, false))
            .or_else(|| task.model.as_deref().map(|m| (m, true)));
        worksgood::execution_selection::require(dir, explicit, "wg spawn")?;
    }

    // Capture audit info before mutable borrows
    let task_title_for_audit = task.title.clone();
    let task_agent_for_audit = task.agent.clone();

    // Look up agency agent preferences if task has an assigned agent identity.
    // These are used later in model/provider resolution.
    let (agent_preferred_model, agent_preferred_provider) =
        if let Some(ref agent_hash) = task_agent_for_audit {
            let agents_dir = dir.join("agency/cache/agents");
            match agency::find_agent_by_prefix(&agents_dir, agent_hash) {
                Ok(agent) => (
                    agent.preferred_model.clone(),
                    agent.preferred_provider.clone(),
                ),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

    // SINGLE SOURCE OF TRUTH: route spawn decisions through plan_spawn so that
    // {executor, model, endpoint} are decided in one place rather than
    // independently in the argv builder. The CLI's --executor flag
    // (`executor_name`) is treated as the caller's chosen executor floor and
    // passed in via the `agent_executor` slot — that gives it priority over
    // [dispatcher].executor, matching the expected CLI override semantics.
    //
    // The plan-derived endpoint is the only source consulted when assembling
    // native-executor argv flags below; there is no fallback ad-hoc lookup.
    let config = Config::load_or_default(dir);
    // Get task model preference. Freeform task tags are inert labels, so they
    // never participate in executor/model routing.
    let task_model = task.model.clone().or_else(|| {
        if let Some(ref tier_str) = task.tier
            && let Ok(tier) = tier_str.parse::<worksgood::config::Tier>()
            && let Some(resolved) = config.resolve_tier(tier)
        {
            return Some(resolved.model);
        }
        None
    });
    let plan_default_model = task_model.as_deref().or(model);
    let explicit_reasoning = reasoning
        .map(str::parse::<ReasoningLevel>)
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = plan_spawn(task, &config, Some(executor_name), plan_default_model)?;
    eprintln!(
        "[{}] {}: {}",
        spawned_by,
        task_id,
        plan.provenance.log_line(&plan)
    );
    let resolved_executor_name = plan.executor.as_str();
    let resolved_model_for_spawn = Some(plan.model.raw.clone());
    let resolved_reasoning = explicit_reasoning.or(task.reasoning).or(plan.reasoning);
    // One opaque run identity ties the spawned route, completion outcome, and
    // dead-agent triage together. Unlike PID/timestamps, it cannot collide on
    // a fast restart or PID reuse.
    let spawn_run_id = uuid::Uuid::new_v4().to_string();
    let health_route = worksgood::service::HealthRouteKey::from_spawn_plan(&plan);

    // Only allow spawning on tasks that are Open or Blocked
    match task.status {
        Status::Open | Status::Blocked | Status::Incomplete => {}
        Status::InProgress => {
            let since = task
                .started_at
                .as_ref()
                .map(|t| format!(" (since {})", t))
                .unwrap_or_default();
            match &task.assigned {
                Some(assigned) => {
                    anyhow::bail!(
                        "Task '{}' is already claimed by @{}{}",
                        task_id,
                        assigned,
                        since
                    );
                }
                None => {
                    anyhow::bail!("Task '{}' is already in progress{}", task_id, since);
                }
            }
        }
        Status::Done => {
            anyhow::bail!("Task '{}' is already done", task_id);
        }
        Status::Failed => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is Failed. Use 'wg retry' first.",
                task_id
            );
        }
        Status::Abandoned => {
            anyhow::bail!("Cannot spawn on task '{}': task is Abandoned", task_id);
        }
        Status::Waiting => {
            anyhow::bail!("Cannot spawn on task '{}': task is Waiting", task_id);
        }
        Status::PendingValidation => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending validation",
                task_id
            );
        }
        Status::PendingEval => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending evaluation",
                task_id
            );
        }
        Status::FailedPendingEval => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending rescue evaluation",
                task_id
            );
        }
    }

    // Resolve context scope (config was loaded earlier for plan_spawn)

    // Check OpenRouter cost caps before proceeding with expensive operations
    if let Some(provider) = resolved_model_for_spawn
        .as_deref()
        .and_then(|m| worksgood::config::parse_model_spec(m).provider)
        && provider == "openrouter"
    {
        check_openrouter_cost_caps(&config, dir, task_id, resolved_model_for_spawn.as_deref())?;
    }

    let scope = resolve_task_scope(task, &config, dir);

    // Build context from dependencies
    let task_context = build_task_context(&graph, task);

    // Build scope context for prompt assembly
    let mut scope_ctx = build_scope_context(&graph, task, scope, &config, dir);

    // Inject previous attempt context on retry OR on in-place eval rescue
    // (rescue_count > 0 means the prior attempt failed the eval gate and we
    // are iterating in place — the evaluator's notes belong in the next
    // agent's context). See `commands::evaluate::check_eval_gate`.
    if task.retry_count > 0 || task.rescue_count > 0 {
        let max_tokens = config.checkpoint.retry_context_tokens;
        scope_ctx.previous_attempt_context = build_previous_attempt_context(task, dir, max_tokens);
    }

    // Create template variables
    let mut vars = TemplateVars::from_task(task, Some(&task_context), Some(dir));

    // Detect failed dependencies for triage mode
    let mut failed_deps_lines = Vec::new();
    for dep_id in &task.after {
        if let Some(dep_task) = graph.get_task(dep_id)
            && dep_task.status == Status::Failed
        {
            let reason = dep_task.failure_reason.as_deref().unwrap_or("unknown");
            failed_deps_lines.push(format!(
                "- {}: \"{}\" — Reason: {}",
                dep_id, dep_task.title, reason
            ));
        }
    }
    if !failed_deps_lines.is_empty() {
        vars.has_failed_deps = true;
        vars.failed_deps_info = failed_deps_lines.join("\n");
    }

    // Pre-task test discovery: scan for test files and inject into agent context.
    if config.coordinator.auto_test_discovery {
        let project_root = dir
            .canonicalize()
            .ok()
            .and_then(|abs| abs.parent().map(|p| p.to_path_buf()));
        if let Some(ref root) = project_root {
            let test_files = discover_test_files(root);
            if !test_files.is_empty() {
                eprintln!(
                    "[spawn] Test discovery: found {} test file(s) for task '{}'",
                    test_files.len(),
                    task_id
                );
                scope_ctx.discovered_tests = format_test_discovery_context(&test_files);
            }
        }
    }

    // Get task exec command for shell executor
    let task_exec = task.exec.clone();
    // Get per-task timeout override
    let task_timeout = task.timeout.clone();
    // Capture the task's quality tier (may be set by tier escalation on retry)
    let task_tier = task.tier.clone();
    // Get session_id for resume (from previous wg wait)
    let resume_session_id = task.session_id.clone();
    // Resolve exec_mode: task.exec_mode > role.default_exec_mode > "full"
    let resolved_exec_mode = resolve_task_exec_mode(task, dir);
    // Load executor config using the registry
    let executor_registry = ExecutorRegistry::new(dir);
    let executor_config = executor_registry.load_config(resolved_executor_name)?;

    // For shell executor, we need an exec command
    if executor_config.executor.executor_type == "shell" && task_exec.is_none() {
        anyhow::bail!("Task '{}' has no exec command for shell executor", task_id);
    }

    // --- Unified model + provider resolution ---
    // Resolves model and provider in a single pass through the precedence hierarchy.
    // At each tier, if the model uses `provider:model` format, the provider is
    // extracted automatically via parse_model_spec().
    let task_provider = graph.get_task(task_id).and_then(|t| t.provider.clone());
    let resolved_task_agent =
        config.resolve_model_for_role(worksgood::config::DispatchRole::TaskAgent);
    let resolved = resolve_model_and_provider(
        resolved_model_for_spawn.clone(),
        task_provider.clone(),
        agent_preferred_model,
        agent_preferred_provider.clone(),
        executor_config.executor.model.clone(),
        Some(resolved_task_agent.model.clone()),
        resolved_task_agent.provider.clone(),
        resolved_model_for_spawn.as_deref(),
        config.coordinator.provider.clone(),
    );

    // --- Model registry alias resolution ---
    // If the effective model string matches a registry entry, resolve it to the
    // actual API model ID, provider, and endpoint. Built-in tier aliases
    // (haiku/sonnet/opus) are kept as-is for backward compatibility with the
    // Claude CLI, which understands them natively.
    let (effective_model, registry_provider, registry_endpoint) = resolve_spawn_model_via_registry(
        resolved_executor_name,
        resolved.model,
        resolved_model_for_spawn.as_ref(),
        &config,
        dir,
    )?;

    // --- Pre-flight model validation ---
    // Validate OpenRouter-style models against the cached model list before spawning.
    // This is a warning/suggestion system, not a hard gate.
    let (effective_model, model_validation_warning) = {
        let mut model = effective_model;
        let mut warning: Option<String> = None;
        if resolved_executor_name != "pi"
            && let Some(ref m) = model
            && m.contains('/')
            && !BUILTIN_TIER_ALIASES.contains(&m.as_str())
        {
            let validation =
                worksgood::executor::native::openai_client::validate_openrouter_model(m, dir);
            if !validation.was_valid {
                if let Some(ref w) = validation.warning {
                    eprintln!("[spawn] WARNING: {}", w);
                }
                warning = validation.warning;
                model = Some(validation.model);
            } else {
                eprintln!("[spawn] Model '{}' validated against model cache", m);
            }
        }
        (model, warning)
    };

    // Provider is still resolved by resolve_model_and_provider() above.
    // The registry may contribute a provider if the model matched a registry entry;
    // use it only when the tier cascade didn't already produce one.
    let effective_provider: Option<String> = resolved.provider.or(registry_provider.clone());

    // Override model in template vars with the effective model. External CLI
    // adapters receive their native model spelling here too, so TOML-backed
    // configs that use `{{model}}` do not have to understand WG's
    // `provider:model` syntax.
    if let Some(ref m) = effective_model {
        vars.model = model_template_value_for_executor(
            &executor_config.executor.executor_type,
            Some(m),
            effective_provider.as_deref(),
        )
        .unwrap_or_else(|| m.clone());
    }

    // Fail-safe direct-spawn admission. The dispatcher performs the same
    // class-specific check so it can skip builds and continue evaluators, but
    // process creation repeats it under the registry lock below to close races.
    let build_class = worksgood::disk_sentinel::classify_task(task);

    // Load agent registry with lock for concurrent safety.
    // The lock is held until save() to prevent two concurrent spawns from
    // reading the same next_agent_id and overwriting each other's registration.
    // Lock hierarchy: graph lock (per-call in load/save_graph) < registry lock (held here).
    let mut locked_registry = AgentRegistry::load_locked(dir)?;

    // The registry lock serializes the measured projection + reservation with
    // process registration. Without this second, projected check two concurrent
    // spawns could both spend the same free bytes after passing the cheap level
    // check above.
    if build_class.is_build_capable() {
        let admission = worksgood::disk_sentinel::build_admission_reclaiming_owned(
            dir,
            &config.coordinator.resource_management,
            build_class,
        );
        if !admission.allowed {
            anyhow::bail!(
                "build admission refused: {} (candidate={} bytes, concurrent-reserve={} bytes; safe retry will reuse this worktree)",
                admission.reason,
                admission.candidate_bytes,
                admission.concurrent_reserved_bytes
            );
        }
    }

    // We need to know the agent ID before spawning to set up the output directory
    let temp_agent_id = format!("agent-{}", locked_registry.next_agent_id);

    if build_class.is_heavy() {
        let active_heavy = locked_registry
            .all()
            .filter(|agent| {
                agent.is_live(
                    config
                        .coordinator
                        .resource_management
                        .disk_agent_heartbeat_seconds,
                ) && graph
                    .get_task(&agent.task_id)
                    .is_some_and(|task| worksgood::disk_sentinel::classify_task(task).is_heavy())
            })
            .count();
        if active_heavy >= config.coordinator.resource_management.max_build_agents {
            anyhow::bail!(
                "build-heavy admission budget full ({}/{})",
                active_heavy,
                config.coordinator.resource_management.max_build_agents
            );
        }
    }
    let output_dir = agent_output_dir(dir, &temp_agent_id);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "Failed to create agent output directory at {:?}",
            output_dir
        )
    })?;

    let output_file = output_dir.join("output.log");
    let output_file_str = output_file.to_string_lossy().to_string();

    // --- Worktree isolation ---
    // See `should_create_worktree` for the gating rules.
    let needs_worktree = should_create_worktree(
        config.coordinator.worktree_isolation,
        task_id,
        resolved_exec_mode.as_str(),
    );

    let worktree_info = if needs_worktree {
        let project_root = dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from {:?}", dir))?;

        // Retry-in-place: if a prior worktree exists for this task (from a
        // previous attempt that hit rate limit, crashed, or was killed),
        // reuse it. Preserves uncommitted WIP and prior commits — the new
        // agent starts in the same dir, on the same branch, with all that
        // context intact. The retention policy in worktree-sweep keeps the
        // dir alive until eval+merge so this path is reachable.
        //
        // Use `wg retry --fresh` to opt out and start clean.
        if let Some((prior_path, prior_branch)) =
            worktree::find_worktree_for_task(project_root, task_id)
        {
            // Clear any cleanup-pending marker that the prior agent's
            // wrapper may have written so the next sweep doesn't reap it.
            let marker =
                prior_path.join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER);
            if marker.exists() {
                let _ = fs::remove_file(&marker);
            }
            eprintln!(
                "[spawn] Reusing prior worktree for task '{}' at {:?} (branch: {}) — retry-in-place",
                task_id, prior_path, prior_branch
            );
            Some(worktree::WorktreeInfo {
                path: prior_path,
                branch: prior_branch,
                project_root: project_root.to_path_buf(),
            })
        } else {
            match worktree::create_worktree(project_root, dir, &temp_agent_id, task_id) {
                Ok(info) => {
                    eprintln!(
                        "[spawn] Created worktree for {} at {:?} (branch: {})",
                        temp_agent_id, info.path, info.branch
                    );
                    Some(info)
                }
                Err(e) => {
                    eprintln!(
                        "[spawn] Worktree creation failed for {}, falling back to shared working directory: {}",
                        temp_agent_id, e
                    );
                    None
                }
            }
        }
    } else {
        if config.coordinator.worktree_isolation {
            eprintln!(
                "[spawn] Skipping worktree for {} (task '{}', exec_mode={}): meta or non-writing task",
                temp_agent_id, task_id, resolved_exec_mode
            );
        }
        None
    };

    vars.in_worktree = worktree_info.is_some();

    let owned_target_path = if build_class.is_build_capable() {
        worksgood::disk_sentinel::target_path_for_agent(
            &config.coordinator.resource_management,
            worktree_info.as_ref().map(|wt| wt.path.as_path()),
            &temp_agent_id,
        )
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                worktree_info
                    .as_ref()
                    .map(|wt| wt.path.join(&path))
                    .or_else(|| std::env::current_dir().ok().map(|cwd| cwd.join(&path)))
                    .unwrap_or(path)
            }
        })
    } else {
        None
    };
    if let Some(path) = owned_target_path.as_ref() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create owned Cargo target {}", path.display()))?;
    }
    let owned_tmp_path = if build_class.is_build_capable() {
        let root = config
            .coordinator
            .resource_management
            .build_tmp_root
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Some(root.join(format!("wg-cargo-tmp-{temp_agent_id}")))
    } else {
        None
    };
    if let Some(path) = owned_tmp_path.as_ref() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create owned build scratch {}", path.display()))?;
    }

    // Apply templates to executor settings (with effective model in vars)
    let mut settings = executor_config.apply_templates(&vars);

    // Universal wg context injection for all executor types.
    // Ensures all executors receive consistent WG context in their prompts,
    // with model-appropriate knowledge tier based on context window and capabilities.
    let model_str = settings.model.as_deref().unwrap_or("");
    let model_tier = super::context::classify_model_tier(model_str);
    scope_ctx.wg_guide_content = super::context::build_tiered_guide(dir, model_tier, model_str);

    // Native executor exposes its own in-process file tools
    // (`read_file`/`write_file`/`edit_file`/`grep`/`glob`). When spawning
    // via native, inject the guidance section that teaches the model about
    // those tools and warns against bash-based file manipulation.
    scope_ctx.native_file_tools = settings.executor_type == "native";

    // Scope-based prompt assembly for built-in executors.
    // When no custom prompt_template is defined (built-in defaults),
    // use build_prompt() to assemble the prompt based on context scope.
    if settings.prompt_template.is_none() && executor_uses_auto_prompt(&settings.executor_type) {
        let prompt = build_prompt(&vars, scope, &scope_ctx);

        // Debug logging: capture spawn metadata if WG_DEBUG_PROMPTS is set
        if std::env::var("WG_DEBUG_PROMPTS").is_ok()
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/wg_debug_prompts.log")
        {
            use std::io::Write;
            let debug_info = format!(
                "=== WG DEBUG: Spawning Agent ===\n\
                    Task ID: {}\n\
                    Executor: {}\n\
                    Model: {}\n\
                    Context Scope: {:?}\n\
                    Execution Mode: {}\n\
                    Agent Identity: {}\n\
                    === End of Spawn Metadata ===\n\n",
                task_id,
                resolved_executor_name,
                vars.model,
                scope,
                resolved_exec_mode.as_str(),
                task.agent
                    .as_deref()
                    .unwrap_or("Default (no specific agent assigned)")
            );
            let _ = file.write_all(debug_info.as_bytes());
        }

        settings.prompt_template = Some(PromptTemplate { template: prompt });
    }

    // Use resolved exec_mode (already accounts for role defaults)
    let exec_mode = resolved_exec_mode.as_str();

    // Endpoint resolution: plan.endpoint is the single source of truth. For
    // executors that don't need an endpoint (claude/codex/shell),
    // plan.endpoint is None and the argv builder skips --endpoint-* flags
    // entirely. For native, plan.endpoint carries the resolved EndpointConfig.
    // No ad-hoc cascade lookup happens here anymore — that decision lives in
    // dispatch::plan::plan_spawn().
    let endpoint_config: Option<&EndpointConfig> = plan.endpoint.as_ref();
    let effective_endpoint: Option<String> = endpoint_config.map(|ep| ep.name.clone());
    let effective_endpoint_url: Option<String> = endpoint_config.and_then(|ep| ep.url.clone());
    let effective_api_key: Option<String> =
        endpoint_config.and_then(|ep| ep.resolve_api_key(Some(dir)).ok().flatten());

    let effective_working_dir = worktree_info
        .as_ref()
        .map(|wt| wt.path.as_path())
        .or_else(|| settings.working_dir.as_deref().map(Path::new));
    preflight_executor_command(&settings, resolved_executor_name, effective_working_dir)?;

    // Validate endpoint resolution for registry-resolved models — but only
    // when the plan actually selected an endpoint. If plan.endpoint is None
    // (e.g. executor=claude), the model registry's endpoint hint is irrelevant
    // to argv assembly and a missing/keyless endpoint must not block the spawn.
    if plan.endpoint.is_some()
        && let Some(ref reg_ep) = registry_endpoint
    {
        if endpoint_config.is_none() {
            anyhow::bail!(
                "Model references endpoint '{}' which is not configured.\n\
                 Add it with: wg endpoint add {} --provider <provider> --url <url>",
                reg_ep,
                reg_ep,
            );
        }
        if effective_api_key.is_none() {
            let ep = endpoint_config.unwrap(); // safe: checked above
            anyhow::bail!(
                "Endpoint '{}' (provider: {}) has no valid API key.\n\
                 Set one with: wg key set {} --value <key>",
                reg_ep,
                ep.provider,
                ep.provider,
            );
        }
    }

    // Build the inner command string first (with optional fallback for session resume)
    let (inner_command, fallback_command) = build_inner_command_with_reasoning(
        &settings,
        exec_mode,
        &output_dir,
        &effective_model,
        &effective_provider,
        resolved_reasoning,
        &effective_endpoint,
        &effective_endpoint_url,
        &effective_api_key,
        &vars,
        &task_exec,
        resume_session_id.as_deref(),
    )?;

    // Resolve effective timeout: CLI param > task.timeout > executor config > coordinator config.
    // Empty string means disabled.
    let effective_timeout_secs: Option<u64> = if let Some(t) = timeout {
        if t.is_empty() {
            None
        } else {
            Some(parse_timeout_secs(t).context("Invalid --timeout value")?)
        }
    } else if let Some(ref t) = task_timeout {
        if t.is_empty() {
            None
        } else {
            Some(parse_timeout_secs(t).context(format!(
                "Invalid task timeout value '{}'. \
                 Run `wg show {}` to inspect, then repair with \
                 `wg edit {} --timeout <30m|4h|1d>` or clear with \
                 `wg edit {} --timeout ''`.",
                t, task_id, task_id, task_id
            ))?)
        }
    } else if let Some(t) = settings.timeout {
        if t == 0 { None } else { Some(t) }
    } else {
        let agent_timeout = &config.coordinator.agent_timeout;
        if agent_timeout.is_empty() {
            None
        } else {
            Some(
                parse_timeout_secs(agent_timeout)
                    .context("Invalid coordinator.agent_timeout config")?,
            )
        }
    };

    // Build the actual command line, optionally wrapped with `timeout`.
    //
    // The timeout binary is resolved at runtime via $WG_TIMEOUT_BIN (set by
    // the wrapper header). macOS ships no GNU timeout — coreutils provides
    // it as `gtimeout`. When neither is available the wrapper warns and the
    // ${VAR:+...} expansion collapses to nothing, so the inner command runs
    // unwrapped instead of failing silently with empty stdin to the executor.
    let timed_command = if let Some(secs) = effective_timeout_secs {
        format!(
            r#"${{WG_TIMEOUT_BIN:+"$WG_TIMEOUT_BIN" --signal=TERM --kill-after=30 {} }}{}"#,
            secs, inner_command
        )
    } else {
        inner_command.clone()
    };
    let timed_fallback = fallback_command.map(|fb| {
        if let Some(secs) = effective_timeout_secs {
            format!(
                r#"${{WG_TIMEOUT_BIN:+"$WG_TIMEOUT_BIN" --signal=TERM --kill-after=30 {} }}{}"#,
                secs, fb
            )
        } else {
            fb
        }
    });

    // Create and write wrapper script
    let wrapper_path = write_wrapper_script(
        &output_dir,
        task_id,
        &output_file_str,
        &timed_command,
        effective_timeout_secs,
        &settings.executor_type,
        timed_fallback.as_deref(),
    )?;

    // Run the wrapper script. On Windows, `wrapper_path` often comes back
    // from PathBuf::canonicalize with the `\\?\` extended-length prefix
    // (e.g. `\\?\C:\src\ontempo\.workgraph\agents\agent-710\run.sh`). Most
    // Windows APIs accept that form, but Git-for-Windows' bash.exe does
    // not — it reports "No such file or directory" before the script can
    // run, and the agent dies instantly with no output.log. Strip the
    // prefix so bash sees a plain `C:\...` path, which it handles fine.
    // Resolve the bash binary via platform_bash so a stock Windows PATH
    // doesn't route us to the WSL shim (`C:\Windows\System32\bash.exe`).
    let bash_path = worksgood::platform_bash::bash_exe_path(config.bash.path.as_deref())
        .context("Failed to resolve bash executable for spawn wrapper")?;
    let mut cmd = Command::new(&bash_path);
    cmd.arg(strip_verbatim_prefix(&wrapper_path));

    // Set environment variables from executor config
    for (key, value) in &settings.env {
        cmd.env(key, value);
    }

    // Add task ID and agent ID to environment
    cmd.env("WG_TASK_ID", task_id);
    if let Some(chat_id) = worksgood::chat_id::parse_chat_task_id(task_id) {
        cmd.env("WG_CHAT_ID", task_id);
        cmd.env(
            "WG_CHAT_REF",
            worksgood::chat_id::format_chat_session_ref(chat_id),
        );
    }
    cmd.env("WG_AGENT_ID", &temp_agent_id);
    cmd.env("WG_EXECUTOR_TYPE", &settings.executor_type);
    // Time budget: inject timeout and spawn epoch for graceful completion
    if let Some(secs) = effective_timeout_secs {
        cmd.env("WG_TASK_TIMEOUT_SECS", secs.to_string());
    }
    cmd.env(
        "WG_SPAWN_EPOCH",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    );
    cmd.env("WG_SPAWN_RUN_ID", &spawn_run_id);
    // Propagate user identity to spawned agents
    cmd.env("WG_USER", worksgood::current_user());
    if let Some(ref m) = effective_model {
        cmd.env("WG_MODEL", m);
    }
    if let Some(reasoning) = resolved_reasoning {
        cmd.env("WG_REASONING", reasoning.as_str());
    }
    {
        let tier_str =
            task_tier.as_deref().unwrap_or_else(
                || match worksgood::config::DispatchRole::TaskAgent.default_tier() {
                    worksgood::config::Tier::Fast => "fast",
                    worksgood::config::Tier::Standard => "standard",
                    worksgood::config::Tier::Premium => "premium",
                },
            );
        cmd.env("WG_TIER", tier_str);
    }
    if let Some(ref ep) = effective_endpoint {
        cmd.env("WG_ENDPOINT", ep);
        cmd.env("WG_ENDPOINT_NAME", ep);
    }
    if let Some(ref provider) = effective_provider {
        cmd.env("WG_LLM_PROVIDER", provider);
    }
    if let Some(ref url) = effective_endpoint_url {
        cmd.env("WG_ENDPOINT_URL", url);
    }
    inject_api_key_env(&mut cmd, endpoint_config, &effective_api_key);

    // Pass through the Claude Code OAuth token (if configured in [auth]
    // of config.toml and not already in the daemon's env). Task agents
    // shell out to the `claude` CLI for the claude executor; without
    // this, agents on a headless Windows install either can't
    // authenticate or the user has to export the token in every shell
    // before starting the daemon. No-op when `claude login` has been
    // run — the CLI picks up `~/.claude/credentials.json` on its own.
    if let Some(token) = config.auth.resolve_claude_oauth_token() {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }

    // Set working directory: worktree overrides settings.working_dir
    if let Some(ref wt) = worktree_info {
        // Strip `\\?\` verbatim prefix: bash.exe (MinGW/Git-for-Windows) silently
        // produces no output when its CWD is a verbatim extended-length path like
        // `\\?\C:\...`. Rust's PathBuf::canonicalize returns these on Windows.
        let clean_worktree_path = sanitize_bash_path(&wt.path.to_string_lossy());
        let clean_project_root = sanitize_bash_path(&wt.project_root.to_string_lossy());
        cmd.current_dir(&clean_worktree_path);
        cmd.env("WG_WORKTREE_PATH", &clean_worktree_path);
        cmd.env("WG_BRANCH", &wt.branch);
        cmd.env("WG_PROJECT_ROOT", &clean_project_root);
        // Signal to Claude Code (and other tools) that this session is already
        // inside a managed worktree — do not create a competing one.
        cmd.env("WG_WORKTREE_ACTIVE", "1");
    } else if let Some(ref wd) = settings.working_dir {
        cmd.current_dir(wd);
    }
    if let Some(path) = owned_target_path.as_ref() {
        // Isolate Cargo and make the exact absolute/temporary path explicit in
        // the ownership registry after the child PID identity is available.
        cmd.env("CARGO_TARGET_DIR", path);
    }
    if let Some(path) = owned_tmp_path.as_ref() {
        cmd.env("TMPDIR", path);
    }

    // Wrapper script handles output redirect internally
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    // Detach the agent into its own session so it survives daemon restart/crash.
    // setsid() creates a new session and process group, making the agent
    // independent of the daemon's process group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    // Windows equivalent: put the child at the root of its own process
    // group so console control events (Ctrl+Break, Ctrl+C, window-close)
    // that reach — or cascade through — the daemon's group don't also
    // terminate the task agent. The coordinator spawn in
    // `coordinator_agent::spawn_claude_process` already sets this flag
    // for the same reason; task agents need the same protection.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x0000_0200);
    }

    // Claim the task BEFORE spawning the process to prevent race conditions
    // where two concurrent spawns both pass the status check.
    // Use modify_graph for atomic claim under flock.
    let spawned_by_clone = spawned_by.to_string();
    let executor_name_clone = resolved_executor_name.to_string();
    let effective_model_clone = effective_model.clone();
    let task_title_for_audit_clone = task_title_for_audit.clone();
    let task_agent_for_audit_clone = task_agent_for_audit.clone();
    let temp_agent_id_clone = temp_agent_id.clone();
    let task_id_str = task_id.to_string();
    let model_validation_warning_clone = model_validation_warning.clone();

    let mut claim_error: Option<anyhow::Error> = None;
    modify_graph(&graph_path, |graph| {
        let task = match graph.get_task_mut(&task_id_str) {
            Some(t) => t,
            None => {
                claim_error = Some(anyhow::anyhow!("Task '{}' not found", task_id_str));
                return false;
            }
        };
        task.status = Status::InProgress;
        task.started_at = Some(Utc::now().to_rfc3339());
        task.assigned = Some(temp_agent_id_clone.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some(temp_agent_id_clone.clone()),
            user: Some(worksgood::current_user()),
            message: format!(
                "Spawned by {} --executor {}{}",
                spawned_by_clone,
                executor_name_clone,
                effective_model_clone
                    .as_ref()
                    .map(|m| format!(" --model {}", m))
                    .unwrap_or_default()
            ),
        });

        // Log pre-flight model validation result
        if let Some(ref warning) = model_validation_warning_clone {
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("spawn".to_string()),
                user: None,
                message: format!("Pre-flight model validation: {}", warning),
            });
        }

        // Create .assign-* audit trail if missing (defense-in-depth).
        let assign_task_id = format!(".assign-{}", task_id_str);
        if !is_system_task(&task_id_str) && graph.get_task(&assign_task_id).is_none() {
            let now = Utc::now().to_rfc3339();
            let audit_desc = if let Some(ref agent_id) = task_agent_for_audit_clone {
                format!(
                    "Direct dispatch: agent={} → '{}'\nNo lightweight assignment flow (auto_assign disabled or skipped)",
                    agent_id, task_id_str
                )
            } else {
                format!(
                    "Direct dispatch: '{}'\nNo agent pre-assigned (auto_assign disabled or skipped)",
                    task_id_str
                )
            };
            graph.add_node(Node::Task(Task {
                id: assign_task_id,
                title: format!("Assign agent for: {}", task_title_for_audit_clone),
                description: Some(audit_desc),
                status: Status::Done,
                before: vec![task_id_str.clone()],
                tags: vec!["assignment".to_string(), "agency".to_string()],
                created_at: Some(now.clone()),
                started_at: Some(now.clone()),
                completed_at: Some(now),
                exec_mode: Some("bare".to_string()),
                visibility: "internal".to_string(),
                log: vec![LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: "Created at spawn time (no prior .assign-* task existed)".to_string(),
                }],
                ..Default::default()
            }));
        }
        true
    })
    .context("Failed to save graph")?;
    if let Some(e) = claim_error {
        return Err(e);
    }

    // Spawn the process (don't wait). If spawn fails, unclaim the task.
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Spawn failed — revert the task claim so it's not stuck
            let task_id_rollback = task_id.to_string();
            let agent_id_rollback = temp_agent_id.clone();
            let err_msg = format!("Spawn failed, reverting claim: {}", e);
            if let Err(rollback_err) = modify_graph(&graph_path, |graph| {
                if let Some(t) = graph.get_task_mut(&task_id_rollback) {
                    t.status = Status::Open;
                    t.started_at = None;
                    t.assigned = None;
                    t.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some(agent_id_rollback.clone()),
                        user: Some(worksgood::current_user()),
                        message: err_msg.clone(),
                    });
                    true
                } else {
                    false
                }
            }) {
                eprintln!(
                    "Warning: failed to rollback graph for task '{}': {}",
                    task_id, rollback_err
                );
            }
            // Worktrees are sacred — preserved even on spawn failure so the
            // user can inspect what was set up. Remove via `wg worktree archive --remove`.
            if let Some(ref wt) = worktree_info {
                eprintln!(
                    "[spawn] Spawn failed for task '{}' — preserving worktree at {:?} (remove via: wg worktree archive <agent-id> --remove)",
                    task_id, wt.path
                );
            }
            return Err(anyhow::anyhow!(
                "Failed to spawn executor '{}' (command: {}): {}",
                resolved_executor_name,
                settings.command,
                e
            ));
        }
    };

    let pid = child.id();

    // Register the agent (with model tracking)
    let agent_id = locked_registry.register_agent_with_model(
        pid,
        task_id,
        resolved_executor_name,
        &output_file_str,
        effective_model.as_deref(),
    );
    // Record the worktree path so the target-dir reaper can detect
    // `wg retry`-in-place: the new agent's ID differs from the directory
    // name (which was minted by a prior, now-dead agent).
    if let Some(ref wt) = worktree_info {
        locked_registry.set_worktree_path(&agent_id, &wt.path);
    }
    // Persist every build-capable worker's exact owned paths before allowing
    // the spawn to become an invisible cache producer. Ownership includes the
    // task, agent, exact PID start identity, mount and lease; names alone are
    // never used by cleanup.
    let lease_seconds = config
        .coordinator
        .resource_management
        .owned_cache_lease_seconds;
    let worktree_path = worktree_info.as_ref().map(|wt| wt.path.as_path());
    let mut ownership_error = None;
    if let Some(path) = owned_target_path.as_ref() {
        let cache = worksgood::disk_sentinel::make_owned_cache(
            path,
            worksgood::disk_sentinel::CacheKind::CargoTarget,
            task_id,
            &agent_id,
            pid,
            worktree_path,
            lease_seconds,
        );
        if let Err(error) = worksgood::disk_sentinel::register_owned_cache(dir, cache) {
            ownership_error = Some(error);
        }
    }
    if ownership_error.is_none()
        && let Some(path) = owned_tmp_path.as_ref()
    {
        let cache = worksgood::disk_sentinel::make_owned_cache(
            path,
            worksgood::disk_sentinel::CacheKind::CargoInstallScratch,
            task_id,
            &agent_id,
            pid,
            worktree_path,
            lease_seconds,
        );
        if let Err(error) = worksgood::disk_sentinel::register_owned_cache(dir, cache) {
            ownership_error = Some(error);
        }
    }
    if let Some(error) = ownership_error {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        let task_id_rollback = task_id.to_string();
        let _ = modify_graph(&graph_path, |graph| {
            if let Some(task) = graph.get_task_mut(&task_id_rollback) {
                task.status = Status::Open;
                task.started_at = None;
                task.assigned = None;
                true
            } else {
                false
            }
        });
        return Err(error.context("failed to persist build-cache ownership; killed spawned worker"));
    }

    // save() consumes the LockedRegistry, releasing the lock after write.
    if let Err(save_err) = locked_registry.save() {
        // Registry save failed — kill the orphaned process to prevent invisible agents
        eprintln!(
            "Warning: failed to save agent registry for {} (PID {}), killing process: {}",
            agent_id, pid, save_err
        );
        #[cfg(unix)]
        {
            // SAFETY: sending SIGKILL to a known PID we just spawned
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        return Err(save_err.context("Failed to persist agent registry after spawn"));
    }

    // Advance message cursor for this agent so queued messages aren't re-read.
    // The queued messages were already included in the prompt via ScopeContext.
    if let Ok(all_msgs) = worksgood::messages::list_messages(dir, task_id)
        && let Some(last) = all_msgs.last()
    {
        let _ = worksgood::messages::write_cursor(dir, &agent_id, task_id, last.id);
    }

    // Write metadata
    let metadata_path = output_dir.join("metadata.json");
    let mut metadata = serde_json::json!({
        "agent_id": agent_id,
        "pid": pid,
        "task_id": task_id,
        "executor": resolved_executor_name,
        "model": &effective_model,
        "reasoning": resolved_reasoning.map(|r| r.as_str()),
        "started_at": Utc::now().to_rfc3339(),
        "run_id": &spawn_run_id,
        "health_route": &health_route,
        "timeout_secs": effective_timeout_secs,
    });
    if let Some(ref wt) = worktree_info {
        metadata["worktree_path"] = serde_json::json!(wt.path.to_string_lossy());
        metadata["worktree_branch"] = serde_json::json!(&wt.branch);
    }
    metadata["owned_cache_paths"] = serde_json::json!(
        [owned_target_path.as_ref(), owned_tmp_path.as_ref()]
            .into_iter()
            .flatten()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
    );
    fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?)?;

    Ok(SpawnResult {
        agent_id,
        pid,
        task_id: task_id.to_string(),
        executor: resolved_executor_name.to_string(),
        executor_type: settings.executor_type.clone(),
        output_file: output_file_str,
        model: effective_model,
        reasoning: resolved_reasoning.map(|r| r.to_string()),
    })
}

/// Decide whether a spawning agent should get its own git worktree.
///
/// Worktrees provide file-level isolation for agents that may edit source code.
/// They are skipped for tasks that don't touch the source tree:
///
/// - **System/meta tasks** (`.assign-*`, `.flip-*`, `.evaluate-*`, `.place-*`,
///   `.compact-*`, etc.) only call LLMs and mutate graph state — they never
///   write to the filesystem. Identified by the leading `.` prefix convention
///   documented in `graph.rs`.
/// - **`bare` exec mode** — coordination-only tasks with just the `wg` CLI;
///   cannot write to the source tree.
/// - **`light` exec mode** — read-only tools for research/review tasks;
///   cannot write to the source tree.
///
/// Code-touching tasks (`full`, `shell`) still get isolated worktrees.
pub(crate) fn should_create_worktree(
    worktree_isolation_enabled: bool,
    task_id: &str,
    exec_mode: &str,
) -> bool {
    if !worktree_isolation_enabled {
        return false;
    }
    if task_id.starts_with('.') {
        return false;
    }
    if matches!(exec_mode, "bare" | "light") {
        return false;
    }
    true
}

/// Built-in executors that ship without a `prompt_template` and rely on
/// `build_prompt()` to assemble the agent prompt at spawn time.
///
/// CLI handlers (claude, codex) and in-process handlers (native) all need
/// the same WG-context preamble, so the spawn pipeline auto-builds a
/// `PromptTemplate` for any of these when the user hasn't supplied one in
/// their executor config. Adding a new built-in handler without listing it
/// here means the spawn writes no prompt.txt and the resulting subprocess
/// receives empty stdin — exactly the codex bug.
fn executor_uses_auto_prompt(executor_type: &str) -> bool {
    matches!(
        executor_type,
        "claude"
            | "codex"
            | "native"
            | "opencode"
            | "aider"
            | "goose"
            | "qwen"
            | "qwen-code"
            | "qwen_code"
            | "cline"
            | "crush"
            | "amplifier"
            | "octomind"
            | "dexto"
            | "pi"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalCliModelStyle {
    /// `--model openrouter/<model-id>`
    ProviderSlashModel,
    /// `--provider openrouter --model <model-id>`
    ProviderFlagAndModel,
    /// `--model <model-id>` after the CLI has been configured for OpenRouter.
    BareOpenRouterModel,
    /// `--model openrouter:<model-id>` — the provider-colon-route spelling
    /// Octomind's `-m`/`--model` flag consumes (e.g.
    /// `--model openrouter:minimax/minimax-m3`).
    ProviderColonModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ExternalCliModelArgs {
    provider: Option<(&'static str, String)>,
    model: Option<(&'static str, String)>,
}

impl ExternalCliModelArgs {
    #[cfg(test)]
    fn to_vec(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some((flag, value)) = &self.provider {
            args.push((*flag).to_string());
            args.push(value.clone());
        }
        if let Some((flag, value)) = &self.model {
            args.push((*flag).to_string());
            args.push(value.clone());
        }
        args
    }
}

fn external_cli_model_style(executor_type: &str) -> Option<ExternalCliModelStyle> {
    match executor_type {
        "opencode" | "aider" | "crush" => Some(ExternalCliModelStyle::ProviderSlashModel),
        "goose" | "cline" | "pi" => Some(ExternalCliModelStyle::ProviderFlagAndModel),
        "qwen" | "qwen-code" | "qwen_code" => Some(ExternalCliModelStyle::BareOpenRouterModel),
        // Octomind's `-m` takes WG's `openrouter:<vendor>/<model>` spelling.
        // (Dexto is intentionally absent: its CLI rejects provider/model
        // routes and requires a generated agent YAML, so it has no worker-path
        // argv model style — prototype-octomind-dexto-chat.)
        "octomind" => Some(ExternalCliModelStyle::ProviderColonModel),
        _ => None,
    }
}

fn openrouter_model_id(model: &str, effective_provider: Option<&str>) -> Option<String> {
    let spec = worksgood::config::parse_model_spec(model);
    let provider_from_model = spec
        .provider
        .as_deref()
        .map(worksgood::config::provider_to_native_provider);
    let provider = effective_provider.or(provider_from_model);
    if provider == Some("openrouter") {
        Some(
            spec.model_id
                .strip_prefix("openrouter/")
                .unwrap_or(&spec.model_id)
                .to_string(),
        )
    } else {
        None
    }
}

fn external_cli_model_args(
    executor_type: &str,
    effective_model: Option<&str>,
    effective_provider: Option<&str>,
) -> ExternalCliModelArgs {
    let Some(model) = effective_model else {
        return ExternalCliModelArgs::default();
    };
    let Some(style) = external_cli_model_style(executor_type) else {
        return ExternalCliModelArgs::default();
    };

    if executor_type == "pi" {
        let inner = model.strip_prefix("pi:").unwrap_or(model).trim();
        if let Some((provider, model_id)) = inner.split_once(':').or_else(|| inner.split_once('/'))
        {
            let provider = provider.trim();
            let model_id = model_id.trim();
            if !provider.is_empty() && !model_id.is_empty() {
                return ExternalCliModelArgs {
                    provider: Some(("--provider", provider.to_string())),
                    model: Some(("--model", model_id.to_string())),
                };
            }
        }
    }

    if let Some(openrouter_model) = openrouter_model_id(model, effective_provider) {
        return match style {
            ExternalCliModelStyle::ProviderSlashModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", format!("openrouter/{}", openrouter_model))),
            },
            ExternalCliModelStyle::ProviderFlagAndModel => ExternalCliModelArgs {
                provider: Some(("--provider", "openrouter".to_string())),
                model: Some(("--model", openrouter_model)),
            },
            ExternalCliModelStyle::BareOpenRouterModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", openrouter_model)),
            },
            ExternalCliModelStyle::ProviderColonModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", format!("openrouter:{}", openrouter_model))),
            },
        };
    }

    ExternalCliModelArgs {
        provider: effective_provider.map(|p| ("--provider", p.to_string())),
        model: Some(("--model", model.to_string())),
    }
}

fn model_template_value_for_executor(
    executor_type: &str,
    effective_model: Option<&str>,
    effective_provider: Option<&str>,
) -> Option<String> {
    let model = effective_model?;
    let style = external_cli_model_style(executor_type)?;
    let openrouter_model = openrouter_model_id(model, effective_provider)?;
    Some(match style {
        ExternalCliModelStyle::ProviderSlashModel => format!("openrouter/{}", openrouter_model),
        ExternalCliModelStyle::ProviderColonModel => format!("openrouter:{}", openrouter_model),
        ExternalCliModelStyle::ProviderFlagAndModel
        | ExternalCliModelStyle::BareOpenRouterModel => openrouter_model,
    })
}

fn args_have_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| {
        flags.iter().any(|flag| {
            arg == flag
                || arg
                    .strip_prefix(flag)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

fn append_external_cli_model_args(
    cmd_parts: &mut Vec<String>,
    existing_args: &[String],
    model_args: ExternalCliModelArgs,
) {
    if let Some((flag, value)) = model_args.provider
        && !args_have_flag(existing_args, &["--provider", "-P"])
    {
        cmd_parts.push(flag.to_string());
        cmd_parts.push(shell_escape(&value));
    }
    if let Some((flag, value)) = model_args.model
        && !args_have_flag(existing_args, &["--model", "-m"])
    {
        cmd_parts.push(flag.to_string());
        cmd_parts.push(shell_escape(&value));
    }
}

fn args_have_codex_config(existing_args: &[String], key: &str) -> bool {
    existing_args.iter().any(|arg| {
        arg.trim_matches(['\'', '"'])
            .split_once('=')
            .is_some_and(|(configured_key, _)| configured_key.trim() == key)
    })
}

fn append_external_cli_reasoning_args(
    cmd_parts: &mut Vec<String>,
    existing_args: &[String],
    executor_type: &str,
    reasoning: Option<ReasoningLevel>,
) {
    let Some(level) = reasoning else {
        return;
    };
    match executor_type {
        "pi" if !args_have_flag(existing_args, &["--thinking"]) => {
            // Pi owns the WG vocabulary and accepts it verbatim.
            cmd_parts.push("--thinking".to_string());
            cmd_parts.push(shell_escape(level.as_str()));
        }
        "codex" if !args_have_codex_config(existing_args, "model_reasoning_effort") => {
            // Codex reasoning effort is a config override. Keep it independent
            // from the separately configured `model_verbosity` setting.
            cmd_parts.push("-c".to_string());
            cmd_parts.push(shell_escape(&format!(
                "model_reasoning_effort=\"{}\"",
                level.as_codex_effort()
            )));
        }
        _ => {}
    }
}

fn write_executor_prompt_file(
    output_dir: &Path,
    settings: &worksgood::service::executor::ExecutorSettings,
) -> Result<std::path::PathBuf> {
    let prompt_content = settings
        .prompt_template
        .as_ref()
        .map(|pt| pt.template.clone())
        .unwrap_or_default();
    let prompt_file = output_dir.join("prompt.txt");
    fs::write(&prompt_file, &prompt_content)
        .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
    Ok(prompt_file)
}

fn external_prompt_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    resolved_reasoning: Option<ReasoningLevel>,
    delivery: ExternalPromptDelivery,
) -> Result<String> {
    // Explicit-model contract: external CLIs that take a `--model` flag MUST
    // receive an explicitly resolved model. Running them with no model means
    // silently inheriting the CLI's own internal default — exactly the
    // "ran on the wrong model" class of bug WG forbids. If model resolution
    // yielded nothing AND the user hasn't hard-coded a model in
    // `[executor].args`, fail loudly instead of falling back. (Amplifier has
    // no model style and is exempt — it manages its own model.)
    if external_cli_model_style(&settings.executor_type).is_some()
        && effective_model.is_none()
        && !args_have_flag(&settings.args, &["--model", "-m"])
    {
        anyhow::bail!(
            "executor '{}' requires an explicitly resolved model, but model resolution \
             produced none. Set a model on the task (`-m {}:openrouter/<vendor>/<model>`), \
             the active profile, or `[agent].model` — WG will not fall back to the CLI's \
             internal default.",
            settings.executor_type,
            settings.executor_type,
        );
    }
    let prompt_file = write_executor_prompt_file(output_dir, settings)?;
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    for arg in &settings.args {
        cmd_parts.push(shell_escape(arg));
    }
    append_external_cli_model_args(
        &mut cmd_parts,
        &settings.args,
        external_cli_model_args(
            &settings.executor_type,
            effective_model.as_deref(),
            effective_provider.as_deref(),
        ),
    );
    append_external_cli_reasoning_args(
        &mut cmd_parts,
        &settings.args,
        &settings.executor_type,
        resolved_reasoning,
    );

    match delivery {
        ExternalPromptDelivery::OpenCodeFile => {
            cmd_parts.push(shell_escape("Complete the attached WG task prompt."));
            if !args_have_flag(&settings.args, &["--file", "-f", "--attach"]) {
                cmd_parts.push("--file".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::AiderMessageFile => {
            if !args_have_flag(&settings.args, &["--message-file", "--message"]) {
                cmd_parts.push("--message-file".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::GooseInputFile => {
            if !args_have_flag(
                &settings.args,
                &["-i", "--input", "--input-file", "-t", "--text"],
            ) {
                cmd_parts.push("-i".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::QwenPromptAndStdin => {
            if !args_have_flag(&settings.args, &["--prompt", "-p"]) {
                cmd_parts.push("--prompt".to_string());
                cmd_parts.push(shell_escape(
                    "Complete the WG task prompt supplied on stdin.",
                ));
            }
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::ClinePositionalPromptAndStdin => {
            cmd_parts.push(shell_escape(
                "Complete the WG task prompt supplied on stdin.",
            ));
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::Stdin => {
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::Argument => Ok(prompt_file_as_last_argument_command(
            &prompt_file.to_string_lossy(),
            &cmd_parts,
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalPromptDelivery {
    OpenCodeFile,
    AiderMessageFile,
    GooseInputFile,
    QwenPromptAndStdin,
    ClinePositionalPromptAndStdin,
    Stdin,
    Argument,
}

fn prompt_file_as_last_argument_command(prompt_file: &str, cmd_parts: &[String]) -> String {
    let mut parts = vec![
        "bash".to_string(),
        "-c".to_string(),
        shell_escape(r#"PROMPT=$(cat "$1"); shift; exec "$@" "$PROMPT""#),
        "--".to_string(),
        shell_escape(prompt_file),
    ];
    parts.extend(cmd_parts.iter().cloned());
    parts.join(" ")
}

fn preflight_executor_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    executor_name: &str,
    working_dir: Option<&Path>,
) -> Result<()> {
    let command = settings.command.trim();
    if command.is_empty() {
        anyhow::bail!(
            "Executor '{}' has an empty command in .wg/executors/{}.toml. \
             Set [executor].command to an installed binary and put flags in [executor].args.",
            executor_name,
            executor_name,
        );
    }

    if command_contains_path_separator(command) {
        let command_path = Path::new(command);
        let candidate = if command_path.is_absolute() {
            command_path.to_path_buf()
        } else if let Some(wd) = working_dir {
            wd.join(command_path)
        } else {
            command_path.to_path_buf()
        };
        if is_executable_file(&candidate) {
            return Ok(());
        }
        anyhow::bail!(
            "Executor '{}' command '{}' is not an executable file at '{}'. \
             Check .wg/executors/{}.toml, install the binary, or set an absolute command path.{}",
            executor_name,
            command,
            candidate.display(),
            executor_name,
            executor_setup_hint(executor_name),
        );
    }

    if which_on_path(command).is_some() {
        return Ok(());
    }

    anyhow::bail!(
        "Executor '{}' command '{}' was not found on PATH. \
         Install the '{}' binary, put it on PATH, or set [executor].command in .wg/executors/{}.toml.{}",
        executor_name,
        command,
        command,
        executor_name,
        executor_setup_hint(executor_name),
    );
}

fn executor_setup_hint(executor_name: &str) -> &'static str {
    match executor_name {
        "amplifier" => {
            " Expected default command: amplifier run --mode single --output-format json --bundle wg <prompt>."
        }
        "crush" => {
            " The built-in Crush surface is experimental; verify `crush run --help` for your installed version or override the template."
        }
        _ => "",
    }
}

fn command_contains_path_separator(command: &str) -> bool {
    command.contains('/') || command.contains('\\')
}

fn which_on_path(command: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn inject_api_key_env(
    cmd: &mut Command,
    endpoint_config: Option<&EndpointConfig>,
    effective_api_key: &Option<String>,
) {
    if let Some(key) = effective_api_key {
        cmd.env("WG_API_KEY", key);
        // Also set the provider-specific env var (e.g. OPENROUTER_API_KEY)
        // so external CLIs can discover the key through their standard
        // environment without putting secrets in argv or run.sh.
        if let Some(ep) = endpoint_config {
            for var_name in EndpointConfig::env_var_names_for_provider(&ep.provider) {
                cmd.env(var_name, key);
            }
        }
    }
}

/// Build the inner command string for the executor.
///
/// Returns `(primary_command, Option<fallback_command>)`. The fallback is
/// provided when the primary attempts session resume — if the session no
/// longer exists, the wrapper can fall back to a fresh session.
#[allow(clippy::too_many_arguments)]
fn build_inner_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    effective_endpoint: &Option<String>,
    effective_endpoint_url: &Option<String>,
    effective_api_key: &Option<String>,
    vars: &TemplateVars,
    task_exec: &Option<String>,
    resume_session_id: Option<&str>,
) -> Result<(String, Option<String>)> {
    build_inner_command_with_reasoning(
        settings,
        exec_mode,
        output_dir,
        effective_model,
        effective_provider,
        None,
        effective_endpoint,
        effective_endpoint_url,
        effective_api_key,
        vars,
        task_exec,
        resume_session_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_inner_command_with_reasoning(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    resolved_reasoning: Option<ReasoningLevel>,
    effective_endpoint: &Option<String>,
    effective_endpoint_url: &Option<String>,
    effective_api_key: &Option<String>,
    vars: &TemplateVars,
    task_exec: &Option<String>,
    resume_session_id: Option<&str>,
) -> Result<(String, Option<String>)> {
    let inner_command = match settings.executor_type.as_str() {
        "claude" if resume_session_id.is_some() && exec_mode != "bare" => {
            // Resume mode: use --resume <session_id> with checkpoint as follow-up message
            let session_id = resume_session_id.unwrap();
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--resume".to_string());
            cmd_parts.push(shell_escape(session_id));
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            // Write the resume context (checkpoint) as the follow-up message
            let resume_msg = vars.task_context.clone();
            let resume_file = output_dir.join("resume_message.txt");
            fs::write(&resume_file, &resume_msg)
                .with_context(|| format!("Failed to write resume message: {:?}", resume_file))?;
            let resume_command = prompt_file_command(&resume_file.to_string_lossy(), &claude_cmd);

            // Build a fresh-session fallback command (same as the full-mode
            // "claude" arm below) so the wrapper can retry if the session is
            // gone. Write prompt.txt alongside resume_message.txt.
            let fallback =
                build_claude_fresh_command(settings, exec_mode, output_dir, effective_model, vars)?;

            return Ok((resume_command, Some(fallback)));
        }
        "claude" if exec_mode == "bare" => {
            // Bare mode: lightweight execution with --system-prompt and no tools.
            // Used for pure-reasoning tasks (synthesis, triage, summarization).
            // The prompt is passed via --system-prompt and stdin provides the task input.
            let prompt_file = output_dir.join("prompt.txt");
            let prompt_content = settings
                .prompt_template
                .as_ref()
                .map(|pt| pt.template.clone())
                .unwrap_or_default();
            fs::write(&prompt_file, &prompt_content)
                .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;

            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--tools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*)"));
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*)"));
            cmd_parts.push("--disable-slash-commands".to_string());
            cmd_parts.push("--system-prompt".to_string());
            cmd_parts.push(shell_escape(&prompt_content));
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            // In bare mode, pipe the task title+description as the user message
            let user_message = format!(
                "Complete this task:\n\nTitle: {}\n\n{}",
                vars.task_id, vars.task_description
            );
            let user_msg_file = output_dir.join("user_message.txt");
            fs::write(&user_msg_file, &user_message).with_context(|| {
                format!("Failed to write user message file: {:?}", user_msg_file)
            })?;
            prompt_file_command(&user_msg_file.to_string_lossy(), &claude_cmd)
        }
        "claude" if exec_mode == "light" => {
            // Light mode: read-only file access + wg CLI tools.
            // Used for research, code review, exploration, analysis tasks.
            // Standard prompt-via-stdin flow with --allowedTools restriction.
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch"));
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape(
                "Edit,Write,NotebookEdit,Agent,EnterWorktree,ExitWorktree",
            ));

            cmd_parts.push("--disable-slash-commands".to_string());
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                // Write prompt to file for safe passing
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
            } else {
                claude_cmd
            }
        }
        "claude" => {
            // Full mode: standard Claude Code session with all tools
            // Write prompt to file and pipe to claude - avoids all quoting issues
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            // Prevent agents from spawning sub-agents outside WG
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));

            cmd_parts.push("--disable-slash-commands".to_string());
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                // Write prompt to file for safe passing
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
            } else {
                claude_cmd
            }
        }
        "codex" => {
            // Codex runs non-interactively via `codex exec`, reading the prompt from stdin.
            // We keep this aligned with the Claude single-shot flow: write the assembled
            // prompt to disk, pipe it in, and let the wrapper capture JSONL output.
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            append_external_cli_reasoning_args(
                &mut cmd_parts,
                &settings.args,
                "codex",
                resolved_reasoning,
            );
            let codex_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &codex_cmd)
            } else {
                codex_cmd
            }
        }
        "native" => {
            // Native executor: runs the agent loop in-process via `wg native-exec`.
            // Prompt is written to a file and passed as an argument. The bundle is
            // resolved from exec_mode by the native-exec subcommand.
            let prompt_content = settings
                .prompt_template
                .as_ref()
                .map(|pt| pt.template.clone())
                .unwrap_or_default();
            let prompt_file = output_dir.join("prompt.txt");
            fs::write(&prompt_file, &prompt_content)
                .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;

            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("native-exec".to_string());
            cmd_parts.push("--prompt-file".to_string());
            cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            cmd_parts.push("--exec-mode".to_string());
            cmd_parts.push(shell_escape(exec_mode));
            cmd_parts.push("--task-id".to_string());
            cmd_parts.push(shell_escape(&vars.task_id));
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            if let Some(p) = effective_provider {
                cmd_parts.push("--provider".to_string());
                cmd_parts.push(shell_escape(p));
            }
            if let Some(ep) = effective_endpoint {
                cmd_parts.push("--endpoint-name".to_string());
                cmd_parts.push(shell_escape(ep));
            }
            if let Some(url) = effective_endpoint_url {
                cmd_parts.push("--endpoint-url".to_string());
                cmd_parts.push(shell_escape(url));
            }
            if let Some(key) = effective_api_key {
                cmd_parts.push("--api-key".to_string());
                cmd_parts.push(shell_escape(key));
            }
            cmd_parts.join(" ")
        }
        "opencode" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::OpenCodeFile,
        )?,
        "aider" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::AiderMessageFile,
        )?,
        "goose" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::GooseInputFile,
        )?,
        "qwen" | "qwen-code" | "qwen_code" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::QwenPromptAndStdin,
        )?,
        "cline" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::ClinePositionalPromptAndStdin,
        )?,
        "crush" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::Stdin,
        )?,
        "pi" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::QwenPromptAndStdin,
        )?,
        "amplifier" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::Argument,
        )?,
        "shell" => {
            format!(
                "{} -c {}",
                shell_escape(&settings.command),
                shell_escape(task_exec.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("shell executor requires task exec command")
                })?)
            )
        }
        _ => {
            let mut parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                parts.push(shell_escape(arg));
            }
            parts.join(" ")
        }
    };
    Ok((inner_command, None))
}

/// Build the fresh (non-resume) claude command for fallback.
/// Mirrors the `"claude"` full-mode arm in `build_inner_command`.
fn build_claude_fresh_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    _vars: &TemplateVars,
) -> Result<String> {
    match exec_mode {
        "light" => {
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch"));
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape(
                "Edit,Write,NotebookEdit,Agent,EnterWorktree,ExitWorktree",
            ));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");
            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                Ok(prompt_file_command(
                    &prompt_file.to_string_lossy(),
                    &claude_cmd,
                ))
            } else {
                Ok(claude_cmd)
            }
        }
        _ => {
            // Full mode
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");
            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                Ok(prompt_file_command(
                    &prompt_file.to_string_lossy(),
                    &claude_cmd,
                ))
            } else {
                Ok(claude_cmd)
            }
        }
    }
}

/// Create and write the wrapper shell script that runs the agent command
/// and handles completion/failure.
///
/// When `fallback_command` is provided (session resume mode), the wrapper
/// detects "No conversation found" errors and retries with a fresh session.
fn write_wrapper_script(
    output_dir: &Path,
    task_id: &str,
    output_file_str: &str,
    timed_command: &str,
    effective_timeout_secs: Option<u64>,
    executor_type: &str,
    fallback_command: Option<&str>,
) -> Result<std::path::PathBuf> {
    let complete_cmd = "wg done \"$TASK_ID\" 2>> \"$OUTPUT_FILE\" || echo \"[wrapper] WARNING: 'wg done' failed with exit code $?\" >> \"$OUTPUT_FILE\"".to_string();
    let complete_msg = "[wrapper] Agent exited successfully, marking task done";

    let timeout_note = if let Some(secs) = effective_timeout_secs {
        format!(
            r#"
# Hard timeout: {secs}s (SIGTERM, then SIGKILL after 30s).
# Resolve a GNU `timeout` binary. macOS ships no GNU timeout — coreutils
# provides it as `gtimeout`. Linux distros usually have `timeout` directly.
# When neither is available we warn and the ${{WG_TIMEOUT_BIN:+...}} expansion
# in the run command collapses to nothing, so the executor still runs (just
# without a hard timeout). Without this probe a missing `timeout` would
# silently break the pipeline and pipe empty stdin into the executor.
WG_TIMEOUT_BIN="$(command -v gtimeout 2>/dev/null || command -v timeout 2>/dev/null || true)"
if [ -z "$WG_TIMEOUT_BIN" ]; then
    echo "[wrapper] WARNING: GNU timeout not found (install coreutils on macOS: 'brew install coreutils'); running without hard timeout" >> "$OUTPUT_FILE"
fi
"#,
            secs = secs
        )
    } else {
        String::new()
    };

    // Pass debug environment variables to spawned subprocesses
    let debug_env_vars = if std::env::var("WG_DEBUG_PROMPTS").is_ok() {
        "export WG_DEBUG_PROMPTS=1\n".to_string()
    } else {
        String::new()
    };

    let stream_file = output_dir.join("stream.jsonl");
    let stream_file_str = stream_file.to_string_lossy().to_string();

    // For Claude executor: split stdout (JSONL) to raw_stream.jsonl, stderr to output.log.
    // Also tee stdout to output.log for backward compatibility.
    // For native: the agent loop writes stream.jsonl directly; wrapper just adds bookends.
    // For shell/other: wrapper emits Init+Result bookend events.
    let (run_command, fallback_run_command, stream_init, stream_result) = match executor_type {
        "claude" | "codex" => {
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            let raw_str = raw_stream_file.to_string_lossy().to_string();
            // Capture JSONL stdout to raw_stream.jsonl and also copy to output.log.
            // stderr goes to output.log only. `tee -a <path>` opens the file
            // itself and fails on `\\?\C:\...` verbatim paths, so sanitize.
            let cmd = format!(
                "{timed_command} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                timed_command = timed_command,
                raw = shell_escape(&sanitize_bash_path(&raw_str)),
            );
            let fb_cmd = fallback_command.map(|fb| {
                format!(
                    "{fb} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                    fb = fb,
                    raw = shell_escape(&raw_str),
                )
            });
            (cmd, fb_cmd, String::new(), String::new())
        }
        "native" => {
            // Native executor writes stream.jsonl itself; wrapper just runs the command.
            let cmd = format!(
                "{timed_command} >> \"$OUTPUT_FILE\" 2>&1",
                timed_command = timed_command,
            );
            (cmd, None, String::new(), String::new())
        }
        "pi" => {
            // Pi (`pi --mode json`) emits NDJSON on stdout. Capture it to
            // raw_stream.jsonl (so the TUI events pane can render per-step
            // events live, like claude/codex) and tee to output.log; stderr
            // -> output.log. After pi exits, `wg pi-stream-bridge` reads that
            // NDJSON and writes the canonical stream.jsonl with REAL summed
            // token/cost usage (replacing the old 0/0 bookend) + a session
            // summary. No bash-emitted bookend — the bridge owns stream.jsonl.
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            let raw_str = raw_stream_file.to_string_lossy().to_string();
            let cmd = format!(
                "{timed_command} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                timed_command = timed_command,
                raw = shell_escape(&raw_str),
            );
            let bridge = format!(
                "wg pi-stream-bridge --agent-dir {dir} --exit-code $EXIT_CODE 2>> \"$OUTPUT_FILE\" \
                 || echo \"[wrapper] WARNING: 'wg pi-stream-bridge' failed with exit code $?\" >> \"$OUTPUT_FILE\"",
                dir = shell_escape(&output_dir.to_string_lossy()),
            );
            (cmd, None, String::new(), bridge)
        }
        _ => {
            // Shell and custom executors: wrapper writes bookend events.
            let cmd = format!(
                "{timed_command} >> \"$OUTPUT_FILE\" 2>&1",
                timed_command = timed_command,
            );
            let ts_cmd = "date +%s%3N"; // milliseconds since epoch
            let init = format!(
                "echo '{{\"type\":\"init\",\"executor_type\":\"{etype}\",\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                etype = executor_type,
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_ok = format!(
                "echo '{{\"type\":\"result\",\"success\":true,\"usage\":{{\"input_tokens\":0,\"output_tokens\":0}},\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_fail = format!(
                "echo '{{\"type\":\"result\",\"success\":false,\"usage\":{{\"input_tokens\":0,\"output_tokens\":0}},\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_block = format!(
                "if [ $EXIT_CODE -eq 0 ]; then\n    {result_ok}\nelse\n    {result_fail}\nfi",
                result_ok = result_ok,
                result_fail = result_fail,
            );
            (cmd, None, init, result_block)
        }
    };

    // Raw stream path for the failure classifier. claude/codex/pi write this file.
    let raw_stream_shell_var = match executor_type {
        "claude" | "codex" | "pi" => {
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            format!(
                "RAW_STREAM={}",
                shell_escape(&raw_stream_file.to_string_lossy())
            )
        }
        _ => "RAW_STREAM=".to_string(),
    };

    // Session resume fallback block: when the primary command tried to resume
    // a stale session (e.g., claude --resume <uuid>), detect the error and
    // retry with a fresh session.
    let session_fallback_block = if let Some(ref fb_cmd) = fallback_run_command {
        format!(
            r#"
# Session resume fallback: if the session no longer exists, start fresh
if [ $EXIT_CODE -ne 0 ]; then
    if grep -qE "No conversation found|session.*not found|invalid session|Could not resume" "$OUTPUT_FILE" 2>/dev/null; then
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Session not resumable, starting fresh session" >> "$OUTPUT_FILE"
        wg log "$TASK_ID" "session not resumable, falling back to fresh session" 2>/dev/null || true
        # The heartbeat guard writer belongs only to this wrapper. Close it
        # around the fallback executor exactly as for the primary executor, so
        # wrapper death produces immediate EOF even while the fallback lives.
        {{
            {fallback_run_command}
        }} {{HEARTBEAT_GUARD_FD}}>&-
        EXIT_CODE=$?
    fi
fi
"#,
            fallback_run_command = fb_cmd,
        )
    } else {
        String::new()
    };

    let wrapper_script = format!(
        r#"#!/bin/bash
TASK_ID={escaped_task_id}
OUTPUT_FILE={escaped_output_file}
{raw_stream_shell_var}

# Allow nested Claude Code sessions (spawned agents are independent).
# The MANAGED_BY_HOST / SDK_HAS_OAUTH_REFRESH vars in particular leak
# through when the daemon was launched from inside a Claude Code
# session and make the spawned claude CLI prefer an inaccessible host
# bridge over the configured token.
unset CLAUDECODE
unset CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST
unset CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH
{timeout_note}
{debug_env_vars}
{stream_init}
# Guarded heartbeat watcher — keeps registry heartbeat fresh while this wrapper
# owns the anonymous pipe's write descriptor. The executor runs with that
# descriptor closed, so even an untrappable wrapper death produces immediate
# EOF and the watcher exits instead of orphaning a `sleep 120` subprocess.
exec {{HEARTBEAT_GUARD_FD}}> >(wg heartbeat-watch "$WG_AGENT_ID" --supervised-pid "$$" 2>/dev/null)
HEARTBEAT_PID=$!

# Run the agent command without inheriting the heartbeat guard writer.
{{
    {run_command}
}} {{HEARTBEAT_GUARD_FD}}>&-
EXIT_CODE=$?
{session_fallback_block}
# Stop the heartbeat watcher and close its guard on normal completion.
exec {{HEARTBEAT_GUARD_FD}}>&-
kill $HEARTBEAT_PID 2>/dev/null; wait $HEARTBEAT_PID 2>/dev/null
{stream_result}

# Check if task is still in progress (agent didn't mark it done/failed)
TASK_STATUS=$(wg show "$TASK_ID" --json 2>/dev/null | grep -o '"status": *"[^"]*"' | head -1 | sed 's/.*"status": *"//;s/"//' || echo "unknown")

if [ "$TASK_STATUS" = "in-progress" ]; then
    if [ $EXIT_CODE -eq 124 ]; then
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Agent killed by hard timeout, marking task failed" >> "$OUTPUT_FILE"
        FAIL_CLASS=$(wg classify-failure --exit-code $EXIT_CODE 2>/dev/null || echo "agent-hard-timeout")
        wg fail "$TASK_ID" --class "$FAIL_CLASS" --reason "Agent exceeded hard timeout" 2>> "$OUTPUT_FILE" || echo "[wrapper] WARNING: 'wg fail' failed with exit code $?" >> "$OUTPUT_FILE"
    elif [ $EXIT_CODE -eq 0 ]; then
        echo "" >> "$OUTPUT_FILE"
        # Safety net: check for unread messages the agent may have missed
        UNREAD=$(wg msg read "$TASK_ID" --agent "$WG_AGENT_ID" 2>/dev/null)
        if [ -n "$UNREAD" ] && ! echo "$UNREAD" | grep -q "No unread messages"; then
            echo "[wrapper] WARNING: Agent finished with unread messages:" >> "$OUTPUT_FILE"
            echo "$UNREAD" >> "$OUTPUT_FILE"
        fi

        # Minimum-work gate: refuse to auto-mark done with no evidence of real work.
        # Catches models that exit 0 with a prose summary and no tool use (e.g. gpt-5.x lazy-completion).
        WG_GIT_DIR="$WG_WORKTREE_PATH"
        if [ -z "$WG_GIT_DIR" ]; then WG_GIT_DIR="."; fi
        LOG_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"event"' || echo 0)
        ARTIFACT_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"artifact"' || echo 0)
        DIFF_BYTES=$(git -C "$WG_GIT_DIR" diff HEAD --stat 2>/dev/null | wc -c || echo 0)
        COMMITS_AHEAD=$(git -C "$WG_GIT_DIR" rev-list --count HEAD ^origin/HEAD 2>/dev/null || echo 0)

        if [ "$LOG_COUNT" -lt 1 ] && [ "$ARTIFACT_COUNT" -lt 1 ] && [ "$DIFF_BYTES" -lt 50 ] && [ "$COMMITS_AHEAD" -lt 1 ]; then
            echo "[wrapper] FAIL-GATE: agent exited 0 with no logs, no artifacts, no diff, no commits — refusing to auto-mark done" >> "$OUTPUT_FILE"
            wg fail "$TASK_ID" --class "agent-no-work" --reason "Agent exited 0 without producing any work (no wg log, no artifacts, no diff, no commits)" 2>> "$OUTPUT_FILE" || true
        elif [ "$ARTIFACT_COUNT" -lt 1 ] && [ "$DIFF_BYTES" -lt 50 ] && [ "$COMMITS_AHEAD" -lt 1 ]; then
            # Guardrail G4 (NoOperationalOutput): the agent "talked but
            # didn't act" — it wrote logs/prose (LOG_COUNT >= 1) but produced
            # no artifacts and no file writes. Ask `wg classify-no-op` for the
            # verdict (single source of truth: the Rust pure function
            # `classify_no_operational_output` in raw_stream_classifier.rs,
            # which also scans output.log for mutation tokens). On a positive
            # verdict, fail with the machine-readable class so the retry path
            # (G3) injects the no-op directive instead of repeating
            # meta/observation work.
            NO_OP_CLASS=$(wg classify-no-op --output-log "$OUTPUT_FILE" --clean-exit --artifacts-empty 2>/dev/null || echo none)
            if [ "$NO_OP_CLASS" = "no-operational-output" ]; then
                echo "[wrapper] FAIL-GATE: agent exited 0 with prose/logs but no artifacts and no file writes (no-operational-output) — refusing to auto-mark done" >> "$OUTPUT_FILE"
                wg fail "$TASK_ID" --class "no-operational-output" --reason "Agent produced observation/summary work only (no artifacts, no file writes, non-empty output.log) — perform the concrete operational actions" 2>> "$OUTPUT_FILE" || true
            else
                echo "{complete_msg}" >> "$OUTPUT_FILE"
                {complete_cmd}
            fi
        else
            echo "{complete_msg}" >> "$OUTPUT_FILE"
            {complete_cmd}
        fi
    else
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Agent exited with code $EXIT_CODE, marking task failed" >> "$OUTPUT_FILE"
        if tail -c 65536 "$OUTPUT_FILE" 2>/dev/null | grep -Eiq 'no space left on device|os error 28|ENOSPC|disk quota exceeded'; then
            FAIL_CLASS="resource-exhausted-disk"
            FAIL_REASON="Disk resource exhausted during agent execution (exit code $EXIT_CODE); source preserved for retry-in-place"
        else
            FAIL_CLASS=$(wg classify-failure --raw-stream "$RAW_STREAM" --exit-code $EXIT_CODE 2>/dev/null || echo "agent-exit-nonzero")
            FAIL_REASON="Agent exited with code $EXIT_CODE"
        fi
        wg fail "$TASK_ID" --class "$FAIL_CLASS" --reason "$FAIL_REASON" 2>> "$OUTPUT_FILE" || echo "[wrapper] WARNING: 'wg fail' failed with exit code $?" >> "$OUTPUT_FILE"
    fi
fi

# --- Worktree Cleanup (merge-back is handled by wg done) ---
# The merge-back squash is now performed inline by `wg done` while the agent is
# still alive, so it can react to conflicts. This wrapper only handles the
# cleanup marker for the explicit worktree cleanup surface.
if [ -n "$WG_WORKTREE_PATH" ] && [ -n "$WG_BRANCH" ] && [ -n "$WG_PROJECT_ROOT" ]; then
    CURRENT_DIR_REAL=$(pwd -P 2>/dev/null || pwd)
    WORKTREE_PATH_REAL=$(cd "$WG_WORKTREE_PATH" 2>/dev/null && pwd -P || printf '%s' "$WG_WORKTREE_PATH")
    if [ "$CURRENT_DIR_REAL" != "$WORKTREE_PATH_REAL" ]; then
        echo "[wrapper] WARNING: Skipping worktree cleanup because current directory '$CURRENT_DIR_REAL' does not match WG_WORKTREE_PATH '$WORKTREE_PATH_REAL' — possible inherited parent agent environment" >> "$OUTPUT_FILE"
    else
    if [ ! -e "$WG_WORKTREE_PATH/.git" ]; then
        echo "[wrapper] WARNING: Worktree .git pointer missing at $WG_WORKTREE_PATH — possible worktree escape detected" >> "$OUTPUT_FILE"
    fi

    TASK_STATUS_FINAL=$(wg show "$TASK_ID" --json 2>/dev/null | grep -o '"status": *"[^"]*"' | head -1 | sed 's/.*"status": *"//;s/"//' || echo "unknown")

    # Mark worktree for cleanup sweep (wg done already placed this marker on
    # the happy path, but the wrapper is a safety net for cases where the agent
    # crashed or was killed before reaching wg done).
    touch "$WG_WORKTREE_PATH/.wg-cleanup-pending" 2>/dev/null || true
    echo "[wrapper] Task finished with status '$TASK_STATUS_FINAL' — marked worktree $WG_WORKTREE_PATH for explicit cleanup (inspect/remove with: wg worktree archive $WG_AGENT_ID --remove)" >> "$OUTPUT_FILE"

    # Build caches are never removed here. The owned-cache sentinel waits for
    # terminal owner/task state, stale exact PID identity, lease expiry, a clean
    # worktree, no registered artifacts, and no open files.
    fi
fi

exit $EXIT_CODE
"#,
        escaped_task_id = shell_escape(task_id),
        // `$OUTPUT_FILE` is used throughout the wrapper as a `>>` redirect
        // target. Bash on Windows (Git-for-Windows) refuses `\\?\C:\...`
        // verbatim paths for redirects with "No such file or directory",
        // so output.log never appears and the agent looks silently dead.
        escaped_output_file = shell_escape(&sanitize_bash_path(output_file_str)),
        raw_stream_shell_var = raw_stream_shell_var,
        run_command = run_command,
        session_fallback_block = session_fallback_block,
        timeout_note = timeout_note,
        debug_env_vars = debug_env_vars,
        stream_init = stream_init,
        stream_result = stream_result,
        complete_cmd = complete_cmd,
        complete_msg = complete_msg,
    );

    // Write wrapper script
    let wrapper_path = output_dir.join("run.sh");
    fs::write(&wrapper_path, &wrapper_script)
        .with_context(|| format!("Failed to write wrapper script: {:?}", wrapper_path))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))?;
    }

    Ok(wrapper_path)
}

/// A resolved model+provider pair from the unified resolution cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedModelProvider {
    /// The winning model string, potentially still containing a `provider:` prefix.
    /// Downstream `resolve_model_via_registry()` handles prefix stripping.
    pub model: Option<String>,
    /// The resolved provider, extracted from the highest-priority tier that
    /// supplies one — either an explicit provider field or a `provider:model` spec.
    pub provider: Option<String>,
}

/// A single tier in the model/provider resolution cascade.
/// Each tier may supply a model, a provider, or both. If the model string
/// contains a `provider:model` spec, the provider is extracted automatically.
struct ResolutionTier {
    model: Option<String>,
    provider: Option<String>,
}

impl ResolutionTier {
    fn new(model: Option<String>, provider: Option<String>) -> Self {
        // If there's an explicit provider, use it directly.
        // Otherwise, try to extract provider from the model spec.
        if provider.is_some() {
            return Self { model, provider };
        }
        if let Some(ref m) = model {
            let spec = worksgood::config::parse_model_spec(m);
            if let Some(ref p) = spec.provider {
                return Self {
                    model,
                    provider: Some(worksgood::config::provider_to_native_provider(p).to_string()),
                };
            }
        }
        Self { model, provider }
    }
}

/// Resolve model and provider from the unified precedence hierarchy.
///
/// Resolution tiers (highest to lowest priority):
///   1. Task-level (task.model, task.provider)
///   2. Agent preferences (agent.preferred_model, agent.preferred_provider)
///   3. Executor defaults (executor.model)
///   4. Role-based config (role_config.model, role_config.provider)
///   5. Coordinator defaults (coordinator.model, coordinator.provider)
///
/// At each tier, if the model string uses `provider:model` format, the provider
/// is extracted from it via `parse_model_spec()`. An explicit provider at the
/// same tier takes precedence over a provider embedded in the model spec.
///
/// Model and provider are resolved independently: the highest-priority tier
/// that supplies a model wins for model, and likewise for provider. This means
/// a task can set `model = "openrouter:deepseek-v3"` (setting both) or just
/// `provider = "openrouter"` (overriding only the provider).
///
/// The returned model string retains any `provider:` prefix so that downstream
/// `resolve_model_via_registry()` can handle prefix stripping per executor type.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_model_and_provider(
    task_model: Option<String>,
    task_provider: Option<String>,
    agent_preferred_model: Option<String>,
    agent_preferred_provider: Option<String>,
    executor_model: Option<String>,
    role_model: Option<String>,
    role_provider: Option<String>,
    coordinator_model: Option<&str>,
    coordinator_provider: Option<String>,
) -> ResolvedModelProvider {
    let tiers = [
        ResolutionTier::new(task_model, task_provider),
        ResolutionTier::new(agent_preferred_model, agent_preferred_provider),
        ResolutionTier::new(executor_model, None),
        ResolutionTier::new(role_model, role_provider),
        ResolutionTier::new(
            coordinator_model.map(|s| s.to_string()),
            coordinator_provider,
        ),
    ];

    let model = tiers.iter().find_map(|t| t.model.clone());
    let provider = tiers.iter().find_map(|t| t.provider.clone());

    ResolvedModelProvider { model, provider }
}

/// Built-in tier alias IDs that the Claude CLI understands natively.
const BUILTIN_TIER_ALIASES: &[&str] = &["haiku", "sonnet", "opus"];

/// Resolve a model string through the model registry.
///
/// If the model matches a registry entry:
/// - Built-in tier aliases (haiku/sonnet/opus) are kept as-is (Claude CLI understands them)
/// - Custom aliases are resolved to their full API model ID
/// - The entry's provider and endpoint are returned for downstream resolution
///
/// If the model is not in the registry:
/// - If the task explicitly specified it → error (user should register it first)
/// - Otherwise (from executor/coordinator defaults) → pass through unchanged
///
/// Returns `(effective_model, registry_provider, registry_endpoint)`.
fn resolve_spawn_model_via_registry(
    executor_name: &str,
    effective_model: Option<String>,
    task_model: Option<&String>,
    config: &Config,
    dir: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    if executor_name == "pi" {
        return Ok((effective_model, None, None));
    }
    resolve_model_via_registry(effective_model, task_model, config, dir)
}

fn resolve_model_via_registry(
    effective_model: Option<String>,
    task_model: Option<&String>,
    config: &Config,
    dir: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let model_str = match effective_model {
        Some(ref s) => s.clone(),
        None => return Ok((None, None, None)),
    };

    // Parse unified provider:model spec. If the model has an explicit provider
    // prefix (e.g. "openrouter:deepseek/deepseek-v3.2"), extract it and use
    // the model ID for registry lookup.
    let spec = worksgood::config::parse_model_spec(&model_str);
    if let Some(ref provider_prefix) = spec.provider {
        let native_provider =
            Some(worksgood::config::provider_to_native_provider(provider_prefix).to_string());
        // Try registry lookup on the bare model part for endpoint resolution
        let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
        let endpoint = merged
            .registry_lookup(&spec.model_id)
            .or_else(|| {
                merged
                    .effective_registry()
                    .into_iter()
                    .find(|e| e.model == spec.model_id)
            })
            .and_then(|e| e.endpoint.clone());
        // CLI-backed executors do not understand provider:model format; pass only
        // the bare model ID. Native/API-backed executors preserve the full spec
        // so downstream provider resolution can re-parse the prefix. For the
        // claude CLI, expand friendly aliases with no CLI shortcut
        // (`claude:fable` → `claude-fable-5`); opus/sonnet/haiku pass through.
        let effective = match worksgood::config::provider_to_executor(provider_prefix) {
            "claude" => worksgood::config::claude_cli_model_arg(&spec.model_id),
            "codex" => spec.model_id.clone(),
            _ => model_str.clone(),
        };
        return Ok((Some(effective), native_provider, endpoint));
    }

    // No provider prefix — fall back to existing resolution logic.
    // Load merged config for registry lookup (includes global + local + builtins)
    let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());

    // Look up by short ID first, then by full model field (e.g., "deepseek/deepseek-chat"
    // matching a registry entry with model = "deepseek/deepseek-chat").
    let registry_entry = merged.registry_lookup(&model_str).or_else(|| {
        merged
            .effective_registry()
            .into_iter()
            .find(|e| e.model == model_str)
    });

    if let Some(entry) = registry_entry {
        // Found in registry
        let is_builtin = BUILTIN_TIER_ALIASES.contains(&model_str.as_str());
        let resolved_model = if is_builtin {
            // Keep tier alias as-is for backward compat with Claude CLI
            model_str
        } else {
            // Custom alias → use actual API model ID
            entry.model.clone()
        };
        Ok((
            Some(resolved_model),
            Some(entry.provider.clone()),
            entry.endpoint.clone(),
        ))
    } else if task_model.is_some() && task_model.map(|s| s.as_str()) == effective_model.as_deref() {
        // Task explicitly specified a model that's not in the registry.
        if model_str.contains('/') {
            // Full provider/model ID (e.g., "deepseek/deepseek-chat") — pass through.
            // The native executor's create_provider_ext() auto-detects the provider
            // from the slash in the model name.
            Ok((effective_model, None, None))
        } else {
            // Short alias that's not registered — try resolving against model cache.
            let resolution = worksgood::executor::native::openai_client::resolve_short_model_name(
                &model_str, dir,
            );
            if let Some(resolved_id) = resolution.resolved {
                eprintln!(
                    "[spawn] Resolved short model name '{}' → 'openrouter:{}'",
                    model_str, resolved_id
                );
                // Re-resolve with the full provider:model format
                let full_spec = format!("openrouter:{}", resolved_id);
                let spec = worksgood::config::parse_model_spec(&full_spec);
                let native_provider = Some(
                    worksgood::config::provider_to_native_provider(
                        spec.provider.as_deref().unwrap_or("openrouter"),
                    )
                    .to_string(),
                );
                let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
                let endpoint = merged
                    .registry_lookup(&spec.model_id)
                    .or_else(|| {
                        merged
                            .effective_registry()
                            .into_iter()
                            .find(|e| e.model == spec.model_id)
                    })
                    .and_then(|e| e.endpoint.clone());
                Ok((Some(full_spec), native_provider, endpoint))
            } else {
                // No resolution possible — error with suggestions.
                let suggestions = if resolution.suggestions.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n  Did you mean one of:\n{}",
                        resolution
                            .suggestions
                            .iter()
                            .map(|s| format!("    - openrouter:{}", s))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                anyhow::bail!(
                    "Model '{}' not found in config or model cache.{}\n  \
                     Try: `wg models search {}` to find valid alternatives\n  \
                     Or:  `wg models list` to see the local registry\n  \
                     Add: `wg model add {} --provider <provider> --model-id <model-id>` to register it\n  \
                     Tip: `openrouter/auto` is a safe default that auto-routes to the best model.",
                    model_str,
                    suggestions,
                    model_str,
                    model_str,
                );
            }
        }
    } else {
        // Model came from executor/coordinator defaults — pass through unchanged.
        // It may be a direct model ID the executor understands.
        Ok((effective_model, None, None))
    }
}

/// Check OpenRouter cost caps before spawning an agent
fn check_openrouter_cost_caps(
    config: &Config,
    workgraph_dir: &Path,
    task_id: &str,
    model: Option<&str>,
) -> Result<()> {
    use crate::commands::service::CoordinatorState;
    use worksgood::executor::native::openai_client::{
        fetch_openrouter_key_status_blocking, resolve_openai_api_key_from_dir,
    };

    // No [openrouter] section → no caps to enforce. The section is
    // emitted only on the openrouter route or when explicitly added.
    let Some(openrouter_config) = config.openrouter.as_ref() else {
        return Ok(());
    };

    // Early exit if no cost caps are configured
    if openrouter_config.cost_cap_global_usd.is_none()
        && openrouter_config.cost_cap_session_usd.is_none()
        && openrouter_config.cost_cap_task_usd.is_none()
    {
        return Ok(());
    }

    // Get OpenRouter API key for status checking
    let api_key = match resolve_openai_api_key_from_dir(workgraph_dir) {
        Ok(key) => key,
        Err(_) => {
            // If no API key available, we can't check costs, so allow the operation
            return Ok(());
        }
    };

    let service_dir = workgraph_dir.join(".wg/service");

    // Load current coordinator state for session cost tracking
    let mut coordinator_state = CoordinatorState::load_for(&service_dir, 0).unwrap_or_default();

    // Check if we should refresh key status
    if coordinator_state
        .cost_tracking
        .should_check_key_status(openrouter_config.key_status_check_interval_minutes)
    {
        match fetch_openrouter_key_status_blocking(&api_key, None) {
            Ok(key_status) => {
                coordinator_state
                    .cost_tracking
                    .update_key_status(key_status);
                // Save updated state
                coordinator_state.save_for(&service_dir, 0);
            }
            Err(e) => {
                // Log warning but don't block operation
                eprintln!("Warning: Failed to check OpenRouter key status: {}", e);
            }
        }
    }

    // Check session cost cap
    if let Some(session_cap) = openrouter_config.cost_cap_session_usd
        && coordinator_state.cost_tracking.session_cost_usd >= session_cap
    {
        return handle_cost_cap_violation(
            &openrouter_config.cap_behavior,
            &format!(
                "Session cost cap of ${:.2} exceeded (current: ${:.2})",
                session_cap, coordinator_state.cost_tracking.session_cost_usd
            ),
            openrouter_config.fallback_model.as_deref(),
            task_id,
            model,
        );
    }

    // Check global cost cap using key status if available
    if let (Some(global_cap), Some(key_status)) = (
        openrouter_config.cost_cap_global_usd,
        &coordinator_state.cost_tracking.key_status,
    ) && key_status.usage >= global_cap
    {
        return handle_cost_cap_violation(
            &openrouter_config.cap_behavior,
            &format!(
                "Global cost cap of ${:.2} exceeded (current: ${:.2})",
                global_cap, key_status.usage
            ),
            openrouter_config.fallback_model.as_deref(),
            task_id,
            model,
        );
    }

    // Check warning thresholds
    if let Some(key_status) = &coordinator_state.cost_tracking.key_status
        && key_status.is_above_threshold(openrouter_config.warn_at_usage_percent as f64)
    {
        eprintln!(
            "Warning: OpenRouter usage at {:.1}% of limit (${:.2}/${:.2})",
            key_status.usage_percentage(),
            key_status.usage,
            key_status.limit
        );
    }

    Ok(())
}

/// Handle cost cap violation according to the configured behavior
fn handle_cost_cap_violation(
    behavior: &CapBehavior,
    message: &str,
    fallback_model: Option<&str>,
    task_id: &str,
    _current_model: Option<&str>,
) -> Result<()> {
    match behavior {
        CapBehavior::Fail => {
            anyhow::bail!("Cost cap exceeded: {}", message);
        }
        CapBehavior::Fallback => {
            if let Some(fallback) = fallback_model {
                eprintln!(
                    "Warning: {} - would fallback to model '{}' for task '{}' (fallback not implemented yet)",
                    message, fallback, task_id
                );
                Ok(())
            } else {
                anyhow::bail!(
                    "Cost cap exceeded: {} (no fallback model configured)",
                    message
                );
            }
        }
        CapBehavior::Escalate => {
            eprintln!("Warning: {} - continuing due to escalate behavior", message);
            Ok(())
        }
        CapBehavior::Readonly => {
            eprintln!("Warning: {} - entering read-only mode", message);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::config::{CLAUDE_FABLE_MODEL_ID, CLAUDE_OPUS_MODEL_ID};

    // --- executor_uses_auto_prompt tests ---

    // Regression for codex-handler-doesn: the spawn pipeline used to hard-code
    // the auto-prompt list as `claude | native`, which silently dropped
    // codex. Codex agents spawned with no prompt_template, fell through to
    // the codex case in build_inner_command's `else { codex_cmd }` branch,
    // and the resulting run.sh had no `cat prompt.txt | ...` prefix. Codex
    // CLI sat reading stdin, got nothing, exited with 'No prompt provided
    // via stdin'. Pin the three built-in handlers here.
    #[test]
    fn test_executor_uses_auto_prompt_includes_codex() {
        assert!(executor_uses_auto_prompt("codex"));
    }

    #[test]
    fn test_executor_uses_auto_prompt_includes_all_builtins() {
        for kind in ["claude", "codex", "native"] {
            assert!(
                executor_uses_auto_prompt(kind),
                "{} must auto-build prompt",
                kind
            );
        }
    }

    #[test]
    fn test_executor_uses_auto_prompt_includes_external_cli_adapters() {
        for kind in worksgood::dispatch::ExecutorKind::EXTERNAL_CLIS
            .iter()
            .map(|kind| kind.as_str())
            .chain(["qwen-code", "qwen_code"])
        {
            assert!(
                executor_uses_auto_prompt(kind),
                "{} must auto-build prompt",
                kind
            );
        }
    }

    #[test]
    fn test_executor_uses_auto_prompt_excludes_shell_and_unknown() {
        assert!(!executor_uses_auto_prompt("shell"));
        assert!(!executor_uses_auto_prompt(""));
        assert!(!executor_uses_auto_prompt("custom"));
    }

    // --- external CLI model normalization tests ---

    fn normalized_args(executor_type: &str) -> Vec<String> {
        external_cli_model_args(
            executor_type,
            Some("openrouter:deepseek/deepseek-v3.2"),
            Some("openrouter"),
        )
        .to_vec()
    }

    #[test]
    fn test_external_cli_model_args_opencode_and_aider_use_provider_slash() {
        for executor_type in ["opencode", "aider"] {
            assert_eq!(
                normalized_args(executor_type),
                vec![
                    "--model".to_string(),
                    "openrouter/deepseek/deepseek-v3.2".to_string()
                ],
                "{} should receive OpenRouter provider/model slash syntax",
                executor_type
            );
        }
    }

    #[test]
    fn test_external_cli_model_args_goose_uses_provider_flag_and_bare_model() {
        assert_eq!(
            normalized_args("goose"),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_cline_uses_provider_flag_and_bare_model() {
        assert_eq!(
            normalized_args("cline"),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_octomind_uses_provider_colon_model() {
        // Octomind's `-m` takes WG's `openrouter:<vendor>/<model>` spelling, so
        // a worker spawn preserves the typed route rather than silently falling
        // back to octomind's default (prototype-octomind-dexto-chat).
        assert_eq!(
            normalized_args("octomind"),
            vec![
                "--model".to_string(),
                "openrouter:deepseek/deepseek-v3.2".to_string()
            ]
        );
        // minimax/minimax-m3 specifically (the task's named regression model).
        assert_eq!(
            external_cli_model_args("octomind", Some("minimax/minimax-m3"), Some("openrouter"))
                .to_vec(),
            vec![
                "--model".to_string(),
                "openrouter:minimax/minimax-m3".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_qwen_uses_bare_model() {
        assert_eq!(
            normalized_args("qwen"),
            vec!["--model".to_string(), "deepseek/deepseek-v3.2".to_string()]
        );
    }

    #[test]
    fn test_external_cli_model_args_crush_uses_provider_slash() {
        assert_eq!(
            normalized_args("crush"),
            vec![
                "--model".to_string(),
                "openrouter/deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_accept_provider_from_resolution() {
        assert_eq!(
            external_cli_model_args(
                "opencode",
                Some("deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .to_vec(),
            vec![
                "--model".to_string(),
                "openrouter/deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_do_not_duplicate_openrouter_prefix() {
        for executor_type in ["opencode", "aider"] {
            assert_eq!(
                external_cli_model_args(
                    executor_type,
                    Some("openrouter/deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                )
                .to_vec(),
                vec![
                    "--model".to_string(),
                    "openrouter/deepseek/deepseek-v3.2".to_string()
                ],
                "{} should accept already-normalized OpenRouter model syntax",
                executor_type
            );
        }
    }

    #[test]
    fn test_pi_external_cli_model_args_split_custom_provider_colon_model() {
        assert_eq!(
            external_cli_model_args("pi", Some("lunaroute:glm-5.2-nvfp4"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "lunaroute".to_string(),
                "--model".to_string(),
                "glm-5.2-nvfp4".to_string(),
            ],
            "Pi custom provider:model routes must be split for pi argv"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:lunaroute:glm-5.2-nvfp4"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "lunaroute".to_string(),
                "--model".to_string(),
                "glm-5.2-nvfp4".to_string(),
            ],
            "The Pi executor prefix should be accepted defensively too"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openai:gpt-4.1"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openai".to_string(),
                "--model".to_string(),
                "gpt-4.1".to_string(),
            ],
            "Pi provider names are Pi-owned and must not be mapped through WG native aliases"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openai-codex:gpt-5.6-sol"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openai-codex".to_string(),
                "--model".to_string(),
                "gpt-5.6-sol".to_string(),
            ],
            "Pi Codex routes must become --provider openai-codex --model gpt-5.6-sol"
        );
    }

    #[test]
    fn test_external_cli_reasoning_args_preserve_pi_and_adapt_codex() {
        let existing: Vec<String> = Vec::new();
        let mut pi_parts = Vec::new();
        append_external_cli_reasoning_args(
            &mut pi_parts,
            &existing,
            "pi",
            Some(ReasoningLevel::Xhigh),
        );
        assert_eq!(
            pi_parts,
            vec!["--thinking".to_string(), "'xhigh'".to_string()]
        );

        let mut max_parts = Vec::new();
        append_external_cli_reasoning_args(
            &mut max_parts,
            &existing,
            "pi",
            Some(ReasoningLevel::Max),
        );
        assert_eq!(
            max_parts,
            vec!["--thinking".to_string(), "'max'".to_string()]
        );

        let mut omitted = Vec::new();
        append_external_cli_reasoning_args(&mut omitted, &existing, "pi", None);
        assert!(omitted.is_empty());

        let mut codex = Vec::new();
        append_external_cli_reasoning_args(
            &mut codex,
            &existing,
            "codex",
            Some(ReasoningLevel::High),
        );
        assert_eq!(
            codex,
            vec![
                "-c".to_string(),
                "'model_reasoning_effort=\"high\"'".to_string(),
            ],
            "Codex must use its config override rather than Pi's --thinking flag"
        );

        let mut existing_flag = Vec::new();
        append_external_cli_reasoning_args(
            &mut existing_flag,
            &["--thinking".to_string(), "low".to_string()],
            "pi",
            Some(ReasoningLevel::High),
        );
        assert!(existing_flag.is_empty(), "explicit Pi executor args win");

        let mut existing_codex = Vec::new();
        append_external_cli_reasoning_args(
            &mut existing_codex,
            &[
                "-c".to_string(),
                "model_reasoning_effort=\"medium\"".to_string(),
                "-c".to_string(),
                "model_verbosity=\"high\"".to_string(),
            ],
            "codex",
            Some(ReasoningLevel::Xhigh),
        );
        assert!(
            existing_codex.is_empty(),
            "explicit Codex effort must win independently of verbosity"
        );
    }

    #[test]
    fn test_pi_external_cli_model_args_keep_openrouter_slash_model() {
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openrouter/test/model"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "test/model".to_string(),
            ],
            "Existing Pi OpenRouter provider/model spelling must keep working"
        );
    }

    #[test]
    fn test_external_cli_model_args_do_not_change_builtin_or_amplifier_defaults() {
        for executor_type in ["claude", "codex", "native", "amplifier"] {
            assert!(
                external_cli_model_args(
                    executor_type,
                    Some("openrouter:deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                )
                .to_vec()
                .is_empty(),
                "{} model behavior should stay on its existing path",
                executor_type
            );
            assert_eq!(
                model_template_value_for_executor(
                    executor_type,
                    Some("openrouter:deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                ),
                None
            );
        }
    }

    #[test]
    fn test_external_cli_model_template_value_matches_cli_style() {
        assert_eq!(
            model_template_value_for_executor(
                "opencode",
                Some("openrouter:deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .as_deref(),
            Some("openrouter/deepseek/deepseek-v3.2")
        );
        assert_eq!(
            model_template_value_for_executor(
                "goose",
                Some("openrouter:deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .as_deref(),
            Some("deepseek/deepseek-v3.2")
        );
    }

    // --- should_create_worktree tests ---

    #[test]
    fn test_worktree_gate_disabled_globally() {
        assert!(!should_create_worktree(false, "my-task", "full"));
        assert!(!should_create_worktree(false, "my-task", "shell"));
    }

    #[test]
    fn test_worktree_gate_full_and_shell_get_worktree() {
        assert!(should_create_worktree(true, "my-task", "full"));
        assert!(should_create_worktree(true, "my-task", "shell"));
    }

    #[test]
    fn test_worktree_gate_bare_and_light_skip() {
        assert!(!should_create_worktree(true, "my-task", "bare"));
        assert!(!should_create_worktree(true, "my-task", "light"));
    }

    #[test]
    fn test_worktree_gate_meta_tasks_skip_regardless_of_exec_mode() {
        // System/meta tasks never get worktrees — they only touch graph state.
        for prefix in [".assign-", ".flip-", ".evaluate-", ".place-", ".compact-"] {
            let task_id = format!("{}my-real-task", prefix);
            for exec_mode in ["full", "shell", "bare", "light"] {
                assert!(
                    !should_create_worktree(true, &task_id, exec_mode),
                    "meta task {} with exec_mode={} should not get worktree",
                    task_id,
                    exec_mode
                );
            }
        }
    }

    #[test]
    fn test_worktree_gate_unknown_exec_mode_defaults_to_worktree() {
        // Conservative default: unrecognized modes get a worktree (fail-safe for writes).
        assert!(should_create_worktree(true, "my-task", "future-mode-xyz"));
    }

    // --- resolve_model_and_provider tests ---

    /// Helper to call resolve_model_and_provider with all None defaults except specified args.
    fn resolve(
        task_model: Option<&str>,
        task_provider: Option<&str>,
        agent_model: Option<&str>,
        agent_provider: Option<&str>,
        executor_model: Option<&str>,
        role_model: Option<&str>,
        role_provider: Option<&str>,
        coordinator_model: Option<&str>,
        coordinator_provider: Option<&str>,
    ) -> ResolvedModelProvider {
        resolve_model_and_provider(
            task_model.map(|s| s.to_string()),
            task_provider.map(|s| s.to_string()),
            agent_model.map(|s| s.to_string()),
            agent_provider.map(|s| s.to_string()),
            executor_model.map(|s| s.to_string()),
            role_model.map(|s| s.to_string()),
            role_provider.map(|s| s.to_string()),
            coordinator_model,
            coordinator_provider.map(|s| s.to_string()),
        )
    }

    #[test]
    fn test_unified_model_task_overrides_agent() {
        let r = resolve(
            Some("task-model"),
            None,
            Some("agent-model"),
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("task-model".to_string()));
    }

    #[test]
    fn test_unified_model_agent_when_no_task() {
        let r = resolve(
            None,
            None,
            Some("agent-model"),
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("agent-model".to_string()));
    }

    #[test]
    fn test_unified_model_executor_when_no_agent() {
        let r = resolve(
            None,
            None,
            None,
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("executor-model".to_string()));
    }

    #[test]
    fn test_unified_model_role_when_no_executor() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            Some("role-model"),
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("role-model".to_string()));
    }

    #[test]
    fn test_unified_model_coordinator_fallback() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("coordinator-model".to_string()));
    }

    #[test]
    fn test_unified_none_when_all_empty() {
        let r = resolve(None, None, None, None, None, None, None, None, None);
        assert_eq!(r.model, None);
        assert_eq!(r.provider, None);
    }

    #[test]
    fn test_unified_provider_task_overrides_agent() {
        let r = resolve(
            None,
            Some("task-provider"),
            None,
            Some("agent-provider"),
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("task-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_agent_when_no_task() {
        let r = resolve(
            None,
            None,
            None,
            Some("agent-provider"),
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("agent-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_config_fallback() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("config-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_none_when_all_empty() {
        let r = resolve(None, None, None, None, None, None, None, None, None);
        assert_eq!(r.provider, None);
    }

    #[test]
    fn test_unified_provider_from_model_spec() {
        // provider:model in task.model extracts provider automatically
        let r = resolve(
            Some("openrouter:deepseek/deepseek-v3"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.model, Some("openrouter:deepseek/deepseek-v3".to_string()));
        assert_eq!(r.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_unified_explicit_provider_beats_model_spec() {
        // An explicit task_provider takes precedence over provider in model spec
        let r = resolve(
            Some("openrouter:deepseek/deepseek-v3"),
            Some("anthropic"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_unified_model_spec_at_agent_tier() {
        // provider:model at agent tier extracts provider when no task-level provider
        let r = resolve(
            None,
            None,
            Some("openai:gpt-5"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.model, Some("openai:gpt-5".to_string()));
        assert_eq!(r.provider, Some("oai-compat".to_string()));
    }

    #[test]
    fn test_unified_model_spec_at_coordinator_tier() {
        // provider:model at coordinator tier as last resort
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("claude:opus"),
            None,
        );
        assert_eq!(r.model, Some("claude:opus".to_string()));
        assert_eq!(r.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_unified_unassigned_task_uses_executor_model() {
        let r = resolve(
            None,
            None,
            None,
            None,
            Some("executor-default"),
            None,
            None,
            Some("coordinator-fallback"),
            None,
        );
        assert_eq!(r.model, Some("executor-default".to_string()));
    }

    #[test]
    fn test_unified_task_provider_overrides_lower_model_spec() {
        // Task has explicit provider, coordinator has provider:model
        // Task provider should win even though coordinator model has a spec
        let r = resolve(
            None,
            Some("anthropic"),
            None,
            None,
            None,
            None,
            None,
            Some("openrouter:deepseek/deepseek-v3"),
            None,
        );
        assert_eq!(r.provider, Some("anthropic".to_string()));
        assert_eq!(r.model, Some("openrouter:deepseek/deepseek-v3".to_string()));
    }

    /// Helper to build an EndpointsConfig for endpoint resolution tests.
    fn test_endpoints_config() -> worksgood::config::EndpointsConfig {
        worksgood::config::EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                worksgood::config::EndpointConfig {
                    name: "my-openrouter".to_string(),
                    provider: "openrouter".to_string(),
                    url: Some("https://openrouter.ai/api/v1".to_string()),
                    api_key: Some("sk-or-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    model: None,
                    api_key_ref: None,
                    is_default: true,
                    context_window: None,
                },
                worksgood::config::EndpointConfig {
                    name: "my-anthropic".to_string(),
                    provider: "anthropic".to_string(),
                    url: Some("https://api.anthropic.com".to_string()),
                    api_key: Some("sk-ant-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    model: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
            ],
        }
    }

    #[test]
    fn test_endpoint_resolution_task_endpoint_takes_priority() {
        let endpoints = test_endpoints_config();

        // task.endpoint is set — should win over everything
        let task_endpoint = Some("my-openrouter".to_string());
        let task_provider: Option<String> = Some("anthropic".to_string());
        let agent_provider: Option<String> = Some("anthropic".to_string());
        let role_endpoint: Option<String> = Some("my-anthropic".to_string());

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_task_provider_lookup() {
        let endpoints = test_endpoints_config();

        // No task.endpoint, but task.provider → find matching endpoint
        let task_endpoint: Option<String> = None;
        let task_provider = Some("openrouter".to_string());
        let agent_provider: Option<String> = None;
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_agent_provider_fallback() {
        let endpoints = test_endpoints_config();

        // No task.endpoint or task.provider, agent.preferred_provider finds endpoint
        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider = Some("anthropic".to_string());
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-anthropic".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_role_config_fallback() {
        let endpoints = test_endpoints_config();

        // Nothing else set, role config endpoint is used
        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider: Option<String> = None;
        let role_endpoint = Some("my-anthropic".to_string());

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-anthropic".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_none_when_all_empty() {
        let endpoints = test_endpoints_config();

        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider: Option<String> = None;
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, None);
    }

    #[test]
    fn test_endpoint_api_key_resolved_from_config() {
        let endpoints = test_endpoints_config();
        let ep = endpoints.find_by_name("my-openrouter").unwrap();
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key, Some("sk-or-test".to_string()));
    }

    // --- resolve_model_via_registry tests ---

    fn setup_registry_dir() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir).unwrap();

        // Create a config with a custom model registry entry
        let mut config = Config::default();
        config.model_registry = vec![worksgood::config::ModelRegistryEntry {
            id: "my-custom".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-3.5-sonnet".to_string(),
            tier: worksgood::config::Tier::Standard,
            endpoint: Some("my-openrouter".to_string()),
            ..Default::default()
        }];
        config.save(dir).unwrap();
        tmp
    }

    #[test]
    fn test_registry_resolves_custom_alias_to_model_id() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("my-custom".to_string()),
            Some(&"my-custom".to_string()),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Custom alias should resolve to actual model ID"
        );
        assert_eq!(
            provider,
            Some("openrouter".to_string()),
            "Provider should come from registry entry"
        );
        assert_eq!(
            endpoint,
            Some("my-openrouter".to_string()),
            "Endpoint should come from registry entry"
        );
    }

    #[test]
    fn test_registry_keeps_builtin_alias_unchanged() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        for alias in &["haiku", "sonnet", "opus"] {
            let (model, provider, _endpoint) = resolve_model_via_registry(
                Some(alias.to_string()),
                Some(&alias.to_string()),
                &config,
                dir,
            )
            .unwrap();

            assert_eq!(
                model.as_deref(),
                Some(*alias),
                "Built-in alias '{}' should be kept as-is",
                alias
            );
            assert_eq!(
                provider,
                Some("anthropic".to_string()),
                "Built-in alias '{}' should resolve to anthropic provider",
                alias
            );
        }
    }

    #[test]
    fn test_registry_errors_on_unknown_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let result = resolve_model_via_registry(
            Some("nonexistent-model".to_string()),
            Some(&"nonexistent-model".to_string()),
            &config,
            dir,
        );

        assert!(
            result.is_err(),
            "Should error when task model is not in registry"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found in config"),
            "Error should mention 'not found in config': {}",
            err
        );
        assert!(
            err.contains("wg model add"),
            "Error should suggest how to register: {}",
            err
        );
    }

    #[test]
    fn test_registry_bypass_for_pi_custom_provider_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let pi_model = "lunaroute:glm-5.2-nvfp4".to_string();
        let (model, provider, endpoint) = resolve_spawn_model_via_registry(
            "pi",
            Some(pi_model.clone()),
            Some(&pi_model),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(pi_model),
            "Pi owns provider:model resolution; WG must not consult its registry/cache"
        );
        assert_eq!(provider, None);
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_still_rejects_custom_provider_colon_for_non_pi_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let custom_provider_model = "lunaroute:glm-5.2-nvfp4".to_string();
        let result = resolve_spawn_model_via_registry(
            "native",
            Some(custom_provider_model.clone()),
            Some(&custom_provider_model),
            &config,
            dir,
        );

        assert!(
            result.is_err(),
            "Non-Pi executors should still validate unknown task models against WG registry/cache"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in config"),
            "Non-Pi validation should keep the existing registry/cache error"
        );
    }

    #[test]
    fn test_registry_passes_through_non_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // Model came from executor/coordinator, not from task — should pass through.
        // CLAUDE_OPUS_MODEL_ID matches the builtin "opus" entry's model field, so
        // it resolves with provider info from that entry.
        let (model, provider, _endpoint) = resolve_model_via_registry(
            Some(CLAUDE_OPUS_MODEL_ID.to_string()),
            None, // no task model
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(CLAUDE_OPUS_MODEL_ID.to_string()),
            "Non-task model should resolve to the same model ID"
        );
        assert_eq!(
            provider,
            Some("anthropic".to_string()),
            "Should find provider from builtin registry entry"
        );
    }

    #[test]
    fn test_registry_claude_fable_expands_to_full_cli_id() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // `claude:fable` has the claude provider prefix → the claude CLI branch
        // must expand the friendly alias `fable` to the full CLI model id
        // `claude-fable-5` (the CLI has no bare `fable` shortcut).
        let (model, provider, _endpoint) = resolve_model_via_registry(
            Some("claude:fable".to_string()),
            Some(&"claude:fable".to_string()),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(CLAUDE_FABLE_MODEL_ID.to_string()),
            "claude:fable must resolve to the full CLI id claude-fable-5"
        );
        assert_eq!(
            provider,
            Some("anthropic".to_string()),
            "claude provider prefix maps to the anthropic native provider"
        );

        // Bare `fable` (no prefix) resolves via the builtin registry entry,
        // whose model field also carries the full CLI id.
        let (bare_model, _p, _e) = resolve_model_via_registry(
            Some("fable".to_string()),
            Some(&"fable".to_string()),
            &config,
            dir,
        )
        .unwrap();
        assert_eq!(bare_model, Some(CLAUDE_FABLE_MODEL_ID.to_string()));
    }

    #[test]
    fn test_registry_truly_unknown_non_task_model_passes_through() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // A model not in the registry at all, from executor/coordinator
        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("totally-unknown-model".to_string()),
            None, // no task model
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("totally-unknown-model".to_string()),
            "Unknown non-task model should pass through unchanged"
        );
        assert_eq!(
            provider, None,
            "No registry provider for truly unknown model"
        );
        assert_eq!(
            endpoint, None,
            "No registry endpoint for truly unknown model"
        );
    }

    #[test]
    fn test_registry_none_model_returns_none() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) =
            resolve_model_via_registry(None, None, &config, dir).unwrap();

        assert_eq!(model, None);
        assert_eq!(provider, None);
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_non_task_model_matching_alias_still_resolves() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // Model came from executor config but happens to match a registry entry
        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("my-custom".to_string()),
            None, // not from task
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Should still resolve even if not from task"
        );
        assert_eq!(provider, Some("openrouter".to_string()));
        assert_eq!(endpoint, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_registry_strips_codex_prefix_for_codex_executor_models() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) =
            resolve_model_via_registry(Some("codex:gpt-5-codex".to_string()), None, &config, dir)
                .unwrap();

        assert_eq!(model, Some("gpt-5-codex".to_string()));
        assert_eq!(provider, Some("oai-compat".to_string()));
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_full_model_id_passthrough_for_task() {
        // Full model IDs with "/" should pass through even when task-specified,
        // allowing OpenRouter-style "provider/model" to work without registration.
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let full_model = "deepseek/deepseek-chat".to_string();
        let (model, provider, endpoint) =
            resolve_model_via_registry(Some(full_model.clone()), Some(&full_model), &config, dir)
                .unwrap();

        assert_eq!(
            model,
            Some("deepseek/deepseek-chat".to_string()),
            "Full model ID with / should pass through unchanged"
        );
        assert_eq!(
            provider, None,
            "No provider from registry — auto-detection will handle it"
        );
        assert_eq!(endpoint, None, "No endpoint from registry");
    }

    #[test]
    fn test_registry_lookup_by_model_field() {
        // If a registry entry has model = "anthropic/claude-3.5-sonnet",
        // using --model "anthropic/claude-3.5-sonnet" should find it.
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let full_model = "anthropic/claude-3.5-sonnet".to_string();
        let (model, provider, endpoint) =
            resolve_model_via_registry(Some(full_model.clone()), Some(&full_model), &config, dir)
                .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Should match registry entry by model field"
        );
        assert_eq!(
            provider,
            Some("openrouter".to_string()),
            "Should get provider from matched entry"
        );
        assert_eq!(
            endpoint,
            Some("my-openrouter".to_string()),
            "Should get endpoint from matched entry"
        );
    }

    #[test]
    fn test_registry_short_alias_still_errors_when_unknown() {
        // Short aliases (no "/") that aren't registered should still error
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let unknown = "some-unknown-alias".to_string();
        let result =
            resolve_model_via_registry(Some(unknown.clone()), Some(&unknown), &config, dir);

        assert!(result.is_err(), "Short unknown aliases should still error");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in config"),
            "Error should mention registration"
        );
    }

    fn external_test_settings(
        executor_type: &str,
        command: &str,
        args: &[&str],
    ) -> worksgood::service::executor::ExecutorSettings {
        worksgood::service::executor::ExecutorSettings {
            executor_type: executor_type.to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Investigate task".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        }
    }

    fn test_template_vars() -> TemplateVars {
        TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        }
    }

    fn default_external_settings(
        workgraph_dir: &Path,
        executor_type: &str,
    ) -> worksgood::service::executor::ExecutorSettings {
        let registry = ExecutorRegistry::new(workgraph_dir);
        let mut settings = registry.load_config(executor_type).unwrap().executor;
        settings.prompt_template = Some(PromptTemplate {
            template: "Investigate task".to_string(),
        });
        settings
    }

    #[test]
    fn test_build_inner_command_pi_external_emits_model_and_thinking() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings("pi", "pi", &["--mode", "json"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command_with_reasoning(
            &settings,
            "full",
            output_dir,
            &Some("pi:openai-codex:gpt-5.6-sol".to_string()),
            &None,
            Some(ReasoningLevel::High),
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("--provider 'openai-codex'"),
            "Pi external command must carry the provider split: {}",
            command
        );
        assert!(
            command.contains("--model 'gpt-5.6-sol'"),
            "Pi external command must carry the model split: {}",
            command
        );
        assert!(
            command.contains("--thinking 'high'"),
            "Pi external command must carry structured reasoning: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_opencode_default_uses_run_json_prompt_file_and_openrouter_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "opencode");
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("'opencode' 'run'"),
            "OpenCode must use documented non-interactive `opencode run`: {}",
            command
        );
        assert!(
            command.contains("'--format' 'json'"),
            "OpenCode should request JSON-capable output: {}",
            command
        );
        assert!(
            command.contains("--model 'openrouter/deepseek/deepseek-v3.2'"),
            "OpenCode should receive provider/model slash syntax: {}",
            command
        );
        assert!(
            command.contains("--file "),
            "OpenCode should receive the WG prompt as an attached file: {}",
            command
        );
        let message_pos = command
            .find("'Complete the attached WG task prompt.'")
            .expect("OpenCode command should include an explicit message");
        let file_pos = command
            .find("--file ")
            .expect("OpenCode command should attach prompt file");
        assert!(
            message_pos < file_pos,
            "OpenCode --file is an array option; message must come before --file so it is not parsed as a second file: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into OpenCode argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_opencode_errors_when_model_unresolved() {
        // Explicit-model contract (fix-opencode-build req #3): the opencode
        // worker path must FAIL when model resolution produced nothing, never
        // silently omit `--model` and inherit opencode's internal default.
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "opencode");
        let vars = test_template_vars();

        let result = build_inner_command(
            &settings, "full", output_dir, &None, // <- no resolved model
            &None, &None, &None, &None, &vars, &None, None,
        );

        let err = result.expect_err("opencode with no resolved model must be a hard error");
        let msg = format!("{err}");
        assert!(
            msg.contains("requires an explicitly resolved model"),
            "error must explain the explicit-model contract, got: {msg}"
        );
    }

    #[test]
    fn test_build_inner_command_aider_default_uses_message_file_and_openrouter_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "aider");
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("'aider'"),
            "Aider command should invoke the aider CLI: {}",
            command
        );
        assert!(
            command.contains("'--yes-always'"),
            "Aider should avoid confirmation prompts in batch mode: {}",
            command
        );
        assert!(
            command.contains("--model 'openrouter/deepseek/deepseek-v3.2'"),
            "Aider should receive provider/model slash syntax: {}",
            command
        );
        assert!(
            command.contains("--message-file "),
            "Aider should receive the WG prompt through --message-file: {}",
            command
        );
        assert!(
            !command.contains(" --message '"),
            "Aider must avoid interactive chat and inline --message prompts: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Aider argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_goose_normalizes_provider_and_model_without_key_leak() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "goose",
            "goose",
            &["run", "--no-session", "--output-format", "json"],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("--provider 'openrouter'"),
            "Goose should receive a provider flag: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Goose should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            command.contains("-i "),
            "Goose should receive the WG prompt as an input file: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_qwen_uses_prompt_headless_model_and_output() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings =
            external_test_settings("qwen", "qwen", &["--output-format", "json", "--yolo"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.starts_with("cat "),
            "Qwen should receive WG's assembled prompt through stdin: {}",
            command
        );
        assert!(
            command.contains("--prompt 'Complete the WG task prompt supplied on stdin.'"),
            "Qwen should run in documented --prompt headless mode: {}",
            command
        );
        assert!(
            command.contains("'--output-format' 'json'"),
            "Qwen should request machine-readable output where supported: {}",
            command
        );
        assert!(
            command.contains("'--yolo'"),
            "Qwen Code's unattended approval flag is deliberately experimental and covered here so future changes are explicit: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Qwen should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            !command.contains("--provider"),
            "Qwen's OpenRouter path relies on configured provider/auth, not a provider argv flag: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Qwen argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_amplifier_uses_prompt_argument_bridge() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "amplifier",
            "amplifier",
            &[
                "run",
                "--mode",
                "single",
                "--output-format",
                "json",
                "--bundle",
                "wg",
            ],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("PROMPT=$(cat \"$1\"); shift; exec \"$@\" \"$PROMPT\""),
            "Amplifier should bridge prompt.txt into a positional argument: {}",
            command
        );
        assert!(
            command.contains(
                "'amplifier' 'run' '--mode' 'single' '--output-format' 'json' '--bundle' 'wg'"
            ),
            "Amplifier default command shape should be preserved: {}",
            command
        );
        assert!(
            !command.contains("--model"),
            "Amplifier defaults should not treat amplifier as a native model provider: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret"),
            "External CLI argv must not contain API keys: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_cline_uses_headless_auto_approve_provider_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings =
            external_test_settings("cline", "cline", &["--json", "--auto-approve", "true"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.starts_with("cat "),
            "Cline should receive WG's assembled prompt through stdin: {}",
            command
        );
        assert!(
            command.contains("--json"),
            "Cline should run in JSON-capable headless mode: {}",
            command
        );
        assert!(
            command.contains("'--auto-approve' 'true'"),
            "Cline's unattended auto-approval flag is deliberately experimental and covered here so future changes are explicit: {}",
            command
        );
        assert!(
            command.contains("--provider 'openrouter'"),
            "Cline should receive a provider flag where supported: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Cline should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            command.ends_with("'Complete the WG task prompt supplied on stdin.'"),
            "Cline should get a positional headless task prompt: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Cline argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_generic_experimental_fallback_is_raw_argv_only() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "experimental-runner",
            "experimental-runner",
            &["run", "--json", "--flag=value"],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert_eq!(
            command, "'experimental-runner' 'run' '--json' '--flag=value'",
            "Unknown experimental executor types should stay on the raw configured argv fallback"
        );
        assert!(
            !output_dir.join("prompt.txt").exists(),
            "The generic fallback must not imply prompt delivery; first-class adapters need explicit branches"
        );
        assert!(
            !command.contains("--model") && !command.contains("--provider"),
            "Generic fallback should not guess model/provider flags: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "Generic fallback argv must not leak API credentials: {}",
            command
        );
    }

    #[test]
    fn test_preflight_executor_command_missing_binary_actionable() {
        let settings =
            external_test_settings("amplifier", "wg-missing-amplifier-test-binary", &["run"]);
        let err = preflight_executor_command(&settings, "amplifier", None)
            .expect_err("missing binary should fail preflight")
            .to_string();

        assert!(
            err.contains("amplifier") && err.contains("not found on PATH"),
            "error should name the missing executor and PATH failure: {}",
            err
        );
        assert!(
            err.contains(".wg/executors/amplifier.toml") && err.contains("Install"),
            "error should tell the user how to fix the executor command: {}",
            err
        );
        assert!(
            err.contains("amplifier run --mode single --output-format json --bundle wg"),
            "Amplifier-specific setup hint should include the expected command: {}",
            err
        );
    }

    #[test]
    fn test_preflight_executor_command_relative_path_uses_working_dir() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let runner = temp_dir.path().join("runner");
        std::fs::write(&runner, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&runner).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&runner, perms).unwrap();
        }

        let settings = external_test_settings("custom", "./runner", &[]);
        preflight_executor_command(&settings, "custom", Some(temp_dir.path()))
            .expect("relative command should resolve from working_dir");
    }

    #[test]
    fn test_inject_api_key_env_sets_openrouter_env_without_file_path() {
        let endpoint = EndpointConfig {
            name: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            url: Some("https://openrouter.ai/api/v1".to_string()),
            model: None,
            api_key: None,
            api_key_file: Some("~/.openrouter.key".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        };
        let mut cmd = Command::new("true");
        inject_api_key_env(&mut cmd, Some(&endpoint), &Some("sk-or-secret".to_string()));

        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();

        assert!(envs.contains(&("WG_API_KEY".to_string(), Some("sk-or-secret".to_string()))));
        assert!(envs.contains(&(
            "OPENROUTER_API_KEY".to_string(),
            Some("sk-or-secret".to_string())
        )));
        assert!(
            envs.iter().all(|(key, value)| {
                !key.contains(".openrouter.key")
                    && value
                        .as_deref()
                        .map_or(true, |value| !value.contains(".openrouter.key"))
            }),
            "The configured key file path should not be propagated or printed: {:?}",
            envs
        );
    }

    #[test]
    fn test_direct_codex_worker_argv_keeps_verbosity_separate_from_reasoning() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let mut settings = default_external_settings(output_dir, "codex");
        settings.prompt_template = Some(PromptTemplate {
            template: "Implement task".to_string(),
        });
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command_with_reasoning(
            &settings,
            "full",
            output_dir,
            &Some("gpt-5.6-sol".to_string()),
            &None,
            Some(ReasoningLevel::Minimal),
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("model_reasoning_effort=\"low\""),
            "WG minimal must map to Codex low effort: {command}"
        );
        assert!(
            command.contains("model_verbosity=\"high\""),
            "the independently configured Codex verbosity must remain present: {command}"
        );
    }

    #[test]
    fn test_direct_codex_reasoning_effort_mapping_is_explicit_for_every_level() {
        let expected = [
            (ReasoningLevel::Off, "none"),
            (ReasoningLevel::Minimal, "low"),
            (ReasoningLevel::Low, "low"),
            (ReasoningLevel::Medium, "medium"),
            (ReasoningLevel::High, "high"),
            (ReasoningLevel::Xhigh, "xhigh"),
            (ReasoningLevel::Max, "max"),
        ];
        for (level, effort) in expected {
            assert_eq!(level.as_codex_effort(), effort);
        }
    }

    #[test]
    fn test_build_inner_command_codex_uses_prompt_file_and_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "codex".to_string(),
            command: "codex".to_string(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Investigate task".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("gpt-5-codex".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(
            fallback.is_none(),
            "Codex should not have a fallback command"
        );
        assert!(
            command.contains("cat "),
            "Expected prompt to be piped from a file: {}",
            command
        );
        assert!(
            command.contains("'codex' 'exec'"),
            "Expected codex exec invocation: {}",
            command
        );
        assert!(
            command.contains("'--json'"),
            "Expected codex JSON mode: {}",
            command
        );
        assert!(
            command.contains("'--skip-git-repo-check'"),
            "Expected codex git check bypass flag: {}",
            command
        );
        assert!(
            command.contains("--model 'gpt-5-codex'"),
            "Expected codex model flag: {}",
            command
        );
        assert!(
            !command.contains("model_reasoning_effort"),
            "unset WG reasoning must inherit ~/.codex/config.toml: {command}"
        );
        let prompt_file = output_dir.join("prompt.txt");
        assert_eq!(
            std::fs::read_to_string(prompt_file).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_claude_resume_produces_fallback() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "claude".to_string(),
            command: "claude".to_string(),
            args: vec![
                "--print".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Full task prompt".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Resume context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("sonnet".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            Some("fake-session-id-12345"),
        )
        .unwrap();

        // Primary command should use --resume
        assert!(
            command.contains("--resume"),
            "Resume command should contain --resume: {}",
            command
        );
        assert!(
            command.contains("fake-session-id-12345"),
            "Resume command should contain session ID: {}",
            command
        );

        // Fallback should exist and NOT contain --resume
        let fb = fallback.expect("Claude resume should produce a fallback command");
        assert!(
            !fb.contains("--resume"),
            "Fallback command should NOT contain --resume: {}",
            fb
        );
        assert!(
            fb.contains("prompt.txt"),
            "Fallback should use prompt.txt: {}",
            fb
        );

        // Both prompt files should be written
        assert!(
            output_dir.join("resume_message.txt").exists(),
            "resume_message.txt should be written"
        );
        assert!(
            output_dir.join("prompt.txt").exists(),
            "prompt.txt should be written for fallback"
        );
    }

    #[test]
    fn test_build_inner_command_claude_no_resume_no_fallback() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "claude".to_string(),
            command: "claude".to_string(),
            args: vec![
                "--print".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Full task prompt".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("sonnet".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            None, // No resume session
        )
        .unwrap();

        assert!(
            !command.contains("--resume"),
            "Fresh command should NOT contain --resume: {}",
            command
        );
        assert!(
            fallback.is_none(),
            "Fresh spawn should not have a fallback command"
        );
    }

    #[test]
    fn test_wrapper_script_contains_session_fallback_when_fallback_provided() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let wrapper_path = write_wrapper_script(
            output_dir,
            "test-task",
            "/tmp/output.log",
            "claude --resume fake-id --print",
            None,
            "claude",
            Some("claude --print < prompt.txt"),
        )
        .unwrap();

        let script = std::fs::read_to_string(&wrapper_path).unwrap();
        assert!(
            script.contains("Session not resumable, starting fresh session"),
            "Wrapper should contain session fallback logic"
        );
        assert!(
            script.contains("No conversation found"),
            "Wrapper should detect 'No conversation found' error"
        );
        assert!(
            script.contains("claude --print < prompt.txt"),
            "Wrapper should contain the fallback command"
        );
        let fallback = script
            .split("Session not resumable, starting fresh session")
            .nth(1)
            .expect("fallback block");
        assert!(
            fallback.contains("} {HEARTBEAT_GUARD_FD}>&-"),
            "fallback executor must not inherit the heartbeat guard writer"
        );
    }

    #[test]
    fn test_wrapper_script_no_fallback_when_none() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let wrapper_path = write_wrapper_script(
            output_dir,
            "test-task",
            "/tmp/output.log",
            "claude --print",
            None,
            "claude",
            None,
        )
        .unwrap();

        let script = std::fs::read_to_string(&wrapper_path).unwrap();
        assert!(
            !script.contains("Session not resumable"),
            "Wrapper should NOT contain session fallback when no fallback provided"
        );
        assert!(
            script.contains("exec {HEARTBEAT_GUARD_FD}> >(wg heartbeat-watch \"$WG_AGENT_ID\""),
            "wrapper must launch the pipe-guarded heartbeat watcher"
        );
        assert!(
            script.matches("{HEARTBEAT_GUARD_FD}>&-").count() >= 2,
            "executor must close the guard writer and wrapper must close it on completion"
        );
        assert!(
            !script
                .lines()
                .any(|line| line.trim_start().starts_with("sleep 120")),
            "wrapper must not generate the orphan-prone heartbeat sleep command"
        );
    }
}
