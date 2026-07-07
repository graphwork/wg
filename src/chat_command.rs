use crate::config::{Config, parse_model_spec};
use crate::graph::Task;
use std::path::{Path, PathBuf};

/// Return the canonical preset name for a chat executor shortcut.
pub fn preset_name_for_executor(executor: Option<&str>, model: Option<&str>) -> String {
    if executor.is_none() && model.is_some_and(|m| m.starts_with("nex:")) {
        return "nex".to_string();
    }
    match executor.unwrap_or("claude") {
        "native" | "nex" => "nex".to_string(),
        "codex" => "codex".to_string(),
        "claude" => "claude".to_string(),
        other => other.to_string(),
    }
}

/// Store an arbitrary command line as an argv that preserves shell syntax.
pub fn argv_for_command_line(command: &str) -> Vec<String> {
    vec!["bash".to_string(), "-lc".to_string(), command.to_string()]
}

/// Normalize a bare (provider-less) OpenCode model route into the spelling the
/// OpenCode CLI expects.
///
/// The WG `opencode` executor is OpenRouter-first (see `wg chat create --exec`
/// help: "opencode runs an OpenRouter (or other OpenCode-supported) model
/// route"). OpenCode itself wants a fully-qualified `provider/model` route, so a
/// bare `vendor/model` such as `minimax/minimax-m3` is an OpenRouter route that
/// is simply missing its `openrouter/` prefix. Without the prefix OpenCode
/// cannot resolve provider `minimax` and silently falls back to its own
/// internal default model — the exact `nano-banana`-instead-of-minimax bug the
/// user reported.
///
/// Rules (applied to a route that already had any `opencode:` / `provider:`
/// prefix stripped):
/// - already `openrouter/…` → unchanged (don't double-prefix),
/// - contains a `/` (i.e. `vendor/model` shape) → prefix `openrouter/`,
/// - single token with no `/` (e.g. a locally-configured OpenCode model id) →
///   unchanged.
fn normalize_opencode_bare_route(id: &str) -> String {
    if id.starts_with("openrouter/") || !id.contains('/') {
        id.to_string()
    } else {
        format!("openrouter/{}", id)
    }
}

/// Normalize a per-chat model string into the `--model` value the OpenCode
/// CLI expects (its `openrouter/<vendor>/<model>` spelling).
///
/// Accepts every reasonable user-facing spelling typed into the TUI launcher's
/// free-text model field:
/// - executor-qualified `opencode:openrouter/<vendor>/<model>` (the chat
///   route a user pins; `opencode` is an executor, not a model provider, so
///   it is deliberately absent from `KNOWN_PROVIDERS` and must be stripped
///   here rather than via `parse_model_spec`),
/// - provider-qualified `openrouter:<vendor>/<model>`,
/// - CLI form `openrouter/<vendor>/<model>`,
/// - bare `vendor/model` (e.g. `minimax/minimax-m3`) — normalized to the
///   OpenRouter route `openrouter/<vendor>/<model>` (see
///   [`normalize_opencode_bare_route`]).
///
/// Returns `None` when no usable model id remains.
pub fn opencode_model_arg(model: &str) -> Option<String> {
    // Strip the executor prefix when present; what follows is the real model
    // route (an `openrouter/…` CLI spelling or a `provider:model` spec).
    let model = model.strip_prefix("opencode:").unwrap_or(model);
    let spec = parse_model_spec(model);
    let provider = spec
        .provider
        .as_deref()
        .map(crate::config::provider_to_native_provider);
    let model_arg = if provider == Some("openrouter") {
        let id = spec
            .model_id
            .strip_prefix("openrouter/")
            .unwrap_or(&spec.model_id);
        format!("openrouter/{}", id)
    } else if !spec.model_id.is_empty() {
        normalize_opencode_bare_route(&spec.model_id)
    } else {
        normalize_opencode_bare_route(model)
    };
    if model_arg.is_empty() {
        None
    } else {
        Some(model_arg)
    }
}

/// Normalize a per-chat model string into the `-m` value the **Octomind** CLI
/// expects: its `<provider>:<vendor>/<model>` spelling (e.g.
/// `openrouter:minimax/minimax-m3`, verified against `octomind run --help`,
/// which documents `-m openrouter:anthropic/claude-sonnet-4`).
///
/// Octomind shares WG's `provider:route` model convention, so preservation is
/// near-lossless. Accepts every spelling a user can type into the launcher's
/// free-text Model field:
/// - executor-qualified `octomind:openrouter:minimax/minimax-m3`
///   (`octomind` is an executor, not a model provider, so it is stripped here
///   rather than via `parse_model_spec`),
/// - executor-qualified CLI-slash `octomind:openrouter/minimax/minimax-m3`,
/// - provider-qualified `openrouter:minimax/minimax-m3`,
/// - CLI-slash `openrouter/minimax/minimax-m3`,
/// - bare `vendor/model` (`minimax/minimax-m3`) → treated as the OpenRouter
///   route it is and prefixed `openrouter:` (Octomind is OpenRouter-first in
///   WG, mirroring OpenCode — without the prefix Octomind would resolve the
///   route against its own default role/model instead),
/// - single-token id (`qwen3-coder`, no `/`) → passed through unchanged so a
///   role/registry model Octomind already knows is not mangled.
///
/// Returns `None` when no usable model id remains.
pub fn octomind_model_arg(model: &str) -> Option<String> {
    let model = model.strip_prefix("octomind:").unwrap_or(model);
    let spec = parse_model_spec(model);
    let arg = match spec.provider.as_deref() {
        Some(provider) => {
            // Preserve the typed provider; only collapse the OpenRouter
            // CLI-slash spelling so `openrouter:` isn't double-applied.
            let native = crate::config::provider_to_native_provider(provider);
            if native == "openrouter" {
                let route = spec
                    .model_id
                    .strip_prefix("openrouter/")
                    .unwrap_or(&spec.model_id);
                format!("openrouter:{}", route)
            } else if spec.model_id.is_empty() {
                provider.to_string()
            } else {
                format!("{}:{}", provider, spec.model_id)
            }
        }
        None => {
            // Bare alias (no provider prefix).
            let id = spec.model_id.as_str();
            if let Some(route) = id.strip_prefix("openrouter/") {
                format!("openrouter:{}", route)
            } else if id.contains('/') {
                // A `vendor/model` OpenRouter route missing its prefix.
                format!("openrouter:{}", id)
            } else {
                id.to_string()
            }
        }
    };
    if arg.is_empty() { None } else { Some(arg) }
}

/// Extract the bare OpenRouter `<vendor>/<model>` route from any user-typed
/// model spelling, for writing into a **Dexto** agent config's `llm.model`.
///
/// Dexto's `-m` flag rejects `provider/model` OpenRouter IDs outright (verified:
/// `dexto run -m minimax/minimax-m3` → "looks like an OpenRouter-format ID …
/// set provider/model explicitly in agent config"), and the chat action has no
/// `--provider` flag. The only reliable way to drive OpenRouter with an
/// arbitrary typed model is an agent YAML that pins `llm.provider: openrouter`
/// plus this raw route as `llm.model`. This strips any `dexto:` / `openrouter:`
/// / `openrouter/` prefix and returns the route verbatim so a typed
/// `minimax/minimax-m3` is preserved exactly (no fallback to a default).
pub fn dexto_openrouter_model(model: &str) -> Option<String> {
    let model = model.strip_prefix("dexto:").unwrap_or(model);
    let spec = parse_model_spec(model);
    let route = match spec.provider.as_deref() {
        Some(_) => spec
            .model_id
            .strip_prefix("openrouter/")
            .unwrap_or(&spec.model_id)
            .to_string(),
        None => spec
            .model_id
            .strip_prefix("openrouter/")
            .unwrap_or(&spec.model_id)
            .to_string(),
    };
    if route.is_empty() { None } else { Some(route) }
}

/// Render a minimal Dexto agent config YAML pinning OpenRouter + the given
/// model route. `model` is any user-typed spelling; the raw route is extracted
/// via [`dexto_openrouter_model`]. When no model is supplied the `llm` block is
/// omitted so Dexto falls back to its own configured default (the launcher
/// always supplies one in practice).
pub fn dexto_agent_yaml(model: Option<&str>) -> String {
    let mut yaml = String::from(
        "# Generated by WG for a Dexto live-chat (prototype-octomind-dexto-chat).\n\
         # Pins OpenRouter so an arbitrary typed model route is preserved\n\
         # (Dexto's --model flag rejects provider/model OpenRouter IDs).\n\
         systemPrompt: \"You are a helpful coding assistant.\"\n",
    );
    if let Some(route) = model.and_then(dexto_openrouter_model) {
        yaml.push_str(&format!(
            "llm:\n  provider: openrouter\n  model: {}\n  apiKey: $OPENROUTER_API_KEY\n",
            route
        ));
    }
    yaml
}

/// Filename for the per-chat Dexto agent config written under the chat dir.
pub const DEXTO_AGENT_CONFIG_FILE: &str = "dexto-agent.yml";

/// Write the per-chat Dexto agent config into `chat_dir` and return its path.
/// Idempotent: overwrites any prior copy so a model change on relaunch is
/// honored. The TUI live-chat PTY path and `wg chat create` both call this so
/// the launched `dexto --agent <path>` always matches the chat's pinned model.
pub fn write_dexto_agent_config(chat_dir: &Path, model: Option<&str>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(chat_dir)?;
    let path = chat_dir.join(DEXTO_AGENT_CONFIG_FILE);
    std::fs::write(&path, dexto_agent_yaml(model))?;
    Ok(path)
}

/// Build the stable command metadata for a built-in chat preset.
pub fn argv_for_preset(
    preset: &str,
    model: Option<&str>,
    endpoint: Option<&str>,
    wg_bin: &str,
) -> Vec<String> {
    match preset {
        "nex" | "native" => {
            let mut argv = vec![wg_bin.to_string(), "nex".to_string()];
            let endpoint = endpoint.filter(|ep| !ep.is_empty());
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                argv.push("-m".to_string());
                // With NO endpoint, a bare `vendor/model` or `openrouter:`
                // route is an OpenRouter route: keep the explicit
                // `openrouter:` spec so `wg nex` targets OpenRouter, never the
                // bare-name oai-compat/local default (nex-optional-openrouter-
                // endpoint). With an explicit endpoint, strip the provider
                // prefix — an oai-compat server reads a colon as a LoRA-adapter
                // reference and 400s otherwise.
                let normalized = crate::config::normalize_bare_openrouter_route(m);
                let model_arg =
                    if endpoint.is_none() && crate::config::model_is_openrouter(&normalized) {
                        normalized
                    } else {
                        let spec = parse_model_spec(m);
                        if spec.model_id.is_empty() {
                            m.to_string()
                        } else {
                            spec.model_id
                        }
                    };
                argv.push(model_arg);
            }
            if let Some(ep) = endpoint {
                argv.push("-e".to_string());
                argv.push(ep.to_string());
            }
            argv
        }
        "codex" => {
            let mut argv = vec![
                "codex".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "--no-alt-screen".to_string(),
            ];
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                let spec = parse_model_spec(m);
                if !spec.model_id.is_empty() {
                    argv.push("--model".to_string());
                    argv.push(spec.model_id);
                }
            }
            argv
        }
        "opencode" => {
            // OpenCode launches its own TUI for an interactive PTY chat.
            // Pass the resolved model in opencode's `openrouter/<vendor>/<model>`
            // spelling (the same form the worker path and `opencode-handler`
            // use) so the interactive session is not left on opencode's
            // internal default.
            let mut argv = vec!["opencode".to_string()];
            if let Some(model_arg) = model.filter(|m| !m.is_empty()).and_then(opencode_model_arg) {
                argv.push("--model".to_string());
                argv.push(model_arg);
            }
            argv
        }
        "octomind" => {
            // Octomind launches its own line-oriented REPL (`octomind run`),
            // which is NOT on the alternate screen (verified), so tmux
            // scrollback works without the OpenCode child-scroll workaround.
            // `-m` takes WG's `openrouter:<vendor>/<model>` spelling directly.
            let mut argv = vec!["octomind".to_string(), "run".to_string()];
            if let Some(model_arg) = model.filter(|m| !m.is_empty()).and_then(octomind_model_arg) {
                argv.push("-m".to_string());
                argv.push(model_arg);
            }
            // Confine filesystem writes to the working dir for a chat pane.
            argv.push("--sandbox".to_string());
            argv
        }
        "dexto" => {
            // Dexto needs an agent YAML to drive OpenRouter with an arbitrary
            // model (its --model flag rejects provider/model routes). This pure
            // builder references the conventional per-chat config filename; the
            // live-launch paths (`wg chat create`, the TUI PTY path) write that
            // file via `write_dexto_agent_config` and substitute its absolute
            // path. `--auto-approve` keeps tool prompts from blocking the pane.
            vec![
                "dexto".to_string(),
                "--agent".to_string(),
                DEXTO_AGENT_CONFIG_FILE.to_string(),
                "--auto-approve".to_string(),
            ]
        }
        "pi" => {
            // Plain Pi chat is a terminal-hosted Pi CLI pane. It must not route
            // through `wg pi-handler` / `pi --mode rpc`, and it must not inherit
            // WG's active profile model. A model marker is stored only when the
            // user explicitly supplied one; runtime launchers translate that
            // explicit WG spec into Pi's provider/model flags.
            let mut argv = vec!["pi".to_string()];
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                argv.push("--model".to_string());
                argv.push(m.to_string());
            }
            argv
        }
        _ => {
            let mut argv = vec![
                "claude".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ];
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                let spec = parse_model_spec(m);
                if !spec.model_id.is_empty() {
                    argv.push("--model".to_string());
                    argv.push(spec.model_id);
                }
            }
            argv
        }
    }
}

pub fn project_root_for_workgraph_dir(dir: &Path) -> PathBuf {
    dir.parent().unwrap_or(dir).to_path_buf()
}

pub fn default_working_dir(dir: &Path) -> String {
    project_root_for_workgraph_dir(dir).display().to_string()
}

/// Fill the new chat command metadata for older chat tasks that only
/// carried model/endpoint plus CoordinatorState overrides.
pub fn migrate_chat_task_metadata(
    task: &mut Task,
    workgraph_dir: &Path,
    executor: Option<&str>,
    model: Option<&str>,
    endpoint: Option<&str>,
) -> bool {
    let mut changed = false;
    if task.working_dir.is_none() {
        task.working_dir = Some(default_working_dir(workgraph_dir));
        changed = true;
    }
    if task.executor_preset_name.is_none() {
        task.executor_preset_name = Some(preset_name_for_executor(executor, model));
        changed = true;
    }
    if task.command_argv.is_empty() {
        let preset = task.executor_preset_name.as_deref().unwrap_or("claude");
        task.command_argv = argv_for_preset(preset, model, endpoint, "wg");
        changed = true;
    }
    changed
}

pub fn default_model_for_preset(config: &Config, preset: &str) -> Option<String> {
    match preset {
        "nex" | "native" => config
            .coordinator
            .model
            .clone()
            .or_else(|| Some(config.agent.model.clone())),
        _ => config.coordinator.model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_model_arg_strips_executor_prefix() {
        // Executor-qualified spelling — the route a user pins on a chat.
        assert_eq!(
            opencode_model_arg("opencode:openrouter/stepfun/step-3.7-flash"),
            Some("openrouter/stepfun/step-3.7-flash".to_string())
        );
    }

    #[test]
    fn argv_for_plain_pi_preset_omits_model_override() {
        assert_eq!(argv_for_preset("pi", None, None, "wg"), vec!["pi"]);
    }

    #[test]
    fn argv_for_explicit_pi_preset_preserves_model_override() {
        assert_eq!(
            argv_for_preset("pi", Some("pi:lunaroute:glm-5.2-nvfp4"), None, "wg"),
            vec!["pi", "--model", "pi:lunaroute:glm-5.2-nvfp4"]
        );
    }

    #[test]
    fn opencode_model_arg_accepts_provider_qualified_spelling() {
        assert_eq!(
            opencode_model_arg("openrouter:stepfun/step-3.7-flash"),
            Some("openrouter/stepfun/step-3.7-flash".to_string())
        );
    }

    #[test]
    fn opencode_model_arg_accepts_bare_cli_spelling() {
        assert_eq!(
            opencode_model_arg("openrouter/stepfun/step-3.7-flash"),
            Some("openrouter/stepfun/step-3.7-flash".to_string())
        );
    }

    // --- minimax/minimax-m3 (fix-tui-opencode) -------------------------------
    // All three spellings a user can type into the TUI launcher model field
    // must normalize to OpenCode's `openrouter/minimax/minimax-m3` route, never
    // a bare `minimax/minimax-m3` (which OpenCode can't resolve → silent
    // fallback to its default `nano-banana` model).

    #[test]
    fn opencode_model_arg_minimax_executor_qualified() {
        assert_eq!(
            opencode_model_arg("opencode:openrouter/minimax/minimax-m3"),
            Some("openrouter/minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn opencode_model_arg_minimax_provider_qualified() {
        assert_eq!(
            opencode_model_arg("openrouter:minimax/minimax-m3"),
            Some("openrouter/minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn opencode_model_arg_minimax_bare_vendor_model_gets_openrouter_prefix() {
        // The realistic regression: a user types the bare OpenRouter vendor/model
        // route. Pre-fix this passed through as `minimax/minimax-m3` and OpenCode
        // fell back to its default. It MUST become `openrouter/minimax/minimax-m3`.
        assert_eq!(
            opencode_model_arg("minimax/minimax-m3"),
            Some("openrouter/minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn opencode_model_arg_single_token_model_passes_through() {
        // A provider-less single-token model id (no `/`) is left alone — only
        // `vendor/model` shapes are treated as OpenRouter routes.
        assert_eq!(
            opencode_model_arg("some-local-model"),
            Some("some-local-model".to_string())
        );
    }

    #[test]
    fn argv_for_opencode_preset_normalizes_bare_minimax_route() {
        let argv = argv_for_preset("opencode", Some("minimax/minimax-m3"), None, "wg");
        assert_eq!(
            argv,
            vec![
                "opencode".to_string(),
                "--model".to_string(),
                "openrouter/minimax/minimax-m3".to_string(),
            ]
        );
    }

    // --- Nex blank-endpoint → OpenRouter (nex-optional-openrouter-endpoint) ---

    #[test]
    fn argv_for_nex_preset_blank_endpoint_bare_route_targets_openrouter() {
        // The canonical TUI [+] flow: nex + `minimax/minimax-m3` + blank
        // endpoint. The persisted argv must carry an explicit `openrouter:`
        // spec and NO `-e` flag, so the launched nex routes to OpenRouter and
        // never a stale/default local endpoint.
        let argv = argv_for_preset("nex", Some("minimax/minimax-m3"), None, "wg");
        assert_eq!(
            argv,
            vec!["wg", "nex", "-m", "openrouter:minimax/minimax-m3"]
        );
        assert!(!argv.iter().any(|a| a == "-e"));
    }

    #[test]
    fn argv_for_nex_preset_blank_endpoint_openrouter_prefixed_keeps_prefix() {
        let argv = argv_for_preset("nex", Some("openrouter:minimax/minimax-m3"), Some(""), "wg");
        assert_eq!(
            argv,
            vec!["wg", "nex", "-m", "openrouter:minimax/minimax-m3"]
        );
    }

    #[test]
    fn argv_for_nex_preset_named_endpoint_strips_prefix_and_keeps_endpoint() {
        // Regression guard for the existing named-endpoint path: an explicit
        // endpoint strips the provider prefix (oai-compat servers reject a
        // colon) and the model is NOT rewritten to openrouter.
        let argv = argv_for_preset("nex", Some("nex:qwen3-coder"), Some("qwen-local"), "wg");
        assert_eq!(
            argv,
            vec!["wg", "nex", "-m", "qwen3-coder", "-e", "qwen-local"]
        );
    }

    #[test]
    fn argv_for_nex_preset_blank_endpoint_bare_alias_unchanged() {
        // A slashless bare alias is NOT an OpenRouter route — it keeps the
        // historical bare-id shape (resolves via local/default at the nex layer).
        let argv = argv_for_preset("nex", Some("qwen3-coder-30b"), None, "wg");
        assert_eq!(argv, vec!["wg", "nex", "-m", "qwen3-coder-30b"]);
    }

    // --- Octomind (prototype-octomind-dexto-chat) ---------------------------
    // Octomind shares WG's `provider:route` spelling; every spelling a user can
    // type must normalize to octomind's `-m openrouter:<vendor>/<model>` value,
    // and a typed minimax/minimax-m3 must NEVER be lost to a default.

    #[test]
    fn octomind_model_arg_provider_qualified() {
        assert_eq!(
            octomind_model_arg("openrouter:minimax/minimax-m3"),
            Some("openrouter:minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_executor_qualified() {
        assert_eq!(
            octomind_model_arg("octomind:openrouter:minimax/minimax-m3"),
            Some("openrouter:minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_executor_qualified_cli_slash() {
        // `octomind:openrouter/<vendor>/<model>` — the CLI-slash spelling after
        // the executor prefix must collapse to a single `openrouter:` provider
        // form, not double-prefix.
        assert_eq!(
            octomind_model_arg("octomind:openrouter/minimax/minimax-m3"),
            Some("openrouter:minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_cli_slash_form() {
        assert_eq!(
            octomind_model_arg("openrouter/minimax/minimax-m3"),
            Some("openrouter:minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_bare_vendor_model_gets_openrouter_prefix() {
        // The realistic regression: a bare OpenRouter route. Without the
        // `openrouter:` prefix octomind resolves it against its own default
        // role/model. It MUST become `openrouter:minimax/minimax-m3`.
        assert_eq!(
            octomind_model_arg("minimax/minimax-m3"),
            Some("openrouter:minimax/minimax-m3".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_single_token_passes_through() {
        assert_eq!(
            octomind_model_arg("qwen3-coder"),
            Some("qwen3-coder".to_string())
        );
    }

    #[test]
    fn octomind_model_arg_preserves_non_openrouter_provider() {
        // A non-OpenRouter provider:route is preserved verbatim.
        assert_eq!(
            octomind_model_arg("anthropic/claude-sonnet-4"),
            Some("openrouter:anthropic/claude-sonnet-4".to_string())
        );
        assert_eq!(
            octomind_model_arg("openai:gpt-5.5"),
            Some("openai:gpt-5.5".to_string())
        );
    }

    #[test]
    fn argv_for_octomind_preset_runs_with_model_and_sandbox() {
        let argv = argv_for_preset("octomind", Some("minimax/minimax-m3"), None, "wg");
        assert_eq!(
            argv,
            vec![
                "octomind".to_string(),
                "run".to_string(),
                "-m".to_string(),
                "openrouter:minimax/minimax-m3".to_string(),
                "--sandbox".to_string(),
            ]
        );
    }

    #[test]
    fn argv_for_octomind_preset_ignores_endpoint() {
        let argv = argv_for_preset(
            "octomind",
            Some("openrouter:minimax/minimax-m3"),
            Some("https://example.invalid:30000"),
            "wg",
        );
        assert!(!argv.iter().any(|a| a == "-e" || a == "--endpoint"));
        assert!(argv.contains(&"openrouter:minimax/minimax-m3".to_string()));
    }

    // --- Dexto (prototype-octomind-dexto-chat) ------------------------------
    // Dexto's --model rejects provider/model routes, so the typed model is
    // carried into a generated agent YAML (provider: openrouter). The raw
    // route must be extracted exactly, with no fallback.

    #[test]
    fn dexto_openrouter_model_strips_all_prefixes() {
        for spelling in [
            "dexto:openrouter:minimax/minimax-m3",
            "openrouter:minimax/minimax-m3",
            "openrouter/minimax/minimax-m3",
            "minimax/minimax-m3",
        ] {
            assert_eq!(
                dexto_openrouter_model(spelling),
                Some("minimax/minimax-m3".to_string()),
                "spelling {spelling} must yield the raw route"
            );
        }
    }

    #[test]
    fn dexto_agent_yaml_pins_openrouter_and_preserves_model() {
        let yaml = dexto_agent_yaml(Some("minimax/minimax-m3"));
        assert!(yaml.contains("provider: openrouter"), "{yaml}");
        assert!(yaml.contains("model: minimax/minimax-m3"), "{yaml}");
        assert!(yaml.contains("apiKey: $OPENROUTER_API_KEY"), "{yaml}");
    }

    #[test]
    fn dexto_agent_yaml_omits_llm_block_without_model() {
        let yaml = dexto_agent_yaml(None);
        assert!(!yaml.contains("provider: openrouter"), "{yaml}");
        assert!(yaml.contains("systemPrompt"), "{yaml}");
    }

    #[test]
    fn write_dexto_agent_config_writes_file_with_model() {
        let tmp = std::env::temp_dir().join(format!("wg-dexto-cfg-test-{}", std::process::id()));
        let path = write_dexto_agent_config(&tmp, Some("minimax/minimax-m3")).unwrap();
        assert!(path.ends_with(DEXTO_AGENT_CONFIG_FILE));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("model: minimax/minimax-m3"), "{body}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn argv_for_dexto_preset_references_agent_config() {
        let argv = argv_for_preset("dexto", Some("minimax/minimax-m3"), None, "wg");
        assert_eq!(
            argv,
            vec![
                "dexto".to_string(),
                "--agent".to_string(),
                DEXTO_AGENT_CONFIG_FILE.to_string(),
                "--auto-approve".to_string(),
            ]
        );
        assert!(!argv.iter().any(|a| a == "-e" || a == "--endpoint"));
    }

    #[test]
    fn argv_for_opencode_preset_passes_openrouter_model_without_endpoint() {
        let argv = argv_for_preset(
            "opencode",
            Some("opencode:openrouter/stepfun/step-3.7-flash"),
            // An endpoint must never leak into the opencode argv — OpenRouter
            // is the implicit route, opencode takes no `-e`/`--endpoint`.
            Some("https://example.invalid:30000"),
            "wg",
        );
        assert_eq!(
            argv,
            vec![
                "opencode".to_string(),
                "--model".to_string(),
                "openrouter/stepfun/step-3.7-flash".to_string(),
            ]
        );
        assert!(!argv.iter().any(|a| a == "-e" || a == "--endpoint"));
    }
}
