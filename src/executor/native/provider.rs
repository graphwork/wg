//! Provider trait and model-based routing.
//!
//! The `Provider` trait abstracts over LLM API wire formats (Anthropic Messages,
//! OpenAI Chat Completions). Implementations handle headers, request/response
//! serialization, and tool call encoding while the agent loop works with a
//! uniform interface.
//!
//! Use `create_provider()` to route a model string to the appropriate backend:
//! - Bare name (`claude-sonnet-4-6`) → Anthropic native API
//! - Bare `vendor/model` with no endpoint or configured provider → OpenRouter
//! - Explicit provider prefixes/config (`openai:...`, `[native_executor].provider`) → that provider

use std::path::Path;

use anyhow::{Context, Result};

use super::client::{AnthropicClient, MessagesRequest, MessagesResponse};
use super::openai_client::OpenAiClient;

/// Provider-agnostic LLM client trait.
///
/// Both `AnthropicClient` and `OpenAiClient` implement this trait so the
/// agent loop can work with any backend without knowing wire format details.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Provider name for logging (e.g., "anthropic", "oai-compat").
    fn name(&self) -> &str;

    /// The model this provider is configured with.
    fn model(&self) -> &str;

    /// Endpoint name or label when known. Used for diagnostics only.
    fn endpoint_name(&self) -> Option<&str> {
        None
    }

    /// Maximum tokens per response.
    fn max_tokens(&self) -> u32;

    /// Context window size in tokens for this provider/model combination.
    fn context_window(&self) -> usize {
        200_000
    }

    /// Send a completion request and return the response.
    ///
    /// The provider translates between the canonical message format and
    /// its wire protocol.
    async fn send(&self, request: &MessagesRequest) -> Result<MessagesResponse>;

    /// Send a streaming completion request with incremental text callbacks.
    ///
    /// `on_text` is called for each text chunk as it arrives from the SSE
    /// stream, enabling progressive display. Returns the full assembled
    /// response as with `send()`. Default: falls back to `send()`.
    async fn send_streaming(
        &self,
        request: &MessagesRequest,
        on_text: &(dyn Fn(String) + Send + Sync),
    ) -> Result<MessagesResponse> {
        let _ = on_text;
        self.send(request).await
    }
}

/// Normalize an OAI-compatible base URL so `OpenAiClient` (which
/// posts to `{base_url}/chat/completions`) hits the canonical
/// `/v1/chat/completions` endpoint that SGLang/vLLM/llama.cpp/Ollama
/// expose. Idempotent — does NOT double `/v1` if already present.
///
/// Both the inline-URL shortcut (`wg nex -e <url>`) and the
/// named-endpoint resolution path (`wg nex -m <m>` resolving against
/// `[[llm_endpoints.endpoints]]`) must call this — the named-endpoint
/// path historically did not, which surfaced as an HTTP 404 fault on
/// the very first message in `wg tui` chat for any user whose
/// `wg init -e <bare-url>` had stored the host without `/v1`.
fn normalize_oai_compat_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{}/v1", trimmed)
    }
}

/// Build an oai-compat client pointed directly at `url`, with an
/// optional key override. Used by the `-e <url>` shortcut so local
/// servers (Ollama, vLLM, llama.cpp) work without any config.
///
/// Per the WG credential contract, this path NEVER reads
/// `OPENAI_API_KEY` / `WG_API_KEY` from the environment. If the user
/// supplied `--api-key` (the only legitimate keyless input besides
/// WG config), we use it; otherwise the client is built with
/// an empty key and the HTTP layer skips the Authorization header.
/// If the endpoint requires auth, the 401 path surfaces a config-
/// pointing error.
fn build_inline_url_client(
    model: &str,
    url: &str,
    api_key_override: Option<&str>,
) -> Result<OpenAiClient> {
    // OpenAiClient constructs `{base_url}/chat/completions`, so
    // base_url must include the `/v1` path segment.
    let base = normalize_oai_compat_base_url(url);
    let key = api_key_override.map(String::from).unwrap_or_default();
    let mut client = OpenAiClient::new(key.clone(), model, None)
        .context("initialize oai-compat client for inline URL")?
        .with_provider_hint("oai-compat")
        .with_endpoint_name(url)
        .with_base_url(&base);
    // Zero-config path (e.g. `wg nex -e http://localhost:8088`): probe the
    // server for its runtime context window so the tool-output channeling
    // budget matches reality (a llama.cpp `-c 8192` server should not be
    // treated as if it had a 128k window). Falls back to the client default
    // when the endpoint can't be probed.
    let context_window = super::context_probe::resolve_context_window(
        None,
        || super::context_probe::probe_context_window_blocking(&base, &key, model, "oai-compat"),
        None,
        super::context_probe::DEFAULT_FALLBACK_CONTEXT_WINDOW,
    );
    client = client.with_context_window(context_window);
    Ok(client)
}

/// Backward-compatible wrapper: routes by model string only.
pub fn create_provider(workgraph_dir: &Path, model: &str) -> Result<Box<dyn Provider>> {
    create_provider_ext(workgraph_dir, model, None, None, None)
}

/// Heuristic: does this bare model name look like a Claude/Anthropic
/// model? Used when a model string has no provider prefix and no slash
/// and no endpoint is set — these bare names fall through to the
/// provider-resolution default, which is `"openai"`, so we need an
/// escape hatch for well-known Anthropic models like `"opus"`,
/// `"sonnet"`, `"haiku"`, and `"claude-sonnet-4-6"`.
///
/// Case-insensitive. Returns true for:
/// - `"opus"`, `"sonnet"`, `"haiku"`, `"fable"` (short aliases for the tiers)
/// - Anything starting with `"claude"` (e.g. `"claude-sonnet-4-6"`,
///   `"claude-opus-4-6"`, `"claude-fable-5"`, `"claude3"`, `"Claude-Sonnet"`)
fn looks_like_claude_model(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "opus" | "sonnet" | "haiku" | "fable") || lower.starts_with("claude")
}

/// Parse the `<endpoint>:<model>` shorthand in the model string.
///
/// Allows callers to write `lambda01:qwen3-coder-30b` as the model
/// string and have it picked up as if `endpoint_name = Some("lambda01")`
/// and `model = "qwen3-coder-30b"` had been passed explicitly. The
/// shorthand is ONLY applied when:
///
/// 1. No explicit `endpoint_name` was passed (explicit always wins).
/// 2. The prefix is NOT a known provider (so `openai:qwen3-coder-30b`
///    keeps its legacy meaning of "openai provider, model qwen3-coder-30b").
/// 3. The prefix matches a named endpoint in the config's
///    `[[llm_endpoints.endpoints]]` table.
///
/// Returns `(endpoint_name, effective_model_string)`. If the shorthand
/// did not apply, returns the inputs unchanged.
fn parse_endpoint_model_shorthand(
    config: &crate::config::Config,
    model: &str,
    endpoint_name: Option<&str>,
) -> (Option<String>, String) {
    if endpoint_name.is_some() {
        return (endpoint_name.map(String::from), model.to_string());
    }
    if let Some((prefix, rest)) = model.split_once(':')
        && !crate::config::KNOWN_PROVIDERS.contains(&prefix)
        && config.llm_endpoints.find_by_name(prefix).is_some()
    {
        return (Some(prefix.to_string()), rest.to_string());
    }
    (None, model.to_string())
}

fn resolve_explicit_endpoint(
    config: &crate::config::Config,
    config_root: &Path,
    name: &str,
) -> Result<Option<crate::config::EndpointConfig>> {
    if let Some(ep) = config.llm_endpoints.find_by_name(name) {
        return Ok(Some(ep.clone()));
    }

    let global = crate::config::Config::load_global()
        .with_context(|| "Failed to load global WG config while resolving named endpoint")?;
    let Some(global) = global else {
        return Ok(None);
    };
    let Some(ep) = global.llm_endpoints.find_by_name(name).cloned() else {
        return Ok(None);
    };

    if matches!(
        crate::config::provider_to_native_provider(&ep.provider),
        "openrouter" | "oai-compat" | "openai" | "local"
    ) {
        eprintln!(
            "[native-exec] using global endpoint '{}' from {}. \
             To make this visible in project config, set [llm_endpoints] inherit_global = true in {}.",
            name,
            crate::config::Config::global_config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.wg/config.toml".to_string()),
            config_root.join("config.toml").display(),
        );
    }
    Ok(Some(ep))
}

/// Create a provider, optionally overriding the provider name, endpoint, and/or API key.
///
/// Resolution order for API key (WG credential contract — see
/// `feedback_native_executor_no_env_vars` and the `native-executor-client`
/// task description):
/// 1. `api_key_override` parameter (pre-resolved by spawn path; eg `wg nex --api-key`)
/// 2. Matching endpoint entry's `api_key` / `api_key_file` (file content
///    inline) / `api_key_env` (when explicitly named — this is
///    user-authorized config, not implicit env-var fallback)
///
/// **No implicit env-var fallback.** `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` /
/// `OPENROUTER_API_KEY` are NEVER consulted by this path. If no key resolves,
/// the client is built with an empty key and the HTTP layer skips the auth
/// header; if the endpoint then rejects the request with 401/403, the
/// resulting error names the `[[llm_endpoints.endpoints]]` block to add
/// `api_key` to — never an env var.
///
/// Resolution order for base URL:
/// 1. Matching endpoint entry's `url` field
/// 2. `[native_executor]` section's `api_base` field (legacy)
pub fn create_provider_ext(
    workgraph_dir: &Path,
    model: &str,
    provider_override: Option<&str>,
    endpoint_name: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<Box<dyn Provider>> {
    let config = crate::config::Config::load_or_default(workgraph_dir);
    let config_val = crate::config::Config::load_merged_toml_value(workgraph_dir).ok();
    create_provider_ext_with_config(
        workgraph_dir,
        &config,
        config_val.as_ref(),
        model,
        provider_override,
        endpoint_name,
        api_key_override,
    )
}

/// Create a provider against an already-resolved config.
///
/// Standalone `nex` uses this to avoid re-loading WG global/project
/// configuration after its `.nex` runtime config has already been merged.
pub fn create_provider_ext_with_config(
    config_root: &Path,
    config: &crate::config::Config,
    config_val: Option<&toml::Value>,
    model: &str,
    provider_override: Option<&str>,
    endpoint_name: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<Box<dyn Provider>> {
    // Test hook: `WG_FAKE_LLM=<path>` swaps in a pre-canned-response
    // provider. Great for smoking the rendering path (streaming,
    // wrapping, markdown rewrite) without burning tokens or waiting
    // on the network. Real smoke targets stay on the real path;
    // only turns on when this env var is set.
    if let Ok(path) = std::env::var("WG_FAKE_LLM")
        && !path.is_empty()
    {
        return Ok(Box::new(FakeProvider::from_file(&path, model)?));
    }

    // Handler-first inner re-parse (design §6.3): a leading `nex:` / `native:`
    // names THIS in-process handler; everything after it is the handler's own
    // native model dialect. Unwrap it so a wire-distinct inner provider
    // (`nex:openrouter:z-ai/glm-5.2` → `openrouter:z-ai/glm-5.2`) drives the
    // provider/endpoint resolution below instead of `nex` collapsing to the
    // oai-compat localhost default and silently targeting the wrong API. A
    // bare nex model with no inner provider (`nex:qwen3-coder`) is left
    // untouched. Mirrors how the CLI adapters strip their own executor prefix.
    let model = crate::config::strip_native_handler_prefix(model);

    // Inline URL shortcut: `-e http://localhost:11434` (or https://)
    // bypasses the named-endpoint config lookup. Builds an OpenAI-
    // compatible client against that URL with no API key (the
    // "local" provider signals no-auth). Lets users talk to an
    // Ollama / llama.cpp / vLLM server with zero config:
    //
    //     wg nex -m qwen3-coder-30b -e http://localhost:11434
    //
    // Overrides still apply in the usual priority order; this just
    // skips the hoop of declaring the endpoint in config.toml.
    if let Some(url) = endpoint_name
        && (url.starts_with("http://") || url.starts_with("https://"))
    {
        // Strip the canonical provider prefix (`nex:`, `local:`,
        // `oai-compat:`, `openrouter:`, etc.) before passing the model
        // name to the wire layer. `wg init` stores models in the
        // prefixed form (`nex:qwen3-coder`), but downstream OAI-compat
        // servers (SGLang, vLLM, llama.cpp, Ollama) treat a colon in
        // the `model` field as a LoRA-adapter reference and reject
        // anything they don't have loaded with HTTP 400 — which broke
        // every `wg nex -e <url> -m <prefixed>` invocation on the
        // first message.
        //
        // Mirrors the prefix handling in the non-inline path below
        // (search for `parse_model_spec` + `spec.model_id`).
        let spec = crate::config::parse_model_spec(model);
        let stripped_model = spec.model_id.as_str();
        return Ok(Box::new(build_inline_url_client(
            stripped_model,
            url,
            api_key_override,
        )?));
    }

    let native_cfg = config_val.and_then(|v| v.get("native_executor"));
    let native_provider = native_cfg
        .and_then(|c| c.get("provider"))
        .and_then(|v| v.as_str());

    // A bare `vendor/model` route with NO endpoint and NO explicit provider is
    // an OpenRouter route.
    // (nex-optional-openrouter-endpoint): `wg nex -m minimax/minimax-m3`
    // should reach OpenRouter, not the bare-name oai-compat/local default.
    // Normalize to `openrouter:<route>` so provider resolution below targets
    // OpenRouter directly. Skipped when an endpoint or provider is given — that
    // explicit route dictates the provider, so the model stays verbatim.
    let openrouter_normalized: String;
    let model =
        if endpoint_name.is_none() && provider_override.is_none() && native_provider.is_none() {
            openrouter_normalized = crate::config::normalize_bare_openrouter_route(model);
            openrouter_normalized.as_str()
        } else {
            model
        };

    // Endpoint-in-model shorthand — see `parse_endpoint_model_shorthand`.
    let (endpoint_name_owned, effective_model_str) =
        parse_endpoint_model_shorthand(config, model, endpoint_name);
    let endpoint_name = endpoint_name_owned.as_deref();
    let model = effective_model_str.as_str();
    let explicit_endpoint = endpoint_name.and_then(|name| {
        resolve_explicit_endpoint(config, config_root, name)
            .ok()
            .flatten()
    });

    // Early endpoint lookup (by name only). If the caller passed an
    // explicit `-e <name>` OR the shorthand matched a named endpoint,
    // we use that endpoint's `provider` field to seed the provider
    // resolution — otherwise bare model names like `qwen3-coder-30b`
    // fall through to the "anthropic" default and the request hits
    // the wrong API shape even though the URL points at an OpenAI-
    // compatible endpoint. Purely additive; doesn't replace the full
    // endpoint lookup below which also handles provider-based and
    // default fallbacks.
    //
    // Endpoint config may contain the legacy "openai" alias; normalize
    // through provider_to_native_provider so the canonical internal
    // tag ("oai-compat") flows downstream and `provider.name()` reports
    // it consistently.
    let endpoint_provider_override: Option<String> = endpoint_name
        .and_then(|name| {
            explicit_endpoint
                .as_ref()
                .filter(|ep| ep.name == name)
                .or_else(|| config.llm_endpoints.find_by_name(name))
        })
        .map(|ep| crate::config::provider_to_native_provider(&ep.provider).to_string());

    // Parse unified provider:model spec (e.g. "openrouter:deepseek/deepseek-v3.2").
    // When a known provider prefix is present, it takes priority over all other
    // provider resolution paths.
    let spec = crate::config::parse_model_spec(model);
    // Keep the original prefix for URL resolution (e.g., "ollama" → localhost:11434)
    let original_prefix = spec.provider.clone();
    let spec_provider = spec
        .provider
        .as_deref()
        .map(crate::config::provider_to_native_provider)
        .map(String::from);

    // Resolve provider name: spec prefix > override > named-endpoint.provider >
    // config > model heuristic > env var > oai-compat default.
    //
    // Two key changes from legacy behavior:
    //
    // 1. The named-endpoint.provider slot makes `-e lambda01 -m
    //    qwen3-coder-30b` work: a bare model name with no slash would
    //    otherwise fall through to the hardcoded default, but the user's
    //    endpoint is explicitly OpenAI-compatible, so we use its
    //    `provider` field instead.
    //
    // 2. The fallback for unrecognized bare names is `"openai"`, not
    //    `"anthropic"`. WG has shifted toward local/open-model-
    //    first operation and the overwhelming majority of new deployments
    //    use OpenAI-compatible endpoints (Ollama, vLLM, llama.cpp, lambda,
    //    etc.). Known Claude-family model names (opus, sonnet, haiku,
    //    claude-*) are still detected heuristically and routed to
    //    anthropic — see `looks_like_claude_model`.
    let provider_name = spec_provider
        .or_else(|| provider_override.map(String::from))
        .or_else(|| endpoint_provider_override.clone())
        .or_else(|| {
            // Legacy [native_executor].provider — preserved verbatim so the
            // user-facing label they configured ("openai", "openrouter", etc.)
            // round-trips through `provider.name()`. Consistent with the
            // explicit `provider_override` path above. The match arm below
            // accepts both the canonical "oai-compat" tag and the legacy
            // "openai" alias, so verbatim preservation does not affect routing.
            native_provider.map(String::from)
        })
        .or_else(|| {
            // Legacy heuristic takes precedence over env var for explicit model prefixes
            if spec.model_id.starts_with("anthropic/") {
                Some("anthropic".to_string())
            } else if spec.model_id.contains('/') {
                Some("oai-compat".to_string())
            } else if looks_like_claude_model(&spec.model_id) {
                Some("anthropic".to_string())
            } else {
                None
            }
        })
        .or_else(|| std::env::var("WG_LLM_PROVIDER").ok())
        .unwrap_or_else(|| {
            // Fallback for bare unrecognized model names — defaults to
            // oai-compat because that covers local model servers
            // (Ollama, vLLM, llama.cpp, lambda, etc.).
            "oai-compat".to_string()
        });

    // Use the parsed model ID (provider prefix stripped) for API calls.
    // Also strip legacy "anthropic/" prefix for backward compatibility.
    let model = if provider_name == "anthropic" {
        spec.model_id
            .strip_prefix("anthropic/")
            .unwrap_or(&spec.model_id)
    } else {
        &spec.model_id
    };

    // Look up endpoint config: by name first, then by provider, then default endpoint
    let endpoint = if let Some(ep) = explicit_endpoint.as_ref() {
        Some(ep)
    } else if let Some(ep) = config.llm_endpoints.find_for_provider(&provider_name) {
        Some(ep)
    } else {
        config
            .llm_endpoints
            .find_default()
            .filter(|ep| provider_name != "openrouter" || ep.provider == "openrouter")
    };
    // STRICT key resolution: read api_key / api_key_file / api_key_env
    // from the matched endpoint's config — NEVER fall back to implicit
    // provider env vars (ANTHROPIC_API_KEY etc). See create_provider_ext
    // doc comment for the WG credential contract.
    let endpoint_key =
        endpoint.and_then(|ep| ep.resolve_api_key_strict(Some(config_root)).ok().flatten());
    let endpoint_url = endpoint.and_then(|ep| ep.url.clone());
    let endpoint_context_window = endpoint.and_then(|ep| ep.context_window);
    let endpoint_name_owned: Option<String> = endpoint.map(|ep| ep.name.clone());

    // Context-window inputs. Final resolution (config > live probe > registry >
    // configurable fallback) happens at the OAI-compat client site below, where
    // the resolved base URL, model, and provider hint are known — the probe
    // needs all three. See `context_probe::resolve_context_window`.
    let registry_context_window = config
        .effective_registry()
        .into_iter()
        .find(|e| e.model == spec.model_id || e.id == spec.model_id)
        .and_then(|e| {
            if e.context_window > 0 {
                Some(e.context_window)
            } else {
                None
            }
        });

    // Base URL resolution: endpoint config > legacy [native_executor] api_base.
    // Per the WG credential contract, env vars (WG_ENDPOINT_URL,
    // OPENAI_BASE_URL, OPENROUTER_BASE_URL) are NOT consulted — endpoint
    // configuration lives in WG config exclusively.
    let api_base: Option<String> = endpoint_url.or_else(|| {
        native_cfg
            .and_then(|c| c.get("api_base"))
            .and_then(|v| v.as_str())
            .map(String::from)
    });

    let max_tokens = native_cfg
        .and_then(|c| c.get("max_tokens"))
        .and_then(|v| v.as_integer())
        .map(|v| v as u32);

    // Configurable fallback context window for the tool-output channeling
    // budget when nothing else resolves (plain OpenAI, unreachable local
    // server). No hardcoded small constant: defaults to a generous 128k.
    let fallback_context_window = native_cfg
        .and_then(|c| c.get("fallback_context_window"))
        .and_then(|v| v.as_integer())
        .filter(|&v| v > 0)
        .map(|v| v as usize)
        .unwrap_or(super::context_probe::DEFAULT_FALLBACK_CONTEXT_WINDOW);

    match provider_name.as_str() {
        "oai-compat" | "openai" | "openrouter" | "local" => {
            // Resolve API key from CONFIG ONLY:
            //   override (e.g. `wg nex --api-key`) > endpoint's config-side fields
            // No env-var fallback. If nothing resolves, build the client with
            // an empty key — the HTTP layer skips the Authorization header
            // and the endpoint decides whether it needs auth. A 401 from
            // the endpoint surfaces a config-pointing error message naming
            // the [[llm_endpoints.endpoints]] block — never an env var.
            //
            // See feedback `native-executor-client` for the rationale: the
            // user's contract is that credentials live in WG config
            // exclusively, and the autohaiku failure was caused by this
            // path bailing with "No Anthropic API key found" before any
            // HTTP call when no env var was set.
            let resolved_key = api_key_override.map(String::from).or(endpoint_key);
            // Keep a copy for the context-window probe (the original is moved
            // into the client below). Empty string => probe sends no auth header.
            let probe_api_key = resolved_key.clone().unwrap_or_default();

            let mut client = OpenAiClient::new(resolved_key.unwrap_or_default(), model, None)
                .context("Failed to initialize OpenAI-compatible client")?;
            client = client.with_provider_hint(&provider_name);
            if let Some(name) = endpoint_name_owned.as_deref() {
                client = client.with_endpoint_name(name);
            }
            if let Some(base) = api_base {
                // Normalize: append `/v1` when missing. `wg init -e
                // <bare-url>` stores the URL without `/v1`, but
                // OpenAiClient appends `/chat/completions` directly to
                // `base_url`, so without this the wire URL becomes
                // `{host}/chat/completions` and OAI-compat servers
                // (SGLang/vLLM/llama.cpp/Ollama) answer 404 — exactly
                // the fault the user reported in `wg tui` chat.
                let normalized = normalize_oai_compat_base_url(&base);
                client = client.with_base_url(&normalized);
            } else {
                // Fall back to the provider's known default URL so that non-OpenRouter
                // providers (e.g. "openai", "local") don't silently hit the OpenRouter
                // endpoint via OpenAiClient's DEFAULT_BASE_URL.
                // Use the original provider prefix (e.g., "ollama", "gemini") for URL
                // lookup, falling back to the resolved provider_name.
                let url_lookup = original_prefix.as_deref().unwrap_or(&provider_name);
                let default_url =
                    crate::config::EndpointConfig::default_url_for_provider(url_lookup);
                if !default_url.is_empty() {
                    client = client.with_base_url(default_url);
                }
            }
            if let Some(mt) = max_tokens {
                client = client.with_max_tokens(mt);
            }
            // Resolve the context window: explicit config > live endpoint probe
            // (llama.cpp /props n_ctx, vLLM /v1/models max_model_len) > model
            // registry > configurable fallback. The probe runs only for the
            // local-ish family and is cached per (base_url, model). This drives
            // the tool-output channeling budget (see channel.rs).
            let context_window = super::context_probe::resolve_context_window(
                endpoint_context_window.map(|v| v as usize),
                || {
                    super::context_probe::probe_context_window_blocking(
                        client.base_url(),
                        &probe_api_key,
                        client.model(),
                        &provider_name,
                    )
                },
                registry_context_window.map(|v| v as usize),
                fallback_context_window,
            );
            client = client.with_context_window(context_window);
            // Validate model against cached OpenRouter model list (openrouter only)
            if provider_name == "openrouter" {
                let validation =
                    super::openai_client::validate_openrouter_model(&client.model, config_root);
                if let Some(ref warning) = validation.warning {
                    eprintln!("[native-exec] WARNING: {}", warning);
                }
                if !validation.was_valid {
                    anyhow::bail!(
                        "Model '{}' not found in OpenRouter model list. {}",
                        client.model,
                        validation
                            .warning
                            .as_deref()
                            .unwrap_or("Run `wg models search <name>` to find valid alternatives.")
                    );
                }
            }
            log::debug!(
                "[native-exec] Using OpenAI-compatible provider ({})",
                client.model
            );
            Ok(Box::new(client))
        }
        _ => {
            // Anthropic path. Resolve API key from CONFIG ONLY:
            //   override > endpoint's config-side fields
            // No env-var fallback (no ANTHROPIC_API_KEY, no
            // ~/.config/anthropic/api_key). If nothing resolves, build the
            // client with an empty key — the HTTP layer skips the
            // x-api-key header and a 401 from api.anthropic.com surfaces
            // a config-pointing error.
            //
            // See feedback `native-executor-client`.
            let resolved_key = api_key_override.map(String::from).or(endpoint_key);
            let mut client = AnthropicClient::new(resolved_key.unwrap_or_default(), model)
                .context("Failed to initialize Anthropic client")?;
            if let Some(name) = endpoint_name_owned.as_deref() {
                client = client.with_endpoint_name(name);
            }
            if let Some(base) = api_base {
                client = client.with_base_url(&base);
            }
            if let Some(mt) = max_tokens {
                client = client.with_max_tokens(mt);
            }
            log::debug!("[native-exec] Using Anthropic provider ({})", client.model);
            Ok(Box::new(client))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EndpointConfig, EndpointsConfig};

    fn config_with_endpoint(name: &str) -> Config {
        let mut config = Config::default();
        config.llm_endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: name.to_string(),
                provider: "openai".to_string(),
                url: Some("https://example.com/v1".to_string()),
                model: None,
                api_key: None,
                api_key_env: None,
                api_key_ref: None,
                api_key_file: None,
                is_default: false,
                context_window: Some(32768),
            }],
        };
        config
    }

    #[test]
    fn shorthand_splits_endpoint_and_model_when_prefix_is_endpoint_name() {
        let config = config_with_endpoint("lambda01");
        let (ep, model) = parse_endpoint_model_shorthand(&config, "lambda01:qwen3-coder-30b", None);
        assert_eq!(ep.as_deref(), Some("lambda01"));
        assert_eq!(model, "qwen3-coder-30b");
    }

    #[test]
    fn shorthand_ignored_when_explicit_endpoint_name_passed() {
        let config = config_with_endpoint("lambda01");
        let (ep, model) = parse_endpoint_model_shorthand(
            &config,
            "lambda01:qwen3-coder-30b",
            Some("other-endpoint"),
        );
        // Explicit wins — the shorthand is NOT applied and the model
        // string is passed through untouched.
        assert_eq!(ep.as_deref(), Some("other-endpoint"));
        assert_eq!(model, "lambda01:qwen3-coder-30b");
    }

    #[test]
    fn shorthand_ignored_when_prefix_is_known_provider() {
        // `openai:...` is a known provider prefix — backward-compat
        // says it keeps meaning "openai provider, model X" even if
        // someone also has an endpoint named "openai" configured.
        let config = config_with_endpoint("openai");
        let (ep, model) = parse_endpoint_model_shorthand(&config, "openai:qwen3-coder-30b", None);
        assert_eq!(ep, None);
        assert_eq!(model, "openai:qwen3-coder-30b");
    }

    #[test]
    fn shorthand_ignored_when_prefix_is_not_a_configured_endpoint() {
        let config = config_with_endpoint("lambda01");
        let (ep, model) =
            parse_endpoint_model_shorthand(&config, "unknown-endpoint:some-model", None);
        // Prefix is not a provider and not an endpoint — passthrough.
        assert_eq!(ep, None);
        assert_eq!(model, "unknown-endpoint:some-model");
    }

    #[test]
    fn shorthand_ignored_for_bare_model_names_without_colon() {
        let config = config_with_endpoint("lambda01");
        let (ep, model) = parse_endpoint_model_shorthand(&config, "qwen3-coder-30b", None);
        assert_eq!(ep, None);
        assert_eq!(model, "qwen3-coder-30b");
    }

    // ── looks_like_claude_model heuristic ──────────────────────────

    #[test]
    fn claude_heuristic_matches_short_aliases() {
        assert!(looks_like_claude_model("opus"));
        assert!(looks_like_claude_model("sonnet"));
        assert!(looks_like_claude_model("haiku"));
    }

    #[test]
    fn claude_heuristic_matches_claude_prefix() {
        assert!(looks_like_claude_model("claude-sonnet-4-6"));
        assert!(looks_like_claude_model("claude-opus-4-6"));
        assert!(looks_like_claude_model("claude-haiku-4-5"));
        assert!(looks_like_claude_model("claude3"));
        assert!(looks_like_claude_model("claude-3-5-sonnet-20241022"));
    }

    #[test]
    fn claude_heuristic_is_case_insensitive() {
        assert!(looks_like_claude_model("Opus"));
        assert!(looks_like_claude_model("SONNET"));
        assert!(looks_like_claude_model("Claude-Sonnet-4-6"));
        assert!(looks_like_claude_model("CLAUDE3"));
    }

    // ── nex-optional-openrouter-endpoint: provider-layer routing ──────────

    fn config_with_local_default() -> Config {
        let mut config = Config::default();
        config.llm_endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "local-gpu".to_string(),
                provider: "local".to_string(),
                url: Some("http://127.0.0.1:8088/v1".to_string()),
                model: None,
                api_key: None,
                api_key_env: None,
                api_key_ref: None,
                api_key_file: None,
                is_default: true,
                context_window: None,
            }],
        };
        config
    }

    #[test]
    fn bare_vendor_model_no_endpoint_routes_to_openrouter_not_local_default() {
        // `wg nex -m minimax/minimax-m3` with no `-e` and a local is_default
        // endpoint configured must route to OpenRouter, NOT the local server.
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_local_default();
        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            None,
            "minimax/minimax-m3",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            client.name(),
            "openrouter",
            "bare vendor/model with no endpoint must resolve to the openrouter provider"
        );
        assert_ne!(
            client.endpoint_name(),
            Some("local-gpu"),
            "must NOT adopt the local is_default endpoint for an openrouter route"
        );
    }

    #[test]
    fn openrouter_prefixed_model_no_endpoint_does_not_adopt_local_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_local_default();
        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            None,
            "openrouter:minimax/minimax-m3",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(client.name(), "openrouter");
        assert_ne!(client.endpoint_name(), Some("local-gpu"));
    }

    #[test]
    fn openrouter_model_prefers_configured_openrouter_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_with_local_default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "my-openrouter".to_string(),
            provider: "openrouter".to_string(),
            url: Some("https://openrouter.ai/api/v1".to_string()),
            model: None,
            api_key: None,
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            api_key_ref: None,
            api_key_file: None,
            is_default: false,
            context_window: None,
        });
        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            None,
            "minimax/minimax-m3",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(client.name(), "openrouter");
        assert_eq!(
            client.endpoint_name(),
            Some("my-openrouter"),
            "configured openrouter endpoint should win over the local default"
        );
    }

    #[test]
    fn native_executor_provider_keeps_bare_vendor_model_off_openrouter() {
        // An explicit legacy provider config is stronger than the bare
        // vendor/model OpenRouter convenience. The model stays unprefixed and
        // routes through the configured OpenAI-compatible provider.
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.llm_endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "test-openai".to_string(),
                provider: "openai".to_string(),
                url: Some("https://example.com/v1".to_string()),
                model: None,
                api_key: Some("test-key".to_string()),
                api_key_env: None,
                api_key_ref: None,
                api_key_file: None,
                is_default: true,
                context_window: None,
            }],
        };
        let config_val: toml::Value = toml::from_str(
            r#"
[native_executor]
provider = "openai"
"#,
        )
        .unwrap();

        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            Some(&config_val),
            "deepseek/deepseek-chat",
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(client.name(), "openai");
        assert_eq!(client.model(), "deepseek/deepseek-chat");
        assert_eq!(client.endpoint_name(), Some("test-openai"));
    }

    #[test]
    fn explicit_endpoint_name_can_resolve_from_global_config() {
        let dir = tempfile::tempdir().unwrap();
        let global_dir = tempfile::tempdir().unwrap();
        let key_file = global_dir.path().join("openrouter.key");
        std::fs::write(&key_file, "test-key\n").unwrap();
        std::fs::write(
            global_dir.path().join("config.toml"),
            format!(
                r#"
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_file = "{}"
"#,
                key_file.display()
            ),
        )
        .unwrap();

        let old = std::env::var_os("WG_GLOBAL_DIR");
        unsafe {
            std::env::set_var("WG_GLOBAL_DIR", global_dir.path());
        }
        let result = create_provider_ext_with_config(
            dir.path(),
            &Config::default(),
            None,
            "openrouter:deepseek/deepseek-v4-flash",
            None,
            Some("openrouter"),
            None,
        );
        unsafe {
            if let Some(old) = old {
                std::env::set_var("WG_GLOBAL_DIR", old);
            } else {
                std::env::remove_var("WG_GLOBAL_DIR");
            }
        }

        let client = result.unwrap();
        assert_eq!(client.name(), "openrouter");
        assert_eq!(client.endpoint_name(), Some("openrouter"));
    }

    #[test]
    fn bare_alias_without_slash_still_uses_local_default() {
        // Regression guard: a slashless bare model is NOT an openrouter route,
        // so the historical local is_default fallback is preserved.
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_local_default();
        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            None,
            "qwen3-coder-30b",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            client.endpoint_name(),
            Some("local-gpu"),
            "slashless bare model keeps the local default endpoint"
        );
    }

    #[test]
    fn explicit_endpoint_keeps_bare_model_off_openrouter() {
        // With an explicit `-e local-gpu`, the bare model is NOT rewritten to
        // openrouter — the endpoint's provider dictates the route.
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_local_default();
        let client = create_provider_ext_with_config(
            dir.path(),
            &config,
            None,
            "minimax/minimax-m3",
            None,
            Some("local-gpu"),
            None,
        )
        .unwrap();
        assert_eq!(
            client.endpoint_name(),
            Some("local-gpu"),
            "an explicit endpoint must still resolve to that endpoint"
        );
        assert_ne!(
            client.name(),
            "openrouter",
            "explicit endpoint means no openrouter normalization"
        );
    }

    #[test]
    fn claude_heuristic_does_not_match_other_models() {
        assert!(!looks_like_claude_model("qwen3-coder-30b"));
        assert!(!looks_like_claude_model("llama3.2"));
        assert!(!looks_like_claude_model("gpt-4o"));
        assert!(!looks_like_claude_model("deepseek-chat"));
        assert!(!looks_like_claude_model("mistral"));
        // Partial match of "claude" in the middle is NOT enough —
        // only a leading prefix counts, to avoid false positives like
        // "my-claude-clone" getting routed to the real Anthropic API.
        assert!(!looks_like_claude_model("my-claude-model"));
        assert!(!looks_like_claude_model("opuscoin"));
    }
}

// ── Fake provider (testing hook) ───────────────────────────────────────
//
// Activated by `WG_FAKE_LLM=<path>`. Reads the file once; each turn
// replays the whole text as a streamed response, split into small
// chunks so the streaming path + markdown rewrite can be exercised
// end-to-end without hitting a real LLM. Round-robins across the
// file's turn boundaries (`---` on a line by itself) so multi-turn
// sessions can be tested too.

use std::sync::Mutex;

use super::client::{ContentBlock, StopReason, Usage};

pub struct FakeProvider {
    /// Canned responses, one per turn. If the script file has
    /// multiple turns separated by lines containing only `---`,
    /// each becomes its own entry. Otherwise the whole file is
    /// one turn that repeats.
    turns: Vec<String>,
    /// Next turn index, wraps around.
    cursor: Mutex<usize>,
    model: String,
}

impl FakeProvider {
    pub fn from_file(path: &str, model: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading WG_FAKE_LLM script at {}", path))?;
        let mut turns: Vec<String> = raw
            .split("\n---\n")
            .map(|s| s.trim_end_matches('\n').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if turns.is_empty() {
            // Empty file: one fallback turn so handlers don't explode.
            turns.push("(fake-llm: empty script)".to_string());
        }
        Ok(Self {
            turns,
            cursor: Mutex::new(0),
            model: model.to_string(),
        })
    }

    /// Whole-text response for the current turn; advances the
    /// round-robin cursor.
    fn next_turn_text(&self) -> String {
        let mut guard = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
        let idx = *guard;
        *guard = (idx + 1) % self.turns.len();
        self.turns[idx].clone()
    }

    /// Chunk the response into ~24-char slices on UTF-8 char
    /// boundaries so streaming looks real. Small enough that
    /// wrapping + markdown rewrite get exercised; large enough
    /// that thousands of chunks don't pound stderr.
    fn chunks_for(text: &str) -> Vec<String> {
        const TARGET: usize = 24;
        let mut out = Vec::new();
        let mut cur = String::new();
        for ch in text.chars() {
            cur.push(ch);
            if cur.len() >= TARGET {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

#[async_trait::async_trait]
impl Provider for FakeProvider {
    fn name(&self) -> &str {
        "fake"
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn max_tokens(&self) -> u32 {
        4096
    }
    async fn send(&self, _req: &MessagesRequest) -> Result<MessagesResponse> {
        let text = self.next_turn_text();
        Ok(MessagesResponse {
            id: "fake-msg".to_string(),
            content: vec![ContentBlock::Text { text }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        })
    }
    async fn send_streaming(
        &self,
        _request: &MessagesRequest,
        on_text: &(dyn Fn(String) + Send + Sync),
    ) -> Result<MessagesResponse> {
        let text = self.next_turn_text();
        // Trickle chunks out with a tiny delay so the streaming
        // spinner + live display path are exercised, then fall
        // through to the same envelope as send().
        for chunk in Self::chunks_for(&text) {
            on_text(chunk);
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        }
        Ok(MessagesResponse {
            id: "fake-msg".to_string(),
            content: vec![ContentBlock::Text { text }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod fake_provider_tests {
    use super::super::client::{Message, MessagesRequest, Role};
    use super::*;

    fn empty_request(model: &str) -> MessagesRequest {
        MessagesRequest {
            model: model.to_string(),
            max_tokens: 100,
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: vec![],
            }],
            tools: vec![],
            stream: true,
        }
    }

    fn write_script(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[tokio::test]
    async fn fake_provider_streams_full_text_across_chunks() {
        let f = write_script("one two three four five six\n");
        let p = FakeProvider::from_file(f.path().to_str().unwrap(), "test-model").unwrap();

        let acc = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let chunk_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let acc2 = acc.clone();
        let cc2 = chunk_count.clone();
        let on_text = move |s: String| {
            acc2.lock().unwrap().push_str(&s);
            cc2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        };

        let req = empty_request("test-model");
        let resp = p.send_streaming(&req, &on_text).await.unwrap();

        // Chunks should concatenate to the full turn.
        assert_eq!(*acc.lock().unwrap(), "one two three four five six");
        // More than one chunk — streaming path exercised.
        assert!(chunk_count.load(std::sync::atomic::Ordering::SeqCst) >= 2);
        // Response envelope has the same text.
        assert!(matches!(
            resp.stop_reason,
            Some(super::super::client::StopReason::EndTurn)
        ));
    }

    #[tokio::test]
    async fn fake_provider_round_robins_turns_on_triple_dash() {
        let script = "turn one\n---\nturn two\n---\nturn three\n";
        let f = write_script(script);
        let p = FakeProvider::from_file(f.path().to_str().unwrap(), "m").unwrap();

        let req = empty_request("m");
        let r1 = p.send(&req).await.unwrap();
        let r2 = p.send(&req).await.unwrap();
        let r3 = p.send(&req).await.unwrap();
        let r4 = p.send(&req).await.unwrap(); // wraps around

        let text = |r: &super::super::client::MessagesResponse| match &r.content[0] {
            super::super::client::ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected Text"),
        };
        assert_eq!(text(&r1), "turn one");
        assert_eq!(text(&r2), "turn two");
        assert_eq!(text(&r3), "turn three");
        assert_eq!(text(&r4), "turn one", "should wrap back to first turn");
    }

    #[test]
    fn inline_url_ensures_v1_suffix() {
        // OpenAiClient constructs `{base_url}/chat/completions`, so
        // the base URL must include `/v1`. Bare host URLs get `/v1`
        // appended; URLs that already have `/v1` are kept as-is.
        let c1 = build_inline_url_client("m", "http://localhost:11434", None).unwrap();
        let c2 = build_inline_url_client("m", "http://localhost:11434/", None).unwrap();
        let c3 = build_inline_url_client("m", "http://localhost:1234/v1", None).unwrap();
        let c4 = build_inline_url_client("m", "http://localhost:1234/v1/", None).unwrap();
        assert_eq!(c1.base_url(), "http://localhost:11434/v1");
        assert_eq!(c2.base_url(), "http://localhost:11434/v1");
        assert_eq!(c3.base_url(), "http://localhost:1234/v1");
        assert_eq!(c4.base_url(), "http://localhost:1234/v1");
    }

    #[tokio::test]
    async fn fake_provider_empty_file_does_not_panic() {
        let f = write_script("");
        let p = FakeProvider::from_file(f.path().to_str().unwrap(), "m").unwrap();
        let req = empty_request("m");
        // Should not panic; falls back to a stub turn.
        let r = p.send(&req).await.unwrap();
        assert!(r.content.len() == 1);
    }
}
