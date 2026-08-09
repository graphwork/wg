//! Configuration management commands

use anyhow::Result;
use std::path::Path;
use worksgood::config::{
    Config, ConfigSource, DispatchRole, EndpointConfig, MatrixConfig, ModelRegistryEntry,
    ReasoningLevel, Tier,
};

fn clear_dispatcher_executor_for_model(config: &mut Config, model: &str) -> Option<String> {
    let spec = worksgood::config::parse_model_spec(model);
    let provider = spec.provider.as_deref()?;
    let implied = worksgood::config::provider_to_executor(provider);
    let previous = config.coordinator.executor.take()?;
    Some(format!(
        "Cleared deprecated dispatcher.executor = \"{}\"; dispatcher.model implies executor = \"{}\"",
        previous, implied
    ))
}

#[allow(dead_code)]
fn print_executor_choices_section() {
    println!("[executor choices]");
    println!(
        "  core = {}",
        worksgood::executor_discovery::CORE_EXECUTORS.join(", ")
    );
    println!(
        "  stable_external = {}",
        worksgood::executor_discovery::STABLE_EXTERNAL_EXECUTORS.join(", ")
    );
    println!(
        "  provider_specific = {}",
        worksgood::executor_discovery::PROVIDER_SPECIFIC_EXECUTORS.join(", ")
    );
    println!(
        "  experimental_external = {}",
        worksgood::executor_discovery::EXPERIMENTAL_EXTERNAL_EXECUTORS.join(", ")
    );
    println!("  discovery = \"wg executors --all\" (shows installed/usable status)");
    println!();
}

/// When model/endpoint changes land, a soft reload (`Reconfigure` IPC)
/// isn't enough — already-spawned coordinator subprocesses keep their
/// old env. We restart the daemon instead so the coordinator respawns
/// reading the just-written config.toml.
///
/// Returns `Ok(true)` when a restart happened, `Ok(false)` when no
/// daemon was running (benign — the config is on disk for next start),
/// or `Err` when stop/start failed.
#[cfg(unix)]
fn try_restart_daemon(dir: &Path) -> Result<bool> {
    use crate::commands::service::{self, ServiceState};

    let running = match ServiceState::load(dir) {
        Ok(Some(state)) => worksgood::service::is_process_alive(state.pid),
        _ => false,
    };
    if !running {
        return Ok(false);
    }

    // Full restart — same code path as `wg service restart`. This tears
    // down the stale CoordinatorAgent subprocesses and spawns fresh
    // ones reading the current config. We pass json=true so its
    // logging goes through structured output; the caller already
    // printed a human-readable summary.
    service::run_restart(dir, false).map(|()| true)
}

#[cfg(not(unix))]
fn try_restart_daemon(_dir: &Path) -> Result<bool> {
    Ok(false)
}

/// Render the `[llm_endpoints]` section of the effective merged config.
/// Always shows `inherit_global` so the user can immediately see whether
/// global endpoints are being cascaded in or not — this is the symptom
/// that motivated the inheritance opt-in change.
pub fn format_endpoints_section(config: &Config) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "[llm_endpoints]");
    let _ = writeln!(
        out,
        "  inherit_global = {}{}",
        config.llm_endpoints.inherit_global,
        if config.llm_endpoints.inherit_global {
            " (legacy cascade enabled — global endpoints merged in)"
        } else {
            " (default — local endpoints fully replace global)"
        }
    );
    if config.llm_endpoints.endpoints.is_empty() {
        let _ = writeln!(out, "  # (no endpoints configured)");
    } else {
        for ep in &config.llm_endpoints.endpoints {
            let url = ep.url.as_deref().unwrap_or("");
            let _ = writeln!(
                out,
                "  {} = {{ provider = \"{}\", url = \"{}\", is_default = {} }}",
                ep.name, ep.provider, url, ep.is_default
            );
        }
    }
    let _ = writeln!(out);
    out
}

/// Scope for config operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Local,
    Global,
}

/// Show current configuration. Legacy model values remain visible only as
/// explicitly non-executable migration data.
pub fn show(dir: &Path, scope: Option<ConfigScope>, json: bool) -> Result<()> {
    // For the default (merged) view, also load the per-key source map so the
    // precedence-sensitive routing/capacity keys can be annotated with the
    // winning source (global / local / project-profile / default). Scoped
    // views (--global/--local) reflect a single layer, so no source annotation
    // is meaningful there. See docs/config-precedence.md.
    let (config, sources): (
        Config,
        Option<std::collections::BTreeMap<String, ConfigSource>>,
    ) = match scope {
        Some(ConfigScope::Global) => (Config::load_global()?.unwrap_or_default(), None),
        Some(ConfigScope::Local) => (Config::load(dir)?, None),
        None => {
            let (c, s) = Config::load_with_sources(dir)?;
            (c, Some(s))
        }
    };
    // Resolve the source label for a dotted key (empty when no source map).
    let src_of = |key: &str| -> String {
        sources
            .as_ref()
            .and_then(|m| m.get(key))
            .map(|s| format!("  [source: {}]", s))
            .unwrap_or_default()
    };

    // Surface collected config-load diagnostics once (deduplicated) for the
    // human-facing view. JSON mode stays silent on stderr so machine
    // consumers aren't disturbed; a clean config has no diagnostics, so this
    // is a no-op for migrated configs (the must-not-over-block requirement).
    if !json {
        config.emit_load_diagnostics();
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&config)?);
    } else {
        println!("WG Configuration");
        println!("========================");
        // Surface the active project profile (if any) + the precedence legend so
        // the winning source per key is discoverable from `--show` directly.
        if sources.is_some() {
            if let Ok(Some(assoc)) = worksgood::profile::project::read_association(dir) {
                println!(
                    "  active project profile: {} (fingerprint {})",
                    assoc.profile, assoc.profile_fingerprint
                );
                println!("  profile authority-owns ROUTING (model/executor/provider/tiers);");
                println!("  explicit local/global writes win for non-routing tuning.");
            }
            println!(
                "  sources: [global] [local] [project-profile] [default]; full detail: `wg config --list`"
            );
        }
        println!();
        println!("[agent]");
        if worksgood::config::parse_supported_execution_route(&config.agent.model).is_ok() {
            println!(
                "  handler = \"{}\"",
                worksgood::dispatch::handler_for_model(&config.agent.model).as_str()
            );
            println!(
                "  model = \"{}\"{}",
                config.agent.model,
                src_of("agent.model")
            );
        } else {
            println!("  handler = UNSELECTED/INVALID");
            println!("  model = UNSELECTED/INVALID");
            if !config.agent.model.trim().is_empty() {
                println!(
                    "  legacy_model = {:?}  # migration data; not executable",
                    config.agent.model
                );
            }
        }
        println!("  interval = {}", config.agent.interval);
        println!("  heartbeat_timeout = {}", config.agent.heartbeat_timeout);
        if let Some(seconds) = config.agent.heartbeat_timeout_seconds {
            println!("  heartbeat_timeout_seconds = {}", seconds);
        }
        if let Some(max) = config.agent.max_tasks {
            println!("  max_tasks = {}", max);
        }
        println!();
        println!("[dispatcher]");
        println!(
            "  max_agents = {}{}",
            config.coordinator.max_agents,
            src_of("dispatcher.max_agents")
        );
        println!(
            "  max_coordinators = {}",
            config.coordinator.max_coordinators
        );
        println!("  interval = {}", config.coordinator.interval);
        println!("  poll_interval = {}", config.coordinator.poll_interval);
        if let Some(ref m) = config.coordinator.model {
            if worksgood::config::parse_supported_execution_route(m).is_ok() {
                println!(
                    "  handler = \"{}\"",
                    worksgood::dispatch::handler_for_model(m).as_str()
                );
                println!("  model = \"{}\"{}", m, src_of("dispatcher.model"));
            } else {
                println!("  handler = INVALID");
                println!("  model = INVALID");
                println!("  legacy_model = {m:?}  # migration data; not executable");
            }
        }
        println!(
            "  max_incomplete_retries = {}",
            config.coordinator.max_incomplete_retries
        );
        println!(
            "  incomplete_retry_delay = \"{}\"",
            config.coordinator.incomplete_retry_delay
        );
        println!(
            "  escalate_on_retry = {}",
            config.coordinator.escalate_on_retry
        );
        println!();
        println!("[model plane]");
        println!("  recommended = \"Pi\"");
        println!("  explicit_worker_handlers = [\"pi\", \"claude\", \"codex\"]");
        println!("  WG stores exact per-role routes + reasoning only; each CLI owns auth/models");
        println!();
        println!("[agency]");
        println!("  auto_evaluate = {}", config.agency.auto_evaluate);
        println!("  auto_assign = {}", config.agency.auto_assign);
        println!("  auto_create = {}", config.agency.auto_create);
        if let Some(ref agent) = config.agency.assigner_agent {
            println!("  assigner_agent = \"{}\"", agent);
        }
        if let Some(ref agent) = config.agency.evaluator_agent {
            println!("  evaluator_agent = \"{}\"", agent);
        }
        if let Some(ref agent) = config.agency.evolver_agent {
            println!("  evolver_agent = \"{}\"", agent);
        }
        if let Some(ref agent) = config.agency.creator_agent {
            println!("  creator_agent = \"{}\"", agent);
        }
        if let Some(ref heuristics) = config.agency.retention_heuristics {
            println!("  retention_heuristics = \"{}\"", heuristics);
        }
        println!("  auto_triage = {}", config.agency.auto_triage);
        if let Some(timeout) = config.agency.triage_timeout {
            println!("  triage_timeout = {}", timeout);
        }
        if let Some(timeout) = config.agency.inference_timeout {
            println!("  inference_timeout = {}", timeout);
        }
        if let Some(max_bytes) = config.agency.triage_max_log_bytes {
            println!("  triage_max_log_bytes = {}", max_bytes);
        }
        if let Some(threshold) = config.agency.eval_gate_threshold {
            println!("  eval_gate_threshold = {}", threshold);
        }
        if config.agency.eval_gate_all {
            println!("  eval_gate_all = {}", config.agency.eval_gate_all);
        }
        if config.agency.flip_enabled {
            println!("  flip_enabled = {}", config.agency.flip_enabled);
        }
        if let Some(threshold) = config.agency.flip_verification_threshold {
            println!("  flip_verification_threshold = {}", threshold);
        }
        if config.evaluation.managed_rollout
            && config.evaluation.rollout_stage
                == worksgood::config::EvaluationRolloutStage::FlipRequired
        {
            println!(
                "  effective_gate = required deep-readonly FLIP for qualifying candidates ; evaluator_threshold = n/a (bounded absent) ; flip_policy = required-primary-pre-merge ; flip_threshold = {}",
                config.agency.flip_verification_threshold.unwrap_or(0.8)
            );
        } else if let Some(eval_threshold) = config.agency.eval_gate_threshold {
            println!(
                "  effective_gate = {} ; evaluator_threshold = {} ; flip_policy = required-strict-when-persisted ; flip_threshold = {}",
                if config.agency.eval_gate_all {
                    "all evaluated tasks"
                } else {
                    "structural deliverables only (others advisory)"
                },
                eval_threshold,
                config
                    .agency
                    .flip_verification_threshold
                    .unwrap_or(eval_threshold)
            );
        } else {
            println!("  effective_gate = advisory-only ; flip_policy = advisory");
        }
        println!("  auto_place = {}", config.agency.auto_place);
        if config.agency.auto_evolve {
            println!("  auto_evolve = {}", config.agency.auto_evolve);
            println!(
                "  evolution_interval = {}",
                config.agency.evolution_interval
            );
            println!(
                "  evolution_threshold = {}",
                config.agency.evolution_threshold
            );
            println!("  evolution_budget = {}", config.agency.evolution_budget);
            println!(
                "  evolution_reactive_threshold = {}",
                config.agency.evolution_reactive_threshold
            );
        }
        println!();

        // Unified agency agents display
        {
            use worksgood::config::DispatchRole;
            println!("[agency agents]");

            // Helper to get auto-toggle status for applicable roles
            let auto_status = |role: &DispatchRole| -> Option<&str> {
                match role {
                    DispatchRole::Placer => Some(if config.agency.auto_place {
                        "on"
                    } else {
                        "off"
                    }),
                    DispatchRole::Assigner | DispatchRole::Evaluator => Some("legacy-inert"),
                    DispatchRole::CoordinatorEval => Some(if config.agency.auto_evaluate {
                        "source-review"
                    } else {
                        "off"
                    }),
                    DispatchRole::Creator => Some(if config.agency.auto_create {
                        "on"
                    } else {
                        "off"
                    }),
                    DispatchRole::Evolver => Some(if config.agency.auto_evolve {
                        "on"
                    } else {
                        "off"
                    }),
                    DispatchRole::Triage => Some(if config.agency.auto_triage {
                        "on"
                    } else {
                        "off"
                    }),
                    _ => None,
                }
            };

            for role in DispatchRole::ALL {
                let auto_str = match auto_status(role) {
                    Some(status) => format!(", auto: {status}"),
                    None => String::new(),
                };
                match config.resolve_execution_route_for_role(*role) {
                    Ok(resolved) => println!(
                        "  {:<14} = {} (handler: {}, reasoning: {}{})",
                        role, resolved.route, resolved.handler, resolved.reasoning, auto_str
                    ),
                    Err(error) => println!("  {:<14} = INVALID ({error}{auto_str})", role),
                }
            }
        }
        println!();
        println!("[guardrails]");
        println!(
            "  max_child_tasks_per_agent = {}",
            config.guardrails.max_child_tasks_per_agent
        );
        println!();
        println!("[tui]");
        println!("  chat_history = {}", config.tui.chat_history);
        println!("  chat_history_max = {}", config.tui.chat_history_max);
        println!("  counters = \"{}\"", config.tui.counters);
        println!("  show_system_tasks = {}", config.tui.show_system_tasks);
        println!(
            "  show_running_system_tasks = {}",
            config.tui.show_running_system_tasks
        );
        println!();
        println!("[viz]");
        println!("  edge_color = \"{}\"", config.viz.edge_color);
        println!();
        if config.project.name.is_some() || config.project.description.is_some() {
            println!("[project]");
            if let Some(ref name) = config.project.name {
                println!("  name = \"{}\"", name);
            }
            if let Some(ref desc) = config.project.description {
                println!("  description = \"{}\"", desc);
            }
            println!();
        }
        // Display unified [models] section
        {
            use worksgood::config::DispatchRole;
            let has_any = config.models.default.is_some()
                || DispatchRole::ALL
                    .iter()
                    .any(|r| config.models.get_role(*r).is_some());
            if has_any {
                println!("[models]");
                if let Some(ref default_cfg) = config.models.default {
                    if let Some(ref m) = default_cfg.model {
                        println!("  default.model = \"{}\"", m);
                    }
                    if let Some(ref p) = default_cfg.provider {
                        println!("  default.provider = \"{}\"", p);
                    }
                    if let Some(reasoning) = default_cfg.reasoning {
                        println!("  default.reasoning = \"{}\"", reasoning);
                    }
                }
                for role in DispatchRole::ALL {
                    if let Some(role_cfg) = config.models.get_role(*role) {
                        if let Some(ref m) = role_cfg.model {
                            println!("  {}.model = \"{}\"", role, m);
                        }
                        if let Some(ref p) = role_cfg.provider {
                            println!("  {}.provider = \"{}\"", role, p);
                        }
                        if let Some(ref t) = role_cfg.tier {
                            println!("  {}.tier = \"{}\"", role, t);
                        }
                        if let Some(reasoning) = role_cfg.reasoning {
                            println!("  {}.reasoning = \"{}\"", role, reasoning);
                        }
                    }
                }
                println!();
            }
        }

        // Legacy endpoint/credential/registry fields remain readable for
        // migration but are intentionally absent from the supported display.

        // Health check
        let validation = config.validate_config();
        if validation.is_clean() {
            println!("[health check]");
            println!("  status = ok");
        } else {
            println!("[health check]");
            if validation.is_ok() {
                println!("  status = warnings");
            } else {
                println!("  status = errors");
            }
            print!("{}", validation.display());
        }
    }

    Ok(())
}

/// Initialize default config file
pub fn init(dir: &Path, scope: Option<ConfigScope>) -> Result<()> {
    if scope == Some(ConfigScope::Global) {
        if Config::init_global()? {
            let path = Config::global_config_path()?;
            println!("Created default global configuration at {}", path.display());
        } else {
            let path = Config::global_config_path()?;
            println!("Global configuration already exists at {}", path.display());
        }
    } else if Config::init(dir)? {
        println!("Created default configuration at .wg/config.toml");
    } else {
        println!("Configuration already exists at .wg/config.toml");
    }
    Ok(())
}

/// Write a graph-only config with no model route. This is intentionally
/// available only through `wg config init --bare`.
pub fn init_graph_only(workgraph_dir: &Path, scope: ConfigScope, force: bool) -> Result<()> {
    let path = match scope {
        ConfigScope::Global => Config::global_config_path()?,
        ConfigScope::Local => workgraph_dir.join("config.toml"),
    };
    if path.exists()
        && !force
        && !std::fs::read_to_string(&path)
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
    }
    if path.exists() && force {
        std::fs::copy(&path, path.with_extension("toml.bak"))?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = match scope {
        ConfigScope::Global => "# WG graph-only configuration; no LLM execution route selected.\n",
        ConfigScope::Local => {
            "# WG graph-only configuration; no LLM execution route selected.\n[project]\n"
        }
    };
    std::fs::write(&path, body)?;
    println!("Created graph-only configuration at {}", path.display());
    Ok(())
}

/// `wg config init` — write a minimal canonical config file for the
/// chosen route. Refuses to overwrite an existing file unless `--force`
/// is set (in which case a `.bak` is made first).
///
/// The output is byte-deterministic for a given (scope, route, bare)
/// triple: every `wg config init` invocation produces the exact same
/// file. This makes `wg migrate config` idempotent and lets us assert
/// against fixtures in tests.
pub fn init_minimal(
    workgraph_dir: &Path,
    scope: ConfigScope,
    route: &str,
    bare: bool,
    force: bool,
) -> Result<()> {
    let route_enum = worksgood::config_defaults::SetupRoute::from_name(route)
        .ok_or_else(|| anyhow::anyhow!("unknown route '{}'. The supported route is: pi", route,))?;

    let path = match scope {
        ConfigScope::Global => Config::global_config_path()?,
        ConfigScope::Local => workgraph_dir.join("config.toml"),
    };

    let body = render_minimal_config(route_enum, scope, bare);

    if path.exists() && !force {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if !existing.trim().is_empty() {
            anyhow::bail!(
                "{} already exists.\n\
                 Run `wg migrate config` to clean up stale keys, or pass --force \
                 to overwrite (a backup is made automatically as `<path>.bak`).",
                path.display(),
            );
        }
    }

    if path.exists() && force {
        let backup = path.with_extension("toml.bak");
        std::fs::copy(&path, &backup).map_err(|e| {
            anyhow::anyhow!(
                "failed to back up {} to {}: {}",
                path.display(),
                backup.display(),
                e
            )
        })?;
        println!("Backed up existing config to {}", backup.display());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {}", parent.display(), e))?;
    }
    std::fs::write(&path, &body)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {}", path.display(), e))?;

    println!(
        "Wrote minimal {} config at {} (route: {})",
        match scope {
            ConfigScope::Global => "global",
            ConfigScope::Local => "local",
        },
        path.display(),
        route_enum.as_name(),
    );
    println!(
        "Edit exact Pi routes/reasoning directly, or run `wg profile pi`; configure providers and endpoints in Pi."
    );
    Ok(())
}

/// Render a minimal canonical config TOML body for the given (route, scope, bare).
///
/// Hand-crafted strings rather than serialized `Config` so the output is:
/// - **deterministic** (no field-ordering surprises from serde)
/// - **minimal** (only keys the design picked as 'always-set' for the route)
/// - **commented** (each section has a one-line note explaining why it's there)
///
/// A non-bare Pi route is complete and explicit. Bare initialization remains
/// graph-only; no built-in LLM route is synthesized.
pub fn render_minimal_config(
    route: worksgood::config_defaults::SetupRoute,
    scope: ConfigScope,
    bare: bool,
) -> String {
    use worksgood::config_defaults::SetupRoute as R;

    if bare {
        return match scope {
            ConfigScope::Local => {
                "# .wg/config.toml — written by `wg config init --local --bare`\n\
                 \n\
                 [project]\n\
                 name = \"\"\n"
                    .to_string()
            }
            ConfigScope::Global => {
                "# ~/.wg/config.toml — written by `wg config init --global --bare`\n\
                 # Graph-only configuration; no LLM execution route selected.\n"
                    .to_string()
            }
        };
    }

    let scope_label = match scope {
        ConfigScope::Global => "~/.wg/config.toml",
        ConfigScope::Local => ".wg/config.toml",
    };
    let scope_flag = match scope {
        ConfigScope::Global => "--global",
        ConfigScope::Local => "--local",
    };
    let header = format!(
        "# {} — written by `wg config init {} --route {}`\n\
         # Only keys that differ from built-in defaults are listed; everything\n\
         # else falls through to the binary's defaults. Edit freely or rerun\n\
         # `wg config init --force` to regenerate.\n\n",
        scope_label,
        scope_flag,
        route.as_name(),
    );

    match (route, scope) {
        // For routes that have a matching profile template, return the profile
        // template content verbatim. This is the single source of truth: `wg
        // config init --route <X>` = "copy profile <X> as starting config."
        // Drops the header comment when serving from the profile template
        // (the template's own `description = "..."` line is the equivalent).
        (R::ClaudeCli, ConfigScope::Global) => {
            worksgood::profile::named::STARTER_CLAUDE.to_string()
        }

        (R::CodexCli, ConfigScope::Global) => worksgood::profile::named::STARTER_CODEX.to_string(),

        (R::Local, ConfigScope::Global) => worksgood::profile::named::STARTER_NEX.to_string(),

        (R::Openrouter, ConfigScope::Global) => format!(
            "{header}\
             [agent]\n\
             model = \"openrouter:anthropic/claude-opus-4-7\"\n\
             \n\
             [tiers]\n\
             fast = \"openrouter:anthropic/claude-haiku-4-5\"\n\
             standard = \"openrouter:anthropic/claude-sonnet-4-6\"\n\
             premium = \"openrouter:anthropic/claude-opus-4-7\"\n\
             \n\
             [[llm_endpoints.endpoints]]\n\
             name = \"openrouter\"\n\
             provider = \"openrouter\"\n\
             url = \"https://openrouter.ai/api/v1\"\n\
             api_key_env = \"OPENROUTER_API_KEY\"\n\
             is_default = true\n",
        ),

        (R::Pi, ConfigScope::Global) => worksgood::profile::named::STARTER_PI.to_string(),

        (R::NexCustom, ConfigScope::Global) => format!(
            "{header}\
             # Edit endpoint url + api_key_env to match your provider.\n\
             [agent]\n\
             model = \"nex:custom-model\"\n\
             \n\
             [[llm_endpoints.endpoints]]\n\
             name = \"custom\"\n\
             provider = \"oai-compat\"\n\
             url = \"https://example.com/v1\"\n\
             api_key_env = \"CUSTOM_API_KEY\"\n\
             is_default = true\n",
        ),

        // Local scope: shadow the global default with a project-specific
        // model + endpoint. Most projects only need `[project]`.
        (R::ClaudeCli, ConfigScope::Local) => format!(
            "{header}\
             [project]\n\
             name = \"\"\n\
             \n\
             [agent]\n\
             model = \"claude:opus\"\n",
        ),

        (R::Openrouter, ConfigScope::Local) => format!(
            "{header}\
             [project]\n\
             name = \"\"\n\
             \n\
             [agent]\n\
             model = \"openrouter:anthropic/claude-opus-4-7\"\n\
             \n\
             [[llm_endpoints.endpoints]]\n\
             name = \"openrouter\"\n\
             provider = \"openrouter\"\n\
             url = \"https://openrouter.ai/api/v1\"\n\
             api_key_env = \"OPENROUTER_API_KEY\"\n\
             is_default = true\n",
        ),

        (R::CodexCli, ConfigScope::Local) => format!(
            "{header}\
             [project]\n\
             name = \"\"\n\
             \n\
             [agent]\n\
             model = \"codex:gpt-5.5\"\n",
        ),

        (R::Pi, ConfigScope::Local) => format!(
            "{}\n[project]\nname = \"\"\n",
            worksgood::profile::named::STARTER_PI
        ),

        (R::Local, ConfigScope::Local) => format!(
            "{header}\
             [project]\n\
             name = \"\"\n\
             \n\
             [agent]\n\
             model = \"nex:qwen2.5-coder:7b\"\n\
             \n\
             [[llm_endpoints.endpoints]]\n\
             name = \"local\"\n\
             provider = \"local\"\n\
             url = \"http://localhost:11434/v1\"\n\
             is_default = true\n",
        ),

        (R::NexCustom, ConfigScope::Local) => format!(
            "{header}\
             [project]\n\
             name = \"\"\n\
             \n\
             [agent]\n\
             model = \"nex:custom-model\"\n\
             \n\
             [[llm_endpoints.endpoints]]\n\
             name = \"custom\"\n\
             provider = \"oai-compat\"\n\
             url = \"https://example.com/v1\"\n\
             api_key_env = \"CUSTOM_API_KEY\"\n\
             is_default = true\n",
        ),
    }
}

/// Update configuration values
#[allow(clippy::too_many_arguments)]
pub fn update(
    dir: &Path,
    scope: ConfigScope,
    executor: Option<&str>,
    model: Option<&str>,
    interval: Option<u64>,
    max_agents: Option<usize>,
    max_coordinators: Option<usize>,
    coordinator_interval: Option<u64>,
    poll_interval: Option<u64>,
    coordinator_executor: Option<&str>,
    coordinator_model: Option<&str>,
    coordinator_provider: Option<&str>,
    auto_evaluate: Option<bool>,
    auto_assign: Option<bool>,
    assigner_agent: Option<&str>,
    evaluator_agent: Option<&str>,
    evolver_agent: Option<&str>,
    creator_agent: Option<&str>,
    retention_heuristics: Option<&str>,
    auto_triage: Option<bool>,
    auto_place: Option<bool>,
    auto_create: Option<bool>,
    triage_timeout: Option<u64>,
    triage_max_log_bytes: Option<usize>,
    max_child_tasks: Option<u32>,
    max_task_depth: Option<u32>,
    viz_edge_color: Option<&str>,
    eval_gate_threshold: Option<f64>,
    eval_gate_all: Option<bool>,
    flip_enabled: Option<bool>,
    flip_verification_threshold: Option<f64>,
    chat_history: Option<bool>,
    chat_history_max: Option<usize>,
    tui_counters: Option<&str>,
    retry_context_tokens: Option<u32>,
    endpoint: Option<&str>,
    tier_specs: &[String],
    set_models: &[String],
    set_providers: &[String],
    set_endpoints: &[String],
    role_models: &[String],
    role_providers: &[String],
    flip_inference_model: Option<&str>,
    flip_comparison_model: Option<&str>,
    flip_model: Option<&str>,
    no_reload: bool,
) -> Result<()> {
    update_with_reasoning(
        dir,
        scope,
        executor,
        model,
        None,
        interval,
        max_agents,
        max_coordinators,
        coordinator_interval,
        poll_interval,
        coordinator_executor,
        coordinator_model,
        coordinator_provider,
        auto_evaluate,
        auto_assign,
        assigner_agent,
        evaluator_agent,
        evolver_agent,
        creator_agent,
        retention_heuristics,
        auto_triage,
        auto_place,
        auto_create,
        triage_timeout,
        triage_max_log_bytes,
        max_child_tasks,
        max_task_depth,
        viz_edge_color,
        eval_gate_threshold,
        eval_gate_all,
        flip_enabled,
        flip_verification_threshold,
        chat_history,
        chat_history_max,
        tui_counters,
        retry_context_tokens,
        endpoint,
        tier_specs,
        set_models,
        &[],
        set_providers,
        set_endpoints,
        role_models,
        role_providers,
        flip_inference_model,
        flip_comparison_model,
        flip_model,
        no_reload,
    )
}

/// Update configuration values, including structured reasoning routing.
#[allow(clippy::too_many_arguments)]
pub fn update_with_reasoning(
    dir: &Path,
    scope: ConfigScope,
    executor: Option<&str>,
    model: Option<&str>,
    reasoning: Option<&str>,
    interval: Option<u64>,
    max_agents: Option<usize>,
    max_coordinators: Option<usize>,
    coordinator_interval: Option<u64>,
    poll_interval: Option<u64>,
    coordinator_executor: Option<&str>,
    coordinator_model: Option<&str>,
    coordinator_provider: Option<&str>,
    auto_evaluate: Option<bool>,
    auto_assign: Option<bool>,
    assigner_agent: Option<&str>,
    evaluator_agent: Option<&str>,
    evolver_agent: Option<&str>,
    creator_agent: Option<&str>,
    retention_heuristics: Option<&str>,
    auto_triage: Option<bool>,
    auto_place: Option<bool>,
    auto_create: Option<bool>,
    triage_timeout: Option<u64>,
    triage_max_log_bytes: Option<usize>,
    max_child_tasks: Option<u32>,
    max_task_depth: Option<u32>,
    viz_edge_color: Option<&str>,
    eval_gate_threshold: Option<f64>,
    eval_gate_all: Option<bool>,
    flip_enabled: Option<bool>,
    flip_verification_threshold: Option<f64>,
    chat_history: Option<bool>,
    chat_history_max: Option<usize>,
    tui_counters: Option<&str>,
    retry_context_tokens: Option<u32>,
    endpoint: Option<&str>,
    tier_specs: &[String],
    set_models: &[String],
    set_reasoning: &[String],
    set_providers: &[String],
    set_endpoints: &[String],
    role_models: &[String],
    role_providers: &[String],
    flip_inference_model: Option<&str>,
    flip_comparison_model: Option<&str>,
    flip_model: Option<&str>,
    no_reload: bool,
) -> Result<()> {
    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(dir)?,
    };
    let mut changed = false;

    if executor.is_some()
        || coordinator_executor.is_some()
        || coordinator_provider.is_some()
        || endpoint.is_some()
        || !set_providers.is_empty()
        || !set_endpoints.is_empty()
        || !role_providers.is_empty()
    {
        anyhow::bail!(
            "WG does not own provider/auth/endpoints. Configure those in Pi, or let the native Claude/Codex CLI own its login; set only explicit Pi/Claude/Codex routes and reasoning in WG."
        );
    }
    for route in model
        .into_iter()
        .chain(coordinator_model)
        .chain(flip_inference_model)
        .chain(flip_comparison_model)
        .chain(flip_model)
    {
        worksgood::config::parse_supported_execution_route(route)?;
    }
    for spec in tier_specs {
        let (_, route) = split_key_value(
            "--tier",
            spec,
            "<tier>=<pi:<provider>:<model>|claude:<native-model>|codex:<native-model>>",
        )?;
        worksgood::config::parse_supported_execution_route(route)?;
    }
    for pair in set_models.chunks(2) {
        if let Some(route) = pair.get(1) {
            worksgood::config::parse_supported_execution_route(route)?;
        }
    }
    for value in role_models {
        let (_, route) = split_key_value(
            "--role-model",
            value,
            "<role>=<pi:<provider>:<model>|claude:<native-model>|codex:<native-model>>",
        )?;
        worksgood::config::parse_supported_execution_route(route)?;
    }

    // Endpoint-driven update: shares semantics with `wg init -m/-e`.
    // Writes a default oai-compat endpoint entry + applies the `nex:`
    // prefix to the model name so the provider:model validator accepts
    // it on reload. Model-only sets flow through the existing validated
    // agent.model / dispatcher.model blocks further down (we re-check
    // here so the existing blocks don't double-apply when we already did).
    let endpoint_handled_model = if endpoint.is_some() {
        let summary = config.apply_model_endpoint(model, endpoint)?;
        for line in &summary {
            println!("Set {}", line);
        }
        if coordinator_executor.is_none()
            && let Some(dispatcher_model) = config.coordinator.model.clone()
            && let Some(summary) =
                clear_dispatcher_executor_for_model(&mut config, &dispatcher_model)
        {
            println!("{}", summary);
        }
        changed = true;
        true
    } else {
        false
    };

    // Agent settings
    if let Some(exec) = executor {
        eprintln!(
            "warning: `wg config --executor {0}` is deprecated; \
             pass a `provider:model` spec to `--model` instead \
             (e.g. `wg config --model claude:opus`). The handler is \
             derived from the model's provider prefix.",
            exec,
        );
        config.agent.executor = exec.to_string();
        println!("Set agent.executor = \"{}\"", exec);
        changed = true;
    }

    if let Some(m) = model
        && !endpoint_handled_model
    {
        // Validate provider:model format
        if let Err(e) = worksgood::config::parse_model_spec_strict(m) {
            anyhow::bail!(
                "Invalid model format: {}. Use provider:model format (e.g., 'claude:opus').",
                e
            );
        }
        // A top-level route selection establishes a complete, explicit Pi
        // plane. Strong and weak initially use the same exact identity; users
        // can then tune them independently with `wg profile pi`.
        config.set_pi_tiers(Some(m), Some(m));
        println!("Set explicit worker strong/weak routes = \"{}\"", m);
        if coordinator_model.is_none() {
            config.coordinator.provider = None;
            println!("Set dispatcher.model = \"{}\"", m);
            if coordinator_executor.is_none()
                && let Some(summary) = clear_dispatcher_executor_for_model(&mut config, m)
            {
                println!("{}", summary);
            }
        }
        changed = true;
    }

    if let Some(i) = interval {
        config.agent.interval = i;
        println!("Set agent.interval = {}", i);
        changed = true;
    }

    // Coordinator settings
    if let Some(max) = max_agents {
        config.coordinator.max_agents = max;
        println!("Set coordinator.max_agents = {}", max);
        changed = true;
    }

    if let Some(max) = max_coordinators {
        config.coordinator.max_coordinators = max;
        println!("Set coordinator.max_coordinators = {}", max);
        changed = true;
    }

    if let Some(i) = coordinator_interval {
        config.coordinator.interval = i;
        println!("Set coordinator.interval = {}", i);
        changed = true;
    }

    if let Some(i) = poll_interval {
        config.coordinator.poll_interval = i;
        println!("Set coordinator.poll_interval = {}", i);
        changed = true;
    }

    if let Some(exec) = coordinator_executor {
        eprintln!(
            "warning: `wg config --dispatcher-executor {0}` (and the legacy \
             `--coordinator-executor` alias) is deprecated; pass a \
             `provider:model` spec to `--model` / `--dispatcher-model` \
             instead (e.g. `wg config --model claude:opus`). The handler \
             is derived from the model's provider prefix.",
            exec,
        );
        config.coordinator.executor = Some(exec.to_string());
        println!("Set dispatcher.executor = \"{}\"", exec);
        changed = true;
    }

    if let Some(m) = coordinator_model {
        // Validate provider:model format
        if let Err(e) = worksgood::config::parse_model_spec_strict(m) {
            anyhow::bail!(
                "Invalid model format: {}. Use provider:model format (e.g., 'claude:opus').",
                e
            );
        }
        config.coordinator.model = Some(m.to_string());
        config.coordinator.provider = None; // Clear deprecated field
        println!("Set dispatcher.model = \"{}\"", m);
        if coordinator_executor.is_none()
            && let Some(summary) = clear_dispatcher_executor_for_model(&mut config, m)
        {
            println!("{}", summary);
        }
        changed = true;
    }

    if let Some(p) = coordinator_provider {
        let suggested_provider = if p == "anthropic" { "claude" } else { p };
        let current_model_raw = config
            .coordinator
            .model
            .as_deref()
            .unwrap_or(&config.agent.model);
        // Extract just the model ID (strip any existing provider prefix)
        let current_model_id = worksgood::config::parse_model_spec(current_model_raw).model_id;
        eprintln!(
            "Warning: --coordinator-provider is deprecated. Use provider:model format in --dispatcher-model instead.\n\
             Example: wg config --dispatcher-model {}:{}",
            suggested_provider, current_model_id,
        );
        config.coordinator.provider = Some(p.to_string());
        println!("Set coordinator.provider = \"{}\"", p);
        changed = true;
    }

    // Agency settings
    if let Some(v) = auto_evaluate {
        config.agency.auto_evaluate = v;
        println!("Set agency.auto_evaluate = {}", v);
        changed = true;
    }

    if let Some(v) = auto_assign {
        config.agency.auto_assign = v;
        println!("Set agency.auto_assign = {}", v);
        changed = true;
    }

    if let Some(v) = assigner_agent {
        config.agency.assigner_agent = Some(v.to_string());
        println!("Set agency.assigner_agent = \"{}\"", v);
        changed = true;
    }

    if let Some(v) = evaluator_agent {
        config.agency.evaluator_agent = Some(v.to_string());
        println!("Set agency.evaluator_agent = \"{}\"", v);
        changed = true;
    }

    if let Some(v) = evolver_agent {
        config.agency.evolver_agent = Some(v.to_string());
        println!("Set agency.evolver_agent = \"{}\"", v);
        changed = true;
    }

    if let Some(v) = creator_agent {
        config.agency.creator_agent = Some(v.to_string());
        println!("Set agency.creator_agent = \"{}\"", v);
        changed = true;
    }

    if let Some(v) = retention_heuristics {
        config.agency.retention_heuristics = Some(v.to_string());
        println!("Set agency.retention_heuristics = \"{}\"", v);
        changed = true;
    }

    if let Some(v) = auto_triage {
        config.agency.auto_triage = v;
        println!("Set agency.auto_triage = {}", v);
        changed = true;
    }

    if let Some(v) = auto_place {
        config.agency.auto_place = v;
        println!("Set agency.auto_place = {}", v);
        changed = true;
    }

    if let Some(v) = auto_create {
        config.agency.auto_create = v;
        println!("Set agency.auto_create = {}", v);
        changed = true;
    }

    if let Some(t) = triage_timeout {
        config.agency.triage_timeout = Some(t);
        println!("Set agency.triage_timeout = {}", t);
        changed = true;
    }

    if let Some(b) = triage_max_log_bytes {
        config.agency.triage_max_log_bytes = Some(b);
        println!("Set agency.triage_max_log_bytes = {}", b);
        changed = true;
    }

    // Guardrails settings
    if let Some(v) = max_child_tasks {
        config.guardrails.max_child_tasks_per_agent = v;
        println!("Set guardrails.max_child_tasks_per_agent = {}", v);
        changed = true;
    }

    if max_task_depth.is_some() {
        eprintln!(
            "warning: --max-task-depth is obsolete and ignored; task dependency depth is unlimited. \
             Use total-size/work budgets and archive completed history to manage large views."
        );
    }

    // Eval gate settings
    if let Some(threshold) = eval_gate_threshold {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            anyhow::bail!(
                "eval_gate_threshold must be finite and in [0.0, 1.0] range, got {}",
                threshold
            );
        }
        config.agency.eval_gate_threshold = Some(threshold);
        println!("Set agency.eval_gate_threshold = {}", threshold);
        changed = true;
    }

    if let Some(v) = eval_gate_all {
        config.agency.eval_gate_all = v;
        println!("Set agency.eval_gate_all = {}", v);
        changed = true;
    }

    // FLIP settings
    if let Some(v) = flip_enabled {
        config.agency.flip_enabled = v;
        println!("Set agency.flip_enabled = {}", v);
        changed = true;
    }

    if let Some(v) = flip_verification_threshold {
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            anyhow::bail!(
                "flip_verification_threshold must be finite and in [0.0, 1.0] range, got {}",
                v
            );
        }
        config.agency.flip_verification_threshold = Some(v);
        println!("Set agency.flip_verification_threshold = {}", v);
        changed = true;
    }

    // TUI chat history settings
    if let Some(v) = chat_history {
        config.tui.chat_history = v;
        println!("Set tui.chat_history = {}", v);
        changed = true;
    }

    if let Some(v) = chat_history_max {
        config.tui.chat_history_max = v;
        println!("Set tui.chat_history_max = {}", v);
        changed = true;
    }

    if let Some(counters) = tui_counters {
        let valid = ["uptime", "cumulative", "active", "session"];
        for part in counters.split(',') {
            let p = part.trim();
            if !p.is_empty() && !valid.contains(&p) {
                anyhow::bail!(
                    "Invalid counter '{}'. Valid: uptime, cumulative, active, session",
                    p
                );
            }
        }
        config.tui.counters = counters.to_string();
        println!("Set tui.counters = \"{}\"", counters);
        changed = true;
    }

    // Checkpoint settings
    if let Some(tokens) = retry_context_tokens {
        config.checkpoint.retry_context_tokens = tokens;
        println!("Set checkpoint.retry_context_tokens = {}", tokens);
        changed = true;
    }

    // Viz settings
    if let Some(color) = viz_edge_color {
        match color {
            "gray" | "white" | "mixed" => {
                config.viz.edge_color = color.to_string();
                println!("Set viz.edge_color = \"{}\"", color);
                changed = true;
            }
            _ => {
                anyhow::bail!(
                    "Invalid edge color '{}'. Valid options: gray, white, mixed",
                    color
                );
            }
        }
    }

    let registry_config = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
    changed |= apply_tier_updates(&mut config, &registry_config, tier_specs)?;
    if let Some(reasoning) = reasoning {
        let level = reasoning.parse::<ReasoningLevel>()?;
        config.models.set_reasoning(DispatchRole::Default, level);
        config.models.set_reasoning(DispatchRole::TaskAgent, level);
        println!("Set models.default.reasoning = \"{}\"", level);
        println!("Set models.task_agent.reasoning = \"{}\"", level);
        changed = true;
    }

    // Handle --flip-model / --flip-inference-model / --flip-comparison-model
    // before explicit role routing, preserving the old precedence where a
    // subsequent --set-model flip_* override wins over the shorthand.
    let flip_inf = flip_inference_model.or(flip_model);
    let flip_cmp = flip_comparison_model.or(flip_model);
    changed |= apply_flip_model_updates(&mut config, flip_inf, flip_cmp)?;
    changed |= apply_model_routing_updates(
        &mut config,
        &registry_config,
        set_models,
        set_reasoning,
        set_providers,
        set_endpoints,
        role_models,
        role_providers,
    )?;

    // Record executor/model config change in launcher history
    if coordinator_executor.is_some() || coordinator_model.is_some() || endpoint.is_some() {
        let mdl = coordinator_model
            .or(config.coordinator.model.as_deref())
            .or(model);
        let exec = coordinator_executor
            .or(config.coordinator.executor.as_deref())
            .map(std::string::ToString::to_string)
            .or_else(|| {
                mdl.and_then(|m| {
                    worksgood::config::parse_model_spec(m)
                        .provider
                        .as_deref()
                        .map(worksgood::config::provider_to_executor)
                        .map(std::string::ToString::to_string)
                })
            })
            .unwrap_or_else(|| config.agent.executor.clone());
        let ep = endpoint;
        let _ = worksgood::launcher_history::record_use(
            &worksgood::launcher_history::HistoryEntry::new(&exec, mdl, ep, "config"),
        );
    }

    let direct_global_routing_change = matches!(scope, ConfigScope::Global)
        && (model.is_some()
            || endpoint.is_some()
            || coordinator_model.is_some()
            || !tier_specs.is_empty()
            || !set_models.is_empty()
            || !set_reasoning.is_empty()
            || !set_providers.is_empty()
            || !set_endpoints.is_empty()
            || !role_models.is_empty()
            || !role_providers.is_empty()
            || flip_inference_model.is_some()
            || flip_comparison_model.is_some()
            || flip_model.is_some());
    let active_profile_to_clear = if direct_global_routing_change {
        worksgood::profile::named::active().unwrap_or(None)
    } else {
        None
    };

    if changed {
        // A managed evaluation rollout owns these safety flags. Validate
        // before backup/write so a direct config command cannot bypass the
        // ordered, evidence-gated controller.
        if matches!(scope, ConfigScope::Local) {
            worksgood::evaluation::rollout::validate_managed_config(dir, &config)?;
        }
        // Snapshot local config.toml before overwriting — only after all
        // validation has passed, so a failed `wg config` run doesn't leave
        // stray backup files behind.
        if matches!(scope, ConfigScope::Local)
            && let Some(backup) = Config::backup_on_disk(dir)?
        {
            println!("Backed up previous config → {}", backup.display());
        }
        match scope {
            ConfigScope::Global => {
                config.save_global()?;
                let path = Config::global_config_path()?;
                println!("Global configuration saved to {}", path.display());
                if let Some(prev) = active_profile_to_clear {
                    worksgood::profile::named::set_active(None)?;
                    println!(
                        "Active profile cleared (was: {}) because global model routing was edited directly.",
                        prev
                    );
                }
            }
            ConfigScope::Local => {
                config.save(dir)?;
                println!("Configuration saved.");
            }
        }
        let _ = worksgood::profile::project::record_successful_event(
            dir,
            worksgood::profile::project::UsageEventCategory::ConfigApplied,
        );

        // Auto-restart: model/endpoint changes don't propagate through a
        // soft Reconfigure IPC — already-running CoordinatorAgent
        // subprocesses hold their spawn-time env (endpoint, executor,
        // model). Full restart is the only reliable way to pick up a
        // new model/endpoint end-to-end. Skip with `--no-reload`.
        let wants_restart = !no_reload
            && matches!(scope, ConfigScope::Local)
            && (endpoint.is_some() || model.is_some() || coordinator_model.is_some());
        if wants_restart {
            match try_restart_daemon(dir) {
                Ok(true) => println!("Daemon restarted (dispatcher respawned with new config)."),
                Ok(false) => {} // no daemon running; nothing to do
                Err(e) => {
                    println!(
                        "Note: config saved but daemon restart failed ({}). Run `wg service restart` to retry.",
                        e
                    );
                }
            }
        }
        // Surface the resolved effective value + its source when a project
        // profile is active, so the user can see what actually took effect
        // (and from where) under the precedence rules in
        // docs/config-precedence.md. Routing keys (model/executor/provider)
        // stay profile-owned; non-routing knobs (max_agents/intervals) an
        // explicit write set stick. Avoids the "settings don't stick" pain.
        if matches!(scope, ConfigScope::Local)
            && worksgood::profile::project::read_association(dir)
                .ok()
                .flatten()
                .is_some()
        {
            println!(
                "  active project profile present: see `wg config --show` / `wg config get <key>` for the effective value + source."
            );
            println!("  (routing keys stay profile-owned; non-routing knobs set here stick.)");
        }
    } else {
        println!("No changes specified. Use --show to view current config.");
    }

    Ok(())
}

/// List merged configuration with source annotations
pub fn list(dir: &Path, json: bool) -> Result<()> {
    let (config, sources) = Config::load_with_sources(dir)?;

    if json {
        let merged_val = toml::Value::try_from(&config)?;
        let mut entries = Vec::new();
        collect_leaf_entries(&merged_val, "", &sources, &mut entries);
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        let merged_val = toml::Value::try_from(&config)?;
        let mut entries = Vec::new();
        collect_leaf_entries(&merged_val, "", &sources, &mut entries);

        println!("WG Configuration (merged)");
        println!("=================================");
        println!();
        for entry in &entries {
            let source = entry["source"].as_str().unwrap_or("default");
            let key = entry["key"].as_str().unwrap_or("");
            let value = &entry["value"];
            println!(
                "  {:40} = {:20} [{}]",
                key,
                format_toml_value(value),
                source
            );
        }
        println!();
        print_executor_choices_section();
    }

    Ok(())
}

/// Format a serde_json::Value for display
fn format_toml_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => format!("\"{}\"", s),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Recursively collect leaf entries from a TOML value for list output
fn collect_leaf_entries(
    val: &toml::Value,
    prefix: &str,
    sources: &std::collections::BTreeMap<String, ConfigSource>,
    entries: &mut Vec<serde_json::Value>,
) {
    if let toml::Value::Table(table) = val {
        for (key, v) in table {
            let full_key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };
            match v {
                toml::Value::Table(_) => {
                    collect_leaf_entries(v, &full_key, sources, entries);
                }
                _ => {
                    let source = sources
                        .get(&full_key)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "default".to_string());
                    let json_val = toml_value_to_json(v);
                    entries.push(serde_json::json!({
                        "key": full_key,
                        "value": json_val,
                        "source": source,
                    }));
                }
            }
        }
    }
}

/// Convert a toml::Value to serde_json::Value for serialization
fn toml_value_to_json(val: &toml::Value) -> serde_json::Value {
    match val {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t {
                map.insert(k.clone(), toml_value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
    }
}

/// Show Matrix configuration
pub fn show_matrix(json: bool) -> Result<()> {
    let config = MatrixConfig::load()?;
    let config_path = MatrixConfig::config_path()?;

    if json {
        // Mask password in JSON output
        let output = serde_json::json!({
            "config_path": config_path.display().to_string(),
            "homeserver_url": config.homeserver_url,
            "username": config.username,
            "password": config.password.as_ref().map(|_| "********"),
            "access_token": config.access_token.as_ref().map(|t| mask_token(t)),
            "default_room": config.default_room,
            "has_credentials": config.has_credentials(),
            "is_complete": config.is_complete(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Matrix Configuration");
        println!("====================");
        println!();
        println!("Config file: {}", config_path.display());
        if !config_path.exists() {
            println!("  (file does not exist yet)");
        }
        println!();

        if let Some(ref url) = config.homeserver_url {
            println!("  homeserver_url = \"{}\"", url);
        } else {
            println!("  homeserver_url = (not set)");
        }

        if let Some(ref user) = config.username {
            println!("  username = \"{}\"", user);
        } else {
            println!("  username = (not set)");
        }

        if config.password.is_some() {
            println!("  password = ********");
        } else {
            println!("  password = (not set)");
        }

        if let Some(ref token) = config.access_token {
            println!("  access_token = {}", mask_token(token));
        } else {
            println!("  access_token = (not set)");
        }

        if let Some(ref room) = config.default_room {
            println!("  default_room = \"{}\"", room);
        } else {
            println!("  default_room = (not set)");
        }

        println!();
        if config.is_complete() {
            println!("Status: Ready (credentials and room configured)");
        } else if config.has_credentials() {
            println!("Status: Credentials set, but no default room");
        } else {
            println!("Status: Not configured");
            println!();
            println!("To configure, use:");
            println!("  wg config --homeserver https://matrix.org \\");
            println!("            --username @user:matrix.org \\");
            println!("            --access-token <token> \\");
            println!("            --room '!roomid:matrix.org'");
        }
    }

    Ok(())
}

/// Update Matrix configuration
pub fn update_matrix(
    homeserver: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
    access_token: Option<&str>,
    room: Option<&str>,
) -> Result<()> {
    let mut config = MatrixConfig::load()?;
    let mut changed = false;

    if let Some(url) = homeserver {
        config.homeserver_url = Some(url.to_string());
        println!("Set homeserver_url = \"{}\"", url);
        changed = true;
    }

    if let Some(user) = username {
        config.username = Some(user.to_string());
        println!("Set username = \"{}\"", user);
        changed = true;
    }

    if let Some(pass) = password {
        config.password = Some(pass.to_string());
        println!("Set password = ********");
        changed = true;
    }

    if let Some(token) = access_token {
        config.access_token = Some(token.to_string());
        println!("Set access_token = {}", mask_token(token));
        changed = true;
    }

    if let Some(r) = room {
        config.default_room = Some(r.to_string());
        println!("Set default_room = \"{}\"", r);
        changed = true;
    }

    if changed {
        config.save()?;
        let config_path = MatrixConfig::config_path()?;
        println!();
        println!("Matrix configuration saved to {}", config_path.display());

        if config.is_complete() {
            println!("Status: Ready");
        } else if config.has_credentials() {
            println!("Status: Credentials set, but no default room configured");
        } else {
            println!("Status: Partially configured (missing credentials)");
        }
    } else {
        println!("No changes specified. Use --matrix to view Matrix config.");
    }

    Ok(())
}

/// Show the supported explicit worker role routing plane.
pub fn show_model_routing(dir: &Path, json: bool) -> Result<()> {
    use worksgood::config::DispatchRole;

    let config = Config::load_merged(dir)?;
    let mut roles = Vec::new();
    for role in std::iter::once(DispatchRole::Default).chain(DispatchRole::ALL.iter().copied()) {
        let resolved = config.resolve_execution_route_for_role(role)?;
        roles.push((role, resolved));
    }
    if json {
        let values = roles
            .into_iter()
            .map(|(role, route)| {
                (
                    role.to_string(),
                    serde_json::json!({
                        "route": route.route,
                        "handler": route.handler,
                        "provider": route.provider,
                        "model": route.model,
                        "reasoning": route.reasoning.as_str(),
                        "source": route.source,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        println!("Explicit Worker Model Plane (Pi recommended)");
        println!("============================================");
        println!(
            "  {:<18} {:<8} {:<48} {:<9} SOURCE",
            "ROLE", "HANDLER", "EXACT ROUTE", "REASON"
        );
        for (role, route) in roles {
            println!(
                "  {:<18} {:<8} {:<48} {:<9} {}",
                role,
                route.handler,
                route.route,
                route.reasoning.as_str(),
                route.source
            );
        }
        println!();
        println!(
            "Pi owns its provider authentication/discovery/endpoints; Codex CLI owns its login and native model IDs."
        );
        println!("WG owns only exact per-role execution routes and inherited reasoning.");
    }
    Ok(())
}

#[allow(dead_code)]
fn show_model_routing_legacy(dir: &Path, json: bool) -> Result<()> {
    use worksgood::config::DispatchRole;

    let config = Config::load_merged(dir)?;

    // Render a model route in its canonical **handler-first** form and the
    // handler it actually resolves to. A bare provider prefix
    // (`openrouter:z-ai/glm-5.2`) is shown as `nex:openrouter:z-ai/glm-5.2`,
    // and the HANDLER column echoes `handler_for_model` so a silent mis-route
    // (the 14h-401 incident) is visible at a glance rather than only when an
    // agent dies.
    let canonical = |model: &str| -> String {
        worksgood::config::handler_first_rewrite(model).unwrap_or_else(|| model.to_string())
    };
    let handler_of =
        |model: &str| -> &'static str { worksgood::dispatch::handler_for_model(model).as_str() };

    if json {
        let mut entries = serde_json::Map::new();
        let insert_role = |entries: &mut serde_json::Map<String, serde_json::Value>,
                           name: &str,
                           resolved: &worksgood::config::ResolvedModel,
                           tier: String,
                           source: &str| {
            // `model`/`provider` keep their original split representation (the
            // bare model id + resolved provider) for back-compat. `route` is
            // the full `provider:model` spec, `canonical` renders it
            // handler-first, and `handler` echoes the resolved handler so a
            // mis-route is machine-visible.
            let spec = resolved.spawn_model_spec();
            entries.insert(
                name.to_string(),
                serde_json::json!({
                    "model": resolved.model,
                    "route": spec,
                    "canonical": canonical(&spec),
                    "handler": handler_of(&spec),
                    "provider": resolved.provider,
                    "reasoning": resolved.reasoning.map(|r| r.as_str()),
                    "endpoint": resolved.endpoint,
                    "tier": tier,
                    "source": source,
                }),
            );
        };
        // Show default
        let resolved = config.resolve_model_for_role(DispatchRole::Default);
        let source = config.resolve_model_source(DispatchRole::Default);
        insert_role(
            &mut entries,
            "default",
            &resolved,
            DispatchRole::Default.default_tier().to_string(),
            source,
        );
        for role in DispatchRole::ALL {
            let resolved = config.resolve_model_for_role(*role);
            let source = config.resolve_model_source(*role);
            insert_role(
                &mut entries,
                &role.to_string(),
                &resolved,
                role.default_tier().to_string(),
                source,
            );
        }
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        println!("Model Routing Configuration");
        println!("===========================");
        println!();
        println!(
            "  {:<18} {:<9} {:<32} {:<9} {:<12} {:<9} {:<14} SOURCE",
            "ROLE", "TIER", "MODEL", "HANDLER", "PROVIDER", "REASON", "ENDPOINT"
        );
        println!("  {}", "-".repeat(122));

        let print_row =
            |role: &str, tier: &str, resolved: &worksgood::config::ResolvedModel, source: &str| {
                let provider_display = resolved
                    .provider
                    .as_deref()
                    .map(worksgood::config::native_provider_to_prefix)
                    .unwrap_or("(not set)");
                // `spawn_model_spec` reattaches the provider into a full
                // `provider:model` route; `canonical` then renders it
                // handler-first and `handler_of` resolves the real handler.
                let spec = resolved.spawn_model_spec();
                println!(
                    "  {:<18} {:<9} {:<32} {:<9} {:<12} {:<9} {:<14} {}",
                    role,
                    tier,
                    canonical(&spec),
                    handler_of(&spec),
                    provider_display,
                    resolved.reasoning.map(|r| r.as_str()).unwrap_or("(omit)"),
                    resolved.endpoint.as_deref().unwrap_or(""),
                    source,
                );
            };

        // Default
        let resolved = config.resolve_model_for_role(DispatchRole::Default);
        let source = config.resolve_model_source(DispatchRole::Default);
        print_row(
            "default",
            &DispatchRole::Default.default_tier().to_string(),
            &resolved,
            &source,
        );

        // Per-role
        for role in DispatchRole::ALL {
            let resolved = config.resolve_model_for_role(*role);
            let source = config.resolve_model_source(*role);
            print_row(
                &role.to_string(),
                &role.default_tier().to_string(),
                &resolved,
                &source,
            );
        }
        println!();
        println!("HANDLER is the subprocess that runs the model (handler_for_model): a bare");
        println!("provider prefix resolves to `native` — the canonical form is shown in MODEL.");
        println!("Sources: explicit = user-set model, tier-default = from default_tier(),");
        println!("         tier-override = from [models.role].tier, legacy = from agency.*_model,");
        println!("         fallback = from [models.default] or agent.model");
        println!();
        println!("Use --set-model <role> <model> to override a role.");
        println!("Use --set-reasoning <role> <level> to override structured reasoning.");
        println!("Use --set-provider <role> <provider> to set a provider.");
        println!("Use --set-endpoint <role> <endpoint-name> to bind an endpoint.");
    }

    Ok(())
}

fn split_key_value<'a>(flag: &str, value: &'a str, expected: &str) -> Result<(&'a str, &'a str)> {
    let parts: Vec<&str> = value.splitn(2, '=').collect();
    if parts.len() != 2 || parts[0].trim().is_empty() || parts[1].trim().is_empty() {
        anyhow::bail!("{} requires format {}, got \"{}\"", flag, expected, value);
    }
    Ok((parts[0].trim(), parts[1].trim()))
}

fn apply_tier_updates(
    config: &mut Config,
    registry_config: &Config,
    tier_specs: &[String],
) -> Result<bool> {
    let mut changed = false;

    for tier_spec in tier_specs {
        let (tier_name, model_id) = split_key_value("--tier", tier_spec, "<tier>=<model-id>")?;
        let _tier: Tier = tier_name.parse()?;

        if registry_config.registry_lookup(model_id).is_none() {
            eprintln!(
                "Warning: '{}' is not in the model registry. \
                 Tier will resolve to it as a bare model name.",
                model_id
            );
        }

        match tier_name {
            "fast" => config.tiers.fast = Some(model_id.to_string()),
            "standard" => config.tiers.standard = Some(model_id.to_string()),
            "premium" => config.tiers.premium = Some(model_id.to_string()),
            _ => unreachable!(), // already validated by Tier::from_str
        }

        println!("Set tiers.{} = \"{}\"", tier_name, model_id);
        changed = true;
    }

    Ok(changed)
}

fn apply_flip_model_updates(
    config: &mut Config,
    inference_model: Option<&str>,
    comparison_model: Option<&str>,
) -> Result<bool> {
    use worksgood::config::DispatchRole;

    let mut changed = false;

    if let Some(model) = inference_model {
        if let Err(e) = worksgood::config::parse_model_spec_strict(model) {
            anyhow::bail!(
                "Invalid model format for --flip-inference-model: {}. Use provider:model format (e.g., 'claude:opus').",
                e
            );
        }
        config.models.set_model(DispatchRole::FlipInference, model);
        println!("Set models.flip_inference.model = \"{}\"", model);
        let spec = worksgood::config::parse_model_spec(model);
        if let Some(ref provider) = spec.provider {
            config
                .models
                .set_provider(DispatchRole::FlipInference, provider);
        }
        changed = true;
    }

    if let Some(model) = comparison_model {
        if let Err(e) = worksgood::config::parse_model_spec_strict(model) {
            anyhow::bail!(
                "Invalid model format for --flip-comparison-model: {}. Use provider:model format (e.g., 'claude:haiku').",
                e
            );
        }
        config.models.set_model(DispatchRole::FlipComparison, model);
        println!("Set models.flip_comparison.model = \"{}\"", model);
        let spec = worksgood::config::parse_model_spec(model);
        if let Some(ref provider) = spec.provider {
            config
                .models
                .set_provider(DispatchRole::FlipComparison, provider);
        }
        changed = true;
    }

    Ok(changed)
}

fn apply_model_for_role(
    config: &mut Config,
    _registry_config: &Config,
    role_name: &str,
    model: &str,
) -> Result<()> {
    use worksgood::config::DispatchRole;

    let role: DispatchRole = role_name.parse()?;
    worksgood::config::parse_supported_execution_route(model)
        .map_err(|error| anyhow::anyhow!("Invalid worker route: {error}"))?;
    config.models.set_model(role, model);
    println!(
        "Set models.{}.model = \"{}\" (handler={})",
        role,
        model,
        worksgood::dispatch::handler_for_model(model).as_str()
    );
    Ok(())
}

fn apply_provider_for_role(config: &mut Config, role_name: &str, provider: &str) -> Result<()> {
    use worksgood::config::DispatchRole;

    let suggested_provider = if provider == "anthropic" {
        "claude"
    } else {
        provider
    };
    eprintln!(
        "Warning: --set-provider is deprecated. Use provider:model format in --set-model instead.\n\
         Example: wg config --set-model {} {}:MODEL",
        role_name, suggested_provider,
    );
    let role: DispatchRole = role_name.parse()?;
    config.models.set_provider(role, provider);
    println!("Set models.{}.provider = \"{}\"", role, provider);
    Ok(())
}

fn apply_endpoint_for_role(
    config: &mut Config,
    role_name: &str,
    endpoint_name: &str,
) -> Result<()> {
    use worksgood::config::DispatchRole;

    let role: DispatchRole = role_name.parse()?;
    if config.llm_endpoints.find_by_name(endpoint_name).is_none() {
        eprintln!(
            "Warning: endpoint '{}' is not configured. Add it with: wg endpoints add {}",
            endpoint_name, endpoint_name
        );
    }

    config.models.set_endpoint(role, endpoint_name);
    println!("Set models.{}.endpoint = \"{}\"", role, endpoint_name);
    Ok(())
}

fn apply_model_routing_updates(
    config: &mut Config,
    registry_config: &Config,
    set_models: &[String],
    set_reasoning: &[String],
    set_providers: &[String],
    set_endpoints: &[String],
    role_models: &[String],
    role_providers: &[String],
) -> Result<bool> {
    let mut changed = false;

    if !set_models.is_empty() {
        if set_models.len() % 2 != 0 {
            anyhow::bail!("--set-model requires pairs of arguments: <role> <model>");
        }
        for pair in set_models.chunks(2) {
            apply_model_for_role(config, registry_config, &pair[0], &pair[1])?;
            changed = true;
        }
    }

    if !set_reasoning.is_empty() {
        if set_reasoning.len() % 2 != 0 {
            anyhow::bail!("--set-reasoning requires pairs of arguments: <role> <level>");
        }
        for pair in set_reasoning.chunks(2) {
            let role: DispatchRole = pair[0].parse()?;
            let level: ReasoningLevel = pair[1].parse()?;
            config.models.set_reasoning(role, level);
            println!("Set models.{}.reasoning = \"{}\"", role, level);
            changed = true;
        }
    }

    if !role_models.is_empty() {
        for kv in role_models {
            let (role, model) = split_key_value("--role-model", kv, "<role>=<model>")?;
            apply_model_for_role(config, registry_config, role, model)?;
            changed = true;
        }
    }

    if !set_providers.is_empty() {
        if set_providers.len() % 2 != 0 {
            anyhow::bail!("--set-provider requires pairs of arguments: <role> <provider>");
        }
        for pair in set_providers.chunks(2) {
            apply_provider_for_role(config, &pair[0], &pair[1])?;
            changed = true;
        }
    }

    if !role_providers.is_empty() {
        for kv in role_providers {
            let (role, provider) = split_key_value("--role-provider", kv, "<role>=<provider>")?;
            apply_provider_for_role(config, role, provider)?;
            changed = true;
        }
    }

    if !set_endpoints.is_empty() {
        if set_endpoints.len() % 2 != 0 {
            anyhow::bail!("--set-endpoint requires pairs of arguments: <role> <endpoint-name>");
        }
        for pair in set_endpoints.chunks(2) {
            apply_endpoint_for_role(config, &pair[0], &pair[1])?;
            changed = true;
        }
    }

    Ok(changed)
}

/// Update model routing configuration (--set-model / --set-provider / --set-endpoint).
pub fn update_model_routing(
    dir: &Path,
    scope: ConfigScope,
    set_model: Option<&[String]>,
    set_provider: Option<&[String]>,
    set_endpoint: Option<&[String]>,
) -> Result<()> {
    if set_provider.is_some_and(|values| !values.is_empty())
        || set_endpoint.is_some_and(|values| !values.is_empty())
    {
        anyhow::bail!(
            "provider/endpoint routing belongs to Pi; WG accepts only exact Pi role routes"
        );
    }
    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(dir)?,
    };

    let registry_config = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
    let empty: &[String] = &[];
    let changed = apply_model_routing_updates(
        &mut config,
        &registry_config,
        set_model.unwrap_or(empty),
        empty,
        set_provider.unwrap_or(empty),
        set_endpoint.unwrap_or(empty),
        empty,
        empty,
    )?;

    if changed {
        match scope {
            ConfigScope::Global => {
                config.save_global()?;
                let path = Config::global_config_path()?;
                println!("Global configuration saved to {}", path.display());
            }
            ConfigScope::Local => {
                config.save(dir)?;
                println!("Configuration saved.");
            }
        }
    }

    Ok(())
}

/// Show all model registry entries (built-in + user-defined).
pub fn show_registry(dir: &Path, json: bool) -> Result<()> {
    let config = Config::load_merged(dir)?;
    let entries = config.effective_registry();

    if json {
        let val: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "provider": e.provider,
                    "model": e.model,
                    "tier": e.tier.to_string(),
                    "context_window": e.context_window,
                    "cost_per_input_mtok": e.cost_per_input_mtok,
                    "cost_per_output_mtok": e.cost_per_output_mtok,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No model registry entries.");
        return Ok(());
    }

    println!(
        "  {:<12} {:<12} {:<30} {:<10} COST (in/out per MTok)",
        "ID", "PROVIDER", "MODEL", "TIER"
    );
    println!("  {}", "-".repeat(85));

    for entry in &entries {
        let cost = if entry.cost_per_input_mtok > 0.0 || entry.cost_per_output_mtok > 0.0 {
            format!(
                "${:.2}/${:.2}",
                entry.cost_per_input_mtok, entry.cost_per_output_mtok
            )
        } else {
            "-".to_string()
        };
        println!(
            "  {:<12} {:<12} {:<30} {:<10} {}",
            entry.id, entry.provider, entry.model, entry.tier, cost,
        );
    }

    Ok(())
}

/// Add a new model entry to the registry.
#[allow(clippy::too_many_arguments)]
pub fn add_registry_entry(
    dir: &Path,
    scope: ConfigScope,
    id: &str,
    provider: &str,
    model: &str,
    tier: &str,
    endpoint: Option<&str>,
    context_window: Option<u64>,
    cost_input: Option<f64>,
    cost_output: Option<f64>,
) -> Result<()> {
    let tier: Tier = tier.parse()?;

    let entry = ModelRegistryEntry {
        id: id.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        tier,
        endpoint: endpoint.map(|s| s.to_string()),
        context_window: context_window.unwrap_or(0),
        cost_per_input_mtok: cost_input.unwrap_or(0.0),
        cost_per_output_mtok: cost_output.unwrap_or(0.0),
        ..Default::default()
    };

    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(dir)?,
    };

    // Check for duplicate ID and update if exists
    let existing_idx = config.model_registry.iter().position(|e| e.id == id);
    if let Some(idx) = existing_idx {
        config.model_registry[idx] = entry;
        println!("Updated registry entry: {}", id);
    } else {
        config.model_registry.push(entry);
        println!("Added registry entry: {}", id);
    }

    save_config(&config, dir, scope)?;

    println!("  {} / {} / {} (tier: {})", id, provider, model, tier);

    Ok(())
}

/// Remove a registry entry by ID. Warns about dependents unless --force is set.
pub fn remove_registry_entry(
    dir: &Path,
    scope: ConfigScope,
    id: &str,
    force: bool,
    json: bool,
) -> Result<()> {
    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(dir)?,
    };

    // Check if entry exists in user config
    let idx = config.model_registry.iter().position(|e| e.id == id);

    if idx.is_none() {
        // Check if it's a built-in
        let merged = Config::load_merged(dir)?;
        if merged.effective_registry().iter().any(|e| e.id == id) {
            anyhow::bail!(
                "'{}' is a built-in registry entry and cannot be removed.\n\
                 To override it, add a custom entry with the same ID using --registry-add.",
                id
            );
        }
        anyhow::bail!("Registry entry '{}' not found.", id);
    }

    // Check for dependents: tier defaults and role overrides
    let mut warnings = Vec::new();

    // Check tier defaults
    let tiers = &config.tiers;
    if tiers.fast.as_deref() == Some(id) {
        warnings.push(format!("tiers.fast = '{}'", id));
    }
    if tiers.standard.as_deref() == Some(id) {
        warnings.push(format!("tiers.standard = '{}'", id));
    }
    if tiers.premium.as_deref() == Some(id) {
        warnings.push(format!("tiers.premium = '{}'", id));
    }

    // Check role overrides (including default, which is excluded from ALL)
    use worksgood::config::DispatchRole;
    if let Some(ref default_cfg) = config.models.default
        && default_cfg.model.as_deref() == Some(id)
    {
        warnings.push(format!("[models.default].model = '{}'", id));
    }
    for role in DispatchRole::ALL {
        if let Some(role_cfg) = config.models.get_role(*role)
            && role_cfg.model.as_deref() == Some(id)
        {
            warnings.push(format!("[models.{}].model = '{}'", role, id));
        }
    }

    if !warnings.is_empty() && !force {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "error": "entry has dependents",
                    "id": id,
                    "dependents": warnings,
                })
            );
        } else {
            eprintln!("Cannot remove '{}': referenced by:", id);
            for w in &warnings {
                eprintln!("  - {}", w);
            }
            eprintln!();
            eprintln!("Use --force to remove anyway, or reassign the dependents first.");
        }
        std::process::exit(1);
    }

    config.model_registry.remove(idx.unwrap());

    save_config(&config, dir, scope)?;

    if !warnings.is_empty() {
        println!(
            "Removed registry entry '{}' (with {} dangling reference(s))",
            id,
            warnings.len()
        );
    } else {
        println!("Removed registry entry '{}'", id);
    }

    Ok(())
}

/// Show current tier→model assignments.
pub fn show_tiers(dir: &Path, json: bool) -> Result<()> {
    let config = Config::load_merged(dir)?;
    let tiers = config.effective_tiers_public();
    let registry = config.effective_registry();

    let resolve = |model_id: Option<&str>| -> String {
        match model_id {
            Some(id) => registry
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.model.clone())
                .unwrap_or_else(|| format!("{} (not in registry)", id)),
            None => "(unset)".to_string(),
        }
    };

    if json {
        let val = serde_json::json!({
            "fast": {
                "model_id": tiers.fast,
                "resolved_model": resolve(tiers.fast.as_deref()),
            },
            "standard": {
                "model_id": tiers.standard,
                "resolved_model": resolve(tiers.standard.as_deref()),
            },
            "premium": {
                "model_id": tiers.premium,
                "resolved_model": resolve(tiers.premium.as_deref()),
            },
        });
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    println!("  {:<12} {:<12} RESOLVED MODEL", "TIER", "MODEL ID");
    println!("  {}", "-".repeat(60));

    println!(
        "  {:<12} {:<12} {}",
        "fast",
        tiers.fast.as_deref().unwrap_or("(unset)"),
        resolve(tiers.fast.as_deref()),
    );
    println!(
        "  {:<12} {:<12} {}",
        "standard",
        tiers.standard.as_deref().unwrap_or("(unset)"),
        resolve(tiers.standard.as_deref()),
    );
    println!(
        "  {:<12} {:<12} {}",
        "premium",
        tiers.premium.as_deref().unwrap_or("(unset)"),
        resolve(tiers.premium.as_deref()),
    );

    Ok(())
}

/// Helper: save config to the appropriate location based on scope.
fn save_config(config: &Config, dir: &Path, scope: ConfigScope) -> Result<()> {
    match scope {
        ConfigScope::Global => config.save_global()?,
        ConfigScope::Local => config.save(dir)?,
    }
    Ok(())
}

/// Check OpenRouter API key validity and credit status
pub fn check_key(dir: &Path, json: bool) -> Result<()> {
    use worksgood::executor::native::openai_client::resolve_openai_api_key_from_dir;

    let key = match resolve_openai_api_key_from_dir(dir) {
        Ok(k) => k,
        Err(_) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"error": "No API key found. Run `wg endpoints add`, set OPENROUTER_API_KEY, or add [native_executor] api_key to config."})
                );
            } else {
                eprintln!("Error: No API key found.");
                eprintln!("Configure a key via:");
                eprintln!("  - wg endpoints add (recommended)");
                eprintln!("  - Set OPENROUTER_API_KEY or OPENAI_API_KEY environment variable");
                eprintln!("  - Add [native_executor] api_key to .wg/config.toml");
            }
            std::process::exit(1);
        }
    };

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("https://openrouter.ai/api/v1/key")
        .header("Authorization", format!("Bearer {}", key))
        .send();

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json()?;
            let data = body.get("data").unwrap_or(&body);

            if json {
                println!("{}", serde_json::to_string_pretty(data)?);
            } else {
                println!("OpenRouter API Key Status");
                println!("========================");
                println!();
                println!("  Key: {}", mask_token(&key));

                if let Some(limit) = data.get("limit") {
                    if limit.is_null() {
                        println!("  Credit limit: unlimited");
                    } else {
                        println!("  Credit limit: ${}", limit);
                    }
                }

                if let Some(remaining) = data.get("limit_remaining") {
                    if remaining.is_null() {
                        println!("  Remaining: unlimited");
                    } else {
                        println!("  Remaining: ${}", remaining);
                    }
                }

                if let Some(usage) = data.get("usage") {
                    println!("  Usage (all-time): ${}", usage);
                }

                if let Some(is_free) = data.get("is_free_tier") {
                    println!(
                        "  Tier: {}",
                        if is_free.as_bool().unwrap_or(false) {
                            "free"
                        } else {
                            "paid"
                        }
                    );
                }

                if let Some(daily) = data.get("usage_daily") {
                    println!("  Usage (today): ${}", daily);
                }

                println!();
                println!("Status: Valid");
            }
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().unwrap_or_default();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"error": format!("HTTP {}", status), "body": body})
                );
            } else {
                eprintln!("Error: API key check failed (HTTP {})", status);
                if !body.is_empty() {
                    eprintln!("  {}", body);
                }
            }
            std::process::exit(1);
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"error": format!("Request failed: {}", e)})
                );
            } else {
                eprintln!("Error: Could not reach OpenRouter API: {}", e);
            }
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Mask a token for display (show first and last 4 chars)
fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 12 {
        "********".to_string()
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }
}

/// Install the current project's config as the global default.
///
/// Copies `.wg/config.toml` → `~/.wg/config.toml`.
/// If the global config already exists and `--force` is not set, shows a diff
/// summary and asks for confirmation on stdin.
pub fn install_global(workgraph_dir: &Path, force: bool) -> Result<()> {
    let global_path = Config::global_config_path()?;
    let global_dir = Config::global_dir()?;
    install_global_to(workgraph_dir, &global_path, &global_dir, force)
}

/// Core logic for install-global, parameterized for testing.
pub fn install_global_to(
    workgraph_dir: &Path,
    global_path: &Path,
    global_dir: &Path,
    force: bool,
) -> Result<()> {
    let local_path = workgraph_dir.join("config.toml");
    if !local_path.exists() {
        anyhow::bail!(
            "No project config found at {}.\nRun `wg config --init` to create one first.",
            local_path.display()
        );
    }

    let local_content = std::fs::read_to_string(&local_path)?;

    if global_path.exists() && !force {
        let global_content = std::fs::read_to_string(global_path)?;
        if local_content == global_content {
            println!("Global config is already identical to project config — nothing to do.");
            return Ok(());
        }
        println!("Global config already exists at {}", global_path.display());
        println!();
        print_diff_summary(&global_content, &local_content);
        println!();
        eprint!("Overwrite global config? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Ensure parent directory exists
    std::fs::create_dir_all(global_dir)?;

    std::fs::copy(&local_path, global_path)?;
    println!("Installed project config as global default");
    println!("  {} → {}", local_path.display(), global_path.display());
    Ok(())
}

/// Reset config to the complete Pi route/reasoning defaults.
///
/// - `route` = `Some(name)` resets to that route's defaults.
/// - `route` = `None` picks the closest route based on the current
///   executor (e.g. `claude` → `claude-cli`).
/// - `keep_keys = true` preserves any existing `[[llm_endpoints.endpoints]]`
///   entries (their api_key / api_key_file / api_key_env are *not* lost).
/// - `dry_run = true` prints the diff but doesn't write.
/// - `yes = false` confirms before overwriting a non-empty config.
///
/// Always backs up `config.toml` to `config.toml.bak-<timestamp>` before
/// writing.
pub fn reset_to_route(
    workgraph_dir: &Path,
    scope: ConfigScope,
    route: Option<&str>,
    keep_keys: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    use worksgood::config_defaults::{RouteParams, SetupRoute, config_for_route};

    // Resolve the target path + load the existing config (if any).
    let (target_path, existing) = match scope {
        ConfigScope::Global => {
            let path = Config::global_config_path()?;
            let cfg = Config::load_global()?.unwrap_or_default();
            (path, cfg)
        }
        ConfigScope::Local => {
            let path = workgraph_dir.join("config.toml");
            let cfg = if path.exists() {
                Config::load(workgraph_dir)?
            } else {
                Config::default()
            };
            (path, cfg)
        }
    };

    // Pick a route: explicit > derived from existing executor.
    let resolved_route: SetupRoute = if let Some(name) = route {
        SetupRoute::from_name(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown/legacy route '{}'. The supported reset route is: pi",
                name,
            )
        })?
    } else {
        let exec = existing
            .coordinator
            .executor
            .as_deref()
            .unwrap_or(&existing.agent.executor);
        let r = SetupRoute::from_executor(exec);
        eprintln!(
            "No --route given; deriving from current executor ({}) → {}",
            exec,
            r.as_name()
        );
        r
    };

    // Build the new config from route defaults. Carry over endpoints if
    // --keep-keys was passed.
    let mut new_config = config_for_route(resolved_route, RouteParams::default());
    if keep_keys && !existing.llm_endpoints.endpoints.is_empty() {
        new_config.llm_endpoints = existing.llm_endpoints.clone();
        eprintln!(
            "Preserved {} existing [[llm_endpoints.endpoints]] entry/entries (--keep-keys).",
            existing.llm_endpoints.endpoints.len()
        );
    }

    // Diff preview.
    let old_toml = toml::to_string_pretty(&existing)
        .map_err(|e| anyhow::anyhow!("serialize old config: {}", e))?;
    let new_toml = toml::to_string_pretty(&new_config)
        .map_err(|e| anyhow::anyhow!("serialize new config: {}", e))?;

    if dry_run {
        println!(
            "# wg config reset --dry-run (route: {})",
            resolved_route.as_name()
        );
        println!("# Target: {}", target_path.display());
        if old_toml == new_toml {
            println!("# No changes.");
        } else {
            println!("# Diff:");
            print_diff_summary(&old_toml, &new_toml);
        }
        println!("---");
        println!("{}", new_toml);
        return Ok(());
    }

    // Confirmation gate (skipped if --yes or if the target file doesn't exist).
    if !yes && target_path.exists() && !old_toml.is_empty() {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Reset {} to '{}' route defaults? Existing config will be backed up.",
                target_path.display(),
                resolved_route.as_name(),
            ))
            .default(false)
            .interact()
            .unwrap_or(false);
        if !confirm {
            println!("Reset cancelled.");
            return Ok(());
        }
    }

    // Backup if a real file is on disk.
    if target_path.exists() {
        let backup = backup_config_file(&target_path)?;
        println!("Backed up existing config → {}", backup.display());
    }

    // Write the new config.
    match scope {
        ConfigScope::Global => new_config.save_global()?,
        ConfigScope::Local => new_config.save(workgraph_dir)?,
    }

    println!(
        "Reset {} to route '{}' (executor={}, tiers={}/{}/{})",
        target_path.display(),
        resolved_route.as_name(),
        resolved_route.executor(),
        new_config.tiers.fast.as_deref().unwrap_or("?"),
        new_config.tiers.standard.as_deref().unwrap_or("?"),
        new_config.tiers.premium.as_deref().unwrap_or("?"),
    );

    Ok(())
}

/// Copy a config file to a `.bak-<timestamp>` sibling. Returns the backup path.
fn backup_config_file(path: &Path) -> Result<std::path::PathBuf> {
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let backup = path.with_file_name(format!(
        "{}.bak-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.toml"),
        stamp,
    ));
    std::fs::copy(path, &backup)
        .map_err(|e| anyhow::anyhow!("Failed to back up {}: {}", path.display(), e))?;
    Ok(backup)
}

/// Print a brief summary of differences between two TOML config strings.
fn print_diff_summary(old: &str, new: &str) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed_keys: Vec<String> = Vec::new();

    // Simple line-by-line diff: collect changed lines
    let max_len = old_lines.len().max(new_lines.len());
    for i in 0..max_len {
        let ol = old_lines.get(i).copied().unwrap_or("");
        let nl = new_lines.get(i).copied().unwrap_or("");
        if ol != nl {
            if ol.is_empty() {
                added += 1;
            } else if nl.is_empty() {
                removed += 1;
            } else {
                // Try to extract key name from TOML line
                if let Some(key) = nl.split('=').next() {
                    let k = key.trim().to_string();
                    if !k.is_empty() && !k.starts_with('[') && !changed_keys.contains(&k) {
                        changed_keys.push(k);
                    }
                }
            }
        }
    }

    println!("Diff summary:");
    if !changed_keys.is_empty() {
        let display: Vec<&str> = changed_keys.iter().take(10).map(|s| s.as_str()).collect();
        println!("  Changed keys: {}", display.join(", "));
        if changed_keys.len() > 10 {
            println!("  ... and {} more", changed_keys.len() - 10);
        }
    }
    if added > 0 {
        println!("  +{} new lines", added);
    }
    if removed > 0 {
        println!("  -{} removed lines", removed);
    }
    if changed_keys.is_empty() && added == 0 && removed == 0 {
        println!("  (content differs but no key-level changes detected)");
    }
}

/// Single-key setter used by the TUI Settings tab.
///
/// Maps a dotted key (e.g. `agent.model`, `dispatcher.max_agents`) to the
/// matching `Config` field, validates where appropriate, and saves to the
/// requested scope. This is intentionally a thin dispatcher over the
/// existing `Config` struct — the canonical CLI setters in this file
/// (`update`, `set_key`) handle their own bespoke flows; this
/// helper covers the simple per-key edits the Settings tab issues.
pub fn set_setting_value(
    workgraph_dir: &Path,
    scope: ConfigScope,
    key: &str,
    value: &str,
) -> Result<()> {
    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(workgraph_dir)?,
    };

    apply_setting(&mut config, key, value)?;

    match scope {
        ConfigScope::Global => config.save_global()?,
        ConfigScope::Local => config.save(workgraph_dir)?,
    }
    Ok(())
}

fn apply_setting(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let v = value.trim();
    let parse_bool = |s: &str| -> Result<bool> {
        match s.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            other => anyhow::bail!("expected boolean (got '{}')", other),
        }
    };

    match key {
        "agent.model" => {
            worksgood::config::parse_model_spec_strict(v).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid model format: {}. Use provider:model (e.g. 'claude:opus').",
                    e
                )
            })?;
            config.agent.model = v.to_string();
        }
        "agent.executor" => {
            config.agent.executor = v.to_string();
        }
        "agent.heartbeat_timeout" => {
            config.agent.heartbeat_timeout = v
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("expected positive integer (minutes)"))?;
        }
        "agent.heartbeat_timeout_seconds" => {
            config.agent.heartbeat_timeout_seconds = Some(
                v.parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("expected positive integer (seconds)"))?,
            );
        }
        "agent.interval" => {
            config.agent.interval = v
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("expected positive integer (seconds)"))?;
        }
        "coordinator.registry_refresh_interval" => {
            config.coordinator.registry_refresh_interval = v.parse::<u64>().map_err(|_| {
                anyhow::anyhow!("expected non-negative integer (seconds; 0 disables refresh)")
            })?;
        }
        "coordinator.archive_retention_days" => {
            config.coordinator.archive_retention_days = v.parse::<u64>().map_err(|_| {
                anyhow::anyhow!("expected non-negative integer (days; 0 disables archival)")
            })?;
        }
        "coordinator.worktree_isolation" => {
            config.coordinator.worktree_isolation = parse_bool(v)?;
        }
        "coordinator.verify_mode" => {
            config.coordinator.verify_mode = v.to_string();
        }
        "coordinator.max_agents" | "dispatcher.max_agents" => {
            config.coordinator.max_agents = v
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("expected positive integer"))?;
        }
        "coordinator.max_coordinators" | "dispatcher.max_coordinators" => {
            config.coordinator.max_coordinators = v
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("expected positive integer"))?;
        }
        "coordinator.poll_interval" | "dispatcher.poll_interval" => {
            config.coordinator.poll_interval = v
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("expected positive integer (seconds)"))?;
        }
        "coordinator.interval" | "dispatcher.interval" => {
            config.coordinator.interval = v
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("expected positive integer (seconds)"))?;
        }
        "coordinator.coordinator_agent" | "dispatcher.coordinator_agent" => {
            config.coordinator.coordinator_agent = parse_bool(v)?;
        }
        "coordinator.model" | "dispatcher.model" => {
            worksgood::config::parse_model_spec_strict(v).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid model format: {}. Use provider:model (e.g. 'claude:opus').",
                    e
                )
            })?;
            config.coordinator.model = Some(v.to_string());
        }
        "coordinator.executor" | "dispatcher.executor" => {
            config.coordinator.executor = Some(v.to_string());
        }
        "coordinator.agent_timeout" => {
            config.coordinator.agent_timeout = v.to_string();
        }
        "agency.auto_evaluate" => {
            config.agency.auto_evaluate = parse_bool(v)?;
        }
        "agency.auto_assign" => {
            config.agency.auto_assign = parse_bool(v)?;
        }
        "agency.auto_triage" => {
            config.agency.auto_triage = parse_bool(v)?;
        }
        "agency.auto_create" => {
            config.agency.auto_create = parse_bool(v)?;
        }
        "agency.inference_timeout" => {
            config.agency.inference_timeout = Some(
                v.parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("expected positive integer (seconds)"))?,
            );
        }
        "tiers.fast" => {
            worksgood::config::parse_model_spec_strict(v)
                .map_err(|e| anyhow::anyhow!("Invalid model format: {}", e))?;
            config.tiers.fast = Some(v.to_string());
        }
        "tiers.fast_reasoning" => {
            config.tiers.fast_reasoning = Some(v.parse::<ReasoningLevel>()?);
        }
        "tiers.standard" => {
            worksgood::config::parse_model_spec_strict(v)
                .map_err(|e| anyhow::anyhow!("Invalid model format: {}", e))?;
            config.tiers.standard = Some(v.to_string());
        }
        "tiers.standard_reasoning" => {
            config.tiers.standard_reasoning = Some(v.parse::<ReasoningLevel>()?);
        }
        "tiers.premium" => {
            worksgood::config::parse_model_spec_strict(v)
                .map_err(|e| anyhow::anyhow!("Invalid model format: {}", e))?;
            config.tiers.premium = Some(v.to_string());
        }
        "tiers.premium_reasoning" => {
            config.tiers.premium_reasoning = Some(v.parse::<ReasoningLevel>()?);
        }
        other => anyhow::bail!(
            "set_setting_value: unsupported key '{}'. Add a match arm in apply_setting() if this is a valid Config field.",
            other
        ),
    }
    Ok(())
}

/// Keys whose value must be a valid handler-first model spec (`provider:model`);
/// `set_dotted` validates these before writing so a typo is caught at the CLI
/// instead of wedging dispatch on the next reload.
const DOTTED_MODEL_SPEC_KEYS: &[&str] = &[
    "agent.model",
    "dispatcher.model",
    "tiers.fast",
    "tiers.standard",
    "tiers.premium",
];

/// Parse a CLI value string into a typed `toml::Value` using simple inference:
/// `true`/`false` → bool, `123` → integer, `1.5` → float, anything else → string.
/// (Array/table values are not supported by the generic setter; use the
/// dedicated `--registry-add` / `--tier` paths for those.)
fn infer_toml_scalar(value: &str) -> toml::Value {
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "true" => return toml::Value::Boolean(true),
        "false" => return toml::Value::Boolean(false),
        _ => {}
    }
    if let Ok(i) = v.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = v.parse::<f64>() {
        return toml::Value::Float(f);
    }
    toml::Value::String(value.to_string())
}

/// Set a dotted TOML key on the chosen scope's config file. Known typed keys
/// are validated (model specs, bool/int fields); unknown paths are written as
/// raw TOML so EVERY knob is reachable without hand-editing files. The file is
/// edited as a TOML tree (not via `Config::save`) so unrelated keys, comments,
/// and unknown sections are preserved. Reloads the running daemon and prints
/// the resolved effective value + its source.
pub fn set_dotted(
    workgraph_dir: &Path,
    scope: ConfigScope,
    key: &str,
    value: &str,
    no_reload: bool,
    json: bool,
) -> Result<()> {
    if key.trim().is_empty() {
        anyhow::bail!("config set: <key> must not be empty");
    }
    let normalized_key = normalize_dotted_key(key);
    if worksgood::evaluation::rollout::evidence_path(workgraph_dir).exists()
        && matches!(
            normalized_key.as_str(),
            "agency.auto_evaluate"
                | "agency.eval_gate_all"
                | "agency.eval_gate_threshold"
                | "agency.flip_enabled"
                | "evaluation.managed_rollout"
                | "evaluation.rollout_stage"
        )
    {
        anyhow::bail!(
            "managed evaluation rollout owns '{}'; use `wg evaluate rollout advance` or `rollback`",
            normalized_key
        );
    }

    // 1. Validate known typed keys up front so a bad value never reaches disk.
    let typed_value = infer_toml_scalar(value);
    validate_dotted_value(&normalized_key, value, &typed_value)?;

    // 2. Load the scope file as a raw TOML tree (preserves everything).
    let path = scope_config_path(workgraph_dir, scope)?;
    let mut doc = Config::load_toml_value(&path)?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!("Failed to create config dir {}: {}", parent.display(), e)
            })?;
        }
    }

    // 3. Apply the dotted key to the tree, creating intermediate tables.
    set_dotted_value(&mut doc, &normalized_key, typed_value.clone());

    // 4. Validate the whole document still deserializes (catches type errors
    //    on known fields, e.g. setting max_agents to a non-integer).
    let scope_label = scope_label(scope);
    if let Err(e) = doc.clone().try_into::<Config>() {
        anyhow::bail!(
            "config set: writing '{}' = '{}' would make the {} config invalid: {}",
            normalized_key,
            value,
            scope_label,
            e
        );
    }

    // 5. Write back atomically.
    let body = toml::to_string_pretty(&doc)
        .map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))?;
    worksgood::atomic_file::write_atomic(&path, body.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;

    // 6. Reload the daemon (soft Reconfigure re-reads config.toml) unless
    //    the change needs a full restart (model/endpoint edits respawn the
    //    coordinator). `--no-reload` skips both.
    let restart = !no_reload && needs_restart(&normalized_key) && scope == ConfigScope::Local;
    let soft_reload = !no_reload && !restart;
    let reload_note = reload_after_write(workgraph_dir, restart, soft_reload)?;

    // 7. Print the resolved effective value + source (from the merged config).
    print_effective_value(
        workgraph_dir,
        &normalized_key,
        value,
        &reload_note,
        json,
        true,
    )?;
    Ok(())
}

/// Read the effective value of a dotted key from the merged config and print
/// it with the winning source (global / local / project-profile / default).
pub fn get_dotted(workgraph_dir: &Path, key: &str, json: bool) -> Result<()> {
    let normalized_key = normalize_dotted_key(key);
    let (config, sources) = Config::load_with_sources(workgraph_dir)?;
    if normalized_key == "dispatcher.resource_management.max_build_agents"
        && config
            .coordinator
            .resource_management
            .max_build_agents
            .is_none()
    {
        let value = config.coordinator.effective_max_build_agents();
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "key": normalized_key,
                    "value": value,
                    "source": "inherited-from-max-agents",
                    "inherited_from": "dispatcher.max_agents",
                }))?
            );
        } else {
            println!("{} = {}", normalized_key, value);
            println!("  [source: inherited from dispatcher.max_agents]");
        }
        return Ok(());
    }
    let merged_val = toml::Value::try_from(&config)?;
    let source = sources
        .get(normalized_key.as_str())
        .copied()
        .unwrap_or(ConfigSource::Default);
    let Some(leaf) = lookup_dotted(&merged_val, &normalized_key) else {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "key": normalized_key,
                    "value": null,
                    "source": "unset",
                    "note": "key is not present in any config layer"
                }))?
            );
        } else {
            println!("{} = <unset>", normalized_key);
            println!("  (key is not present in any config layer)");
        }
        return Ok(());
    };
    let json_val = toml_value_to_json(&leaf);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "key": normalized_key,
                "value": json_val,
                "source": source.to_string(),
            }))?
        );
    } else {
        println!("{} = {}", normalized_key, format_toml_value(&json_val));
        println!("  [source: {}]", source);
    }
    Ok(())
}

/// The config file path for a given scope.
fn scope_config_path(workgraph_dir: &Path, scope: ConfigScope) -> Result<std::path::PathBuf> {
    Ok(match scope {
        ConfigScope::Global => Config::global_config_path()?,
        ConfigScope::Local => workgraph_dir.join("config.toml"),
    })
}

fn scope_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Local => "local",
    }
}

/// Normalize a user key to the CANONICAL serde name. The dispatch table serializes
/// as `dispatcher` (`#[serde(rename = "dispatcher", alias = "coordinator")]`), so a
/// user-typed `coordinator.*` is rewritten to `dispatcher.*` for both the write
/// and the effective-value lookup. This keeps the written file lint-clean (no
/// `[coordinator]` deprecation warning on every load) and makes `get` find the
/// value regardless of which spelling the user typed.
fn normalize_dotted_key(key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() >= 2 && parts[0] == "coordinator" {
        let mut out = vec!["dispatcher"];
        out.extend_from_slice(&parts[1..]);
        out.join(".")
    } else {
        key.to_string()
    }
}

/// Validate the value for known typed keys (model specs, registry interval).
fn validate_dotted_value(key: &str, raw: &str, typed: &toml::Value) -> Result<()> {
    if DOTTED_MODEL_SPEC_KEYS.contains(&key) {
        worksgood::config::parse_model_spec_strict(raw).map_err(|e| {
            anyhow::anyhow!(
                "Invalid model spec for '{}': {}. Use provider:model (e.g. 'claude:opus').",
                key,
                e
            )
        })?;
        return Ok(());
    }
    // coordinator.registry_refresh_interval must be a non-negative integer.
    if matches!(key, "dispatcher.registry_refresh_interval") {
        match typed {
            toml::Value::Integer(i) if *i >= 0 => {}
            other => anyhow::bail!(
                "dispatcher.registry_refresh_interval expects a non-negative integer (seconds; 0 disables); got '{}'",
                other
            ),
        }
    }
    Ok(())
}

/// Set a dotted key (`table.sub.leaf`) on a TOML tree, creating intermediate
/// tables as needed. Replaces any existing value at the leaf.
fn set_dotted_value(doc: &mut toml::Value, dotted: &str, value: toml::Value) {
    let segments: Vec<&str> = dotted.split('.').collect();
    let (path_segs, leaf) = segments.split_at(segments.len() - 1);
    let leaf = leaf[0];
    let mut cursor = doc;
    for seg in path_segs {
        let entry = match cursor.as_table_mut() {
            Some(t) => t
                .entry(seg.to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new())),
            None => return,
        };
        cursor = entry;
    }
    if let Some(table) = cursor.as_table_mut() {
        table.insert(leaf.to_string(), value);
    }
}

/// Look up a dotted key on a TOML tree; returns `None` if any segment is absent
/// or not a table along the way.
fn lookup_dotted(doc: &toml::Value, dotted: &str) -> Option<toml::Value> {
    let segments: Vec<&str> = dotted.split('.').collect();
    let (path_segs, leaf) = segments.split_at(segments.len() - 1);
    let leaf = leaf[0];
    let mut cursor = doc;
    for seg in path_segs {
        cursor = cursor.as_table()?.get(*seg)?;
    }
    cursor.as_table()?.get(leaf).cloned()
}

/// Model/endpoint edits require a full daemon restart (running coordinator
/// subprocesses keep their spawn-time env). Everything else is picked up by a
/// soft Reconfigure (re-read config.toml).
fn needs_restart(key: &str) -> bool {
    DOTTED_MODEL_SPEC_KEYS.contains(&key)
        || key.starts_with("agent.model")
        || key == "agent.executor"
        || key == "coordinator.executor"
        || key == "dispatcher.executor"
}

/// Send a soft reload (Reconfigure with no flags → re-read config.toml) and/or
/// a full restart. Returns a human-readable note for the caller to print.
fn reload_after_write(workgraph_dir: &Path, restart: bool, soft_reload: bool) -> Result<String> {
    if restart {
        match try_restart_daemon(workgraph_dir) {
            Ok(true) => Ok("daemon restarted".to_string()),
            Ok(false) => Ok("no daemon running (config on disk for next start)".to_string()),
            Err(e) => Ok(format!("config saved but daemon restart failed ({})", e)),
        }
    } else if soft_reload {
        match crate::commands::service::run_reload(workgraph_dir, None, None, None, None, false) {
            Ok(()) => Ok("daemon reloaded".to_string()),
            Err(e) => Ok(format!("config saved but daemon reload failed ({})", e)),
        }
    } else {
        Ok("no reload (--no-reload)".to_string())
    }
}

/// Print the effective value of a key from the merged config plus its source,
/// so the user can see what actually took effect (and from where) after a write.
fn print_effective_value(
    workgraph_dir: &Path,
    key: &str,
    written_value: &str,
    reload_note: &str,
    json: bool,
    is_set: bool,
) -> Result<()> {
    let (config, sources) = Config::load_with_sources(workgraph_dir)?;
    let merged_val = toml::Value::try_from(&config)?;
    let source = sources.get(key).copied().unwrap_or(ConfigSource::Default);
    if json {
        let entry = match lookup_dotted(&merged_val, key) {
            Some(v) => serde_json::json!({
                "key": key,
                "written_value": written_value,
                "effective_value": toml_value_to_json(&v),
                "source": source.to_string(),
                "reload": reload_note,
            }),
            None => serde_json::json!({
                "key": key,
                "written_value": written_value,
                "effective_value": null,
                "source": "unset",
                "reload": reload_note,
                "note": "written but not present in merged config (may be a profile-owned routing key that was overlaid away; use `wg profile select`/`wg profile clear` to change routing under an active profile)",
            }),
        };
        println!("{}", serde_json::to_string_pretty(&entry)?);
        return Ok(());
    }
    let verb = if is_set { "Set" } else { "Effective" };
    match lookup_dotted(&merged_val, key) {
        Some(v) => {
            println!(
                "{} {} = {}  [source: {}]",
                verb,
                key,
                format_toml_value(&toml_value_to_json(&v)),
                source
            );
        }
        None => {
            println!(
                "{} {} = {} (written) but effective value is unset",
                verb, key, written_value
            );
            println!("  note: this key is profile-owned routing under the active project profile;");
            println!(
                "        use `wg profile select <name>` / `wg profile clear` to change routing."
            );
        }
    }
    if is_set {
        println!("  {}", reload_note);
    }
    Ok(())
}

/// Set an API key file reference for a provider's endpoint.
///
/// If an endpoint for the provider already exists, updates its `api_key_file`.
/// Otherwise, creates a new endpoint entry with the file reference.
pub fn set_key(
    workgraph_dir: &Path,
    scope: ConfigScope,
    provider: &str,
    file_path: &str,
) -> Result<()> {
    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(workgraph_dir)?,
    };

    // Find existing endpoint for provider, or create new one
    let mut found = false;
    for ep in &mut config.llm_endpoints.endpoints {
        if ep.provider == provider {
            ep.api_key_file = Some(file_path.to_string());
            ep.api_key = None; // Clear inline key when switching to file
            found = true;
            break;
        }
    }

    if !found {
        let is_first = config.llm_endpoints.endpoints.is_empty();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: provider.to_string(),
            provider: provider.to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(file_path.to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: is_first,
            context_window: None,
        });
    }

    match scope {
        ConfigScope::Global => config.save_global()?,
        ConfigScope::Local => config.save(workgraph_dir)?,
    }

    println!("Set API key file for '{}': {}", provider, file_path);
    Ok(())
}

// ---------------------------------------------------------------------------
// `wg config lint` — read-only companion to `wg migrate config`.
// ---------------------------------------------------------------------------

/// Which scope(s) `wg config lint` should walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintTarget {
    /// Only the global config (~/.wg/config.toml).
    Global,
    /// Only the local project config (.wg/config.toml).
    Local,
    /// Both global and local in sequence — the default.
    Merged,
}

/// Does this model spec route to `ExecutorKind::Pi`? `pi` is an executor name
/// (not a provider prefix), so a `pi:` prefixed spec maps to the pi handler.
fn spec_routes_to_pi(spec: &str) -> bool {
    spec.split_once(':')
        .map(|(prefix, _)| prefix)
        .and_then(worksgood::dispatch::ExecutorKind::from_str)
        == Some(worksgood::dispatch::ExecutorKind::Pi)
}

/// Whether the merged config pins a `pi:` route anywhere it matters (the agent
/// model, the coordinator model, or any `[models]` role). Best-effort scan of
/// the primary route fields — enough to decide whether the pi satisfiability
/// lint applies.
pub(crate) fn config_has_pi_route(config: &Config) -> bool {
    let mut specs: Vec<&str> = vec![config.agent.model.as_str()];
    if let Some(m) = config.coordinator.model.as_deref() {
        specs.push(m);
    }
    let mr = &config.models;
    for rc in [
        mr.default.as_ref(),
        mr.task_agent.as_ref(),
        mr.evaluator.as_ref(),
        mr.flip_inference.as_ref(),
        mr.flip_comparison.as_ref(),
        mr.assigner.as_ref(),
        mr.evolver.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(m) = rc.model.as_deref() {
            specs.push(m);
        }
    }
    specs.iter().any(|s| spec_routes_to_pi(s))
}

/// Pi-route satisfiability lint. A `pi:` route is satisfiable by EITHER a `pi`
/// binary (Topology A) OR Node + `wg-pi-host.mjs` + the built plugin bundle
/// (Topology B). When the config pins a `pi:` route but neither transport is
/// present, the route can never run — return a warning string
/// (`integration-plan-v2.md` §4). Pure over the injected availability so it is
/// unit-testable without the live environment.
fn pi_route_lint(
    config: &Config,
    avail: &worksgood::executor_discovery::PiRouteAvailability,
) -> Option<String> {
    if !config_has_pi_route(config) {
        return None;
    }
    if avail.satisfiable() {
        return None;
    }
    Some(
        "warning: a configured model route targets the `pi` executor, but neither a `pi` \
         binary nor the Node host bundle (node + wg-pi-host.mjs + pi-worksgood/index.js) is available \
         — this `pi:` route cannot run. Install pi (`pi`) or build pi-worksgood \
         (`npm --prefix worksgood-pi run build`), or repoint the route to an available executor."
            .to_string(),
    )
}

/// Walk the chosen config file(s) and report everything `wg migrate config`
/// would change, without rewriting. This is the "what's stale?" exploration
/// step before committing to a migration.
///
/// Implementation reuses `migrate::migrate_one(path, dry_run=true)` so the
/// predicates (deprecated keys, legacy renames, stale model strings) stay
/// in lockstep with `wg migrate config`.
pub fn lint_config(workgraph_dir: &Path, target: LintTarget, json: bool) -> Result<()> {
    use crate::commands::migrate::{ConfigMigrateResult, migrate_one};

    let global_path = Config::global_config_path()?;
    let local_path = workgraph_dir.join("config.toml");

    let mut results: Vec<ConfigMigrateResult> = Vec::new();
    match target {
        LintTarget::Global => {
            results.push(migrate_one(&global_path, true)?);
        }
        LintTarget::Local => {
            results.push(migrate_one(&local_path, true)?);
        }
        LintTarget::Merged => {
            results.push(migrate_one(&global_path, true)?);
            results.push(migrate_one(&local_path, true)?);
        }
    }

    // Pi-route satisfiability: an unrunnable `pi:` route is a config defect the
    // same way a stale key is, so surface it alongside the migrate findings.
    let merged = Config::load_or_default(workgraph_dir);
    let pi_warning = pi_route_lint(
        &merged,
        &worksgood::executor_discovery::pi_route_availability(),
    );
    let (execution_selection, selection_error) =
        match worksgood::execution_selection::resolve(workgraph_dir, None) {
            Ok(selection) => (selection, None),
            Err(error) => (
                worksgood::execution_selection::ExecutionSelection::unselected(),
                Some(format!("{error:#}")),
            ),
        };
    let pi_plane_error =
        if execution_selection.state == worksgood::execution_selection::SelectionState::Selected {
            merged
                .validate_execution_model_plane()
                .err()
                .map(|error| format!("{error:#}"))
        } else {
            None
        };
    let legacy_model_plane = (!merged.model_registry.is_empty()
        || !merged.llm_endpoints.endpoints.is_empty()
        || merged.openrouter.is_some())
        .then_some("legacy WG model/provider data is retained for migration only and has no Pi dispatch authority");
    let predictive_admission_enabled = merged.coordinator.resource_management.disk_sentinel_enabled;
    let predictive_admission_guidance = if predictive_admission_enabled {
        "advanced explicit opt-in: historical build high-water projections may intentionally defer launches; set dispatcher.resource_management.disk_sentinel_enabled = false (or remove the key) to use the availability-first default"
    } else {
        "disabled (default): dispatch/recovery is not blocked by hypothetical cold-build reservations; explicit cleanup and preservation safeguards remain available"
    };
    let max_build_agents = merged.coordinator.effective_max_build_agents();
    let max_build_agents_source = merged.coordinator.max_build_agents_source();
    let build_throttle_active =
        max_build_agents_source == "explicit" && max_build_agents < merged.coordinator.max_agents;
    let build_throttle_command = format!(
        "wg config set dispatcher.resource_management.max_build_agents {}",
        merged.coordinator.max_agents
    );

    if json {
        let payload = serde_json::json!({
            "execution_selection": execution_selection,
            "files": results
                .iter()
                .map(|r| serde_json::json!({
                    "path": r.path.display().to_string(),
                    "existed": r.existed,
                    "removed_keys": r.removed_keys,
                    "renamed_keys": r.renamed_keys,
                    "rewritten_values": r.rewritten_values,
                    "clean": r.is_noop(),
                }))
                .collect::<Vec<_>>(),
            "pi_route_warning": pi_warning,
            "pi_model_plane_error": pi_plane_error,
            "legacy_model_plane_warning": legacy_model_plane,
            "predictive_build_admission": {
                "enabled": predictive_admission_enabled,
                "mode": if predictive_admission_enabled { "advanced-opt-in" } else { "disabled-default" },
                "guidance": predictive_admission_guidance,
            },
            "build_heavy_capacity": {
                "max": max_build_agents,
                "source": max_build_agents_source,
                "throttle_active": build_throttle_active,
                "remediation_command": build_throttle_active.then_some(&build_throttle_command),
            },
            "selection_error": selection_error,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let mut total_findings = 0usize;
    println!("execution-selection:");
    match execution_selection.state {
        worksgood::execution_selection::SelectionState::Selected => {
            println!("  state: selected");
            println!(
                "  route: {}",
                execution_selection.route.as_deref().unwrap_or("?")
            );
            if let Some(system) = &execution_selection.system {
                println!("  system: ({}, {})", system.handler, system.wire);
            }
            println!("  source: {:?}", execution_selection.source);
        }
        worksgood::execution_selection::SelectionState::Unselected => {
            println!("  state: missing");
            println!("  No Pi model plane is selected.");
            println!("  Choose explicitly:");
            println!("    wg setup --route pi --yes --model pi:<provider>:<model>");
            println!("    wg profile select pi");
            println!("  No change was made automatically; graph access remains credential-free.");
            total_findings += 1;
        }
    }
    if let Some(error) = &selection_error {
        println!("  legacy route rejected: {error}");
        total_findings += 1;
    }
    println!();
    println!("predictive-build-admission:");
    println!(
        "  state: {}",
        if predictive_admission_enabled {
            "enabled (advanced explicit opt-in)"
        } else {
            "disabled (default)"
        }
    );
    println!("  {predictive_admission_guidance}");
    println!();
    println!("build-heavy-capacity:");
    println!("  max: {max_build_agents} ({max_build_agents_source})");
    if build_throttle_active {
        println!(
            "  warning: explicit build-heavy throttle is below dispatcher.max_agents={} (legacy generated value cannot be distinguished safely from operator intent)",
            merged.coordinator.max_agents
        );
        println!("  raise/remove throttle: {build_throttle_command}");
        total_findings += 1;
    }
    for r in &results {
        print_lint_one(r);
        total_findings += r.removed_keys.len() + r.renamed_keys.len() + r.rewritten_values.len();
    }

    if let Some(warning) = &pi_warning {
        println!();
        println!("{}", warning);
        total_findings += 1;
    }
    if let Some(error) = &pi_plane_error {
        println!();
        println!("error: {error}");
        total_findings += 1;
    }
    if let Some(warning) = legacy_model_plane {
        println!();
        println!("warning: {warning}");
        total_findings += 1;
    }

    if total_findings == 0 {
        // All inspected files were either missing or already canonical.
        println!();
        println!("All inspected files are clean — no migration needed.");
    } else {
        println!();
        println!(
            "Found {} issue{} that `wg migrate config` would fix.",
            total_findings,
            if total_findings == 1 { "" } else { "s" },
        );
        println!(
            "Run `wg migrate config --dry-run` to preview the rewrite, then \
             `wg migrate config` to apply.",
        );
    }
    Ok(())
}

fn print_lint_one(r: &crate::commands::migrate::ConfigMigrateResult) {
    if !r.existed {
        println!(
            "{}: file does not exist — nothing to lint",
            r.path.display(),
        );
        return;
    }
    if r.is_noop() {
        println!("{}: clean — no stale keys found", r.path.display());
        return;
    }
    println!("{}:", r.path.display());
    for k in &r.removed_keys {
        println!("  warning: deprecated key — would be removed: {}", k);
    }
    for (old, new) in &r.renamed_keys {
        println!(
            "  warning: legacy key — would be renamed: {} → {}",
            old, new,
        );
    }
    for (k, old, new) in &r.rewritten_values {
        println!(
            "  warning: stale value at {} — would be rewritten: {:?} → {:?}",
            k, old, new,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_and_show() {
        let temp_dir = TempDir::new().unwrap();

        // Init should create config
        let result = init(temp_dir.path(), None);
        assert!(result.is_ok());

        // Show should work (local scope)
        let result = show(temp_dir.path(), Some(ConfigScope::Local), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_pi_route_lint_rejects_when_neither_transport_present() {
        use worksgood::executor_discovery::{PiNodeHost, PiRouteAvailability};

        let mut config = Config::default();
        config.agent.model = "pi:openrouter/anthropic/claude-3.5-haiku".to_string();
        assert!(config_has_pi_route(&config));

        // Neither a pi binary nor a node host → the route can't run → warn.
        let none = PiRouteAvailability::default();
        assert!(
            pi_route_lint(&config, &none).is_some(),
            "unsatisfiable pi: route must produce a lint warning"
        );

        // A pi binary satisfies it (Topology A) → no warning.
        let with_pi = PiRouteAvailability {
            pi_binary: Some("/usr/bin/pi".into()),
            node_host: None,
        };
        assert!(pi_route_lint(&config, &with_pi).is_none());

        // The node host satisfies it (Topology B) → no warning.
        let with_host = PiRouteAvailability {
            pi_binary: None,
            node_host: Some(PiNodeHost {
                node: "/usr/bin/node".into(),
                host_script: "/p/host/wg-pi-host.mjs".into(),
                plugin_bundle: "/p/pi-worksgood/index.js".into(),
            }),
        };
        assert!(pi_route_lint(&config, &with_host).is_none());
    }

    #[test]
    fn test_pi_route_lint_silent_without_a_pi_route() {
        use worksgood::executor_discovery::PiRouteAvailability;
        // The default config pins no pi: route → no warning even when nothing
        // pi-related is installed.
        let config = Config::default();
        assert!(!config_has_pi_route(&config));
        assert!(pi_route_lint(&config, &PiRouteAvailability::default()).is_none());
    }

    #[test]
    fn test_spec_routes_to_pi() {
        assert!(spec_routes_to_pi(
            "pi:openrouter/anthropic/claude-3.5-haiku"
        ));
        assert!(spec_routes_to_pi("pi:anything"));
        assert!(!spec_routes_to_pi("claude:opus"));
        assert!(!spec_routes_to_pi("openrouter:minimax/minimax-m3"));
        assert!(!spec_routes_to_pi("opus"));
    }

    #[test]
    fn test_set_setting_value_writes_local_scope() {
        let temp = TempDir::new().unwrap();
        // Seed default config so set_setting_value has something to load.
        Config::default().save(temp.path()).unwrap();

        set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "agent.model",
            "claude:sonnet",
        )
        .expect("set_setting_value should succeed for a known key");

        let reloaded = Config::load(temp.path()).unwrap();
        assert_eq!(reloaded.agent.model, "claude:sonnet");
    }

    #[test]
    fn test_set_setting_value_validates_model_format() {
        let temp = TempDir::new().unwrap();
        Config::default().save(temp.path()).unwrap();

        // Missing provider prefix should be rejected by parse_model_spec_strict.
        let res = set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "agent.model",
            "claude_no_colon",
        );
        assert!(res.is_err(), "bare model name should be rejected");

        // Disk file unchanged.
        let reloaded = Config::load(temp.path()).unwrap();
        assert_eq!(reloaded.agent.model, Config::default().agent.model);
    }

    #[test]
    fn test_set_setting_value_parses_numeric_keys() {
        let temp = TempDir::new().unwrap();
        Config::default().save(temp.path()).unwrap();

        set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "coordinator.max_agents",
            "12",
        )
        .unwrap();

        let reloaded = Config::load(temp.path()).unwrap();
        assert_eq!(reloaded.coordinator.max_agents, 12);

        // Bogus integer is rejected.
        let res = set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "coordinator.max_agents",
            "abc",
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_set_setting_value_parses_bool_keys() {
        let temp = TempDir::new().unwrap();
        Config::default().save(temp.path()).unwrap();

        set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "agency.auto_evaluate",
            "false",
        )
        .unwrap();
        let reloaded = Config::load(temp.path()).unwrap();
        assert!(!reloaded.agency.auto_evaluate);

        set_setting_value(
            temp.path(),
            ConfigScope::Local,
            "agency.auto_evaluate",
            "true",
        )
        .unwrap();
        let reloaded = Config::load(temp.path()).unwrap();
        assert!(reloaded.agency.auto_evaluate);
    }

    #[test]
    fn test_set_setting_value_unknown_key_errors() {
        let temp = TempDir::new().unwrap();
        Config::default().save(temp.path()).unwrap();
        let res = set_setting_value(temp.path(), ConfigScope::Local, "totally.unknown.key", "x");
        assert!(res.is_err());
    }

    #[test]
    fn test_update_rejects_legacy_executor_and_non_pi_route() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        let result = update(
            temp_dir.path(),
            ConfigScope::Local,
            Some("opencode"),
            Some("openai:gpt-4"),
            Some(30),
            None, // max_agents
            None, // max_coordinators
            None,
            None,
            None,
            None, // coordinator_model
            None, // coordinator_provider
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // auto_triage
            None, // auto_place
            None, // auto_create
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // endpoint
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            false, // no_reload
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_update_coordinator_rejects_legacy_executor() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        let result = update(
            temp_dir.path(),
            ConfigScope::Local,
            None,
            None,
            None,
            Some(8), // max_agents
            None,    // max_coordinators
            Some(60),
            None,
            Some("shell"),
            None, // coordinator_model
            None, // coordinator_provider
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // auto_triage
            None, // auto_place
            None, // auto_create
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // endpoint
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            false, // no_reload
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_update_poll_interval() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        let result = update(
            temp_dir.path(),
            ConfigScope::Local,
            None,
            None,
            None,
            None, // max_agents
            None, // max_coordinators
            None,
            Some(120),
            None,
            None, // coordinator_model
            None, // coordinator_provider
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // auto_triage
            None, // auto_place
            None, // auto_create
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // endpoint
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            false, // no_reload
        );
        assert!(result.is_ok());

        let config = Config::load(temp_dir.path()).unwrap();
        assert_eq!(config.coordinator.poll_interval, 120);
    }

    #[test]
    fn test_update_agency() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        let result = update(
            temp_dir.path(),
            ConfigScope::Local,
            None,
            None,
            None,
            None, // max_agents
            None, // max_coordinators
            None,
            None,
            None,
            None, // coordinator_model
            None, // coordinator_provider
            Some(true),
            Some(true),
            Some("assigner-hash"),
            Some("evaluator-hash"),
            Some("evolver-hash"),
            Some("creator-hash"),
            Some("Retire below 0.3 after 10 evals"),
            None, // auto_triage
            None, // auto_place
            None, // auto_create
            None, // triage_timeout
            None, // triage_max_log_bytes
            None, // max_child_tasks
            None, // max_task_depth
            None, // viz_edge_color
            None, // eval_gate_threshold
            None, // eval_gate_all
            None, // flip_enabled
            None, // flip_verification_threshold
            None, // chat_history
            None, // chat_history_max
            None, // tui_counters
            None, // retry_context_tokens
            None, // endpoint
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            false, // no_reload
        );
        assert!(result.is_ok());

        let config = Config::load(temp_dir.path()).unwrap();
        assert!(config.agency.auto_evaluate);
        assert!(config.agency.auto_assign);
        assert_eq!(
            config.agency.assigner_agent,
            Some("assigner-hash".to_string())
        );
        assert_eq!(
            config.agency.evaluator_agent,
            Some("evaluator-hash".to_string())
        );
        assert_eq!(
            config.agency.evolver_agent,
            Some("evolver-hash".to_string())
        );
        assert_eq!(
            config.agency.creator_agent,
            Some("creator-hash".to_string())
        );
        assert_eq!(
            config.agency.retention_heuristics,
            Some("Retire below 0.3 after 10 evals".to_string())
        );
    }

    #[test]
    fn test_mask_token_short() {
        assert_eq!(mask_token("abc"), "********");
        assert_eq!(mask_token("123456789012"), "********");
    }

    #[test]
    fn test_mask_token_long() {
        assert_eq!(mask_token("abcdefghijklm"), "abcd...jklm");
    }

    #[test]
    fn test_mask_token_unicode_no_panic() {
        // Multi-byte chars should not panic
        assert_eq!(
            mask_token("🎯🎯🎯🎯🎯🎯🎯🎯🎯🎯🎯🎯🎯"),
            "🎯🎯🎯🎯...🎯🎯🎯🎯"
        );
    }

    #[test]
    fn test_show_merged() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        // Show with no scope = merged
        let result = show(temp_dir.path(), None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        // List should work and show source annotations
        let result = list(temp_dir.path(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_json() {
        let temp_dir = TempDir::new().unwrap();
        init(temp_dir.path(), None).unwrap();

        let result = list(temp_dir.path(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_show_merged_displays_effective() {
        // The merged-config display (rendered by `wg config --merged`) must
        // surface the effective endpoint state, including the inherit_global
        // flag. This is what users will check when asking "is openrouter
        // still being inherited from global?"
        use worksgood::config::{EndpointConfig, EndpointsConfig};

        // Effective config with NO endpoints and inherit_global=false (the
        // new default behavior — what the user wants).
        let mut config = Config::default();
        config.llm_endpoints = EndpointsConfig::default();
        let rendered = format_endpoints_section(&config);
        assert!(
            rendered.contains("[llm_endpoints]"),
            "merged display must include [llm_endpoints] header; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("inherit_global = false"),
            "merged display must show inherit_global flag; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("default — local endpoints fully replace global"),
            "merged display must explain the default behavior; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("(no endpoints configured)"),
            "empty endpoints must be visible to the user; got:\n{}",
            rendered
        );

        // Effective config WITH a local-only endpoint (the user's override).
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "claude-direct".to_string(),
            provider: "anthropic".to_string(),
            url: Some("https://api.anthropic.com/v1".to_string()),
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        });
        let rendered = format_endpoints_section(&config);
        assert!(
            rendered.contains("claude-direct"),
            "endpoint name must appear in merged display; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("is_default = true"),
            "is_default flag must appear in merged display; got:\n{}",
            rendered
        );
        assert!(
            !rendered.contains("openrouter"),
            "no global openrouter leakage in effective display; got:\n{}",
            rendered
        );

        // Flip inherit_global on and verify the explanatory text changes.
        config.llm_endpoints.inherit_global = true;
        let rendered = format_endpoints_section(&config);
        assert!(
            rendered.contains("inherit_global = true"),
            "inherit_global=true must be displayed; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("legacy cascade enabled"),
            "explanatory text for inherit_global=true must appear; got:\n{}",
            rendered
        );
    }

    #[test]
    fn test_config_install_global() {
        // Set up a project dir with a config
        let project_dir = TempDir::new().unwrap();
        init(project_dir.path(), None).unwrap();

        // Set up a separate "global" dir
        let global_dir = TempDir::new().unwrap();
        let global_path = global_dir.path().join("config.toml");

        // Install with --force (no global exists yet)
        let result = install_global_to(project_dir.path(), &global_path, global_dir.path(), true);
        assert!(result.is_ok());
        assert!(global_path.exists(), "Global config should be created");

        // Verify contents match
        let local_content =
            std::fs::read_to_string(project_dir.path().join("config.toml")).unwrap();
        let global_content = std::fs::read_to_string(&global_path).unwrap();
        assert_eq!(local_content, global_content);

        // Install again with --force should overwrite
        let result = install_global_to(project_dir.path(), &global_path, global_dir.path(), true);
        assert!(result.is_ok());

        // Without project config should fail
        let empty_dir = TempDir::new().unwrap();
        let result = install_global_to(empty_dir.path(), &global_path, global_dir.path(), true);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No project config"),
            "Should mention missing project config"
        );
    }

    #[test]
    fn test_config_install_global_creates_parent_dir() {
        let project_dir = TempDir::new().unwrap();
        init(project_dir.path(), None).unwrap();

        // Point to a nested global path that doesn't exist yet
        let global_base = TempDir::new().unwrap();
        let global_dir = global_base.path().join("nested").join(".wg");
        let global_path = global_dir.join("config.toml");

        let result = install_global_to(project_dir.path(), &global_path, &global_dir, true);
        assert!(result.is_ok());
        assert!(global_path.exists());
    }

    #[test]
    fn test_diff_summary() {
        // Just verify it doesn't panic
        print_diff_summary(
            "key1 = \"old\"\nkey2 = \"same\"\n",
            "key1 = \"new\"\nkey2 = \"same\"\nkey3 = \"added\"\n",
        );
    }
}
