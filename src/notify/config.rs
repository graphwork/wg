//! Parse `notify.toml` configuration for notification routing.
//!
//! The config file lives at `~/.config/workgraph/notify.toml` (or the project's
//! `.workgraph/notify.toml`). See `docs/research/human-in-the-loop-channels.md`
//! for the full schema.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{EventType, RoutingRule};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Parsed representation of `notify.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifyConfig {
    /// Routing rules keyed by event category.
    #[serde(default)]
    pub routing: RoutingConfig,

    /// Escalation timeouts in seconds.
    #[serde(default)]
    pub escalation: EscalationConfig,

    /// Per-channel configuration sections (opaque — each channel parses its own).
    #[serde(flatten)]
    pub channels: HashMap<String, toml::Value>,
}

/// Which channels to use for each event category.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoutingConfig {
    /// Default channels when no specific rule matches.
    #[serde(default)]
    pub default: Vec<String>,

    /// Channels for urgent events (escalation chain order).
    #[serde(default)]
    pub urgent: Vec<String>,

    /// Channels for approval requests.
    #[serde(default)]
    pub approval: Vec<String>,

    /// Channels for digest/summary messages.
    #[serde(default)]
    pub digest: Vec<String>,
}

/// Escalation timeout configuration (values in seconds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationConfig {
    /// Seconds before escalating an unanswered approval request.
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout: u64,

    /// Seconds before escalating an unanswered urgent notification.
    #[serde(default = "default_urgent_timeout")]
    pub urgent_timeout: u64,
}

impl Default for EscalationConfig {
    fn default() -> Self {
        Self {
            approval_timeout: default_approval_timeout(),
            urgent_timeout: default_urgent_timeout(),
        }
    }
}

fn default_approval_timeout() -> u64 {
    1800
}

fn default_urgent_timeout() -> u64 {
    3600
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl NotifyConfig {
    /// Load from an explicit path.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    /// Load from the default location (`~/.config/workgraph/notify.toml`).
    /// Returns `Ok(None)` if the file does not exist.
    pub fn load_default() -> Result<Option<Self>> {
        if let Some(path) = default_config_path() {
            if path.exists() {
                return Ok(Some(Self::load_from(&path)?));
            }
        }
        Ok(None)
    }

    /// Load from project-local `.workgraph/notify.toml`, falling back to global.
    pub fn load(project_root: Option<&Path>) -> Result<Option<Self>> {
        if let Some(root) = project_root {
            let local = root.join(".workgraph").join("notify.toml");
            if local.exists() {
                return Ok(Some(Self::load_from(&local)?));
            }
        }
        Self::load_default()
    }

    /// Convert routing config into a list of [`RoutingRule`]s.
    pub fn to_routing_rules(&self) -> Vec<RoutingRule> {
        let mut rules = Vec::new();

        let mapping: &[(EventType, &[String], Option<u64>)] = &[
            (
                EventType::Urgent,
                &self.routing.urgent,
                Some(self.escalation.urgent_timeout),
            ),
            (
                EventType::Approval,
                &self.routing.approval,
                Some(self.escalation.approval_timeout),
            ),
        ];

        for &(event_type, channels, timeout) in mapping {
            if !channels.is_empty() {
                rules.push(RoutingRule {
                    event_type,
                    channels: channels.to_vec(),
                    escalation_timeout: timeout.map(Duration::from_secs),
                });
            }
        }

        // Digest doesn't get escalation — it's inherently async.
        if !self.routing.digest.is_empty() {
            // We don't have a Digest event type yet, but the channel list is available
            // via self.routing.digest for future use.
        }

        rules
    }

    /// The default channel list from config.
    pub fn default_channels(&self) -> &[String] {
        &self.routing.default
    }

    /// Check whether a named channel section exists in the config.
    pub fn has_channel_config(&self, name: &str) -> bool {
        self.channels.contains_key(name)
    }

    /// Return a human-readable status summary of the notification config.
    pub fn status_summary(&self) -> String {
        let mut lines = Vec::new();

        if self.routing.default.is_empty() {
            lines.push("Default channels: (none)".to_string());
        } else {
            lines.push(format!("Default channels: {}", self.routing.default.join(", ")));
        }

        if !self.routing.urgent.is_empty() {
            lines.push(format!(
                "Urgent chain: {} (escalate after {}s)",
                self.routing.urgent.join(" → "),
                self.escalation.urgent_timeout
            ));
        }

        if !self.routing.approval.is_empty() {
            lines.push(format!(
                "Approval chain: {} (escalate after {}s)",
                self.routing.approval.join(" → "),
                self.escalation.approval_timeout
            ));
        }

        if !self.routing.digest.is_empty() {
            lines.push(format!("Digest channels: {}", self.routing.digest.join(", ")));
        }

        let configured: Vec<&String> = self.channels.keys().collect();
        if !configured.is_empty() {
            lines.push(format!(
                "Configured channel sections: {}",
                configured
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        lines.join("\n")
    }
}

/// Return the default global config path.
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("workgraph").join("notify.toml"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
[routing]
default = ["telegram"]
"#;
        let config: NotifyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.routing.default, vec!["telegram"]);
        assert!(config.routing.urgent.is_empty());
        assert_eq!(config.escalation.approval_timeout, 1800);
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[routing]
default = ["telegram"]
urgent = ["telegram", "sms"]
approval = ["telegram"]
digest = ["email"]

[escalation]
approval_timeout = 900
urgent_timeout = 1800

[telegram]
bot_token = "123:ABC"
chat_id = "456"

[email]
smtp_host = "smtp.example.com"
to = ["user@example.com"]
"#;
        let config: NotifyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.routing.default, vec!["telegram"]);
        assert_eq!(config.routing.urgent, vec!["telegram", "sms"]);
        assert_eq!(config.escalation.approval_timeout, 900);
        assert!(config.has_channel_config("telegram"));
        assert!(config.has_channel_config("email"));
        assert!(!config.has_channel_config("slack"));
    }

    #[test]
    fn to_routing_rules_generates_correct_rules() {
        let config = NotifyConfig {
            routing: RoutingConfig {
                default: vec!["telegram".into()],
                urgent: vec!["telegram".into(), "sms".into()],
                approval: vec!["telegram".into()],
                digest: vec!["email".into()],
            },
            escalation: EscalationConfig {
                approval_timeout: 900,
                urgent_timeout: 1800,
            },
            channels: HashMap::new(),
        };

        let rules = config.to_routing_rules();
        assert_eq!(rules.len(), 2);

        let urgent_rule = rules.iter().find(|r| r.event_type == EventType::Urgent).unwrap();
        assert_eq!(urgent_rule.channels, vec!["telegram", "sms"]);
        assert_eq!(urgent_rule.escalation_timeout, Some(Duration::from_secs(1800)));

        let approval_rule = rules.iter().find(|r| r.event_type == EventType::Approval).unwrap();
        assert_eq!(approval_rule.channels, vec!["telegram"]);
        assert_eq!(approval_rule.escalation_timeout, Some(Duration::from_secs(900)));
    }

    #[test]
    fn status_summary_formats_nicely() {
        let config = NotifyConfig {
            routing: RoutingConfig {
                default: vec!["telegram".into()],
                urgent: vec!["telegram".into(), "sms".into()],
                approval: vec![],
                digest: vec![],
            },
            escalation: EscalationConfig::default(),
            channels: HashMap::new(),
        };

        let summary = config.status_summary();
        assert!(summary.contains("Default channels: telegram"));
        assert!(summary.contains("Urgent chain: telegram → sms"));
    }

    #[test]
    fn empty_config_is_valid() {
        let config: NotifyConfig = toml::from_str("").unwrap();
        assert!(config.routing.default.is_empty());
        assert_eq!(config.to_routing_rules().len(), 0);
    }

    #[test]
    fn load_from_nonexistent_errors() {
        let result = NotifyConfig::load_from(Path::new("/nonexistent/notify.toml"));
        assert!(result.is_err());
    }
}
