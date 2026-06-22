//! `handler_for_model` — single source of truth mapping a `(model, endpoint)`
//! pair to the internal handler subprocess that executes it.
//!
//! ## Why this module exists
//!
//! WG has historically exposed six overlapping user-facing concepts —
//! executor, provider, endpoint, route, handler, model — that reduce to
//! exactly two real axes:
//!
//! - **delegate-to-CLI vs in-process** (claude / codex are CLIs;
//!   native / nex is in-process)
//! - **wire protocol** (Anthropic vs OAI-compat)
//!
//! From a user's standpoint, the only knobs that matter are the **model**
//! (`claude:opus`, `nex:qwen3-coder`, `openrouter:anthropic/claude-opus-4-6`,
//! ...) and an optional **endpoint** URL. Everything else — which subprocess
//! to spawn, which wire protocol to speak, whether to ask the CLI to handle
//! its own auth or pass an explicit `--endpoint` — is a derived consequence
//! of the model spec.
//!
//! `handler_for_model` is the **one function** that performs that derivation.
//! Anywhere else in the codebase that picks a handler/executor based on a
//! model spec is a bug — that decision must funnel through here so we can
//! evolve the mapping (add aider/llm/…) in a single place.
//!
//! ## Mapping
//!
//! | Model prefix              | Handler        | Wire        | Endpoint required |
//! |---------------------------|----------------|-------------|-------------------|
//! | `claude:*` (and bare)     | `claude` CLI   | Anthropic   | no (CLI auths)    |
//! | `codex:*`                 | `codex` CLI    | OAI-compat  | no (CLI auths)    |
//! | `nex:*` (canonical)       | `native` (nex) | OAI-compat  | yes               |
//! | `openrouter:*`            | `native` (nex) | OAI-compat  | optional          |
//! | `local:*` (deprecated)    | `native` (nex) | OAI-compat  | yes               |
//! | `oai-compat:*` (deprecated) / `openai:*` | `native` (nex) | OAI-compat  | yes               |
//! | `ollama:*`                | `native` (nex) | OAI-compat  | yes               |
//! | `vllm:*`/`llamacpp:*`     | `native` (nex) | OAI-compat  | yes               |
//! | `gemini:*`                | `native` (nex) | (per impl)  | yes               |
//! | `native:*`                | `native` (nex) | OAI-compat  | yes               |
//!
//! `local:` and `oai-compat:` are deprecated aliases for `nex:` — they
//! still route to the same handler for one release with a stderr warning,
//! and `wg migrate config` rewrites them in existing configs.
//!
//! Bare aliases without a provider prefix (`opus`, `sonnet`, `haiku`) default
//! to the `claude` handler — they're Anthropic models by convention.
//!
//! Future handlers (aider, llm, …) plug in here by adding new arms; the rest
//! of the codebase doesn't need to know.

use crate::config::{parse_model_spec, provider_to_executor};
use crate::dispatch::ExecutorKind;

/// Decide which internal handler subprocess will execute a spawn for the
/// given model spec.
///
/// This is the ONE function that does this. Everything else that needs to
/// know "which binary do I spawn for this model?" must go through here.
///
/// `model` is a model spec string as the user wrote it (`claude:opus`,
/// `nex:qwen3-coder`, `opus`, etc). The function:
///
/// 1. Parses the provider prefix (lenient — bare names → `None`).
/// 2. Maps prefix → executor kind via `provider_to_executor`.
/// 3. Falls back to `Claude` for bare names (matches the historical default
///    convention: `opus`, `sonnet`, `haiku` are Anthropic).
///
/// Endpoint policy (which executors require an explicit `--endpoint` URL)
/// is handled by the caller via `ExecutorKind::needs_endpoint()` — this
/// function answers the handler question only.
pub fn handler_for_model(model: &str) -> ExecutorKind {
    // External CLIs (`opencode`, `aider`, `goose`, …) are addressed by
    // *executor* name, not a model provider prefix:
    // `opencode:openrouter/<provider>/<model>`. Those names are intentionally
    // NOT in `KNOWN_PROVIDERS` (an executor is not a provider), so the lenient
    // `parse_model_spec` below would treat the whole string as a bare alias and
    // mis-route it to the claude handler. Intercept the executor prefix here so
    // this function stays the single source of truth even for those routes —
    // matching what `parse_executor_model_route` does on the spawn path.
    if let Some((prefix, rest)) = model.split_once(':') {
        if !rest.trim().is_empty() {
            if let Some(kind) = ExecutorKind::from_str(prefix) {
                if kind.is_external_cli() {
                    return kind;
                }
            }
        }
    }

    let spec = parse_model_spec(model);
    match spec.provider.as_deref() {
        Some(prefix) => {
            let exec = provider_to_executor(prefix);
            ExecutorKind::from_str(exec).unwrap_or(ExecutorKind::Native)
        }
        None => {
            // Bare alias (no provider prefix). Anthropic-style names are the
            // historical default: `opus`, `sonnet`, `haiku` → claude CLI.
            ExecutorKind::Claude
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_prefix_routes_to_claude_handler() {
        assert_eq!(handler_for_model("claude:opus"), ExecutorKind::Claude);
        assert_eq!(handler_for_model("claude:sonnet-4-6"), ExecutorKind::Claude);
    }

    #[test]
    fn test_bare_alias_routes_to_claude_handler() {
        // Bare names default to claude CLI by convention.
        assert_eq!(handler_for_model("opus"), ExecutorKind::Claude);
        assert_eq!(handler_for_model("sonnet"), ExecutorKind::Claude);
        assert_eq!(handler_for_model("haiku"), ExecutorKind::Claude);
    }

    #[test]
    fn test_nex_prefix_routes_to_native() {
        // `nex:` is the canonical prefix for the in-process nex handler
        // (matches the `wg nex` subcommand name).
        assert_eq!(handler_for_model("nex:qwen3-coder"), ExecutorKind::Native);
    }

    #[test]
    fn test_local_prefix_routes_to_native() {
        // `local:` is the deprecated alias for `nex:` — still routes to
        // the same handler for one release with a stderr warning.
        assert_eq!(handler_for_model("local:qwen3-coder"), ExecutorKind::Native);
    }

    #[test]
    fn test_openrouter_prefix_routes_to_native() {
        assert_eq!(
            handler_for_model("openrouter:anthropic/claude-opus-4-6"),
            ExecutorKind::Native
        );
    }

    #[test]
    fn test_oai_compat_prefix_routes_to_native() {
        // `oai-compat:` is the deprecated alias for `nex:` — still routes
        // to the same handler for one release with a stderr warning.
        assert_eq!(handler_for_model("oai-compat:gpt-5"), ExecutorKind::Native);
        // "openai" is the legacy alias.
        assert_eq!(handler_for_model("openai:gpt-5"), ExecutorKind::Native);
    }

    #[test]
    fn test_codex_prefix_routes_to_codex() {
        assert_eq!(handler_for_model("codex:gpt-5"), ExecutorKind::Codex);
    }

    #[test]
    fn test_opencode_prefix_routes_to_opencode_handler() {
        // `opencode` is a worker-only external CLI, not a model provider. The
        // executor prefix must route to the OpenCode handler here (single
        // source of truth), matching the spawn-path `parse_executor_model_route`
        // — NOT fall through to the claude default like an unknown provider.
        assert_eq!(
            handler_for_model("opencode:openrouter/stepfun/step-3.7-flash"),
            ExecutorKind::OpenCode
        );
        assert_eq!(
            handler_for_model("opencode:openrouter/x"),
            ExecutorKind::OpenCode
        );
    }

    #[test]
    fn test_other_worker_only_externals_route_to_their_kind() {
        // The interception is generic over worker-only externals, so aider /
        // goose / qwen / cline / crush / amplifier compose without new arms.
        assert_eq!(handler_for_model("aider:openrouter/x"), ExecutorKind::Aider);
        assert_eq!(handler_for_model("goose:openrouter/x"), ExecutorKind::Goose);
        assert_eq!(handler_for_model("qwen:openrouter/x"), ExecutorKind::Qwen);
    }

    #[test]
    fn test_pi_prefix_routes_to_pi_handler() {
        // `pi` is a chat-capable external CLI (in EXTERNAL_CLIS, NOT
        // WORKER_ONLY_EXTERNALS — like OpenCode). The external-CLI prefix
        // interception routes `pi:<route>` to the Pi handler with NO new
        // match arm; `parse_executor_model_route` normalizes the inner
        // `openrouter/<vendor>/<model>` shorthand on the spawn path.
        assert_eq!(
            handler_for_model("pi:openrouter/anthropic/claude-3.5-haiku"),
            ExecutorKind::Pi
        );

        // Pi is an external CLI but, like OpenCode, is chat-capable —
        // it must NOT be classified worker-only.
        assert!(ExecutorKind::Pi.is_external_cli());
        assert!(!ExecutorKind::Pi.is_worker_only_external());
    }

    #[test]
    fn test_unknown_prefix_treated_as_bare_name() {
        // `foobar:baz` has an unknown prefix, so parse_model_spec treats the
        // whole string as a bare name → claude. This matches the lenient
        // parser. Strict validation lives at CLI/config-load entry points.
        assert_eq!(handler_for_model("foobar:gpt-4"), ExecutorKind::Claude);
    }
}
