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
