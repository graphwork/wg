//! Lightweight LLM dispatch for internal WG calls (triage, checkpoint, etc.).
//!
//! Resolves model + provider via `resolve_model_for_role()` and dispatches to either:
//! - Claude CLI (`claude --model X --print --dangerously-skip-permissions PROMPT`)
//! - Native Anthropic API client (when provider is "anthropic" and native executor is configured)
//! - Native OpenAI-compatible API client (when provider is "openai"/"openrouter")

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{
    CLAUDE_FABLE_MODEL_ID, CLAUDE_HAIKU_MODEL_ID, CLAUDE_OPUS_MODEL_ID, CLAUDE_SONNET_MODEL_ID,
    Config, DispatchRole, ExecutionSystemKey, ModelRegistryEntry, ReasoningLevel,
    execution_system_key, parse_model_spec, strip_native_handler_prefix,
};
use crate::dispatch::{ExecutorKind, handler_for_model};
use crate::graph::TokenUsage;

/// Result of a lightweight LLM call, including both the text response and token usage.
#[derive(Debug, Clone)]
pub struct LlmCallResult {
    pub text: String,
    pub token_usage: Option<TokenUsage>,
}

/// Maximum output tokens for lightweight LLM calls.
///
/// Triage calls produce short text (~200 tokens) but evaluation and FLIP calls
/// produce structured JSON with multiple dimensions, notes, and reasoning that
/// can easily exceed 1024 tokens. 4096 provides comfortable headroom.
const LIGHTWEIGHT_MAX_TOKENS: u32 = 4096;

/// Roles whose only output is a one-shot JSON scoring/assignment response —
/// the agency pipeline. These resolve their model from the profile's **weak**
/// two-tier label (`tiers.fast`, the cheap tier) rather than the project-level
/// cascade (e.g. `coordinator.model = "openrouter:..."`), so a two-tier Pi
/// profile that points `weak` at DeepSeek actually moves agency onto DeepSeek
/// while a wrong `coordinator.model` can never quietly route them through a
/// provider that lacks credentials.
///
/// A user who *explicitly* sets `[models.<role>].provider` or
/// `[models.<role>].model` for one of these roles still gets the
/// configured native path — only cascade fallthrough is overridden.
///
/// This is the **dispatch-side** definition of the system evaluation roles.
/// The **assignment-side** mirror lives in [`crate::assignment_eligibility`]
/// — `SYSTEM_EVALUATION_ROLE_NAMES` (the role names `Reviewer` / `Evaluator` /
/// `Assigner` / `Evolver` / `Agent Creator`) structurally excludes those agents
/// from the ordinary work-task candidate pool, and `role_is_system_evaluation`
/// is the membership check. The two notions name the system-evaluation
/// personas from two angles (dispatch role vs role name); the assignment-side
/// set is the broader union (it includes `Evolver` / `Agent Creator`, which
/// are system meta personas but not one-shot LLM dispatch roles). See task
/// `make-evaluator-and`.
fn is_agency_oneshot_role(role: DispatchRole) -> bool {
    matches!(
        role,
        DispatchRole::Evaluator
            | DispatchRole::FlipInference
            | DispatchRole::FlipComparison
            | DispatchRole::Assigner
            | DispatchRole::Reviewer
    )
}

/// The dispatch target for an agency one-shot LLM call (.assign-* /
/// .evaluate-* / .flip-*). Computed in one place so the spawn site (which
/// labels the agent in the registry) and the LLM call site (which actually
/// invokes the binary) cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgencyDispatch {
    /// Which CLI/handler will execute this call.
    pub handler: ExecutorKind,
    /// The full model spec as the user wrote it (e.g. `"codex:gpt-5.4-mini"`,
    /// `"claude:haiku"`). Stored in the agent registry for `wg agents`
    /// display and for diagnostic logs.
    pub raw_spec: String,
    /// The bare model id (no provider prefix) — passed to `--model` on the
    /// CLI subprocess.
    pub model_id: String,
    /// Structured reasoning level resolved independently from the model.
    pub reasoning: Option<ReasoningLevel>,
}

/// Convert provider/model Claude IDs into the bare aliases accepted by the
/// Claude CLI. OpenRouter and registry entries use IDs like
/// `anthropic/claude-haiku-4-5`, but the CLI expects `haiku`, `sonnet`, or
/// `opus`.
fn claude_cli_alias_for_model(model_id: &str) -> Option<&'static str> {
    let lower = model_id.to_ascii_lowercase();
    let claude_part = lower.strip_prefix("anthropic/").unwrap_or(lower.as_str());

    // Fable 5 has no bare CLI shortcut: both the friendly alias `fable` and any
    // dated id (`claude-fable-5`, `anthropic/claude-fable-5`) resolve to the
    // full CLI model id. Unlike opus/sonnet/haiku, `fable` alone does NOT start
    // with "claude", so it is matched before the "claude" gate below.
    if claude_part == "fable" || claude_part.contains("fable") {
        return Some(CLAUDE_FABLE_MODEL_ID);
    }

    if !claude_part.starts_with("claude") {
        return None;
    }

    if claude_part.contains("haiku") {
        Some(CLAUDE_HAIKU_MODEL_ID)
    } else if claude_part.contains("sonnet") {
        Some(CLAUDE_SONNET_MODEL_ID)
    } else if claude_part.contains("opus") {
        Some(CLAUDE_OPUS_MODEL_ID)
    } else {
        None
    }
}

fn normalize_claude_cli_model(model_id: &str) -> String {
    claude_cli_alias_for_model(model_id)
        .unwrap_or(model_id)
        .to_string()
}

/// Build an [`AgencyDispatch`] from a raw model spec. The leading handler is
/// authoritative. Claude-family model recognition may normalize a model only
/// *inside* an explicitly Claude-routed call; it must never turn an OpenRouter,
/// Pi, Codex, or nex route into a Claude CLI call.
fn agency_dispatch_for_spec(raw_spec: &str, reasoning: Option<ReasoningLevel>) -> AgencyDispatch {
    let spec = parse_model_spec(raw_spec);
    let handler = handler_for_model(raw_spec);
    let model_id = if handler == ExecutorKind::Claude {
        normalize_claude_cli_model(&spec.model_id)
    } else {
        spec.model_id
    };

    AgencyDispatch {
        handler,
        raw_spec: raw_spec.trim().to_string(),
        model_id,
        reasoning,
    }
}

/// One failed route attempt included in the stable loud error surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAttemptFailure {
    pub route: String,
    pub handler: String,
    pub provider: String,
    pub failure: String,
}

/// Structured failure returned after the selected route and every explicitly
/// authorized same-system fallback fail. Callers can downcast this through
/// `anyhow::Error` and keep agency work retryable instead of fabricating a
/// verdict.
#[derive(Debug)]
pub struct ExecutionRouteFailure {
    pub code: &'static str,
    pub role: DispatchRole,
    pub selected_route: String,
    pub system: ExecutionSystemKey,
    pub attempts: Vec<RouteAttemptFailure>,
}

impl std::fmt::Display for ExecutionRouteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "error[{}]: lightweight LLM route failed; role={} selected_route={:?} handler={} provider={} attempts=",
            self.code, self.role, self.selected_route, self.system.handler, self.system.provider,
        )?;
        for (index, attempt) in self.attempts.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(
                f,
                "route={:?} handler={} provider={} failure={:?}",
                attempt.route, attempt.handler, attempt.provider, attempt.failure
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ExecutionRouteFailure {}

/// Resolve the native-HTTP provider label a raw agency / reviewer model spec
/// targets.
///
/// Applies the **handler-first inner re-parse** (design §6.3) FIRST: a leading
/// `nex:` / `native:` names the in-process native handler, so the actual
/// provider lives in the INNER dialect. `nex:openrouter:<model>` therefore
/// resolves to `"openrouter"` — NOT the `oai-compat` localhost default the
/// lenient `parse_model_spec` would pick from the bare `nex` prefix (which sent
/// the canonical handler-first tier spec the rest of the codebase pushes to the
/// wrong wire and fail-closed every reviewer item). A bare in-process
/// `nex:<model>` (no inner provider) stays `"oai-compat"`; a prefix-less spec
/// defaults to `"anthropic"`. Shared by [`agency_native_creds_available`] and
/// [`agency_native_call_for_spec`] so the credential decision and the actual
/// call agree on which wire they hit. Mirrors `src/executor/native/provider.rs`.
fn native_provider_for_spec(raw_spec: &str) -> &'static str {
    parse_model_spec(strip_native_handler_prefix(raw_spec))
        .provider
        .as_deref()
        .map(crate::config::provider_to_resolved_provider)
        .unwrap_or("oai-compat")
}

/// Whether a native-HTTP agency model has a usable API key available.
///
/// Mirrors the key resolution that [`call_openai_native`] / [`call_anthropic_native`]
/// perform (provider env var → matching endpoint config), so the credential
/// decision made here matches what the actual call would find. Only the
/// providers that genuinely require a key are gated; localhost / in-process
/// providers (`local`, OAI-compat `nex` endpoints) and the self-authenticating
/// CLIs are never blocked.
fn agency_native_creds_available(config: &Config, raw_spec: &str) -> bool {
    let provider = native_provider_for_spec(raw_spec);

    let env_present = |vars: &[&str]| -> bool {
        vars.iter()
            .any(|v| std::env::var(v).ok().is_some_and(|k| !k.trim().is_empty()))
    };
    let endpoint_present = |provider: &str| -> bool {
        config
            .llm_endpoints
            .find_for_provider(provider)
            .and_then(|ep| ep.resolve_api_key(None).ok().flatten())
            .is_some()
    };

    match provider {
        "openrouter" => env_present(&["OPENROUTER_API_KEY"]) || endpoint_present(provider),
        "openai" => env_present(&["OPENAI_API_KEY"]) || endpoint_present(provider),
        "anthropic" => env_present(&["ANTHROPIC_API_KEY"]) || endpoint_present(provider),
        // `local` / `oai-compat` (localhost nex) require no key, and any other
        // provider is not our concern — don't block agency dispatch on it.
        _ => true,
    }
}

/// Resolve the explicitly selected handler+model for an agency one-shot role.
/// A role override wins; otherwise the explicitly configured/profile weak tier
/// is used. Built-in tier defaults and project-wide worker routes do not
/// authorize evaluator, reviewer, FLIP, or assignment execution.
pub fn resolve_agency_dispatch(config: &Config, role: DispatchRole) -> Result<AgencyDispatch> {
    debug_assert!(
        is_agency_oneshot_role(role),
        "resolve_agency_dispatch is only valid for agency one-shot roles"
    );

    let raw_spec = if let Some(configured) = config
        .models
        .get_role(role)
        .and_then(|role_config| role_config.model.as_deref())
    {
        if execution_system_key(configured).is_ok() {
            // A complete handler-first route is already authoritative. Do not
            // decompose nested dialects such as nex:openrouter:… or pi:… .
            configured.trim().to_string()
        } else {
            // Legacy generated profiles persisted `provider = "codex"` beside
            // a bare `model = "gpt-…"`. Reconstruct that explicit selection
            // once at the configuration boundary; callers persist/use only the
            // canonical handler-first route from here onward.
            config.resolve_model_for_role(role).spawn_model_spec()
        }
    } else {
        config.weak_tier_spec().ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-EXEC-UNSELECTED]: no explicit LLM route for agency role={role}; configure models.{role}.model or tiers.fast"
            )
        })?
    };
    execution_system_key(&raw_spec)
        .with_context(|| format!("invalid explicit agency route for role={role}: {raw_spec:?}"))?;
    Ok(agency_dispatch_for_spec(
        &raw_spec,
        config.resolve_reasoning_for_role(role),
    ))
}

/// Whether a usable credential exists for the content reviewer's weak **or** strong
/// tier. Used by `review::reviewer::model_review_available` to decide whether the
/// live model-review path runs (a real deployment with a key) or the deterministic
/// decode-then-detect fallback does (credential-free CI / claude-CLI-only).
///
/// Mirrors [`agency_native_creds_available`]: only native-HTTP providers that
/// genuinely need a key are gated; the self-authenticating CLIs (claude / codex)
/// report `false` here so a claude-CLI-only deployment stays on the deterministic
/// path unless it explicitly opts in via `WG_REVIEW_MODEL=1`.
pub fn review_native_creds_available(config: &Config) -> bool {
    [config.weak_tier_spec(), config.strong_tier_spec()]
        .into_iter()
        .flatten()
        .any(|spec| {
            handler_for_model(&spec) == ExecutorKind::Native
                && agency_native_creds_available(config, &spec)
        })
}

fn call_dispatch_route(
    config: &Config,
    dispatch: &AgencyDispatch,
    endpoint: Option<&str>,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    match dispatch.handler {
        ExecutorKind::Claude => call_claude_cli(&dispatch.model_id, prompt, timeout_secs),
        ExecutorKind::Codex => {
            call_codex_cli(&dispatch.model_id, dispatch.reasoning, prompt, timeout_secs)
        }
        ExecutorKind::Native => {
            agency_native_call_for_spec(config, &dispatch.raw_spec, endpoint, prompt, timeout_secs)
        }
        ExecutorKind::Pi => call_pi_cli(
            config,
            &dispatch.raw_spec,
            dispatch.reasoning,
            prompt,
            timeout_secs,
        ),
        other => anyhow::bail!(
            "handler {} does not support lightweight one-shot calls for route {:?}",
            other.as_str(),
            dispatch.raw_spec
        ),
    }
}

fn run_dispatch_with_same_system_fallback<F>(
    config: &Config,
    role: DispatchRole,
    primary: AgencyDispatch,
    mut attempt: F,
) -> Result<LlmCallResult>
where
    F: FnMut(&AgencyDispatch) -> Result<LlmCallResult>,
{
    let primary_system = execution_system_key(&primary.raw_spec)?;
    let mut routes = Vec::with_capacity(1 + config.execution.models_for(&primary.raw_spec).len());
    routes.push(primary.raw_spec.clone());
    routes.extend_from_slice(config.execution.models_for(&primary.raw_spec));

    // Validate the complete declaration before making the primary call. A
    // cross-system candidate invalidates the policy instead of being skipped.
    for candidate in routes.iter().skip(1) {
        let candidate_system = execution_system_key(candidate)?;
        if candidate_system != primary_system {
            anyhow::bail!(
                "error[WG-EXEC-FALLBACK-CROSS-SYSTEM]: role={role} primary={:?} system={} candidate={candidate:?} candidate_system={}",
                primary.raw_spec,
                primary_system,
                candidate_system
            );
        }
    }

    // A repeated candidate is not another authorized attempt. Preserve the
    // declaration's order while ensuring each exact route runs at most once.
    let mut seen = HashSet::new();
    routes.retain(|route| seen.insert(route.trim().to_string()));

    let mut failures = Vec::new();
    for (index, route) in routes.iter().enumerate() {
        let dispatch = agency_dispatch_for_spec(route, primary.reasoning);
        let system = execution_system_key(route)?;
        match attempt(&dispatch) {
            Ok(result) => {
                if index > 0 {
                    eprintln!(
                        "[agency-dispatch] role={role} selected_route={:?} handler={} provider={} fallback_route={route:?} outcome=success",
                        primary.raw_spec, primary_system.handler, primary_system.provider,
                    );
                }
                return Ok(result);
            }
            Err(error) => {
                let failure = format!("{error:#}");
                let next = routes.get(index + 1).map(String::as_str).unwrap_or("none");
                eprintln!(
                    "[agency-dispatch] role={role} selected_route={:?} attempted_route={route:?} handler={} provider={} failure={failure:?} next_same_system_fallback={next:?}",
                    primary.raw_spec, system.handler, system.provider,
                );
                failures.push(RouteAttemptFailure {
                    route: route.clone(),
                    handler: system.handler,
                    provider: system.provider,
                    failure,
                });
            }
        }
    }

    Err(ExecutionRouteFailure {
        code: "WG-EXEC-ROUTE-FAILED",
        role,
        selected_route: primary.raw_spec,
        system: primary_system,
        attempts: failures,
    }
    .into())
}

/// Dispatch one content-review LLM call at the explicitly selected weak or
/// strong tier. Transport failure is returned to the review pipeline, which
/// fail-closes it as pending/quarantine; it never changes execution system.
pub fn run_review_llm_call(
    config: &Config,
    strong: bool,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let dispatch = if strong {
        let spec = config.strong_tier_spec().ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-EXEC-UNSELECTED]: no explicit strong reviewer route; configure tiers.premium or tiers.standard"
            )
        })?;
        execution_system_key(&spec)?;
        agency_dispatch_for_spec(
            &spec,
            config.resolve_reasoning_for_role(DispatchRole::Verification),
        )
    } else {
        resolve_agency_dispatch(config, DispatchRole::Reviewer)?
    };

    run_dispatch_with_same_system_fallback(config, DispatchRole::Reviewer, dispatch, |route| {
        call_dispatch_route(config, route, None, prompt, timeout_secs)
    })
}

/// Drive a one-shot model call for an **arbitrary model spec** (the handler is resolved
/// from the spec's leading token via [`agency_dispatch_for_spec`]), returning the reply
/// text + token usage.
///
/// This is the lib-crate primitive the **WG-Exec real worker** (`wg provider run`) uses
/// to drive the model the authorizer named in the grant — replacing the constant-diff
/// stub (audit-exec F10) with a genuine model-handler call whose usage is the real
/// per-call accounting (FR-V3), not a canned figure. CLI handlers (`claude` / `codex`)
/// self-authenticate; a `native` spec needs its provider key (else the call errors and
/// the caller surfaces the failure — there is no silent constant fallback). Any other
/// handler is not a sensible one-shot worker target and is a hard error.
///
/// Like the review reviewer path, this is exercised credential-free only when a worker
/// command backend is configured; the live-LLM path here runs in a real deployment with
/// a key (or a self-authenticating CLI).
pub fn run_model_oneshot(
    config: &Config,
    model_spec: &str,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let dispatch = agency_dispatch_for_spec(model_spec, None);
    execution_system_key(model_spec)?;
    run_dispatch_with_same_system_fallback(config, DispatchRole::TaskAgent, dispatch, |route| {
        call_dispatch_route(config, route, None, prompt, timeout_secs)
    })
}

/// Make a native-HTTP one-shot call resolving the provider directly from a model
/// **spec** (not a role). Used by the reviewer strong-tier path, where the model is
/// the premium tier rather than the cascade-resolved role model.
fn agency_native_call_for_spec(
    config: &Config,
    raw_spec: &str,
    endpoint: Option<&str>,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    // Handler-first inner re-parse (design §6.3): unwrap a leading `nex:` /
    // `native:` handler prefix so a wire-distinct inner provider drives
    // resolution (`nex:openrouter:<model>` → provider `openrouter`, model_id
    // `<model>`). Without this the lenient `parse_model_spec` reads `nex` as
    // the provider and the call collapses to the oai-compat localhost default
    // (bogus model id `openrouter:<model>`) instead of hitting OpenRouter —
    // fail-closing every reviewer item. `native_provider_for_spec` and this
    // `model_id` both derive from the same stripped spec so they stay in sync.
    let spec = parse_model_spec(strip_native_handler_prefix(raw_spec));
    let provider = native_provider_for_spec(raw_spec);
    match provider {
        "anthropic" => call_anthropic_native(
            config,
            "anthropic",
            &spec.model_id,
            prompt,
            timeout_secs,
            None,
            endpoint,
        ),
        prov @ ("oai-compat" | "openai" | "openrouter" | "local") => call_openai_native(
            config,
            prov,
            &spec.model_id,
            prompt,
            timeout_secs,
            None,
            endpoint,
        ),
        other => anyhow::bail!("reviewer spec {raw_spec:?} provider {other:?} is not native HTTP"),
    }
}

/// If the daemon has a Claude OAuth token configured via `[auth]` in
/// config.toml, inject it into the child's env so the spawned `claude`
/// CLI can authenticate without requiring the caller to have exported
/// `CLAUDE_CODE_OAUTH_TOKEN` beforehand. No-op when the token is already
/// present in the env or not configured (falls back to the CLI's own
/// credential resolution — `~/.claude/credentials.json`, etc.).
fn inject_claude_oauth_token(cmd: &mut process::Command) {
    // Best-effort: load config from cwd's workgraph dir. This is a pure
    // read of the file and can silently skip on any failure.
    let dir = std::env::current_dir()
        .ok()
        .map(|p| p.join(".workgraph"))
        .filter(|p| p.exists());
    if let Some(dir) = dir
        && let Ok(cfg) = Config::load_merged(&dir)
        && let Some(token) = cfg.auth.resolve_claude_oauth_token()
    {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }
}

fn configured_lightweight_route(config: &Config, role: DispatchRole) -> Result<String> {
    if let Some(route) = config
        .models
        .get_role(role)
        .and_then(|model| model.model.clone())
    {
        return Ok(route);
    }
    if let Some(tier) = config.models.get_role(role).and_then(|model| model.tier)
        && let Some(route) = config.configured_tier_spec(tier)
    {
        return Ok(route);
    }
    if let Some(route) = config.configured_tier_spec(role.default_tier()) {
        return Ok(route);
    }
    if let Some(route) = config
        .models
        .get_role(DispatchRole::Default)
        .and_then(|model| model.model.clone())
    {
        return Ok(route);
    }
    if let Some(route) = config.coordinator.model.clone() {
        return Ok(route);
    }
    if config.agent_model_is_local && !config.agent.model.trim().is_empty() {
        return Ok(config.agent.model.clone());
    }
    anyhow::bail!("error[WG-EXEC-UNSELECTED]: no explicit LLM route for lightweight role={role}")
}

/// Run a lightweight (no tool-use) LLM call without crossing execution
/// systems. Agency roles use their explicit role/weak route; other roles use
/// their explicit role/tier/default selection. Only `[[execution.fallbacks]]`
/// candidates with the same handler+provider key may run after failure.
pub fn run_lightweight_llm_call(
    config: &Config,
    role: DispatchRole,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let dispatch = if is_agency_oneshot_role(role) {
        resolve_agency_dispatch(config, role)?
    } else {
        let route = configured_lightweight_route(config, role)?;
        execution_system_key(&route)?;
        agency_dispatch_for_spec(&route, config.resolve_reasoning_for_role(role))
    };

    run_dispatch_with_same_system_fallback(config, role, dispatch, |route| {
        call_dispatch_route(config, route, None, prompt, timeout_secs)
    })
}

/// Run a lightweight call on an invocation-scoped, explicitly selected route.
/// This is used by commands such as `wg evaluate --model ...`: the CLI route is
/// authoritative for that invocation and still receives only its explicitly
/// configured same-system fallback list.
pub fn run_lightweight_llm_call_for_route(
    config: &Config,
    role: DispatchRole,
    route: &str,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    execution_system_key(route).with_context(|| {
        format!("invalid explicit lightweight route for role={role}: {route:?}")
    })?;
    let dispatch = agency_dispatch_for_spec(route, config.resolve_reasoning_for_role(role));
    run_dispatch_with_same_system_fallback(config, role, dispatch, |candidate| {
        call_dispatch_route(config, candidate, None, prompt, timeout_secs)
    })
}

/// Execute one call from a persisted agency plan. Route, reasoning, endpoint,
/// and fallback order come from the plan; ambient role routing and fallback
/// configuration are deliberately ignored.
pub fn run_lightweight_llm_call_for_plan(
    config: &Config,
    role: DispatchRole,
    call: &crate::eval_lifecycle::AgencyCallPlan,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let actual_system = execution_system_key(&call.route)?;
    if actual_system != call.system {
        anyhow::bail!(
            "error[WG-EXEC-AGENCY-SYSTEM-MISMATCH]: planned={} actual={}",
            call.system,
            actual_system
        );
    }
    let mut routes = Vec::with_capacity(1 + call.fallbacks.len());
    routes.push(call.route.clone());
    routes.extend(call.fallbacks.iter().cloned());
    let mut seen = HashSet::new();
    routes.retain(|route| seen.insert(route.clone()));
    for route in &routes {
        if execution_system_key(route)? != call.system {
            anyhow::bail!(
                "error[WG-EXEC-FALLBACK-CROSS-SYSTEM]: planned primary={} candidate={route:?}",
                call.system
            );
        }
    }

    let mut failures = Vec::new();
    for route in routes {
        let dispatch = agency_dispatch_for_spec(&route, call.reasoning);
        match call_dispatch_route(
            config,
            &dispatch,
            call.endpoint.as_deref(),
            prompt,
            timeout_secs,
        ) {
            Ok(result) => return Ok(result),
            Err(error) => failures.push(RouteAttemptFailure {
                route,
                handler: call.system.handler.clone(),
                provider: call.system.provider.clone(),
                failure: format!("{error:#}"),
            }),
        }
    }
    Err(ExecutionRouteFailure {
        code: "WG-EXEC-ROUTE-FAILED",
        role,
        selected_route: call.route.clone(),
        system: call.system.clone(),
        attempts: failures,
    }
    .into())
}

/// Estimate cost in USD from token counts and registry pricing data.
fn estimate_cost(entry: &ModelRegistryEntry, usage: &TokenUsage) -> f64 {
    let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * entry.cost_per_input_mtok;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * entry.cost_per_output_mtok;
    let cache_read_cost = if entry.prompt_caching && entry.cache_read_discount > 0.0 {
        (usage.cache_read_input_tokens as f64 / 1_000_000.0)
            * entry.cost_per_input_mtok
            * entry.cache_read_discount
    } else {
        0.0
    };
    let cache_write_cost = if entry.prompt_caching && entry.cache_write_premium > 0.0 {
        (usage.cache_creation_input_tokens as f64 / 1_000_000.0)
            * entry.cost_per_input_mtok
            * entry.cache_write_premium
    } else {
        0.0
    };
    input_cost + output_cost + cache_read_cost + cache_write_cost
}

fn call_claude_cli(model: &str, prompt: &str, timeout_secs: u64) -> Result<LlmCallResult> {
    use std::io::Write as _;

    let model = normalize_claude_cli_model(model);

    // Pipe the prompt via stdin instead of passing it as a CLI argument.
    // Eval prompts can be very large (30KB+ with diffs, logs, artifacts) and
    // passing them as arguments can hit OS arg-length limits or cause the
    // `timeout` wrapper to fail with exit 124 before the API call even starts.
    let (mut child, _killer) = crate::platform_timeout::spawn_with_timeout(
        "claude",
        |cmd| {
            cmd.arg("--model")
                .arg(model)
                .arg("--print")
                .arg("--output-format")
                .arg("json")
                .arg("--dangerously-skip-permissions")
                // Strip Claude-Code-specific env vars that leak through when the
                // daemon itself was spawned from a Claude Code session (common on
                // Windows where people launch the daemon from a cmd.exe spawned
                // by Claude Code):
                //   - CLAUDECODE / CLAUDE_CODE_ENTRYPOINT: make the CLI refuse
                //     to run or behave as a nested session
                //   - CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST: tells the CLI to
                //     prefer the host bridge (Claude Code's auth IPC) over the
                //     configured API key / OAuth token. In a detached daemon
                //     there's no host bridge to resolve to, so the CLI 401s on
                //     every call and surfaces its synthetic "Invalid API key"
                //     placeholder.
                //   - CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH: hints the CLI that an
                //     SDK-level refresh loop is running and it shouldn't refresh
                //     its own token. True inside Claude Code, false in a headless
                //     daemon; leaving it set disables the CLI's own refresh.
                .env_remove("CLAUDECODE")
                .env_remove("CLAUDE_CODE_ENTRYPOINT")
                .env_remove("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST")
                .env_remove("CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH")
                .stdin(process::Stdio::piped())
                .stdout(process::Stdio::piped())
                .stderr(process::Stdio::piped());
            // Inject the configured `[auth]` OAuth token (if any) so a headless
            // daemon without an exported CLAUDE_CODE_OAUTH_TOKEN can still auth.
            inject_claude_oauth_token(cmd);
            cmd
        },
        timeout_secs,
    )
    .context("Failed to spawn claude CLI for lightweight LLM call")?;

    // Write prompt to stdin and close the pipe to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .context("Failed to write prompt to claude CLI stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for claude CLI output")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "Claude CLI call failed (exit {:?}): stderr={:?} stdout={:?}",
            output.status.code(),
            stderr.chars().take(500).collect::<String>(),
            stdout.chars().take(500).collect::<String>()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim())
        .context("Failed to parse JSON output from claude CLI")?;
    let text = val
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let token_usage = extract_json_usage(&val);

    if text.is_empty() {
        anyhow::bail!("Empty response from claude CLI");
    }
    Ok(LlmCallResult { text, token_usage })
}

/// One-shot LLM call via the Codex CLI (`codex exec --json`).
///
/// Codex is single-shot by nature — `codex exec` reads a prompt on stdin,
/// runs the turn, prints JSONL events, and exits. We parse the JSONL
/// stream to extract the final `agent_message` text and `turn.completed`
/// usage. Output format mirrors `call_claude_cli` so the caller doesn't
/// need to special-case which CLI ran.
fn codex_one_shot_command_args(model: &str, reasoning: Option<ReasoningLevel>) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        "--model".to_string(),
        model.to_string(),
    ];
    if let Some(level) = reasoning {
        args.extend([
            "-c".to_string(),
            format!("model_reasoning_effort=\"{}\"", level.as_codex_effort()),
        ]);
    }
    args
}

fn call_codex_cli(
    model: &str,
    reasoning: Option<ReasoningLevel>,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    use std::io::Write as _;

    let args = codex_one_shot_command_args(model, reasoning);
    // Cross-platform `timeout(1)` replacement — Windows has no equivalent.
    // Same in-process call-site treatment njt's #22 applied to call_claude_cli.
    let (mut child, _killer) = crate::platform_timeout::spawn_with_timeout(
        "codex",
        |cmd| {
            cmd.args(&args)
                .stdin(process::Stdio::piped())
                .stdout(process::Stdio::piped())
                .stderr(process::Stdio::piped())
        },
        timeout_secs,
    )
    .context("Failed to spawn codex CLI for lightweight LLM call")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .context("Failed to write prompt to codex CLI stdin")?;
    }

    // Stream-parse stdout line-by-line — codex emits one JSON event per
    // line. We track the most recent `agent_message` text and the
    // `turn.completed` usage block.
    let stdout = child.stdout.take().context("codex stdout take")?;
    let reader = BufReader::new(stdout);
    let mut last_agent_text: Option<String> = None;
    let mut token_usage: Option<TokenUsage> = None;

    for line in reader.lines().map_while(|l| l.ok()) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ty = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "item.completed" | "item.updated" => {
                if let Some(item) = val.get("item")
                    && item.get("type").and_then(|t| t.as_str()) == Some("agent_message")
                    && let Some(text) = item.get("text").and_then(|t| t.as_str())
                {
                    last_agent_text = Some(text.to_string());
                }
            }
            "turn.completed" => {
                if let Some(usage) = val.get("usage") {
                    let input_tokens = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("cached_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    token_usage = Some(TokenUsage {
                        cost_usd: 0.0,
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens: cache_read,
                        cache_creation_input_tokens: 0,
                    });
                }
            }
            _ => {}
        }
    }

    let stderr_buf = child
        .stderr
        .take()
        .map(|stderr| {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut buf);
            buf
        })
        .unwrap_or_default();

    let status = child
        .wait()
        .context("Failed to wait for codex CLI output")?;

    if !status.success() {
        let stderr_trim = stderr_buf.trim();
        anyhow::bail!(
            "Codex CLI call failed (exit {:?}): {}",
            status.code(),
            stderr_trim.chars().take(500).collect::<String>()
        );
    }

    let text = last_agent_text
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("Empty response from codex CLI");
    }
    Ok(LlmCallResult { text, token_usage })
}

/// The `--provider`/`--model` argv pair pi expects for a one-shot agency call.
struct PiOneShotModelArg {
    provider: String,
    model: String,
}

fn pi_one_shot_command_args(
    marg: &PiOneShotModelArg,
    reasoning: Option<ReasoningLevel>,
) -> Vec<String> {
    let mut args = vec![
        "--mode".to_string(),
        "json".to_string(),
        "--print".to_string(),
        "-ne".to_string(),
        "--no-tools".to_string(),
        "--no-session".to_string(),
        "--provider".to_string(),
        marg.provider.clone(),
        "--model".to_string(),
        marg.model.clone(),
    ];
    if let Some(reasoning) = reasoning {
        args.extend(["--thinking".to_string(), reasoning.as_str().to_string()]);
    }
    args
}

/// Parse a handler-first pi spec (`pi:openrouter:<vendor>/<model>` or a bare
/// `<vendor>/<model>` OpenRouter route) into the `--provider`/`--model` argv
/// pi expects. Mirrors `commands::pi_handler::pi_model_arg` (binary crate) —
/// kept inline here in the lib crate so the agency one-shot path can drive pi
/// without a binary-crate dependency. Returns `None` when no provider can be
/// resolved (a bare single-token alias gives pi no provider to target).
fn pi_one_shot_model_arg(raw_spec: &str) -> Option<PiOneShotModelArg> {
    let raw = raw_spec.trim();
    let inner = raw.strip_prefix("pi:").unwrap_or(raw).trim();
    if inner.is_empty() {
        return None;
    }
    if let Some((provider, model_id)) = inner.split_once(':') {
        let provider = provider.trim();
        let model_id = model_id.trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let provider = if crate::config::KNOWN_PROVIDERS.contains(&provider) {
                crate::config::provider_to_native_provider(provider)
            } else {
                provider
            };
            let model_id = if provider == "openrouter" {
                model_id.strip_prefix("openrouter/").unwrap_or(model_id)
            } else {
                model_id
            };
            return Some(PiOneShotModelArg {
                provider: provider.to_string(),
                model: model_id.to_string(),
            });
        }
    }
    let spec = parse_model_spec(inner);
    let (provider, model_id) = match spec.provider.as_deref() {
        Some(prov) => {
            let native = crate::config::provider_to_native_provider(prov);
            let id = if native == "openrouter" {
                spec.model_id
                    .strip_prefix("openrouter/")
                    .unwrap_or(&spec.model_id)
                    .to_string()
            } else {
                spec.model_id.clone()
            };
            (native.to_string(), id)
        }
        None => {
            let id = spec.model_id.as_str();
            if let Some(route) = id.strip_prefix("openrouter/") {
                ("openrouter".to_string(), route.to_string())
            } else if id.contains('/') {
                ("openrouter".to_string(), id.to_string())
            } else {
                return None;
            }
        }
    };
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(PiOneShotModelArg {
        provider,
        model: model_id,
    })
}

/// Resolve the WG endpoint + api key for a pi provider and return the env var
/// pairs to inject into the spawned pi process (credentials by env ONLY, never
/// argv). Only a matching provider endpoint is eligible; a default endpoint
/// for another provider must never leak credentials or change the selected
/// wire. The daemon's ambient provider env remains available to pi.
fn pi_env_pairs_for(
    config: &Config,
    workgraph_dir: Option<&std::path::Path>,
    pi_provider: &str,
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let resolved_key = config
        .llm_endpoints
        .find_for_provider(pi_provider)
        .and_then(|ep| {
            let key = ep.resolve_api_key(workgraph_dir).ok().flatten();
            key.map(|k| (ep.url.clone(), k))
        });
    if let Some((url, key)) = resolved_key {
        for var_name in crate::config::EndpointConfig::env_var_names_for_provider(pi_provider) {
            pairs.push((var_name.to_string(), key.clone()));
        }
        if let Some(url) = url {
            pairs.push(("WG_ENDPOINT_URL".to_string(), url));
        }
    }
    pairs
}

/// One-shot LLM call via the Pi CLI (`pi --mode json --print`).
///
/// The agency / FLIP one-shot path honors a handler-first `pi:` route (e.g.
/// `pi:openrouter:deepseek/deepseek-chat`) by driving `pi` as a NON-interactive
/// one-shot — NOT the long-lived `--mode rpc` worker used for chat/task agents.
/// We parse the NDJSON `--mode json` stream with `translate_pi_stream` to
/// recover the final assistant text and the summed per-turn usage. Tools are
/// disabled (`--no-tools`), extension discovery is disabled (`-ne`), and the
/// session is not persisted (`--no-session`), making the call hermetic with
/// respect to user-installed Pi extensions.
///
/// Credentials are supplied by environment ONLY (never `--api-key`): a
/// WG-resolved endpoint key is injected as the provider's env var
/// (`OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / …) so pi's own provider
/// clients discover it, mirroring `wg pi-handler`. When no WG endpoint key
/// resolves, pi falls back to its own auth (env / OAuth / `~/.pi` login).
fn call_pi_cli(
    config: &Config,
    raw_spec: &str,
    reasoning: Option<ReasoningLevel>,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    use std::io::Write as _;

    let marg = pi_one_shot_model_arg(raw_spec).with_context(|| {
        format!("pi one-shot could not resolve provider/model from {raw_spec:?} — expected a `pi:<provider>/<model>` or `<vendor>/<model>` spec")
    })?;

    let workgraph_dir = std::env::current_dir()
        .ok()
        .map(|p| p.join(".workgraph"))
        .filter(|p| p.exists());
    let env_pairs = pi_env_pairs_for(config, workgraph_dir.as_deref(), &marg.provider);

    let (mut child, _killer) = crate::platform_timeout::spawn_with_timeout(
        "pi",
        |cmd| {
            for arg in pi_one_shot_command_args(&marg, reasoning) {
                cmd.arg(arg);
            }
            cmd.stdin(process::Stdio::piped())
                .stdout(process::Stdio::piped())
                .stderr(process::Stdio::piped());
            for (k, v) in &env_pairs {
                cmd.env(k, v);
            }
            cmd
        },
        timeout_secs,
    )
    .context("Failed to spawn pi CLI for lightweight LLM call")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .context("Failed to write prompt to pi CLI stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for pi CLI output")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "Pi CLI call failed (exit {:?}): stderr={:?} stdout={:?}",
            output.status.code(),
            stderr.chars().take(500).collect::<String>(),
            stdout.chars().take(500).collect::<String>(),
        );
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let translation = crate::stream_event::translate_pi_stream(&stdout_str, None, true);
    let text = translation
        .final_text
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("Empty response from pi CLI");
    }
    let token_usage = TokenUsage {
        cost_usd: translation.total.cost_usd.unwrap_or(0.0),
        input_tokens: translation.total.input_tokens,
        output_tokens: translation.total.output_tokens,
        cache_read_input_tokens: translation.total.cache_read_input_tokens.unwrap_or(0),
        cache_creation_input_tokens: translation.total.cache_creation_input_tokens.unwrap_or(0),
    };
    Ok(LlmCallResult {
        text,
        token_usage: Some(token_usage),
    })
}

/// Parse stream-json output from Claude CLI to extract text content and token usage.
///
/// Stream-json lines include `type=assistant` (with content) and `type=result` (with usage).
/// Retained for potential future use with --output-format stream-json.
#[cfg(test)]
fn parse_stream_json_output(stdout: &str) -> (String, Option<TokenUsage>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut token_usage: Option<TokenUsage> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let event_type = match val.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => continue,
        };

        match event_type {
            "assistant" => {
                // Extract text from message.content[] blocks
                if let Some(content) = val
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                text_parts.push(t.to_string());
                            }
                        }
                    }
                }
            }
            "result" => {
                // Extract token usage from the result line
                let cost_usd = val
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let usage = val.get("usage");

                let input_tokens = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let output_tokens = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let cache_read = usage
                    .and_then(|u| {
                        u.get("cache_read_input_tokens")
                            .or_else(|| u.get("cacheReadInputTokens"))
                    })
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let cache_creation = usage
                    .and_then(|u| {
                        u.get("cache_creation_input_tokens")
                            .or_else(|| u.get("cacheCreationInputTokens"))
                    })
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                token_usage = Some(TokenUsage {
                    cost_usd,
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens: cache_read,
                    cache_creation_input_tokens: cache_creation,
                });
            }
            _ => {}
        }
    }

    (text_parts.join("").trim().to_string(), token_usage)
}

/// Extract token usage from a `--output-format json` result object.
fn extract_json_usage(val: &serde_json::Value) -> Option<TokenUsage> {
    let cost_usd = val
        .get("total_cost_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let usage = val.get("usage");

    let input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .and_then(|u| {
            u.get("cache_read_input_tokens")
                .or_else(|| u.get("cacheReadInputTokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation = usage
        .and_then(|u| {
            u.get("cache_creation_input_tokens")
                .or_else(|| u.get("cacheCreationInputTokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Some(TokenUsage {
        cost_usd,
        input_tokens,
        output_tokens,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
    })
}

fn call_anthropic_native(
    config: &Config,
    provider_name: &str,
    model: &str,
    prompt: &str,
    timeout_secs: u64,
    registry_entry: Option<&ModelRegistryEntry>,
    endpoint_name: Option<&str>,
) -> Result<LlmCallResult> {
    use crate::executor::native::client::{
        AnthropicClient, ContentBlock, Message, MessagesRequest, Role,
    };
    use crate::executor::native::provider::Provider;

    // Look up endpoint: by name first, then by provider
    let endpoint = endpoint_name
        .and_then(|name| config.llm_endpoints.find_by_name(name))
        .or_else(|| config.llm_endpoints.find_for_provider(provider_name));
    let endpoint_key = endpoint.and_then(|ep| ep.resolve_api_key(None).ok().flatten());
    let endpoint_url = endpoint.and_then(|ep| ep.url.clone());

    // Resolve API key. Priority: env var > endpoint config > from_env fallbacks
    let env_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let mut client = if let Some(key) = env_key {
        AnthropicClient::new(key, model)
    } else if let Some(key) = endpoint_key {
        AnthropicClient::new(key, model)
    } else {
        AnthropicClient::from_env(model)
    }
    .context("Failed to create Anthropic client for lightweight call")?;
    if let Some(url) = endpoint_url {
        client = client.with_base_url(&url);
    }

    let request = MessagesRequest {
        model: model.to_string(),
        max_tokens: LIGHTWEIGHT_MAX_TOKENS,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
        }],
        tools: vec![],
        stream: false,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;

    let response = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(timeout_secs), client.send(&request))
            .await
            .context("Native Anthropic call timed out")?
    })?;

    let mut usage = TokenUsage {
        cost_usd: 0.0,
        input_tokens: u64::from(response.usage.input_tokens),
        output_tokens: u64::from(response.usage.output_tokens),
        cache_read_input_tokens: response
            .usage
            .cache_read_input_tokens
            .map(u64::from)
            .unwrap_or(0),
        cache_creation_input_tokens: response
            .usage
            .cache_creation_input_tokens
            .map(u64::from)
            .unwrap_or(0),
    };
    if let Some(entry) = registry_entry {
        usage.cost_usd = estimate_cost(entry, &usage);
    }
    let token_usage = Some(usage);

    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let text = text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("Empty response from native Anthropic call");
    }
    Ok(LlmCallResult { text, token_usage })
}

fn call_openai_native(
    config: &Config,
    provider_name: &str,
    model: &str,
    prompt: &str,
    timeout_secs: u64,
    registry_entry: Option<&ModelRegistryEntry>,
    endpoint_name: Option<&str>,
) -> Result<LlmCallResult> {
    use crate::executor::native::client::{ContentBlock, Message, MessagesRequest, Role};
    use crate::executor::native::openai_client::OpenAiClient;
    use crate::executor::native::provider::Provider;

    // Look up endpoint: by name first, then by provider
    let endpoint = endpoint_name
        .and_then(|name| config.llm_endpoints.find_by_name(name))
        .or_else(|| config.llm_endpoints.find_for_provider(provider_name));
    let endpoint_key = endpoint.and_then(|ep| ep.resolve_api_key(None).ok().flatten());
    let endpoint_url = endpoint.and_then(|ep| ep.url.clone());

    // Resolve API key. Priority: env var > endpoint config > from_env fallbacks
    let env_key = match provider_name {
        "openrouter" => std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|k| !k.is_empty()),
        "openai" => std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.is_empty()),
        _ => None,
    };
    let resolved_key = env_key.or(endpoint_key);

    let mut client = if let Some(key) = resolved_key {
        OpenAiClient::new(key, model, None)
            .context("Failed to create OpenAI client for lightweight call")?
    } else if matches!(provider_name, "local" | "oai-compat") {
        // Local/OAI-compatible endpoints do not require auth unless their
        // matching endpoint explicitly supplies a key.
        OpenAiClient::new("local".to_string(), model, None).expect("infallible with static args")
    } else {
        anyhow::bail!(
            "missing credential for selected native provider {provider_name:?}; configure its matching endpoint or provider environment variable"
        )
    };
    if let Some(url) = endpoint_url {
        client = client.with_base_url(&url);
    }
    client = client.with_provider_hint(provider_name);

    let request = MessagesRequest {
        model: model.to_string(),
        max_tokens: LIGHTWEIGHT_MAX_TOKENS,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
        }],
        tools: vec![],
        stream: false,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;

    let response = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(timeout_secs), client.send(&request))
            .await
            .context("Native OpenAI call timed out")?
    })?;

    let mut usage = TokenUsage {
        cost_usd: 0.0,
        input_tokens: u64::from(response.usage.input_tokens),
        output_tokens: u64::from(response.usage.output_tokens),
        cache_read_input_tokens: response
            .usage
            .cache_read_input_tokens
            .map(u64::from)
            .unwrap_or(0),
        cache_creation_input_tokens: response
            .usage
            .cache_creation_input_tokens
            .map(u64::from)
            .unwrap_or(0),
    };
    if let Some(entry) = registry_entry {
        usage.cost_usd = estimate_cost(entry, &usage);
    }
    let token_usage = Some(usage);

    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    let text = text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("Empty response from native OpenAI call");
    }
    Ok(LlmCallResult { text, token_usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CLAUDE_HAIKU_MODEL_ID, Config, DispatchRole, ExecutionFallback, ModelRegistryEntry, Tier,
    };

    #[test]
    fn test_direct_codex_agency_argv_carries_reasoning_without_verbosity() {
        let args = codex_one_shot_command_args("gpt-5.6-luna", Some(ReasoningLevel::Off));
        assert_eq!(
            args,
            vec![
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "--model",
                "gpt-5.6-luna",
                "-c",
                "model_reasoning_effort=\"none\"",
            ]
        );
        assert!(
            args.iter().all(|arg| !arg.contains("model_verbosity")),
            "agency reasoning must not silently set response verbosity"
        );

        let inherited = codex_one_shot_command_args("gpt-5.6-luna", None);
        assert!(
            inherited
                .iter()
                .all(|arg| !arg.contains("model_reasoning_effort")),
            "unset WG reasoning must leave ~/.codex/config.toml authoritative"
        );
    }

    #[test]
    fn test_lightweight_llm_dispatch_resolves_model() {
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(
            resolved.provider,
            Some("anthropic".to_string()),
            "Default triage should resolve via Fast tier registry"
        );
    }

    #[test]
    fn test_lightweight_llm_dispatch_with_provider_override() {
        let mut config = Config::default();
        config.models.set_model(DispatchRole::Triage, "gpt-4o-mini");
        config.models.set_provider(DispatchRole::Triage, "openai");

        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, "gpt-4o-mini");
        assert_eq!(resolved.provider, Some("openai".to_string()));
    }

    #[test]
    fn test_is_agency_oneshot_role_covers_eval_flip_assign() {
        assert!(is_agency_oneshot_role(DispatchRole::Evaluator));
        assert!(is_agency_oneshot_role(DispatchRole::FlipInference));
        assert!(is_agency_oneshot_role(DispatchRole::FlipComparison));
        assert!(is_agency_oneshot_role(DispatchRole::Assigner));
    }

    #[test]
    fn test_is_agency_oneshot_role_excludes_other_roles() {
        // Triage, Compactor, TaskAgent, Evolver etc. keep their cascade
        // behavior — only the agency pipeline is pinned to claude CLI.
        assert!(!is_agency_oneshot_role(DispatchRole::Triage));
        assert!(!is_agency_oneshot_role(DispatchRole::Compactor));
        assert!(!is_agency_oneshot_role(DispatchRole::TaskAgent));
        assert!(!is_agency_oneshot_role(DispatchRole::Default));
        assert!(!is_agency_oneshot_role(DispatchRole::Evolver));
        assert!(!is_agency_oneshot_role(DispatchRole::Verification));
    }

    #[test]
    fn test_resolve_agency_dispatch_without_explicit_role_or_weak_tier_is_unselected() {
        let mut config = Config::default();
        config.coordinator.model = Some("nex:openrouter:anthropic/claude-sonnet-4-6".into());

        let error = resolve_agency_dispatch(&config, DispatchRole::Assigner).unwrap_err();
        assert!(format!("{error:#}").contains("WG-EXEC-UNSELECTED"));
    }

    #[test]
    fn test_explicit_cli_routes_are_preserved_for_every_agency_role() {
        for (route, expected_handler, expected_model) in [
            ("codex:gpt-5.4-mini", ExecutorKind::Codex, "gpt-5.4-mini"),
            ("claude:sonnet", ExecutorKind::Claude, "sonnet"),
            (
                "nex:openrouter:qwen/qwen3-coder",
                ExecutorKind::Native,
                "openrouter:qwen/qwen3-coder",
            ),
            (
                "pi:openai-codex:gpt-5.6-terra",
                ExecutorKind::Pi,
                "pi:openai-codex:gpt-5.6-terra",
            ),
        ] {
            for role in [
                DispatchRole::Evaluator,
                DispatchRole::FlipInference,
                DispatchRole::FlipComparison,
                DispatchRole::Assigner,
                DispatchRole::Reviewer,
            ] {
                let mut config = Config::default();
                config.models.set_model(role, route);
                let dispatch = resolve_agency_dispatch(&config, role).unwrap();
                assert_eq!(
                    dispatch.handler, expected_handler,
                    "role={role} route={route}"
                );
                assert_eq!(
                    dispatch.model_id, expected_model,
                    "role={role} route={route}"
                );
            }
        }
    }

    #[test]
    fn test_openrouter_claude_model_stays_on_openrouter_native_handler() {
        let mut config = Config::default();
        config.models.set_model(
            DispatchRole::Evaluator,
            "nex:openrouter:anthropic/claude-sonnet-4-6",
        );

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator).unwrap();
        assert_eq!(dispatch.handler, ExecutorKind::Native);
        assert_eq!(
            dispatch.raw_spec,
            "nex:openrouter:anthropic/claude-sonnet-4-6"
        );
        assert_ne!(dispatch.model_id, "sonnet");
    }

    #[test]
    fn test_claude_cli_model_normalization() {
        assert_eq!(
            normalize_claude_cli_model("anthropic/claude-haiku-4-5"),
            "haiku"
        );
        assert_eq!(
            normalize_claude_cli_model("anthropic/claude-sonnet-4-6"),
            "sonnet"
        );
        assert_eq!(
            normalize_claude_cli_model("anthropic/claude-opus-4-7"),
            "opus"
        );
        assert_eq!(normalize_claude_cli_model("haiku"), "haiku");
        assert_eq!(
            normalize_claude_cli_model("qwen/qwen3-coder"),
            "qwen/qwen3-coder"
        );
    }

    #[test]
    fn test_claude_cli_model_normalization_fable() {
        assert_eq!(normalize_claude_cli_model("fable"), CLAUDE_FABLE_MODEL_ID);
        assert_eq!(
            normalize_claude_cli_model("claude-fable-5"),
            CLAUDE_FABLE_MODEL_ID
        );
        assert_eq!(
            normalize_claude_cli_model("anthropic/claude-fable-5"),
            CLAUDE_FABLE_MODEL_ID
        );
    }

    #[test]
    fn test_claude_fable_normalizes_only_on_explicit_claude_handler() {
        let dispatch = agency_dispatch_for_spec("claude:fable", None);
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.model_id, CLAUDE_FABLE_MODEL_ID);

        let dispatch = agency_dispatch_for_spec("nex:openrouter:anthropic/claude-fable-5", None);
        assert_eq!(dispatch.handler, ExecutorKind::Native);
        assert_ne!(dispatch.model_id, CLAUDE_FABLE_MODEL_ID);
    }

    #[test]
    fn test_agency_role_ignores_project_worker_route_without_fabricating_claude() {
        let mut config = Config::default();
        config.coordinator.model = Some("codex:gpt-5.5".to_string());
        let error = resolve_agency_dispatch(&config, DispatchRole::Evaluator).unwrap_err();
        assert!(format!("{error:#}").contains("WG-EXEC-UNSELECTED"));
    }

    #[test]
    fn test_lightweight_llm_parse_stream_json_output() {
        // Simulate Claude CLI stream-json output
        let stdout = format!(
            r#"{{"type":"system","session_id":"abc","model":"{CLAUDE_HAIKU_MODEL_ID}"}}
{{"type":"assistant","message":{{"id":"msg_1","type":"message","role":"assistant","content":[{{"type":"text","text":"The answer is 42."}}],"usage":{{"input_tokens":100,"output_tokens":20}}}}}}
{{"type":"result","total_cost_usd":0.0012,"usage":{{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":50,"cache_creation_input_tokens":10}}}}
"#
        );
        let (text, token_usage) = parse_stream_json_output(&stdout);
        assert_eq!(text, "The answer is 42.");
        let usage = token_usage.expect("should have token usage");
        assert!((usage.cost_usd - 0.0012).abs() < f64::EPSILON);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_input_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 10);
    }

    #[test]
    fn test_lightweight_llm_parse_stream_json_empty() {
        let (text, token_usage) = parse_stream_json_output("");
        assert!(text.is_empty());
        assert!(token_usage.is_none());
    }

    #[test]
    fn test_lightweight_llm_parse_stream_json_no_result() {
        // If the result line is missing, we still get text but no token usage
        let stdout = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":10,"output_tokens":5}}}
"#;
        let (text, token_usage) = parse_stream_json_output(stdout);
        assert_eq!(text, "hello");
        assert!(token_usage.is_none());
    }

    #[test]
    fn test_lightweight_llm_estimate_cost() {
        let entry = ModelRegistryEntry {
            id: "haiku".to_string(),
            provider: "anthropic".to_string(),
            model: CLAUDE_HAIKU_MODEL_ID.to_string(),
            tier: Tier::Fast,
            endpoint: None,
            context_window: 200_000,
            max_output_tokens: 8192,
            cost_per_input_mtok: 0.80,
            cost_per_output_mtok: 4.0,
            prompt_caching: true,
            cache_read_discount: 0.1,
            cache_write_premium: 1.25,
            descriptors: vec![],
        };

        let usage = TokenUsage {
            cost_usd: 0.0,
            input_tokens: 1_000_000, // 1M tokens
            output_tokens: 100_000,  // 100K tokens
            cache_read_input_tokens: 500_000,
            cache_creation_input_tokens: 200_000,
        };

        let cost = estimate_cost(&entry, &usage);
        // input: 1.0 * 0.80 = 0.80
        // output: 0.1 * 4.0 = 0.40
        // cache_read: 0.5 * 0.80 * 0.1 = 0.04
        // cache_write: 0.2 * 0.80 * 1.25 = 0.20
        let expected = 0.80 + 0.40 + 0.04 + 0.20;
        assert!(
            (cost - expected).abs() < 0.001,
            "expected {}, got {}",
            expected,
            cost
        );
    }

    #[test]
    fn test_call_claude_cli_json_parsing() {
        // Simulates the --output-format json output from Claude CLI
        let json_output = r#"{
            "type": "result",
            "result": "The answer is 42.",
            "total_cost_usd": 0.0012,
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_read_input_tokens": 50,
                "cache_creation_input_tokens": 10
            }
        }"#;

        let val: serde_json::Value = serde_json::from_str(json_output).unwrap();
        let text = val
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let token_usage = extract_json_usage(&val);

        assert_eq!(text, "The answer is 42.");
        let usage = token_usage.expect("should have token usage");
        assert!((usage.cost_usd - 0.0012).abs() < f64::EPSILON);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_input_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 10);
    }

    #[test]
    fn test_call_claude_cli_json_no_usage() {
        // JSON result with no usage data
        let json_output = r#"{"type": "result", "result": "hello world"}"#;
        let val: serde_json::Value = serde_json::from_str(json_output).unwrap();
        let text = val
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let token_usage = extract_json_usage(&val);

        assert_eq!(text, "hello world");
        // No usage block → should still return Some with zeroed fields and cost
        let usage = token_usage.expect("should have token usage with defaults");
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn test_lightweight_llm_estimate_cost_no_caching() {
        let entry = ModelRegistryEntry {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            tier: Tier::Standard,
            endpoint: None,
            context_window: 128_000,
            max_output_tokens: 4096,
            cost_per_input_mtok: 2.50,
            cost_per_output_mtok: 10.0,
            prompt_caching: false,
            cache_read_discount: 0.0,
            cache_write_premium: 0.0,
            descriptors: vec![],
        };

        let usage = TokenUsage {
            cost_usd: 0.0,
            input_tokens: 500,
            output_tokens: 200,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };

        let cost = estimate_cost(&entry, &usage);
        // input: 0.0005 * 2.50 = 0.00125
        // output: 0.0002 * 10.0 = 0.002
        let expected = 0.00125 + 0.002;
        assert!(
            (cost - expected).abs() < 0.0001,
            "expected {}, got {}",
            expected,
            cost
        );
    }

    // -- Agency routing to the WEAK tier (fix-route-agency) -----------------

    /// Save/override an env var for the duration of a test, restoring on drop.
    /// Tests that use this MUST be `#[serial]` because env is process-global.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: Option<&str>) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests using EnvGuard are #[serial], so no other thread is
            // concurrently reading/writing the environment.
            unsafe {
                match val {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
            EnvGuard { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    const ALL_AGENCY_ROLES: [DispatchRole; 5] = [
        DispatchRole::Evaluator,
        DispatchRole::FlipInference,
        DispatchRole::FlipComparison,
        DispatchRole::Assigner,
        DispatchRole::Reviewer,
    ];

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_deepseek_with_key() {
        // The two-tier setter wrote the handler-first OpenRouter route into
        // tiers.fast. With an OpenRouter key present, agency one-shots route to
        // that DeepSeek model via the native HTTP handler — NOT the old
        // hardcoded claude:haiku. (Covers validation item 1.)
        let _key = EnvGuard::set("OPENROUTER_API_KEY", Some("sk-or-test-deepseek"));
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role).unwrap();
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Native,
                "role {role:?} weak-tier deepseek must dispatch via the native HTTP handler",
            );
            assert_eq!(dispatch.raw_spec, "nex:openrouter:deepseek/deepseek-chat");
            assert_eq!(dispatch.model_id, "openrouter:deepseek/deepseek-chat");
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_key_from_endpoint() {
        // The credential can come from a configured endpoint instead of an env
        // var — same outcome: agency routes to the native DeepSeek model.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:deepseek/deepseek-chat".to_string());
        config
            .llm_endpoints
            .endpoints
            .push(crate::config::EndpointConfig {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: Some("https://openrouter.ai/api/v1".to_string()),
                model: None,
                api_key: Some("sk-or-endpoint-key".to_string()),
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: true,
                context_window: None,
            });

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator).unwrap();
        assert_eq!(dispatch.handler, ExecutorKind::Native);
        assert_eq!(dispatch.raw_spec, "nex:openrouter:deepseek/deepseek-chat");
        assert_eq!(dispatch.model_id, "openrouter:deepseek/deepseek-chat");
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_missing_native_key_does_not_switch_handler() {
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role).unwrap();
            assert_eq!(dispatch.handler, ExecutorKind::Native, "role={role}");
            assert_eq!(dispatch.raw_spec, "nex:openrouter:deepseek/deepseek-chat");
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_explicit_override_wins_over_weak_tier() {
        // tiers.fast points weak at DeepSeek, but an explicit [models.evaluator]
        // override must still win for that role — explicit overrides beat the
        // tier default. (Covers validation item 2.)
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:deepseek/deepseek-chat".to_string());
        config
            .models
            .set_model(DispatchRole::Evaluator, "claude:sonnet");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator).unwrap();
        assert_eq!(
            dispatch.raw_spec, "claude:sonnet",
            "explicit override must win"
        );
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.model_id, "sonnet");

        // A sibling role remains on the selected weak OpenRouter route. Missing
        // credentials are a call error, never permission to invoke Claude.
        let assigner = resolve_agency_dispatch(&config, DispatchRole::Assigner).unwrap();
        assert_ne!(assigner.raw_spec, "claude:sonnet");
        assert_eq!(assigner.handler, ExecutorKind::Native);
        assert_eq!(assigner.raw_spec, "nex:openrouter:deepseek/deepseek-chat");
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_explicit_codex_override_unaffected_by_weak_tier() {
        // A codex override routes to the codex CLI regardless of the weak tier;
        // codex self-authenticates, so it is not subject to the credential net.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:deepseek/deepseek-chat".to_string());
        config
            .models
            .set_model(DispatchRole::Assigner, "codex:gpt-5.4-mini");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Assigner).unwrap();
        assert_eq!(dispatch.handler, ExecutorKind::Codex);
        assert_eq!(dispatch.raw_spec, "codex:gpt-5.4-mini");
        assert_eq!(dispatch.model_id, "gpt-5.4-mini");
    }

    #[test]
    #[serial_test::serial]
    fn test_agency_native_creds_available_matches_provider() {
        // Direct coverage of the credential predicate the dispatch relies on.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let config = Config::default();

        // No key, openrouter weak tier -> unavailable.
        assert!(!agency_native_creds_available(
            &config,
            "openrouter:deepseek/deepseek-chat"
        ));

        // A localhost/in-process nex (oai-compat) model needs no key.
        assert!(agency_native_creds_available(
            &config,
            "nex:qwen3-coder-30b"
        ));

        // With a key present, openrouter is available.
        let _k = EnvGuard::set("OPENROUTER_API_KEY", Some("sk-or-x"));
        assert!(agency_native_creds_available(
            &config,
            "openrouter:deepseek/deepseek-chat"
        ));
    }

    #[test]
    #[serial_test::serial]
    fn test_native_credentials_never_cross_provider_boundary() {
        let _or = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _oa = EnvGuard::set("OPENAI_API_KEY", Some("sk-openai-only"));
        let config = Config::default();
        assert!(!agency_native_creds_available(
            &config,
            "nex:openrouter:z-ai/glm-5.2"
        ));
        assert!(agency_native_creds_available(
            &config,
            "nex:openai:gpt-5-mini"
        ));
    }

    #[test]
    fn test_native_provider_for_spec_strips_handler_first_prefix() {
        // The regression this task fixes: the canonical handler-first spec
        // `nex:openrouter:<model>` must resolve to the OpenRouter native client,
        // NOT the oai-compat localhost default (what the lenient parse yields
        // from the bare `nex` prefix) and NOT anthropic.
        assert_eq!(
            native_provider_for_spec("nex:openrouter:openai/gpt-4o-mini"),
            "openrouter"
        );
        // `native:` is the legacy handler alias — same unwrap.
        assert_eq!(
            native_provider_for_spec("native:openrouter:openai/gpt-4o-mini"),
            "openrouter"
        );
        // The bare (already-working) form resolves identically — parity so
        // `wg config`'s recommended `nex:` form and the legacy bare form agree.
        assert_eq!(
            native_provider_for_spec("openrouter:openai/gpt-4o-mini"),
            "openrouter"
        );
        // A bare in-process nex model (no inner provider) still targets the
        // oai-compat localhost wire — the strip is a no-op here.
        assert_eq!(
            native_provider_for_spec("nex:qwen3-coder-30b"),
            "oai-compat"
        );
        // A prefix-less nex/native dialect uses the OAI-compatible wire; it
        // must not silently select Anthropic.
        assert_eq!(native_provider_for_spec("some-bare-model"), "oai-compat");
    }

    #[test]
    fn test_agency_dispatch_for_spec_routes_pi_handler_first() {
        // The bug this task fixes: a handler-first `pi:` agency spec (e.g.
        // `pi:openrouter:deepseek/deepseek-chat`) MUST resolve to the Pi
        // handler — NOT the claude CLI catch-all in `run_lightweight_llm_call`.
        // The previous bug silently fell into the claude-CLI arm and failed
        // with "Claude CLI call failed ... subscription access disabled".
        let dispatch = agency_dispatch_for_spec("pi:openrouter:deepseek/deepseek-chat", None);
        assert_eq!(
            dispatch.handler,
            ExecutorKind::Pi,
            "pi:* agency specs must dispatch via the pi handler, not claude CLI"
        );
        assert_eq!(dispatch.raw_spec, "pi:openrouter:deepseek/deepseek-chat");
        // `parse_model_spec` only recognizes provider prefixes (claude/codex/
        // openrouter/...); `pi:` is a handler, so the full spec is preserved as
        // the model_id. `call_pi_cli` re-parses `raw_spec` via
        // `pi_one_shot_model_arg`, so the handler-first inner route is honored.
        assert_eq!(dispatch.model_id, "pi:openrouter:deepseek/deepseek-chat");
    }

    #[test]
    fn test_pi_one_shot_codex_model_and_reasoning_args() {
        let marg = pi_one_shot_model_arg("pi:openai-codex:gpt-5.6-sol")
            .expect("pi codex route should resolve");
        assert_eq!(marg.provider, "openai-codex");
        assert_eq!(marg.model, "gpt-5.6-sol");

        let high = pi_one_shot_command_args(&marg, Some(ReasoningLevel::High));
        let tidx = high.iter().position(|a| a == "--thinking").unwrap();
        assert_eq!(high[tidx + 1], "high");

        let max = pi_one_shot_command_args(&marg, Some(ReasoningLevel::Max));
        let tidx = max.iter().position(|a| a == "--thinking").unwrap();
        assert_eq!(max[tidx + 1], "max");

        let omitted = pi_one_shot_command_args(&marg, None);
        assert!(omitted.contains(&"-ne".to_string()));
        assert!(
            !omitted.contains(&"--thinking".to_string()),
            "omitted reasoning must not emit --thinking: {:?}",
            omitted
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_pi_routes_to_pi_handler() {
        // Two-tier Pi profile writes `pi:openrouter:deepseek/deepseek-chat` into
        // tiers.fast. ALL agency one-shot roles must resolve to the Pi handler
        // (NOT claude CLI, NOT native). A `pi:` route self-authenticates via env
        // / pi OAuth. Failure remains on Pi unless the user explicitly configured
        // a same-system fallback.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("pi:openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role).unwrap();
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Pi,
                "role {role:?}: pi:* weak tier must dispatch via the pi handler, not claude CLI",
            );
            assert_eq!(dispatch.raw_spec, "pi:openrouter:deepseek/deepseek-chat");
        }
    }

    #[test]
    fn test_pi_one_shot_model_arg_resolves_handler_first_spec() {
        // The argv pi expects for a one-shot agency call, mirroring
        // `commands::pi_handler::pi_model_arg`.
        let marg = pi_one_shot_model_arg("pi:openrouter:deepseek/deepseek-chat").unwrap();
        assert_eq!(marg.provider, "openrouter");
        assert_eq!(marg.model, "deepseek/deepseek-chat");

        // A bare vendor/model (no provider prefix) is an OpenRouter route.
        let marg = pi_one_shot_model_arg("deepseek/deepseek-chat").unwrap();
        assert_eq!(marg.provider, "openrouter");
        assert_eq!(marg.model, "deepseek/deepseek-chat");

        // A redundant `openrouter/` prefix on the model id is stripped once
        // (mirrors `commands::pi_handler::pi_model_arg`).
        let marg =
            pi_one_shot_model_arg("pi:openrouter:openrouter/deepseek/deepseek-chat").unwrap();
        assert_eq!(marg.provider, "openrouter");
        assert_eq!(marg.model, "deepseek/deepseek-chat");

        // A bare single-token alias gives pi no provider to target — unresolved.
        assert!(pi_one_shot_model_arg("pi:haiku").is_none());
        assert!(pi_one_shot_model_arg("haiku").is_none());
    }

    #[test]
    #[serial_test::serial]
    fn test_agency_native_creds_available_handler_first_openrouter() {
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let config = Config::default();

        assert!(!agency_native_creds_available(
            &config,
            "nex:openrouter:openai/gpt-4o-mini"
        ));

        let _k = EnvGuard::set("OPENROUTER_API_KEY", Some("sk-or-x"));
        assert!(agency_native_creds_available(
            &config,
            "nex:openrouter:openai/gpt-4o-mini"
        ));
    }

    fn fake_success(text: &str) -> LlmCallResult {
        LlmCallResult {
            text: text.to_string(),
            token_usage: None,
        }
    }

    #[test]
    fn test_pi_failure_without_fallback_is_loud_and_never_attempts_claude() {
        let config = Config::default();
        let primary = agency_dispatch_for_spec("pi:openai-codex:gpt-5.6-terra", None);
        let mut attempted = Vec::new();
        let error = run_dispatch_with_same_system_fallback(
            &config,
            DispatchRole::Evaluator,
            primary,
            |route| {
                attempted.push((route.handler, route.raw_spec.clone()));
                anyhow::bail!("injected Pi failure")
            },
        )
        .unwrap_err();

        assert_eq!(
            attempted,
            vec![(ExecutorKind::Pi, "pi:openai-codex:gpt-5.6-terra".into())]
        );
        let structured = error.downcast_ref::<ExecutionRouteFailure>().unwrap();
        assert_eq!(structured.code, "WG-EXEC-ROUTE-FAILED");
        assert_eq!(structured.system.handler, "pi");
        assert_eq!(structured.system.provider, "openai-codex");
        assert_eq!(structured.attempts.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn persisted_plan_invokes_exact_codex_pi_and_claude_handlers() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for (name, script) in [
            (
                "codex",
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$WG_TEST_CODEX_ARGS\"\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"codex-ok\"}}'\nprintf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n",
            ),
            (
                "pi",
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$WG_TEST_PI_ARGS\"\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"pi-ok\"}],\"usage\":{\"input\":1,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":2,\"cost\":{\"total\":0.0}}}}'\n",
            ),
            (
                "claude",
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$WG_TEST_CLAUDE_ARGS\"\ncat >/dev/null\nprintf '%s\\n' '{\"result\":\"claude-ok\"}'\n",
            ),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old_path = std::env::var("PATH").unwrap_or_default();
        let _path = EnvGuard::set("PATH", Some(&format!("{}:{old_path}", bin.display())));
        let codex_args = temp.path().join("codex.args");
        let pi_args = temp.path().join("pi.args");
        let claude_args = temp.path().join("claude.args");
        let _codex = EnvGuard::set("WG_TEST_CODEX_ARGS", codex_args.to_str());
        let _pi = EnvGuard::set("WG_TEST_PI_ARGS", pi_args.to_str());
        let _claude = EnvGuard::set("WG_TEST_CLAUDE_ARGS", claude_args.to_str());

        let mut config = Config::default();
        config
            .models
            .set_model(DispatchRole::Evaluator, "claude:wrong-ambient");
        for (route, expected) in [
            ("codex:gpt-5.5", "codex-ok"),
            ("pi:openai-codex:gpt-5.6-sol", "pi-ok"),
            ("claude:haiku", "claude-ok"),
        ] {
            let call = crate::eval_lifecycle::AgencyCallPlan {
                stage: crate::eval_lifecycle::AgencyStage::Evaluate,
                route: route.into(),
                endpoint: None,
                reasoning: None,
                system: execution_system_key(route).unwrap(),
                source: crate::eval_lifecycle::DispatchSelectionSource::PersistedPlan,
                fallbacks: vec![],
            };
            let result = run_lightweight_llm_call_for_plan(
                &config,
                DispatchRole::Evaluator,
                &call,
                "return exact handler",
                10,
            )
            .unwrap();
            assert_eq!(result.text, expected);
        }
        assert!(
            std::fs::read_to_string(codex_args)
                .unwrap()
                .contains("--model gpt-5.5")
        );
        let pi_argv = std::fs::read_to_string(pi_args).unwrap();
        assert!(pi_argv.contains("--provider openai-codex"));
        assert!(pi_argv.contains("--model gpt-5.6-sol"));
        assert!(
            std::fs::read_to_string(claude_args)
                .unwrap()
                .contains("--model haiku")
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn test_failing_pi_process_never_executes_claude_process() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let pi_marker = temp.path().join("pi.marker");
        let claude_marker = temp.path().join("claude.marker");
        let pi = bin.join("pi");
        let claude = bin.join("claude");
        std::fs::write(
            &pi,
            "#!/bin/sh\nprintf invoked > \"$WG_TEST_PI_MARKER\"\ncat >/dev/null\necho injected-pi-failure >&2\nexit 41\n",
        )
        .unwrap();
        std::fs::write(
            &claude,
            "#!/bin/sh\nprintf invoked > \"$WG_TEST_CLAUDE_MARKER\"\nexit 42\n",
        )
        .unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();

        let old_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{old_path}", bin.display());
        let _path = EnvGuard::set("PATH", Some(&path));
        let _pi_marker = EnvGuard::set("WG_TEST_PI_MARKER", Some(pi_marker.to_str().unwrap()));
        let _claude_marker = EnvGuard::set(
            "WG_TEST_CLAUDE_MARKER",
            Some(claude_marker.to_str().unwrap()),
        );

        let mut config = Config::default();
        config.tiers.fast = Some("pi:openai-codex:gpt-5.6-terra".into());
        let error =
            run_lightweight_llm_call(&config, DispatchRole::Evaluator, "return a verdict", 10)
                .unwrap_err();

        assert!(pi_marker.exists(), "selected Pi process was not attempted");
        assert!(
            !claude_marker.exists(),
            "Claude process ran after a selected Pi route failed"
        );
        let structured = error.downcast_ref::<ExecutionRouteFailure>().unwrap();
        assert_eq!(structured.code, "WG-EXEC-ROUTE-FAILED");
        assert_eq!(structured.attempts.len(), 1);
        assert!(
            structured.attempts[0]
                .failure
                .contains("injected-pi-failure")
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn test_generic_lightweight_routes_never_cross_system_and_explicit_claude_still_runs() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let codex_marker = temp.path().join("codex.marker");
        let claude_marker = temp.path().join("claude.marker");
        let pi_marker = temp.path().join("pi.marker");
        for (name, script) in [
            (
                "codex",
                "#!/bin/sh\nprintf invoked > \"$WG_TEST_CODEX_MARKER\"\ncat >/dev/null\necho injected-codex-failure >&2\nexit 43\n",
            ),
            (
                "claude",
                "#!/bin/sh\nprintf invoked > \"$WG_TEST_CLAUDE_MARKER\"\ncat >/dev/null\nprintf '%s\\n' '{\"result\":\"explicit claude success\"}'\n",
            ),
            (
                "pi",
                "#!/bin/sh\nprintf invoked > \"$WG_TEST_PI_MARKER\"\ncat >/dev/null\nexit 44\n",
            ),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{old_path}", bin.display());
        let _path = EnvGuard::set("PATH", Some(&path));
        let _codex_marker =
            EnvGuard::set("WG_TEST_CODEX_MARKER", Some(codex_marker.to_str().unwrap()));
        let _claude_marker = EnvGuard::set(
            "WG_TEST_CLAUDE_MARKER",
            Some(claude_marker.to_str().unwrap()),
        );
        let _pi_marker = EnvGuard::set("WG_TEST_PI_MARKER", Some(pi_marker.to_str().unwrap()));
        let _openrouter_key = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _openai_key = EnvGuard::set("OPENAI_API_KEY", None);
        let _anthropic_key = EnvGuard::set("ANTHROPIC_API_KEY", None);

        let mut codex_config = Config::default();
        codex_config
            .models
            .set_model(DispatchRole::Triage, "codex:gpt-5.5");
        let error = run_lightweight_llm_call(&codex_config, DispatchRole::Triage, "summarize", 10)
            .unwrap_err();
        assert!(codex_marker.exists(), "selected Codex process did not run");
        assert!(!claude_marker.exists(), "Codex failure invoked Claude");
        assert!(!pi_marker.exists(), "Codex failure invoked Pi");
        assert!(error.downcast_ref::<ExecutionRouteFailure>().is_some());

        let mut native_config = Config::default();
        native_config
            .models
            .set_model(DispatchRole::Triage, "nex:openrouter:z-ai/glm-5.2");
        let error = run_lightweight_llm_call(&native_config, DispatchRole::Triage, "summarize", 10)
            .unwrap_err();
        assert!(format!("{error:#}").contains("missing credential"));
        assert!(!claude_marker.exists(), "OpenRouter failure invoked Claude");
        assert!(!pi_marker.exists(), "OpenRouter failure invoked Pi");

        let mut claude_config = Config::default();
        claude_config
            .models
            .set_model(DispatchRole::Triage, "claude:haiku");
        let result =
            run_lightweight_llm_call(&claude_config, DispatchRole::Triage, "summarize", 10)
                .unwrap();
        assert_eq!(result.text, "explicit claude success");
        assert!(
            claude_marker.exists(),
            "explicit Claude route did not run Claude"
        );
        assert!(!pi_marker.exists(), "explicit Claude route invoked Pi");
    }

    #[test]
    fn test_explicit_same_system_fallback_runs_in_file_order() {
        let mut config = Config::default();
        config.execution.fallbacks.push(ExecutionFallback {
            primary: "pi:openai-codex:gpt-5.6-terra".into(),
            models: vec!["pi:openai-codex:gpt-5.6-sol".into()],
        });
        let primary = agency_dispatch_for_spec("pi:openai-codex:gpt-5.6-terra", None);
        let mut attempted = Vec::new();
        let result = run_dispatch_with_same_system_fallback(
            &config,
            DispatchRole::Reviewer,
            primary,
            |route| {
                attempted.push(route.raw_spec.clone());
                if route.raw_spec.ends_with("terra") {
                    anyhow::bail!("injected primary failure")
                }
                Ok(fake_success("fallback verdict"))
            },
        )
        .unwrap();

        assert_eq!(result.text, "fallback verdict");
        assert_eq!(
            attempted,
            vec![
                "pi:openai-codex:gpt-5.6-terra",
                "pi:openai-codex:gpt-5.6-sol"
            ]
        );
    }

    #[test]
    fn test_duplicate_same_system_fallback_route_is_attempted_once() {
        let mut config = Config::default();
        config.execution.fallbacks.push(ExecutionFallback {
            primary: "codex:gpt-5.5".into(),
            models: vec![
                "codex:gpt-5.5".into(),
                "codex:gpt-5.5-mini".into(),
                "codex:gpt-5.5-mini".into(),
            ],
        });
        let mut attempted = Vec::new();
        let error = run_dispatch_with_same_system_fallback(
            &config,
            DispatchRole::Evaluator,
            agency_dispatch_for_spec("codex:gpt-5.5", None),
            |route| {
                attempted.push(route.raw_spec.clone());
                anyhow::bail!("injected failure")
            },
        )
        .unwrap_err();

        assert_eq!(attempted, ["codex:gpt-5.5", "codex:gpt-5.5-mini"]);
        assert_eq!(
            error
                .downcast_ref::<ExecutionRouteFailure>()
                .unwrap()
                .attempts
                .len(),
            2
        );
    }

    #[test]
    fn test_cross_system_fallback_is_rejected_before_any_call() {
        let mut config = Config::default();
        config.execution.fallbacks.push(ExecutionFallback {
            primary: "codex:gpt-5.5".into(),
            models: vec!["claude:haiku".into()],
        });
        let mut called = false;
        let error = run_dispatch_with_same_system_fallback(
            &config,
            DispatchRole::Assigner,
            agency_dispatch_for_spec("codex:gpt-5.5", None),
            |_| {
                called = true;
                Ok(fake_success("must not run"))
            },
        )
        .unwrap_err();
        assert!(!called);
        assert!(format!("{error:#}").contains("WG-EXEC-FALLBACK-CROSS-SYSTEM"));
    }

    #[test]
    fn test_every_one_shot_role_obeys_no_cross_system_failure_contract() {
        for (role, route, expected_handler) in [
            (
                DispatchRole::Evaluator,
                "codex:gpt-5.5",
                ExecutorKind::Codex,
            ),
            (
                DispatchRole::Assigner,
                "claude:sonnet",
                ExecutorKind::Claude,
            ),
            (
                DispatchRole::FlipInference,
                "pi:openai-codex:gpt-5.6-terra",
                ExecutorKind::Pi,
            ),
            (
                DispatchRole::FlipComparison,
                "nex:openrouter:z-ai/glm-5.2",
                ExecutorKind::Native,
            ),
            (
                DispatchRole::Reviewer,
                "pi:openai-codex:gpt-5.6-terra",
                ExecutorKind::Pi,
            ),
            (DispatchRole::Triage, "codex:gpt-5.5", ExecutorKind::Codex),
            (
                DispatchRole::TaskAgent,
                "pi:openai-codex:gpt-5.6-terra",
                ExecutorKind::Pi,
            ),
        ] {
            let config = Config::default();
            let mut attempted = Vec::new();
            let error = run_dispatch_with_same_system_fallback(
                &config,
                role,
                agency_dispatch_for_spec(route, None),
                |dispatch| {
                    attempted.push(dispatch.handler);
                    anyhow::bail!("injected failure")
                },
            )
            .unwrap_err();
            assert_eq!(attempted, vec![expected_handler], "role={role}");
            assert!(error.downcast_ref::<ExecutionRouteFailure>().is_some());
        }
    }

    #[test]
    fn test_production_agency_dispatch_has_no_hardcoded_claude_fallback() {
        let source = include_str!("llm.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("AGENCY_CLAUDE_HAIKU_SPEC"));
        assert!(!production.contains("falling back to claude"));
        assert!(!production.contains("call_claude_cli(CLAUDE_HAIKU_MODEL_ID"));
        assert!(production.contains("fallback_route={route:?} outcome=success"));
    }
}
