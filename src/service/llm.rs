//! Lightweight LLM dispatch for internal WG calls (triage, checkpoint, etc.).
//!
//! Resolves model + provider via `resolve_model_for_role()` and dispatches to either:
//! - Claude CLI (`claude --model X --print --dangerously-skip-permissions PROMPT`)
//! - Native Anthropic API client (when provider is "anthropic" and native executor is configured)
//! - Native OpenAI-compatible API client (when provider is "openai"/"openrouter")

use std::io::{BufRead, BufReader};
use std::process;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{
    CLAUDE_FABLE_MODEL_ID, CLAUDE_HAIKU_MODEL_ID, CLAUDE_OPUS_MODEL_ID, CLAUDE_SONNET_MODEL_ID,
    Config, DispatchRole, ModelRegistryEntry, parse_model_spec, strip_native_handler_prefix,
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

/// The provider-qualified spec the agency pipeline degrades to when its weak
/// tier points at a keyless native provider. Written with the `claude:` prefix
/// (rather than the bare `haiku` alias) so the registry label matches the
/// default weak-tier value `effective_tiers()` produces.
const AGENCY_CLAUDE_HAIKU_SPEC: &str = "claude:haiku";

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

/// Build an [`AgencyDispatch`] from a raw model spec. Claude-family models
/// (even when written `anthropic/…` or `openrouter:anthropic/…`) route through
/// the claude CLI with the bare family alias; everything else routes via
/// `handler_for_model`.
fn agency_dispatch_for_spec(raw_spec: &str) -> AgencyDispatch {
    let spec = parse_model_spec(raw_spec);
    let claude_cli_alias = claude_cli_alias_for_model(&spec.model_id);
    let handler = if claude_cli_alias.is_some() {
        ExecutorKind::Claude
    } else {
        handler_for_model(raw_spec)
    };

    AgencyDispatch {
        handler,
        raw_spec: raw_spec.to_string(),
        model_id: claude_cli_alias.unwrap_or(&spec.model_id).to_string(),
    }
}

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
        .unwrap_or("anthropic")
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
        "openrouter" | "openai" => {
            env_present(&["OPENROUTER_API_KEY", "OPENAI_API_KEY"]) || endpoint_present(provider)
        }
        "anthropic" => env_present(&["ANTHROPIC_API_KEY"]) || endpoint_present(provider),
        // `local` / `oai-compat` (localhost nex) require no key, and any other
        // provider is not our concern — don't block agency dispatch on it.
        _ => true,
    }
}

/// Resolve which handler+model an agency one-shot role should dispatch to.
///
/// Contract (matches CLAUDE.md "explicit overrides win, cascade does not"):
///
/// - Explicit `[models.<role>].model` → use that spec; route via
///   `handler_for_model` (so `codex:X` runs on `codex` CLI, `openrouter:X`
///   runs through the native HTTP path, etc).
/// - No explicit per-role model → resolve the profile's **weak** two-tier
///   label (`tiers.fast`, the cheap tier). For the default / no-profile config
///   the weak tier IS `claude:haiku`, so the historical agency pin is
///   preserved; a two-tier Pi profile that sets `--weak
///   openrouter:deepseek/<model>` now flows through to `.flip` / `.assign` /
///   `.evaluate`. Project-level cascade from `coordinator.model` /
///   `[models.default]` is still ignored on purpose.
///
/// Credential safety: when the model comes from the **weak-tier fallback** and
/// resolves to a native-HTTP provider that needs an API key (OpenRouter /
/// OpenAI / Anthropic-native) with none available, fall back to `claude:haiku`
/// on the claude CLI and warn loudly. This preserves the original pin's second
/// guarantee — agency verdicts are never *silently* dropped because "openrouter
/// is configured but there's no key". An **explicit** per-role override is NOT
/// pre-empted at resolve time (explicit overrides win and keep their declared
/// route); it is still protected from silent drops at call time, where
/// `agency_native_lightweight_call` falls back to claude:haiku on any native
/// failure. (claude / codex CLI targets self-authenticate, so they are never
/// downgraded either way.)
pub fn resolve_agency_dispatch(config: &Config, role: DispatchRole) -> AgencyDispatch {
    debug_assert!(
        is_agency_oneshot_role(role),
        "resolve_agency_dispatch is only valid for agency one-shot roles"
    );

    // Explicit per-role override ([models.evaluator] / [models.assigner] /
    // [models.flip_*]) wins and routes to its declared handler unconditionally.
    if let Some(spec) = config.models.get_role(role).and_then(|c| c.model.clone()) {
        return agency_dispatch_for_spec(&spec);
    }

    // No override: resolve the WEAK tier (tiers.fast). `weak_tier_spec()` always
    // yields a value (hardcoded `claude:haiku` for the bare default), so the
    // `unwrap_or_else` is purely defensive.
    let raw_spec = config
        .weak_tier_spec()
        .unwrap_or_else(|| CLAUDE_HAIKU_MODEL_ID.to_string());
    let dispatch = agency_dispatch_for_spec(&raw_spec);

    // Credential safety net for the default weak-tier path: a native-HTTP target
    // with no usable key would otherwise produce a confusing 401 (or a broken
    // "fall back to claude CLI with a non-claude --model" call) and silently
    // drop the verdict. Detect that here and degrade to claude:haiku with a loud
    // warning instead, so the spawn-site registry label and the call site agree.
    if dispatch.handler == ExecutorKind::Native && !agency_native_creds_available(config, &raw_spec)
    {
        eprintln!(
            "[agency-dispatch] role={role} resolved to weak-tier model '{raw_spec}', but no API \
             key is available for its provider — falling back to claude:haiku on the claude CLI \
             so agency verdicts are not silently dropped. Configure the key (e.g. set \
             OPENROUTER_API_KEY or `wg endpoints add`) to run agency on the configured model."
        );
        return agency_dispatch_for_spec(AGENCY_CLAUDE_HAIKU_SPEC);
    }

    dispatch
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

/// Dispatch one content-review LLM call at the weak or strong tier (the real reviewer
/// silicon). Weak resolves via [`resolve_agency_dispatch`] for
/// [`DispatchRole::Reviewer`] (so an explicit `[models.reviewer]` override wins, else
/// the weak two-tier label, with the same loud claude:haiku credential safety net);
/// strong resolves [`Config::strong_tier_spec`] (premium → standard). On a native
/// call failure the dispatch falls back to claude:haiku so the call still returns a
/// reply — the *content* fail-closed decision is the caller's
/// (`review::reviewer::review_with_llm`), here we only avoid a silent transport drop.
pub fn run_review_llm_call(
    config: &Config,
    strong: bool,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let dispatch = if strong {
        let spec = config
            .strong_tier_spec()
            .unwrap_or_else(|| CLAUDE_OPUS_MODEL_ID.to_string());
        agency_dispatch_for_spec(&spec)
    } else {
        resolve_agency_dispatch(config, DispatchRole::Reviewer)
    };

    match dispatch.handler {
        ExecutorKind::Claude => call_claude_cli(&dispatch.model_id, prompt, timeout_secs),
        ExecutorKind::Codex => call_codex_cli(&dispatch.model_id, prompt, timeout_secs),
        ExecutorKind::Native => {
            match agency_native_call_for_spec(config, &dispatch.raw_spec, prompt, timeout_secs) {
                Ok(result) => Ok(result),
                Err(e) => {
                    eprintln!(
                        "[review-dispatch] native {} reviewer call failed: {e:#} — \
                         falling back to claude:haiku for the call",
                        dispatch.raw_spec
                    );
                    call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs)
                }
            }
        }
        ExecutorKind::Pi => {
            // Handler-first `pi:` route — drive `pi` as a one-shot, falling
            // back to claude:haiku on any failure so the reviewer transport is
            // never silently dropped (the content fail-closed decision stays
            // the caller's). Mirrors the agency one-shot pi arm.
            match call_pi_cli(config, &dispatch.raw_spec, prompt, timeout_secs) {
                Ok(result) => Ok(result),
                Err(e) => {
                    eprintln!(
                        "[review-dispatch] pi {} reviewer call failed: {e:#} — \
                         falling back to claude:haiku for the call",
                        dispatch.raw_spec
                    );
                    call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs)
                }
            }
        }
        // Any other handler is not a sensible one-shot reviewer target — degrade to
        // the safe default (claude CLI on haiku).
        _ => call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs),
    }
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
    let dispatch = agency_dispatch_for_spec(model_spec);
    match dispatch.handler {
        ExecutorKind::Claude => call_claude_cli(&dispatch.model_id, prompt, timeout_secs),
        ExecutorKind::Codex => call_codex_cli(&dispatch.model_id, prompt, timeout_secs),
        ExecutorKind::Native => {
            agency_native_call_for_spec(config, &dispatch.raw_spec, prompt, timeout_secs)
        }
        other => anyhow::bail!(
            "model spec {model_spec:?} resolves to handler {other:?}, which is not a supported \
             one-shot worker backend (use a claude/codex/native model, or set --worker-cmd)"
        ),
    }
}

/// Make a native-HTTP one-shot call resolving the provider directly from a model
/// **spec** (not a role). Used by the reviewer strong-tier path, where the model is
/// the premium tier rather than the cascade-resolved role model.
fn agency_native_call_for_spec(
    config: &Config,
    raw_spec: &str,
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
            None,
        ),
        prov @ ("oai-compat" | "openai" | "openrouter" | "local") => call_openai_native(
            config,
            prov,
            &spec.model_id,
            prompt,
            timeout_secs,
            None,
            None,
        ),
        other => anyhow::bail!("reviewer spec {raw_spec:?} provider {other:?} is not native HTTP"),
    }
}

/// Make the native-HTTP lightweight call for an agency role whose weak tier
/// resolved to a native provider (`openrouter` / `openai` / `oai-compat` /
/// `local` / `anthropic`). Returns `Err` if the resolved provider is not a
/// native HTTP one, or the call itself fails — the caller falls back to
/// `claude:haiku` so the agency verdict is never dropped.
fn agency_native_lightweight_call(
    config: &Config,
    role: DispatchRole,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    let resolved = config.resolve_model_for_role(role);
    let model = &resolved.model;
    let registry_entry = resolved.registry_entry.as_ref();
    let endpoint_name = resolved.endpoint.as_deref();

    match resolved.provider.as_deref() {
        Some("anthropic") => call_anthropic_native(
            config,
            "anthropic",
            model,
            prompt,
            timeout_secs,
            registry_entry,
            endpoint_name,
        ),
        Some(prov @ ("oai-compat" | "openai" | "openrouter" | "local")) => call_openai_native(
            config,
            prov,
            model,
            prompt,
            timeout_secs,
            registry_entry,
            endpoint_name,
        ),
        other => anyhow::bail!(
            "agency role {role} weak-tier provider {other:?} is not a native HTTP provider"
        ),
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

/// Run a lightweight (no tool-use) LLM call for an internal dispatch role.
///
/// Resolves the model and provider for the given role, then dispatches via:
/// 1. Agency one-shot roles (Evaluator, FlipInference, FlipComparison,
///    Assigner) resolve via `resolve_agency_dispatch`: an explicit per-role
///    override, else the profile's weak tier (`tiers.fast`), with a loud
///    fall back to `claude:haiku` when the weak tier is a keyless native
///    provider. This keeps agency cheap and immune to `coordinator.model`
///    cascade silently routing them through a provider that lacks credentials.
/// 2. If `provider` is set to a native provider ("anthropic", "openai",
///    "openrouter"), attempts a direct API call using the native client.
///    Native-call errors are surfaced (logged to stderr) before falling
///    back to the claude CLI.
/// 3. Falls back to shelling out to `claude` CLI.
///
/// Returns both the text response and token usage when available.
pub fn run_lightweight_llm_call(
    config: &Config,
    role: DispatchRole,
    prompt: &str,
    timeout_secs: u64,
) -> Result<LlmCallResult> {
    if is_agency_oneshot_role(role) {
        let dispatch = resolve_agency_dispatch(config, role);
        // For CLI-handler targets (claude, codex), route directly to the
        // CLI — the `provider_to_native_provider` mapping in the cascade
        // resolver collapses `codex` → `oai-compat`, which would otherwise
        // misroute the call into the OpenAI-compat HTTP client (no key /
        // wrong endpoint for codex CLI users).
        match dispatch.handler {
            ExecutorKind::Claude => {
                return call_claude_cli(&dispatch.model_id, prompt, timeout_secs);
            }
            ExecutorKind::Codex => {
                return call_codex_cli(&dispatch.model_id, prompt, timeout_secs);
            }
            ExecutorKind::Native => {
                // Weak tier points at a native HTTP provider (e.g.
                // `openrouter:deepseek/...`). Make the call directly; on ANY
                // failure (invalid key, timeout, 5xx) fall back to claude:haiku
                // so the agency verdict is never silently dropped. The
                // missing-key case is already redirected to claude inside
                // `resolve_agency_dispatch`, so reaching here means a key was
                // present — but it could still be rejected at request time.
                match agency_native_lightweight_call(config, role, prompt, timeout_secs) {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        eprintln!(
                            "[agency-dispatch] native weak-tier call for role={role} failed: \
                             {e:#} — falling back to claude:haiku so the agency verdict is not \
                             dropped",
                        );
                        return call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs);
                    }
                }
            }
            ExecutorKind::Pi => {
                // Handler-first `pi:` route (e.g.
                // `pi:openrouter:deepseek/deepseek-chat`). Drive `pi` as a
                // one-shot `--mode json` call and parse the NDJSON stream. On
                // ANY failure (no key, no binary, 5xx, empty reply) fall back
                // to claude:haiku so the agency verdict is never silently
                // dropped — mirroring the native handler's safety net. This is
                // the fix for the bug where a `pi:` weak tier silently fell
                // into the claude-CLI catch-all below and failed with
                // "Claude CLI call failed" instead of honoring the pi route.
                match call_pi_cli(config, &dispatch.raw_spec, prompt, timeout_secs) {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        eprintln!(
                            "[agency-dispatch] pi weak-tier call for role={role} \
                             (spec={spec}) failed: {e:#} — falling back to claude:haiku so the \
                             agency verdict is not dropped",
                            spec = dispatch.raw_spec,
                        );
                        return call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs);
                    }
                }
            }
            ExecutorKind::Shell
            | ExecutorKind::OpenCode
            | ExecutorKind::Aider
            | ExecutorKind::Goose
            | ExecutorKind::Qwen
            | ExecutorKind::Cline
            | ExecutorKind::Crush
            | ExecutorKind::Amplifier
            | ExecutorKind::Octomind
            | ExecutorKind::Dexto
            | ExecutorKind::RemoteRunner => {
                // Shell, external task/chat executors, and the WG-Exec remote runner do
                // not make sense for a lightweight one-shot LLM call; degrade to the
                // safe default (claude CLI on haiku).
                return call_claude_cli(CLAUDE_HAIKU_MODEL_ID, prompt, timeout_secs);
            }
        }
    }

    let resolved = config.resolve_model_for_role(role);
    let model = &resolved.model;
    let provider = resolved.provider.as_deref();
    let registry_entry = resolved.registry_entry.as_ref();
    let endpoint_name = resolved.endpoint.as_deref();

    // Try native API call if provider is explicitly configured. Native-call
    // errors used to be swallowed here, leaving the daemon log silent on
    // why we fell back. Surface the error so misconfigurations (e.g. an
    // openrouter provider with no API key) are diagnosable.
    if let Some(prov) = provider {
        match prov {
            "anthropic" => match call_anthropic_native(
                config,
                prov,
                model,
                prompt,
                timeout_secs,
                registry_entry,
                endpoint_name,
            ) {
                Ok(result) => return Ok(result),
                Err(e) => eprintln!(
                    "[lightweight-llm] native anthropic call failed for role={role} model={model}: {e:#} — falling back to claude CLI",
                ),
            },
            "oai-compat" | "openai" | "openrouter" | "local" => {
                match call_openai_native(
                    config,
                    prov,
                    model,
                    prompt,
                    timeout_secs,
                    registry_entry,
                    endpoint_name,
                ) {
                    Ok(result) => return Ok(result),
                    Err(e) => eprintln!(
                        "[lightweight-llm] native {prov} call failed for role={role} model={model}: {e:#} — falling back to claude CLI",
                    ),
                }
            }
            "codex" => return call_codex_cli(model, prompt, timeout_secs),
            _ => {}
        }
    }

    call_claude_cli(model, prompt, timeout_secs)
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
fn call_codex_cli(model: &str, prompt: &str, timeout_secs: u64) -> Result<LlmCallResult> {
    use std::io::Write as _;

    // Cross-platform `timeout(1)` replacement — Windows has no equivalent.
    // Same in-process call-site treatment njt's #22 applied to call_claude_cli.
    let (mut child, _killer) = crate::platform_timeout::spawn_with_timeout(
        "codex",
        |cmd| {
            cmd.arg("exec")
                .arg("--json")
                .arg("--skip-git-repo-check")
                .arg("--dangerously-bypass-approvals-and-sandbox")
                .arg("--model")
                .arg(model)
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
/// argv). Mirrors `commands::pi_handler::PiEndpointSecret` resolution: a
/// matching provider endpoint → default endpoint → provider env vars only. The
/// daemon's own ambient env (e.g. an exported `OPENROUTER_API_KEY`) is left
/// untouched so pi's own provider clients still discover it.
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
        })
        .or_else(|| {
            config.llm_endpoints.find_default().and_then(|ep| {
                ep.resolve_api_key(workgraph_dir)
                    .ok()
                    .flatten()
                    .map(|k| (ep.url.clone(), k))
            })
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
/// disabled (`--no-tools`) and the session is not persisted (`--no-session`),
/// matching the no-tool-use contract of the other agency one-shot callers.
///
/// Credentials are supplied by environment ONLY (never `--api-key`): a
/// WG-resolved endpoint key is injected as the provider's env var
/// (`OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / …) so pi's own provider
/// clients discover it, mirroring `wg pi-handler`. When no WG endpoint key
/// resolves, pi falls back to its own auth (env / OAuth / `~/.pi` login).
fn call_pi_cli(
    config: &Config,
    raw_spec: &str,
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
            cmd.arg("--mode")
                .arg("json")
                .arg("--print")
                .arg("--no-tools")
                .arg("--no-session")
                .arg("--provider")
                .arg(&marg.provider)
                .arg("--model")
                .arg(&marg.model)
                .stdin(process::Stdio::piped())
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
    let env_key = ["OPENROUTER_API_KEY", "OPENAI_API_KEY"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|k| !k.is_empty()));
    let resolved_key = env_key.or(endpoint_key);

    let mut client = if let Some(key) = resolved_key {
        OpenAiClient::new(key, model, None)
            .context("Failed to create OpenAI client for lightweight call")?
    } else if provider_name == "local" {
        // Local providers don't require auth
        OpenAiClient::new("local".to_string(), model, None).expect("infallible with static args")
    } else {
        // Legacy fallback
        OpenAiClient::from_env(model)
            .context("Failed to create OpenAI client for lightweight call")?
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
    use crate::config::{CLAUDE_HAIKU_MODEL_ID, Config, DispatchRole, ModelRegistryEntry, Tier};

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
    fn test_resolve_agency_dispatch_default_weak_tier_is_claude_haiku() {
        // No [models.<role>] override and no [tiers] — the weak tier defaults to
        // claude:haiku on the claude CLI handler, ignoring any project-level
        // cascade. The historical pin is preserved, now expressed as the
        // provider-qualified weak-tier spec.
        let mut config = Config::default();
        config.coordinator.model = Some("openrouter:anthropic/claude-sonnet-4-6".to_string());

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Assigner);
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.raw_spec, "claude:haiku");
        assert_eq!(dispatch.model_id, CLAUDE_HAIKU_MODEL_ID);
    }

    #[test]
    fn test_resolve_agency_dispatch_codex_override_routes_to_codex_cli() {
        // Reproduces the autohaiku regression: `wg init --route codex-cli`
        // writes [models.assigner].model = "codex:gpt-5.4-mini" but the
        // runtime fell back to claude. The fix routes via handler_for_model
        // so the explicit override actually lands on the codex CLI.
        let mut config = Config::default();
        config
            .models
            .set_model(DispatchRole::Assigner, "codex:gpt-5.4-mini");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Assigner);
        assert_eq!(
            dispatch.handler,
            ExecutorKind::Codex,
            "explicit codex:* override must dispatch via codex CLI, not claude"
        );
        assert_eq!(dispatch.raw_spec, "codex:gpt-5.4-mini");
        assert_eq!(
            dispatch.model_id, "gpt-5.4-mini",
            "model_id must strip the provider prefix for `--model` arg"
        );
    }

    #[test]
    fn test_resolve_agency_dispatch_codex_override_for_evaluator_and_flip() {
        // Same TDD coverage for Evaluator, FlipInference, FlipComparison —
        // the codex-cli init route writes ALL FOUR roles, so they must all
        // route via codex CLI.
        for role in [
            DispatchRole::Evaluator,
            DispatchRole::FlipInference,
            DispatchRole::FlipComparison,
            DispatchRole::Assigner,
        ] {
            let mut config = Config::default();
            config.models.set_model(role, "codex:gpt-5.4-mini");
            let dispatch = resolve_agency_dispatch(&config, role);
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Codex,
                "role {:?} with codex override must route to codex CLI",
                role
            );
            assert_eq!(dispatch.model_id, "gpt-5.4-mini", "role {:?}", role);
        }
    }

    #[test]
    fn test_resolve_agency_dispatch_claude_override_keeps_claude_cli() {
        // A user who explicitly sets `[models.evaluator].model = "claude:sonnet"`
        // gets claude CLI on sonnet (not the haiku default).
        let mut config = Config::default();
        config
            .models
            .set_model(DispatchRole::Evaluator, "claude:sonnet");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.raw_spec, "claude:sonnet");
        assert_eq!(dispatch.model_id, "sonnet");
    }

    #[test]
    fn test_resolve_agency_dispatch_anthropic_slash_model_routes_to_claude_cli() {
        // Regression for fix-evaluator-role: registry/OpenRouter model IDs
        // like `anthropic/claude-haiku-4-5` are Claude models, but the
        // Claude CLI only accepts the bare family alias.
        let mut config = Config::default();
        config
            .models
            .set_model(DispatchRole::Evaluator, "anthropic/claude-haiku-4-5");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(
            dispatch.handler,
            ExecutorKind::Claude,
            "slash-form Claude evaluator model must bypass native/OpenRouter"
        );
        assert_eq!(dispatch.raw_spec, "anthropic/claude-haiku-4-5");
        assert_eq!(dispatch.model_id, "haiku");
    }

    #[test]
    fn test_resolve_agency_dispatch_openrouter_claude_model_bypasses_openrouter() {
        // Even when the model is written with an OpenRouter prefix, agency
        // one-shot roles should not detour through OpenRouter just to call a
        // Claude-family model.
        let mut config = Config::default();
        config.models.set_model(
            DispatchRole::Evaluator,
            "openrouter:anthropic/claude-sonnet-4-6",
        );

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.raw_spec, "openrouter:anthropic/claude-sonnet-4-6");
        assert_eq!(dispatch.model_id, "sonnet");
    }

    #[test]
    fn test_resolve_agency_dispatch_anthropic_slash_model_for_all_agency_roles() {
        // Audit coverage: evaluator, assigner, and both FLIP phases share the
        // same one-shot dispatch path.
        for (role, model, expected_alias) in [
            (
                DispatchRole::Evaluator,
                "anthropic/claude-haiku-4-5",
                "haiku",
            ),
            (
                DispatchRole::FlipInference,
                "anthropic/claude-sonnet-4-6",
                "sonnet",
            ),
            (
                DispatchRole::FlipComparison,
                "anthropic/claude-opus-4-7",
                "opus",
            ),
            (
                DispatchRole::Assigner,
                "anthropic/claude-3.5-haiku",
                "haiku",
            ),
        ] {
            let mut config = Config::default();
            config.models.set_model(role, model);
            let dispatch = resolve_agency_dispatch(&config, role);
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Claude,
                "role {:?} should route Claude-family slash models to Claude CLI",
                role
            );
            assert_eq!(dispatch.model_id, expected_alias, "role {:?}", role);
        }
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
        // Fable has no bare CLI shortcut: the friendly alias and every dated
        // spelling normalize to the full CLI id `claude-fable-5`.
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
    fn test_agency_dispatch_claude_fable_routes_to_claude_cli() {
        // `claude:fable` as an explicit agency role override must dispatch on
        // the claude CLI handler with the expanded `claude-fable-5` model id.
        let dispatch = agency_dispatch_for_spec("claude:fable");
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.model_id, CLAUDE_FABLE_MODEL_ID);

        // A full anthropic/openrouter spelling also routes to the claude CLI.
        let dispatch = agency_dispatch_for_spec("openrouter:anthropic/claude-fable-5");
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.model_id, CLAUDE_FABLE_MODEL_ID);
    }

    #[test]
    fn test_resolve_agency_dispatch_native_override_routes_to_native() {
        // openrouter:* / local:* / oai-compat:* explicit overrides for
        // non-Claude models keep the existing native HTTP dispatch path.
        let mut config = Config::default();
        config
            .models
            .set_model(DispatchRole::Assigner, "openrouter:qwen/qwen3-coder");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Assigner);
        assert_eq!(dispatch.handler, ExecutorKind::Native);
        assert_eq!(dispatch.raw_spec, "openrouter:qwen/qwen3-coder");
        assert_eq!(dispatch.model_id, "qwen/qwen3-coder");
    }

    #[test]
    fn test_agency_role_ignores_coordinator_model_cascade() {
        // Reproduces today's outage: project sets coordinator.model to an
        // openrouter spec, no per-role config exists. Without the bypass,
        // the resolved provider for Evaluator cascades to "openrouter" and
        // the call would silently route through the OpenAI-compat path.
        // After the fix, agency one-shot roles ignore this cascade and we
        // run claude CLI on claude:haiku regardless.
        let mut config = Config::default();
        config.coordinator.model = Some("openrouter:anthropic/claude-sonnet-4-6".to_string());

        // Sanity: the cascade *would* have polluted the resolved provider.
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(
            resolved.provider.as_deref(),
            Some("openrouter"),
            "cascade pollution exists at the resolver level — exactly the case the bypass guards against"
        );

        // The bypass kicks in because no per-role explicit override is set —
        // resolve_agency_dispatch ignores cascade and resolves the weak tier,
        // which for the default config is claude:haiku on the claude CLI.
        assert!(is_agency_oneshot_role(DispatchRole::Evaluator));
        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.raw_spec, "claude:haiku");
        assert_eq!(dispatch.model_id, CLAUDE_HAIKU_MODEL_ID);
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

    const ALL_AGENCY_ROLES: [DispatchRole; 4] = [
        DispatchRole::Evaluator,
        DispatchRole::FlipInference,
        DispatchRole::FlipComparison,
        DispatchRole::Assigner,
    ];

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_deepseek_with_key() {
        // The two-tier setter wrote `--weak openrouter:deepseek/deepseek-chat`
        // into tiers.fast. With an OpenRouter key present, agency one-shots
        // route to that DeepSeek model via the native HTTP handler — NOT the
        // old hardcoded claude:haiku. (Covers validation item 1.)
        let _key = EnvGuard::set("OPENROUTER_API_KEY", Some("sk-or-test-deepseek"));
        let mut config = Config::default();
        config.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role);
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Native,
                "role {role:?} weak-tier deepseek must dispatch via the native HTTP handler",
            );
            assert_eq!(dispatch.raw_spec, "openrouter:deepseek/deepseek-chat");
            assert_eq!(dispatch.model_id, "deepseek/deepseek-chat");
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
        config.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());
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

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(dispatch.handler, ExecutorKind::Native);
        assert_eq!(dispatch.raw_spec, "openrouter:deepseek/deepseek-chat");
        assert_eq!(dispatch.model_id, "deepseek/deepseek-chat");
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_missing_key_falls_back_to_haiku() {
        // Same weak tier, but NO OpenRouter/OpenAI key anywhere. The dispatch
        // must NOT silently route to a keyless OpenRouter call (which 401s and
        // drops the verdict). It falls back loudly to claude:haiku on the claude
        // CLI. (Covers validation item 3: never a silent no-op.)
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role);
            assert_eq!(
                dispatch.handler,
                ExecutorKind::Claude,
                "role {role:?}: missing key must fall back to the claude CLI, not a keyless 401",
            );
            assert_eq!(dispatch.model_id, CLAUDE_HAIKU_MODEL_ID);
            assert_eq!(dispatch.raw_spec, "claude:haiku");
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
        config.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());
        config
            .models
            .set_model(DispatchRole::Evaluator, "claude:sonnet");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Evaluator);
        assert_eq!(
            dispatch.raw_spec, "claude:sonnet",
            "explicit override must win"
        );
        assert_eq!(dispatch.handler, ExecutorKind::Claude);
        assert_eq!(dispatch.model_id, "sonnet");

        // A sibling agency role with no override still resolves via the weak
        // tier (here: no key -> haiku fallback) — proving the override is
        // role-scoped, not global.
        let assigner = resolve_agency_dispatch(&config, DispatchRole::Assigner);
        assert_ne!(assigner.raw_spec, "claude:sonnet");
        assert_eq!(assigner.handler, ExecutorKind::Claude);
        assert_eq!(assigner.model_id, CLAUDE_HAIKU_MODEL_ID);
    }

    #[test]
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_explicit_codex_override_unaffected_by_weak_tier() {
        // A codex override routes to the codex CLI regardless of the weak tier;
        // codex self-authenticates, so it is not subject to the credential net.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());
        config
            .models
            .set_model(DispatchRole::Assigner, "codex:gpt-5.4-mini");

        let dispatch = resolve_agency_dispatch(&config, DispatchRole::Assigner);
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
        // A prefix-less spec defaults to anthropic-native, unchanged.
        assert_eq!(native_provider_for_spec("some-bare-model"), "anthropic");
    }

    #[test]
    fn test_agency_dispatch_for_spec_routes_pi_handler_first() {
        // The bug this task fixes: a handler-first `pi:` agency spec (e.g.
        // `pi:openrouter:deepseek/deepseek-chat`) MUST resolve to the Pi
        // handler — NOT the claude CLI catch-all in `run_lightweight_llm_call`.
        // The previous bug silently fell into the claude-CLI arm and failed
        // with "Claude CLI call failed ... subscription access disabled".
        let dispatch = agency_dispatch_for_spec("pi:openrouter:deepseek/deepseek-chat");
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
    #[serial_test::serial]
    fn test_resolve_agency_dispatch_weak_tier_pi_routes_to_pi_handler() {
        // Two-tier Pi profile writes `pi:openrouter:deepseek/deepseek-chat` into
        // tiers.fast. ALL agency one-shot roles must resolve to the Pi handler
        // (NOT claude CLI, NOT native). The credential safety net only redirects
        // keyless *native* providers; a `pi:` route self-authenticates via env
        // / pi OAuth, so it is never redirected to claude:haiku at resolve time.
        let _o = EnvGuard::set("OPENROUTER_API_KEY", None);
        let _a = EnvGuard::set("OPENAI_API_KEY", None);
        let mut config = Config::default();
        config.tiers.fast = Some("pi:openrouter:deepseek/deepseek-chat".to_string());

        for role in ALL_AGENCY_ROLES {
            let dispatch = resolve_agency_dispatch(&config, role);
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
        // With the handler-first `nex:openrouter:` spec, the credential gate must
        // behave exactly like the bare `openrouter:` spec: unavailable with no
        // key (proves it resolves to openrouter, NOT the ungated oai-compat
        // localhost path and NOT anthropic), available once the key is present.
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
}
