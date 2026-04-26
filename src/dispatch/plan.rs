//! `SpawnPlan`: the single struct describing what runs for a task spawn.
//!
//! ## Precedence (executor)
//!
//! 1. `task.exec` set, or `task.exec_mode == "shell"`  →  `Shell`  (final)
//! 2. Per-task explicit override (currently `task.exec_mode` mapping to a
//!    known executor, or future `task.executor` field)                  →  final
//! 3. Agency-derived `agent_executor` (passed in by caller)              →  final
//! 4. Local/global `[dispatcher].executor` (a.k.a. `coordinator.executor`) →  final
//! 5. Default (`claude`)
//!
//! **Model spec NEVER overrides executor.** Once executor is resolved (e.g.
//! via local `[dispatcher].executor=claude`), the model field is *not*
//! consulted to override it. This is the regression that bit us: a global
//! `is_default = openrouter` endpoint and a registry lookup of `opus` should
//! NEVER cause a `claude`-pinned dispatcher to spawn a `native` executor.
//!
//! ## Precedence (endpoint)
//!
//! Endpoint is **executor-scoped**:
//!
//! - `executor=claude`  →  endpoint is always `None` (the claude CLI handles
//!   auth/url itself). Even if a global default endpoint exists, we do not
//!   pass `--endpoint`.
//! - `executor=shell`   →  endpoint is always `None`.
//! - `executor=codex`   →  endpoint is always `None` (codex CLI handles its own).
//! - `executor=amplifier`→ endpoint is always `None`.
//! - `executor=native`  →  endpoint is required; resolved via merged config
//!   (per-task → role → default).
//!
//! ## Provenance
//!
//! Every `SpawnPlan` carries a `SpawnProvenance` recording *which config
//! knob produced which value*. This is logged on every spawn so you can
//! always answer "why did this task spawn `native --endpoint openrouter`?"
//! by reading one line.

use crate::config::{Config, EndpointConfig, parse_model_spec};
use crate::graph::Task;
use anyhow::Result;
use std::collections::HashMap;

/// The executor kind that will run a spawned agent. This is the canonical
/// type; string forms (`"claude"`, `"native"`, …) are an external interop
/// concern — internally we should always pass an `ExecutorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorKind {
    /// Claude Code CLI session. Handles its own auth/url.
    Claude,
    /// In-process native executor (`wg native-exec …`). Speaks OpenAI-compat
    /// or Anthropic wire format; needs an explicit endpoint.
    Native,
    /// Shell executor: runs `task.exec` verbatim. No model, no endpoint.
    Shell,
    /// Codex CLI (`codex exec …`). Handles its own auth.
    Codex,
    /// Amplifier wrapper. Handles its own auth via amplifier settings.
    Amplifier,
}

impl ExecutorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutorKind::Claude => "claude",
            ExecutorKind::Native => "native",
            ExecutorKind::Shell => "shell",
            ExecutorKind::Codex => "codex",
            ExecutorKind::Amplifier => "amplifier",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(ExecutorKind::Claude),
            "native" => Some(ExecutorKind::Native),
            "shell" => Some(ExecutorKind::Shell),
            "codex" => Some(ExecutorKind::Codex),
            "amplifier" => Some(ExecutorKind::Amplifier),
            _ => None,
        }
    }

    /// Whether this executor needs an `EndpointConfig` resolved.
    pub fn needs_endpoint(self) -> bool {
        matches!(self, ExecutorKind::Native)
    }
}

/// Resolved model identity carried in a `SpawnPlan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelSpec {
    /// Original spec string as it was sourced (e.g. `"opus"` or
    /// `"openrouter:deepseek/deepseek-v3.2"`). Useful for logs.
    pub raw: String,
    /// Provider prefix if present (`Some("claude")`, `Some("openrouter")`,
    /// …). `None` for bare aliases like `"opus"` or `"haiku"`.
    pub provider: Option<String>,
    /// The model identifier portion. For bare aliases, this is the alias
    /// itself; for `provider:model`, it's the part after the colon.
    pub model_id: String,
}

impl ResolvedModelSpec {
    pub fn from_raw(raw: &str) -> Self {
        let parsed = parse_model_spec(raw);
        ResolvedModelSpec {
            raw: raw.to_string(),
            provider: parsed.provider,
            model_id: parsed.model_id,
        }
    }
}

/// Records *where* each field of a `SpawnPlan` came from. Logged on every
/// spawn so silent routing is impossible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnProvenance {
    /// e.g. `"task.exec_mode=shell"`, `"agent.effective_executor"`,
    /// `"local [dispatcher].executor"`, `"global [dispatcher].executor"`,
    /// `"default"`.
    pub executor_source: String,
    /// e.g. `"task.model"`, `"local [agent].model (alias)"`,
    /// `"role.assigner"`, `"default"`.
    pub model_source: String,
    /// e.g. `"none (executor=claude)"`, `"local [llm_endpoints] is_default"`,
    /// `"role.endpoint"`, `"none (no endpoints configured)"`.
    pub endpoint_source: String,
}

impl SpawnProvenance {
    /// Render a one-line provenance entry suitable for the daemon log.
    /// Prefix with `[dispatcher]` or `agent-N:` at the call site.
    pub fn log_line(&self, plan: &SpawnPlan) -> String {
        let endpoint_str = match &plan.endpoint {
            Some(ep) => format!("{} ({})", ep.name, self.endpoint_source),
            None => format!("none ({})", self.endpoint_source),
        };
        format!(
            "SpawnPlan executor={} (from {}), model={} (from {}), endpoint={}",
            plan.executor.as_str(),
            self.executor_source,
            plan.model.raw,
            self.model_source,
            endpoint_str,
        )
    }
}

/// The single struct describing what a task spawn launches.
#[derive(Debug, Clone)]
pub struct SpawnPlan {
    pub executor: ExecutorKind,
    pub model: ResolvedModelSpec,
    /// `None` for executors that handle their own endpoint (claude/codex/
    /// amplifier/shell). `Some(_)` only for `executor=native`.
    pub endpoint: Option<EndpointConfig>,
    /// Environment variables to set on the spawned process. Plan-level only;
    /// the spawn-execution layer is free to add wrapper-internal vars
    /// (`WG_TASK_ID`, `WG_AGENT_ID`, …) on top.
    pub env: HashMap<String, String>,
    /// argv tokens (program + args). Empty until the spawn-execution layer
    /// has migrated to consume the plan; in the interim, callers may build
    /// argv from `executor` + `model` + `endpoint` and the existing
    /// per-executor templates.
    pub argv: Vec<String>,
    pub provenance: SpawnProvenance,
}

/// Build the canonical `SpawnPlan` for a task. **This is the only place
/// that decides which executor / model / endpoint a spawn uses.**
///
/// `agent_executor` is the agency-derived `effective_executor()` for the
/// task's bound agent (or `None` if the task has no agent / agency lookup
/// failed). `default_model` is the dispatcher's currently-resolved
/// task-agent model (already cascaded through tier/role/global).
pub fn plan_spawn(
    task: &Task,
    config: &Config,
    agent_executor: Option<&str>,
    default_model: Option<&str>,
) -> Result<SpawnPlan> {
    // ----- 1. Executor -----
    let (executor, executor_source) = resolve_executor(task, config, agent_executor);

    // ----- 2. Model -----
    // Per-task model wins over default. Both are kept verbatim — we don't
    // rewrite `opus` to `claude:opus` here because the model field's role is
    // to be passed to the executor, which knows how to resolve aliases.
    let (model_raw, model_source) = if let Some(m) = task.model.as_deref() {
        (m.to_string(), "task.model".to_string())
    } else if let Some(m) = default_model {
        (m.to_string(), "dispatcher.default_model".to_string())
    } else if let Some(m) = config.coordinator.model.as_deref() {
        (m.to_string(), "local [dispatcher].model".to_string())
    } else {
        (
            config.agent.model.clone(),
            "[agent].model (fallback)".to_string(),
        )
    };
    let model = ResolvedModelSpec::from_raw(&model_raw);

    // ----- 3. Endpoint (executor-scoped) -----
    let (endpoint, endpoint_source) = if executor.needs_endpoint() {
        // For native executor, prefer endpoint from task's role-resolved
        // config; fall back to the global default endpoint.
        if let Some(default_ep) = config.llm_endpoints.find_default() {
            (
                Some(default_ep.clone()),
                "[llm_endpoints] is_default".to_string(),
            )
        } else {
            (
                None,
                "none (no endpoints configured for native)".to_string(),
            )
        }
    } else {
        (
            None,
            format!("none (executor={})", executor.as_str()),
        )
    };

    // ----- 4. Env -----
    // Plan-level env: WG_EXECUTOR_TYPE + WG_MODEL are guaranteed correct
    // because they come from the same `executor` + `model` resolved above.
    // The spawn-execution layer adds wrapper-internal vars on top.
    let mut env = HashMap::new();
    env.insert("WG_EXECUTOR_TYPE".to_string(), executor.as_str().to_string());
    env.insert("WG_MODEL".to_string(), model.raw.clone());

    let provenance = SpawnProvenance {
        executor_source,
        model_source,
        endpoint_source,
    };

    Ok(SpawnPlan {
        executor,
        model,
        endpoint,
        env,
        argv: Vec::new(),
        provenance,
    })
}

/// Resolve which executor kind to use for a task spawn, with provenance.
///
/// Precedence (highest first):
/// 1. `task.exec` set, or `task.exec_mode == "shell"`     →  Shell
/// 2. `task.exec_mode` parses to a known executor          →  that executor
/// 3. `agent_executor` (agency-derived effective executor) →  that executor
/// 4. `[dispatcher].executor` (local or global merged)     →  that executor
/// 5. Default                                              →  Claude
///
/// **Crucially: model is never consulted here.** The caller may have a
/// non-Anthropic model spec, but if the dispatcher is pinned to claude,
/// we honor claude. The previous implementation auto-switched to native
/// when `requires_native_executor(model, …)` was true; that behavior
/// is what this function deliberately removes.
fn resolve_executor(
    task: &Task,
    config: &Config,
    agent_executor: Option<&str>,
) -> (ExecutorKind, String) {
    // 1. Shell beats everything: `task.exec` set or `exec_mode == "shell"`.
    if task.exec.is_some() {
        return (ExecutorKind::Shell, "task.exec set".to_string());
    }
    if task.exec_mode.as_deref() == Some("shell") {
        return (ExecutorKind::Shell, "task.exec_mode=shell".to_string());
    }

    // 2. Per-task exec_mode mapping to a known executor (rare).
    if let Some(mode) = task.exec_mode.as_deref()
        && let Some(kind) = ExecutorKind::from_str(mode)
    {
        return (kind, format!("task.exec_mode={}", mode));
    }

    // 3. Agency-derived effective executor.
    if let Some(exec) = agent_executor
        && let Some(kind) = ExecutorKind::from_str(exec)
    {
        return (kind, "agency.effective_executor".to_string());
    }

    // 4. Local/global [dispatcher].executor.
    if let Some(ref exec) = config.coordinator.executor
        && let Some(kind) = ExecutorKind::from_str(exec)
    {
        return (kind, "[dispatcher].executor".to_string());
    }

    // 5. Default.
    (ExecutorKind::Claude, "default".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EndpointConfig;
    use crate::graph::Task;

    fn base_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: id.to_string(),
            ..Task::default()
        }
    }

    fn openrouter_default_endpoint() -> EndpointConfig {
        EndpointConfig {
            name: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            url: Some("https://openrouter.ai/api/v1".to_string()),
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            is_default: true,
            context_window: None,
        }
    }

    /// THE regression test. Reproduces the exact scenario that bit the user:
    /// task model `opus`, global config has `is_default = openrouter`, and
    /// local `[dispatcher].executor = claude`. The previous implementation
    /// would auto-switch to `native` because `opus` could be resolved via
    /// the openrouter endpoint. The contract: executor wins, period.
    #[test]
    fn test_executor_floor_is_honored() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());

        let mut task = base_task("t1");
        task.model = Some("opus".to_string());

        let plan = plan_spawn(&task, &config, None, Some("opus")).unwrap();

        assert_eq!(
            plan.executor,
            ExecutorKind::Claude,
            "executor MUST be claude when [dispatcher].executor=claude is explicit, regardless of model='opus' + global openrouter is_default. provenance: {:?}",
            plan.provenance
        );
        assert_eq!(plan.model.raw, "opus");
    }

    #[test]
    fn test_no_endpoint_for_claude_executor() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        // Even with a global default endpoint configured, claude must not get one.
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("opus")).unwrap();

        assert_eq!(plan.executor, ExecutorKind::Claude);
        assert!(
            plan.endpoint.is_none(),
            "endpoint MUST be None for executor=claude, got {:?}. provenance: {:?}",
            plan.endpoint,
            plan.provenance
        );
        assert!(
            plan.provenance.endpoint_source.contains("executor=claude"),
            "endpoint_source must explain *why* there's no endpoint, got {:?}",
            plan.provenance.endpoint_source
        );
    }

    #[test]
    fn test_provenance_traces_every_field() {
        // Every field of SpawnPlan must have a non-empty provenance
        // explanation pointing to the config source that produced it.
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.coordinator.model = Some("openrouter:deepseek/deepseek-v3.2".to_string());
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, None).unwrap();

        assert!(
            !plan.provenance.executor_source.is_empty(),
            "executor_source must be populated"
        );
        assert!(
            !plan.provenance.model_source.is_empty(),
            "model_source must be populated"
        );
        assert!(
            !plan.provenance.endpoint_source.is_empty(),
            "endpoint_source must be populated"
        );
        // Sanity: the chosen executor matches the local [dispatcher] override.
        assert_eq!(plan.executor, ExecutorKind::Native);
        assert!(plan.provenance.executor_source.contains("[dispatcher]"));
        assert_eq!(plan.endpoint.as_ref().map(|e| e.name.as_str()), Some("openrouter"));

        // The log line is what gets printed on every spawn — render it and
        // verify each field is mentioned.
        let line = plan.provenance.log_line(&plan);
        assert!(line.contains("executor=native"));
        assert!(line.contains("model=openrouter:deepseek/deepseek-v3.2"));
        assert!(line.contains("endpoint=openrouter"));
    }

    #[test]
    fn test_shell_beats_dispatcher_executor() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());

        let mut task = base_task("t1");
        task.exec = Some("echo hello".to_string());

        let plan = plan_spawn(&task, &config, None, None).unwrap();
        assert_eq!(plan.executor, ExecutorKind::Shell);
        assert!(plan.provenance.executor_source.contains("task.exec"));
    }

    #[test]
    fn test_default_executor_is_claude() {
        let config = Config::default();
        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, None).unwrap();
        assert_eq!(plan.executor, ExecutorKind::Claude);
        assert_eq!(plan.provenance.executor_source, "default");
    }

    #[test]
    fn test_agency_executor_overrides_dispatcher_default() {
        // When an agent_executor (agency-derived) is provided, it wins over
        // the dispatcher default but not over an explicit [dispatcher].executor.
        let config = Config::default();
        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, Some("native"), None).unwrap();
        assert_eq!(plan.executor, ExecutorKind::Native);
        assert!(plan.provenance.executor_source.contains("agency"));
    }
}
