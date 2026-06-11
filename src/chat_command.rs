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

/// Normalize a per-chat model string into the `--model` value the OpenCode
/// CLI expects (its `openrouter/<vendor>/<model>` spelling).
///
/// Accepts all three user-facing spellings:
/// - executor-qualified `opencode:openrouter/<vendor>/<model>` (the chat
///   route a user pins; `opencode` is an executor, not a model provider, so
///   it is deliberately absent from `KNOWN_PROVIDERS` and must be stripped
///   here rather than via `parse_model_spec`),
/// - provider-qualified `openrouter:<vendor>/<model>`,
/// - bare CLI form `openrouter/<vendor>/<model>`.
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
        spec.model_id.clone()
    } else {
        model.to_string()
    };
    if model_arg.is_empty() {
        None
    } else {
        Some(model_arg)
    }
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
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                let spec = parse_model_spec(m);
                argv.push("-m".to_string());
                argv.push(if spec.model_id.is_empty() {
                    m.to_string()
                } else {
                    spec.model_id
                });
            }
            if let Some(ep) = endpoint.filter(|ep| !ep.is_empty()) {
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
