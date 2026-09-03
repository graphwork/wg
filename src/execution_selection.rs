//! Source-aware execution-system selection.
//!
//! WG's serde defaults are useful for display/catalog compatibility, but they
//! are not permission to dispatch an LLM.  This module deliberately consults
//! the source map produced from on-disk configuration before accepting a route.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigSource, parse_exact_pi_route, parse_supported_execution_route};
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

pub fn canonical_explicit_route(raw: &str) -> Option<String> {
    let raw = raw.trim();
    parse_supported_execution_route(raw)
        .ok()
        .map(|_| raw.to_string())
}

/// Return a route only when its leading token names an execution handler.
/// Provider-qualified Pi model dialects such as `openrouter:vendor/model`
/// deliberately return `None`: a dispatcher binding must reconstruct those as
/// `pi:openrouter:vendor/model`, never silently reinterpret them as nex.
pub fn handler_qualified_explicit_route(raw: &str) -> Option<String> {
    let prefix = raw.trim().split_once(':')?.0;
    let handler = crate::dispatch::ExecutorKind::from_str(prefix)?;
    if matches!(
        handler,
        crate::dispatch::ExecutorKind::Shell | crate::dispatch::ExecutorKind::RemoteRunner
    ) {
        return None;
    }
    canonical_explicit_route(raw)
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
        // A global label is compatibility-inspection data only. Even a caller
        // that constructs an old source map cannot turn it into authority.
        if matches!(source, ConfigSource::Default | ConfigSource::Global) {
            continue;
        }
        let route = raw.trim().to_string();
        parse_exact_pi_route(&route).map_err(|error| {
            anyhow::anyhow!(
                "error[WG-PI-ROUTE-REQUIRED]: {key} selects non-Pi project route {raw:?}: {error}. Select an exact `pi:<provider>:<model>` route; no machine-global or cross-system fallback was attempted"
            )
        })?;
        let path = match source {
            ConfigSource::ProjectFile | ConfigSource::ProjectProfileImport => {
                crate::project_config::path_for_graph(dir).ok_or_else(|| {
                    anyhow::anyhow!("project-file source has no bound project path")
                })?
            }
            ConfigSource::Local => dir.join("config.toml"),
            ConfigSource::ProjectProfile => crate::profile::project::association_path(dir),
            ConfigSource::Global | ConfigSource::Default => continue,
        };
        let selection_source = match source {
            ConfigSource::ProjectProfile => {
                let association =
                    crate::profile::project::read_association(dir)?.ok_or_else(|| {
                        anyhow::anyhow!("legacy project-profile source has no association")
                    })?;
                ExecutionSelectionSource::Profile {
                    name: association.profile,
                    path,
                }
            }
            ConfigSource::ProjectProfileImport => {
                let document = crate::project_config::load_for_graph(dir)?.ok_or_else(|| {
                    anyhow::anyhow!("project-profile-import source has no project document")
                })?;
                let origin = document.profile_origin.ok_or_else(|| {
                    anyhow::anyhow!("project-profile-import source has no profile_origin")
                })?;
                ExecutionSelectionSource::Profile {
                    name: origin.name,
                    path,
                }
            }
            _ => ExecutionSelectionSource::Config {
                scope: *source,
                path,
                key: key.into(),
            },
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
        "error[{UNSELECTED_CODE}]: No project Pi route is selected.\nThis WG is available for graph-only use, but `{operation}` requires an LLM route.\n\nSelect the route for this project explicitly:\n  wg profile select pi\n  wg setup --route pi --yes --model pi:<provider>:<model>\n\nPi owns provider authentication, endpoints, and model discovery. WG global config, WG secrets, and ~/.wg/active-profile never select a project route. `wg init`, graph reads, graph edits, and the setup-neutral TUI remain credential-free and do not create a route."
    )
}

fn ignored_legacy_machine_routing(dir: &Path) -> Vec<String> {
    // A source-owned project file makes legacy project inputs inactive too;
    // mention them without reading values.
    let mut ignored = Vec::new();
    if crate::project_config::exists_for_graph(dir) {
        let local = dir.join("config.toml");
        if local.exists() {
            ignored.push(format!("{} (legacy project config)", local.display()));
        }
        let association = crate::profile::project::association_path(dir);
        if association.exists() {
            ignored.push(format!(
                "{} (legacy profile association)",
                association.display()
            ));
        }
    }

    if let Ok(path) = Config::global_config_path()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(value) = text.parse::<toml::Value>()
    {
        let has_route = value
            .get("agent")
            .and_then(|table| table.get("model"))
            .is_some()
            || value
                .get("dispatcher")
                .or_else(|| value.get("coordinator"))
                .and_then(|table| table.get("model"))
                .is_some()
            || value.get("models").is_some()
            || value.get("tiers").is_some()
            || value.get("profile").is_some();
        if has_route {
            ignored.push(format!("{} (legacy machine routing)", path.display()));
        }
    }
    if let Ok(path) = crate::profile::named::active_pointer_path()
        && let Ok(name) = std::fs::read_to_string(&path)
    {
        let name = name.trim();
        if !name.is_empty() {
            ignored.push(format!("{} ({name})", path.display()));
        }
    }
    ignored
}

pub fn unselected_message_for(dir: &Path, operation: &str) -> String {
    let mut message = unselected_message(operation);
    let ignored = ignored_legacy_machine_routing(dir);
    if !ignored.is_empty() {
        message.push_str(
            "\n\nIgnored legacy routing (inactive; it does not configure this project):\n  ",
        );
        message.push_str(&ignored.join("\n  "));
    }
    message
}

pub fn require(
    dir: &Path,
    model: Option<(&str, bool)>,
    operation: &str,
) -> Result<ExecutionSelection> {
    let selection = resolve(dir, model)?;
    if selection.state == SelectionState::Unselected {
        bail!(unselected_message_for(dir, operation));
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
    fn provider_dialect_is_not_mistaken_for_a_handler_qualified_route() {
        assert_eq!(
            handler_qualified_explicit_route("pi:openrouter:vendor/model").as_deref(),
            Some("pi:openrouter:vendor/model")
        );
        assert_eq!(
            handler_qualified_explicit_route("openrouter:vendor/model"),
            None
        );
        assert_eq!(
            handler_qualified_explicit_route("codex:gpt-5.5").as_deref(),
            Some("codex:gpt-5.5")
        );
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
