//! Source-aware execution-system selection.
//!
//! WG's serde defaults are useful for display/catalog compatibility, but they
//! are not permission to dispatch an LLM.  This module deliberately consults
//! the source map produced from on-disk configuration before accepting a route.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigSource, parse_supported_execution_route};
use crate::dispatch::handler_for_model;

pub const UNSELECTED_CODE: &str = "WG-EXEC-UNSELECTED";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionState {
    Unselected,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSystemKey {
    pub handler: String,
    pub wire: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionSelectionSource {
    Cli {
        flag: String,
    },
    Task {
        field: String,
    },
    Profile {
        name: String,
        path: PathBuf,
    },
    Config {
        scope: ConfigSource,
        path: PathBuf,
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSelection {
    pub state: SelectionState,
    pub route: Option<String>,
    pub system: Option<ExecutionSystemKey>,
    pub source: Option<ExecutionSelectionSource>,
}

impl ExecutionSelection {
    pub fn unselected() -> Self {
        Self {
            state: SelectionState::Unselected,
            route: None,
            system: None,
            source: None,
        }
    }
}

fn canonical_explicit_route(raw: &str) -> Option<String> {
    let raw = raw.trim();
    parse_supported_execution_route(raw)
        .ok()
        .map(|_| raw.to_string())
}

pub fn system_key(route: &str) -> Option<ExecutionSystemKey> {
    let handler = handler_for_model(route).as_str().to_string();
    let wire = match handler.as_str() {
        "claude" => "anthropic-cli".to_string(),
        "codex" => "openai-codex-cli".to_string(),
        "native" => {
            let inner = route.split_once(':')?.1;
            inner
                .split_once(':')
                .map(|(wire, _)| wire)
                .unwrap_or("oai-compat")
                .to_string()
        }
        "pi" => {
            let inner = route.split_once(':')?.1;
            inner
                .split_once([':', '/'])
                .map(|(wire, _)| wire)
                .unwrap_or("pi-native")
                .to_string()
        }
        other => format!("{other}-native"),
    };
    Some(ExecutionSystemKey { handler, wire })
}

/// Resolve a route only when its winning value came from CLI/task/on-disk
/// configuration. Values whose source is `Default` remain inactive.
pub fn resolve(dir: &Path, cli_or_task_model: Option<(&str, bool)>) -> Result<ExecutionSelection> {
    if let Some((raw, is_task)) = cli_or_task_model {
        let route = canonical_explicit_route(raw).ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-EXEC-ROUTE-REQUIRED]: explicit model `{raw}` is not `pi:<provider>:<model>`, `claude:<native-model>`, or `codex:<native-model>`; no fallback was attempted"
            )
        })?;
        let source = if is_task {
            ExecutionSelectionSource::Task {
                field: "task.model".into(),
            }
        } else {
            ExecutionSelectionSource::Cli {
                flag: "--model".into(),
            }
        };
        return Ok(ExecutionSelection {
            state: SelectionState::Selected,
            system: system_key(&route),
            route: Some(route),
            source: Some(source),
        });
    }

    let (config, sources) = Config::load_with_sources(dir)?;
    resolve_config_sources(dir, &config, &sources)
}

fn resolve_config_sources(
    dir: &Path,
    config: &Config,
    sources: &std::collections::BTreeMap<String, ConfigSource>,
) -> Result<ExecutionSelection> {
    let candidates: [(&str, Option<&str>); 4] = [
        ("dispatcher.model", config.coordinator.model.as_deref()),
        (
            "models.task_agent.model",
            config
                .models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
        ),
        (
            "models.default.model",
            config
                .models
                .default
                .as_ref()
                .and_then(|m| m.model.as_deref()),
        ),
        ("agent.model", Some(config.agent.model.as_str())),
    ];
    for (key, value) in candidates {
        let Some(raw) = value else { continue };
        let Some(source) = sources.get(key) else {
            continue;
        };
        if *source == ConfigSource::Default {
            continue;
        }
        let Some(route) = canonical_explicit_route(raw) else {
            anyhow::bail!(
                "error[WG-EXEC-ROUTE-REQUIRED]: {key} selects unsupported route {raw:?}; worker roles require explicit `pi:<provider>:<model>`, `claude:<native-model>`, or `codex:<native-model>` and never fall back"
            );
        };
        let path = match source {
            ConfigSource::Global => Config::global_config_path()?,
            ConfigSource::Local => dir.join("config.toml"),
            ConfigSource::ProjectProfile => crate::profile::project::read_association(dir)?
                .map(|association| crate::profile::named::profile_path(&association.profile))
                .transpose()?
                .unwrap_or_else(|| crate::profile::project::association_path(dir)),
            ConfigSource::Default => continue,
        };
        let selection_source = if *source == ConfigSource::ProjectProfile {
            let association = crate::profile::project::read_association(dir)?
                .ok_or_else(|| anyhow::anyhow!("project-profile source has no association"))?;
            ExecutionSelectionSource::Profile {
                name: association.profile,
                path,
            }
        } else if *source == ConfigSource::Global {
            if let Ok(Some(name)) = crate::profile::named::active() {
                ExecutionSelectionSource::Profile {
                    name: name.clone(),
                    path: crate::profile::named::profile_path(&name)?,
                }
            } else {
                ExecutionSelectionSource::Config {
                    scope: source.clone(),
                    path,
                    key: key.into(),
                }
            }
        } else {
            ExecutionSelectionSource::Config {
                scope: source.clone(),
                path,
                key: key.into(),
            }
        };
        return Ok(ExecutionSelection {
            state: SelectionState::Selected,
            system: system_key(&route),
            route: Some(route),
            source: Some(selection_source),
        });
    }
    Ok(ExecutionSelection::unselected())
}

pub fn unselected_message(operation: &str) -> String {
    format!(
        "error[{UNSELECTED_CODE}]: no LLM execution system has been selected.\nThis WG is available for graph-only use, but `{operation}` requires an LLM route.\n\nChoose Pi explicitly (recommended):\n  wg setup --route pi --yes --model pi:<provider>:<model>\n  wg profile select pi\n  wg config --global --model pi:<provider>:<model>\n  wg config --local  --model pi:<provider>:<model>\n\nOr opt into a native CLI:\n  wg profile select claude\n  wg config --local --model claude:<native-model>\n  wg profile select codex\n  wg config --local --model codex:<native-model>\n\nPi owns its providers and authentication; the Claude and Codex CLIs own login and accept native model IDs unchanged. Native Claude worker/task support does not imply Claude chat support. `wg init`, graph reads, graph edits, and the TUI remain credential-free and do not create a route."
    )
}

pub fn require(
    dir: &Path,
    model: Option<(&str, bool)>,
    operation: &str,
) -> Result<ExecutionSelection> {
    let selection = resolve(dir, model)?;
    if selection.state == SelectionState::Unselected {
        bail!(unselected_message(operation));
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_source_is_inactive() {
        let dir = TempDir::new().unwrap();
        let config = Config::default();
        let sources =
            std::collections::BTreeMap::from([("agent.model".to_string(), ConfigSource::Default)]);
        assert_eq!(
            resolve_config_sources(dir.path(), &config, &sources)
                .unwrap()
                .state,
            SelectionState::Unselected
        );
    }

    #[test]
    fn explicit_local_source_selects() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.agent.model = "pi:test:worker".into();
        let sources =
            std::collections::BTreeMap::from([("agent.model".to_string(), ConfigSource::Local)]);
        let selected = resolve_config_sources(dir.path(), &config, &sources).unwrap();
        assert_eq!(selected.state, SelectionState::Selected);
        assert_eq!(selected.route.as_deref(), Some("pi:test:worker"));
    }

    #[test]
    fn system_keys_separate_handler_and_wire() {
        assert_eq!(
            system_key("pi:openrouter:z-ai/glm").unwrap().wire,
            "openrouter"
        );
        assert_eq!(
            system_key("nex:openrouter:z-ai/glm").unwrap().handler,
            "native"
        );
        assert_ne!(
            system_key("pi:openrouter:x"),
            system_key("nex:openrouter:x")
        );
    }
}
