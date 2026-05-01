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
//! - `executor=native`  →  endpoint is required; resolved via merged config
//!   (per-task → role → default).
//!
//! ## Provenance
//!
//! Every `SpawnPlan` carries a `SpawnProvenance` recording *which config
//! knob produced which value*. This is logged on every spawn so you can
//! always answer "why did this task spawn `native --endpoint openrouter`?"
//! by reading one line.

use crate::config::{Config, EndpointConfig, parse_model_spec, provider_to_executor};
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
}

impl ExecutorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutorKind::Claude => "claude",
            ExecutorKind::Native => "native",
            ExecutorKind::Shell => "shell",
            ExecutorKind::Codex => "codex",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(ExecutorKind::Claude),
            "native" => Some(ExecutorKind::Native),
            "shell" => Some(ExecutorKind::Shell),
            "codex" => Some(ExecutorKind::Codex),
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
    /// shell). `Some(_)` only for `executor=native`.
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

    // ----- 2b. Model-compat override -----
    // The claude CLI cannot run non-Anthropic models — it would 404. If we
    // ended up at executor=claude (whether via dispatcher.executor floor,
    // agency-derived choice, or default fall-through) but the model has a
    // non-Anthropic provider prefix (`local:`, `openrouter:`, `oai-compat:`,
    // `openai:`), switch to the executor the model actually requires.
    //
    // This is the autohaiku-regression fix, moved here from
    // `Agent::effective_executor_for_model` so it doesn't fire BEFORE the
    // dispatcher's explicit executor choice (which is the bug
    // `agency-still-picks` tracked: `wg init -x codex` was being silently
    // rewritten to native because the agency-level override sat in
    // resolve_executor's precedence step 3 and shadowed step 4).
    let (executor, executor_source) =
        enforce_model_compat(executor, executor_source, &model);

    // ----- 3. Endpoint (executor-scoped) -----
    //
    // Precedence (highest first):
    //   1. `task.endpoint` — set by `wg add -e <url|name>`, or by the IPC
    //      `CreateChat` handler from the user's launcher choice.
    //      - http(s):// URL  → synthesized inline `EndpointConfig` (matches
    //        the `-e http(s)://...` shortcut at provider.rs:208-230).
    //        The name carries the URL itself; spawn_task.rs:233 forwards
    //        `plan.endpoint.name` as the `-e` arg, so `wg nex` receives
    //        the literal URL and routes through the inline-URL path.
    //      - bare name      → looked up via `find_by_name`.
    //   2. Named endpoint matching `agent_executor`-derived role (TODO).
    //   3. Global `is_default` endpoint.
    //
    // Without precedence step 1, IPC-created chats with a custom endpoint
    // (e.g. TUI new-chat dialog with `-e https://lambda01...`) silently
    // dropped the URL and fell back to whatever endpoint was marked
    // `is_default` in config — talking to the wrong server.
    // (fix-nex-chat / Fix C — diagnose-wg-nex root cause #2.)
    let (endpoint, endpoint_source) = if executor.needs_endpoint() {
        if let Some(ep_str) = task.endpoint.as_deref() {
            if ep_str.starts_with("http://") || ep_str.starts_with("https://") {
                let synth = EndpointConfig {
                    name: ep_str.to_string(),
                    provider: "oai-compat".to_string(),
                    url: Some(ep_str.to_string()),
                    model: None,
                    api_key: None,
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                };
                (
                    Some(synth),
                    format!("task.endpoint (inline URL: {})", ep_str),
                )
            } else if let Some(ep) = config.llm_endpoints.find_by_name(ep_str) {
                (
                    Some(ep.clone()),
                    format!("task.endpoint (named: {})", ep_str),
                )
            } else if let Some(default_ep) = config.llm_endpoints.find_default() {
                (
                    Some(default_ep.clone()),
                    format!(
                        "[llm_endpoints] is_default (task.endpoint={:?} not found)",
                        ep_str
                    ),
                )
            } else {
                (
                    None,
                    format!(
                        "none (task.endpoint={:?} not found and no default)",
                        ep_str
                    ),
                )
            }
        } else if let Some(default_ep) = config.llm_endpoints.find_default() {
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

/// If the resolved executor is `claude` but the model spec carries a
/// non-Anthropic provider prefix, switch to the executor the model actually
/// needs. The claude CLI cannot speak OpenAI-compat / openrouter / local
/// endpoints — running it against `local:qwen3-coder` returns 404 and burns
/// the spawn (the autohaiku regression).
///
/// This override only fires when the resolved executor is `claude`. It does
/// NOT touch explicit non-claude executor choices (`-x codex`, `-x native`)
/// — those are kept even with `local:` models, on the assumption that the
/// chosen executor is OAI-compat-aware (codex, native) or the user knows
/// what they're doing.
///
/// Bare aliases like `opus` / `sonnet` (no provider prefix) do NOT trigger
/// the override — they're claude-compatible by convention.
fn enforce_model_compat(
    executor: ExecutorKind,
    executor_source: String,
    model: &ResolvedModelSpec,
) -> (ExecutorKind, String) {
    if !matches!(executor, ExecutorKind::Claude) {
        return (executor, executor_source);
    }
    let Some(ref provider) = model.provider else {
        return (executor, executor_source);
    };
    let required = provider_to_executor(provider);
    if required == "claude" {
        return (executor, executor_source);
    }
    let Some(kind) = ExecutorKind::from_str(required) else {
        return (executor, executor_source);
    };
    let new_source = format!(
        "model-compat override: was claude (from {}), model={} prefix={} requires {}",
        executor_source, model.raw, provider, required,
    );
    eprintln!(
        "[dispatch] model-compat: claude (from {}) cannot run model '{}' (prefix '{}'); routing to '{}'",
        executor_source, model.raw, provider, required,
    );
    (kind, new_source)
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
/// based on a model→provider lookup; that behavior is what this function
/// deliberately removes.
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
            api_key_ref: None,
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

    /// Regression: agency-still-picks. With `wg init -x codex -m qwen3-coder
    /// -e https://...`, the dispatcher's explicit `-x codex` MUST win, even
    /// though the model is `local:qwen3-coder`. The previous fix
    /// (agency-picks-claude) put a model-compat override INSIDE
    /// `Agent::effective_executor_for_model` that fired any time the agent's
    /// default-claude executor met a `local:` model — converting to "native"
    /// and (via resolve_executor's precedence) overriding the dispatcher's
    /// `-x codex` choice. This test pins down the correct behaviour: when
    /// dispatcher explicitly chose codex, codex wins.
    #[test]
    fn test_codex_executor_routes_codex_not_claude() {
        let mut config = Config::default();
        config.coordinator.executor = Some("codex".to_string());
        config.coordinator.model = Some("local:qwen3-coder".to_string());

        let task = base_task("t1");
        // agent_executor=None simulates an agent with no explicit choice
        // (default claude executor). The dispatcher's `-x codex` floor
        // must win.
        let plan = plan_spawn(&task, &config, None, Some("local:qwen3-coder")).unwrap();

        assert_eq!(
            plan.executor,
            ExecutorKind::Codex,
            "dispatcher.executor=codex MUST win when explicitly set, even with model={}. provenance: {:?}",
            plan.model.raw,
            plan.provenance
        );
    }

    /// Companion test: `wg init -x nex -m qwen3-coder` (canonicalised to
    /// `native`). Native executor with `local:` model is the entire reason
    /// you'd configure nex/native, so this is the must-not-break case.
    #[test]
    fn test_nex_executor_routes_native_not_claude() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.coordinator.model = Some("local:qwen3-coder".to_string());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("local:qwen3-coder")).unwrap();

        assert_eq!(
            plan.executor,
            ExecutorKind::Native,
            "dispatcher.executor=native (nex) MUST be honored. provenance: {:?}",
            plan.provenance
        );
    }

    /// The autohaiku regression case, now enforced at the dispatch layer
    /// rather than agency: dispatcher.executor=claude (explicit) +
    /// model=local:qwen3-coder MUST switch to native, because the claude
    /// CLI literally cannot run a local model and would 404.
    #[test]
    fn test_claude_executor_with_local_model_overrides_to_native() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.model = Some("local:qwen3-coder".to_string());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("local:qwen3-coder")).unwrap();

        assert_eq!(
            plan.executor,
            ExecutorKind::Native,
            "claude CLI cannot run local: models — must override to native. provenance: {:?}",
            plan.provenance
        );
        assert!(
            plan.provenance.executor_source.contains("model-compat"),
            "provenance must record the model-compat override, got {:?}",
            plan.provenance.executor_source
        );
    }

    /// Default dispatcher (no executor configured) + agent default (claude)
    /// + local: model → must override to native. Same root cause as
    /// autohaiku, just driven by the default fall-through rather than an
    /// explicit `-x claude`.
    #[test]
    fn test_default_executor_with_local_model_overrides_to_native() {
        let mut config = Config::default();
        config.coordinator.model = Some("local:qwen3-coder".to_string());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("local:qwen3-coder")).unwrap();

        assert_eq!(
            plan.executor,
            ExecutorKind::Native,
            "default claude executor with local: model must override to native. provenance: {:?}",
            plan.provenance
        );
    }

    /// `claude:opus` (and bare `opus`) MUST NOT trigger an override — the
    /// claude CLI is the right choice for Anthropic models. This locks in
    /// the boundary: only non-Anthropic provider prefixes (local, openrouter,
    /// oai-compat, openai) trigger the model-compat switch.
    #[test]
    fn test_claude_executor_with_anthropic_model_no_override() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());

        for model in ["opus", "sonnet", "claude:opus", "claude:sonnet"] {
            let task = base_task("t1");
            let plan = plan_spawn(&task, &config, None, Some(model)).unwrap();
            assert_eq!(
                plan.executor,
                ExecutorKind::Claude,
                "model={} must keep claude executor (no override). provenance: {:?}",
                model,
                plan.provenance
            );
        }
    }

    /// Codex with a local: model is fine — codex is OAI-compat-aware. The
    /// model-compat override only fires for the claude executor (the only
    /// CLI that genuinely can't speak non-Anthropic protocols). Without
    /// this guard the dispatcher's explicit codex choice would be silently
    /// rewritten.
    #[test]
    fn test_codex_executor_with_local_model_no_override() {
        let mut config = Config::default();
        config.coordinator.executor = Some("codex".to_string());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("local:qwen3-coder")).unwrap();
        assert_eq!(plan.executor, ExecutorKind::Codex);
        assert!(
            !plan.provenance.executor_source.contains("model-compat"),
            "codex must not be rewritten by model-compat. provenance: {:?}",
            plan.provenance
        );
    }

    /// Fix C regression-guard (fix-nex-chat / diagnose-wg-nex root cause #2):
    /// When a task has `task.endpoint` set to an http(s):// URL, plan_spawn
    /// MUST synthesize an `EndpointConfig` carrying that URL — NOT silently
    /// fall back to `find_default()`. Without this, IPC `CreateChat` with
    /// `endpoint=Some("https://lambda01...")` had its URL dropped on the
    /// floor: `wg spawn-task --dry-run .chat-N` emitted no `-e` flag.
    #[test]
    fn test_task_endpoint_inline_url_overrides_default() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());

        let mut task = base_task(".chat-32");
        task.model = Some("nex:qwen3-coder".to_string());
        task.endpoint = Some("https://lambda01.tail334fe6.ts.net:30000".to_string());

        let plan = plan_spawn(&task, &config, None, None).unwrap();

        assert_eq!(plan.executor, ExecutorKind::Native);
        let ep = plan.endpoint.as_ref().expect("endpoint must be Some");
        assert_eq!(
            ep.name, "https://lambda01.tail334fe6.ts.net:30000",
            "synthesized endpoint name must equal the inline URL so spawn_task forwards it as the -e arg"
        );
        assert_eq!(
            ep.url.as_deref(),
            Some("https://lambda01.tail334fe6.ts.net:30000")
        );
        assert!(
            plan.provenance.endpoint_source.contains("task.endpoint"),
            "endpoint_source must trace to task.endpoint, got {:?}",
            plan.provenance.endpoint_source
        );
        assert!(
            !plan.provenance.endpoint_source.contains("is_default"),
            "MUST NOT fall back to is_default when task.endpoint is set, got {:?}",
            plan.provenance.endpoint_source
        );
    }

    /// Fix C variant: `task.endpoint` referencing a configured endpoint by
    /// NAME (not URL) must look it up via `find_by_name`, not fall back to
    /// the default.
    #[test]
    fn test_task_endpoint_named_overrides_default() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "lambda01".to_string(),
            provider: "oai-compat".to_string(),
            url: Some("https://lambda01.example:30000".to_string()),
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });

        let mut task = base_task(".chat-1");
        task.endpoint = Some("lambda01".to_string());

        let plan = plan_spawn(&task, &config, None, Some("nex:qwen3-coder")).unwrap();

        let ep = plan.endpoint.as_ref().expect("endpoint must be Some");
        assert_eq!(ep.name, "lambda01");
        assert_eq!(ep.url.as_deref(), Some("https://lambda01.example:30000"));
        assert!(
            plan.provenance.endpoint_source.contains("named"),
            "endpoint_source should record the named-lookup path, got {:?}",
            plan.provenance.endpoint_source
        );
    }

    /// Fix C boundary: when no `task.endpoint` is set, behaviour is
    /// unchanged — the default endpoint still wins. Locks in that the
    /// new branch is purely additive.
    #[test]
    fn test_no_task_endpoint_falls_back_to_default() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.llm_endpoints.endpoints.push(openrouter_default_endpoint());

        let task = base_task("t1");
        let plan = plan_spawn(&task, &config, None, Some("nex:qwen3-coder")).unwrap();

        let ep = plan.endpoint.as_ref().expect("endpoint must be Some");
        assert_eq!(ep.name, "openrouter");
        assert!(
            plan.provenance.endpoint_source.contains("is_default"),
            "with no task.endpoint, fall back to is_default. got {:?}",
            plan.provenance.endpoint_source
        );
    }

    /// Fix C boundary: claude executor must continue to ignore task.endpoint
    /// (the claude CLI handles its own auth/url). Per the precedence rules
    /// at the top of this file, executor=claude → endpoint=None always.
    #[test]
    fn test_claude_executor_ignores_task_endpoint() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());

        let mut task = base_task("t1");
        task.endpoint = Some("https://anything.example".to_string());

        let plan = plan_spawn(&task, &config, None, Some("opus")).unwrap();
        assert_eq!(plan.executor, ExecutorKind::Claude);
        assert!(
            plan.endpoint.is_none(),
            "executor=claude must produce no endpoint, even with task.endpoint set"
        );
    }
}
