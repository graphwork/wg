//! Context-window probing for OpenAI-compatible endpoints.
//!
//! The native (nex) handler channels oversized tool outputs to disk instead of
//! returning them inline (see [`super::channel`]). The inline budget is derived
//! from the model's context window via
//! [`super::channel::threshold_for_context_window`]. That makes the *threshold*
//! context-aware, but it is only as good as the context-window number we feed
//! it.
//!
//! Historically that number came from endpoint config, the model registry, or a
//! blind `128_000` default — never from the server itself. For a local
//! llama.cpp server booted with `-c 8192`, or a vLLM server with a small
//! `--max-model-len`, the blind default badly over-estimated the budget; for a
//! 130k-context local model it under-estimated and dumped easily-inlinable
//! output to a file. This module closes that gap by *probing the endpoint* for
//! its real runtime context length.
//!
//! ## Per-provider answer (how context length is queryable)
//!
//! | Provider                       | Endpoint                | Field                                   |
//! |--------------------------------|-------------------------|-----------------------------------------|
//! | llama.cpp server (local)       | `GET /props`            | runtime `n_ctx` (the `-c` it booted with) |
//! | vLLM / generic OpenAI-compat   | `GET /v1/models`        | `max_model_len`                         |
//! | llama.cpp `/v1/models` (newer) | `GET /v1/models`        | `meta.n_ctx` / `meta.n_ctx_train`       |
//! | OpenRouter                     | `GET /api/v1/models`    | `context_length`                        |
//! | plain OpenAI                   | (not exposed)           | — needs a configurable fallback         |
//!
//! `/props` `n_ctx` is the **runtime** ceiling — the actual `-c` the server was
//! launched with — so we prefer it over any trained-max number, exactly as the
//! task requires.
//!
//! ## Cost / caching
//!
//! Probing is a live HTTP call, so we:
//! - only probe the local-ish OpenAI-compat family (`local` / `oai-compat` /
//!   `openai`); OpenRouter context lengths come from the cached model registry
//!   so we never hammer its large `/models` payload on every agent spawn;
//! - use a short (2s) timeout and treat any failure as "unknown" (return
//!   `None`) — probing is best-effort and never blocks agent startup;
//! - cache the result per `(base_url, model)` for the life of the process so
//!   repeated `create_provider` calls (e.g. sub-agents) probe at most once.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Fallback context window (tokens) when neither config, a live probe, nor the
/// model registry yields a value — e.g. plain OpenAI (which exposes no context
/// length) or an unreachable local server. Generous on purpose: the channeling
/// budget derived from it is still clamped to a terminal-readable byte range by
/// [`super::channel::threshold_for_context_window`]. Operators can override it
/// per deployment via `[native_executor].fallback_context_window`.
pub const DEFAULT_FALLBACK_CONTEXT_WINDOW: usize = 128_000;

/// Provider hints for which a live `/props` + `/v1/models` probe is worthwhile.
///
/// OpenRouter is deliberately excluded: its context lengths live in the cached
/// model registry (`model_cache.json`, refreshed by `wg models`), so a live
/// probe would just re-download a multi-hundred-model payload on every spawn.
const PROBEABLE_HINTS: &[&str] = &["local", "oai-compat", "openai"];

/// Timeout for a single probe request. Local servers answer `/props` in
/// microseconds; this only bites when an endpoint is unreachable, in which case
/// we fall back to config/registry/default after the timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn cache() -> &'static Mutex<HashMap<String, Option<usize>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<usize>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve the effective context window for an OpenAI-compatible client.
///
/// Precedence:
/// 1. `explicit` — an operator-set `context_window` in endpoint config. The
///    operator knows their deployment; an explicit value always wins.
/// 2. `probe` — the live runtime ceiling from the server (`/props` `n_ctx`,
///    `/v1/models` `max_model_len`). Authoritative for local llama.cpp / vLLM,
///    where the registry has nothing.
/// 3. `registry` — the model registry's context window (populated for
///    OpenRouter and other catalogued models).
/// 4. `fallback` — a configurable default used when nothing else resolves
///    (plain OpenAI, or an unreachable local server).
///
/// `probe` is a closure so the (potentially blocking) network call is only made
/// when steps 1 never short-circuits — i.e. only when config did not pin a
/// value.
pub fn resolve_context_window(
    explicit: Option<usize>,
    probe: impl FnOnce() -> Option<usize>,
    registry: Option<usize>,
    fallback: usize,
) -> usize {
    explicit
        .filter(|&v| v > 0)
        .or_else(|| probe().filter(|&v| v > 0))
        .or(registry.filter(|&v| v > 0))
        .unwrap_or(fallback)
}

/// Extract a context length from a single `/v1/models` (or OpenRouter
/// `/api/v1/models`) model entry.
///
/// Checks, in order: vLLM's `max_model_len`, OpenRouter's `context_length`,
/// then llama.cpp's `meta.n_ctx` (runtime) / `meta.n_ctx_train` (trained max).
fn ctx_from_model_entry(entry: &serde_json::Value) -> Option<usize> {
    for key in ["max_model_len", "context_length"] {
        if let Some(v) = entry.get(key).and_then(|v| v.as_u64())
            && v > 0
        {
            return Some(v as usize);
        }
    }
    if let Some(meta) = entry.get("meta") {
        for key in ["n_ctx", "n_ctx_train"] {
            if let Some(v) = meta.get(key).and_then(|v| v.as_u64())
                && v > 0
            {
                return Some(v as usize);
            }
        }
    }
    None
}

/// Parse a `/v1/models` response, picking the entry matching `model`.
///
/// Falls back to the sole entry when the list has exactly one model (a common
/// case for single-model local servers whose `id` may not match the alias the
/// caller used).
pub fn parse_models_context_len(body: &serde_json::Value, model: &str) -> Option<usize> {
    let data = body.get("data")?.as_array()?;
    let entry = data
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(model))
        .or_else(|| if data.len() == 1 { data.first() } else { None })?;
    ctx_from_model_entry(entry)
}

/// Parse a llama.cpp `GET /props` response for the runtime `n_ctx`.
///
/// Shape varies across builds: newer servers expose `n_ctx` at the top level;
/// others nest it under `default_generation_settings.n_ctx`. We prefer the
/// top-level value, then the nested one.
pub fn parse_props_n_ctx(body: &serde_json::Value) -> Option<usize> {
    body.get("n_ctx")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            body.get("default_generation_settings")
                .and_then(|s| s.get("n_ctx"))
                .and_then(|v| v.as_u64())
        })
        .filter(|&v| v > 0)
        .map(|v| v as usize)
}

/// Strip a trailing `/v1` segment from a normalized OAI-compat base URL so
/// llama.cpp's server-root `/props` endpoint can be reached. `base_url` is
/// normalized to end with `/v1` (see `normalize_oai_compat_base_url`).
fn server_root(base_url: &str) -> &str {
    base_url
        .trim_end_matches('/')
        .strip_suffix("/v1")
        .unwrap_or_else(|| base_url.trim_end_matches('/'))
}

/// Async probe: try llama.cpp `/props` first, then `/v1/models`.
async fn probe(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Option<usize> {
    let auth = (!api_key.is_empty()).then(|| format!("Bearer {}", api_key));

    // 1) llama.cpp runtime n_ctx via /props (server root, not under /v1).
    let props_url = format!("{}/props", server_root(base_url));
    if let Some(v) = fetch_json(http, &props_url, auth.as_deref())
        .await
        .and_then(|b| parse_props_n_ctx(&b))
    {
        return Some(v);
    }

    // 2) vLLM max_model_len / llama.cpp meta / OpenRouter context_length via /v1/models.
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));
    if let Some(v) = fetch_json(http, &models_url, auth.as_deref())
        .await
        .and_then(|b| parse_models_context_len(&b, model))
    {
        return Some(v);
    }

    None
}

async fn fetch_json(
    http: &reqwest::Client,
    url: &str,
    auth: Option<&str>,
) -> Option<serde_json::Value> {
    let mut req = http.get(url);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<serde_json::Value>().await.ok()
}

/// Blocking, cached context-window probe for use from the (synchronous)
/// provider-creation path.
///
/// Returns `None` (rather than blocking or panicking) when the hint is not a
/// probeable local-family provider, when the endpoint is unreachable, or when
/// the server does not expose a context length. The async probe runs on an
/// isolated thread with its own current-thread runtime so this is safe to call
/// from inside an existing async runtime (where `Runtime::block_on` would
/// otherwise panic).
pub fn probe_context_window_blocking(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_hint: &str,
) -> Option<usize> {
    if !PROBEABLE_HINTS.contains(&provider_hint) {
        return None;
    }
    let cache_key = format!("{}|{}", base_url, model);
    if let Some(cached) = cache().lock().ok().and_then(|c| c.get(&cache_key).copied()) {
        return cached;
    }

    let (b, k, m) = (base_url.to_string(), api_key.to_string(), model.to_string());
    let result = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        rt.block_on(async {
            let http = reqwest::Client::builder()
                .timeout(PROBE_TIMEOUT)
                .build()
                .ok()?;
            probe(&http, &b, &k, &m).await
        })
    })
    .join()
    .unwrap_or(None);

    if let Ok(mut c) = cache().lock() {
        c.insert(cache_key, result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn props_top_level_n_ctx_preferred() {
        let body = json!({
            "n_ctx": 8192,
            "default_generation_settings": { "n_ctx": 4096 }
        });
        assert_eq!(parse_props_n_ctx(&body), Some(8192));
    }

    #[test]
    fn props_nested_n_ctx_fallback() {
        let body = json!({
            "default_generation_settings": { "n_ctx": 4096 },
            "total_slots": 1
        });
        assert_eq!(parse_props_n_ctx(&body), Some(4096));
    }

    #[test]
    fn props_missing_returns_none() {
        assert_eq!(parse_props_n_ctx(&json!({ "total_slots": 1 })), None);
        // Zero is treated as "not set".
        assert_eq!(parse_props_n_ctx(&json!({ "n_ctx": 0 })), None);
    }

    #[test]
    fn models_vllm_max_model_len() {
        let body = json!({
            "object": "list",
            "data": [ { "id": "my-model", "max_model_len": 32768 } ]
        });
        assert_eq!(parse_models_context_len(&body, "my-model"), Some(32768));
    }

    #[test]
    fn models_openrouter_context_length() {
        let body = json!({
            "data": [
                { "id": "other/model", "context_length": 8000 },
                { "id": "anthropic/claude", "context_length": 200000 }
            ]
        });
        assert_eq!(
            parse_models_context_len(&body, "anthropic/claude"),
            Some(200000)
        );
    }

    #[test]
    fn models_llamacpp_meta_n_ctx() {
        let body = json!({
            "data": [ { "id": "local", "meta": { "n_ctx": 16384, "n_ctx_train": 131072 } } ]
        });
        // Runtime n_ctx wins over trained max.
        assert_eq!(parse_models_context_len(&body, "local"), Some(16384));
    }

    #[test]
    fn models_single_entry_fallback_when_id_mismatch() {
        let body = json!({
            "data": [ { "id": "served-name", "max_model_len": 65536 } ]
        });
        // Caller's alias doesn't match the served id, but there's only one model.
        assert_eq!(parse_models_context_len(&body, "my-alias"), Some(65536));
    }

    #[test]
    fn models_multi_entry_no_match_returns_none() {
        let body = json!({
            "data": [
                { "id": "a", "max_model_len": 1000 },
                { "id": "b", "max_model_len": 2000 }
            ]
        });
        assert_eq!(parse_models_context_len(&body, "c"), None);
    }

    #[test]
    fn server_root_strips_v1() {
        assert_eq!(
            server_root("http://localhost:8088/v1"),
            "http://localhost:8088"
        );
        assert_eq!(
            server_root("http://localhost:8088/v1/"),
            "http://localhost:8088"
        );
        assert_eq!(
            server_root("http://localhost:8088"),
            "http://localhost:8088"
        );
    }

    #[test]
    fn resolve_precedence_explicit_wins() {
        let r = resolve_context_window(Some(40_000), || Some(8_000), Some(200_000), 128_000);
        assert_eq!(r, 40_000);
    }

    #[test]
    fn resolve_precedence_probe_over_registry() {
        // No explicit config: the live runtime ceiling beats the registry's
        // trained-max number. This is the llama.cpp `-c 8192` case.
        let r = resolve_context_window(None, || Some(8_192), Some(131_072), 128_000);
        assert_eq!(r, 8_192);
    }

    #[test]
    fn resolve_precedence_registry_when_probe_unknown() {
        let r = resolve_context_window(None, || None, Some(200_000), 128_000);
        assert_eq!(r, 200_000);
    }

    #[test]
    fn resolve_precedence_fallback_when_all_unknown() {
        let r = resolve_context_window(None, || None, None, 96_000);
        assert_eq!(r, 96_000);
    }

    #[test]
    fn resolve_ignores_zero_values() {
        // A zero anywhere is treated as "unset" and skipped.
        let r = resolve_context_window(Some(0), || Some(0), Some(0), 128_000);
        assert_eq!(r, 128_000);
    }

    #[test]
    fn probe_skips_non_local_hints() {
        // OpenRouter must never trigger a live probe.
        assert_eq!(
            probe_context_window_blocking("https://openrouter.ai/api/v1", "k", "m", "openrouter"),
            None
        );
    }
}
