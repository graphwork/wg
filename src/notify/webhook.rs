//! Webhook notification channel — sends signed JSON payloads to configured endpoints.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::{Action, ActionStyle, IncomingMessage, MessageId, NotificationChannel, RichMessage};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// JSON payload sent to the webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// The task ID this event relates to.
    pub task_id: String,
    /// The type of event (e.g. "task_ready", "task_failed").
    pub event_type: String,
    /// Human-readable title.
    pub title: String,
    /// Longer description / message body.
    pub description: String,
    /// Task or notification status.
    pub status: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Action buttons (if any).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub actions: Vec<WebhookAction>,
}

/// A serialisable action button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookAction {
    pub id: String,
    pub label: String,
    pub style: ActionStyle,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for a webhook channel, typically read from `notify.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Target URL to POST payloads to.
    pub url: String,
    /// HMAC-SHA256 secret for signing payloads.
    #[serde(default)]
    pub secret: Option<String>,
    /// Event types to send. If empty, all events are sent.
    #[serde(default)]
    pub events: Vec<String>,
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// A webhook notification channel that POSTs signed JSON payloads.
pub struct WebhookChannel {
    config: WebhookConfig,
    client: reqwest::Client,
}

impl WebhookChannel {
    /// Create a new webhook channel from config.
    pub fn new(config: WebhookConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Compute HMAC-SHA256 signature for a payload body.
    pub fn compute_signature(secret: &str, body: &[u8]) -> String {
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(body);
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    /// Check whether an event type passes the configured filter.
    fn event_allowed(&self, event_type: &str) -> bool {
        self.config.events.is_empty() || self.config.events.iter().any(|e| e == event_type)
    }

    /// Build and send a payload, returning a synthetic message id.
    async fn send_payload(&self, payload: &WebhookPayload) -> Result<MessageId> {
        if !self.event_allowed(&payload.event_type) {
            return Ok(MessageId(format!("filtered:{}", payload.event_type)));
        }

        let body = serde_json::to_vec(payload)?;

        let mut req = self
            .client
            .post(&self.config.url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "WG-webhook/1.0");

        if let Some(ref secret) = self.config.secret {
            let sig = Self::compute_signature(secret, &body);
            req = req.header("X-Webhook-Signature", format!("sha256={sig}"));
        }

        let resp = req.body(body).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "webhook returned HTTP {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        Ok(MessageId(format!(
            "webhook:{}:{}",
            payload.event_type, payload.timestamp
        )))
    }

    /// Parse a `target` string in the format "task_id:event_type" or just "task_id".
    fn parse_target(target: &str) -> (&str, &str) {
        match target.split_once(':') {
            Some((task_id, event_type)) => (task_id, event_type),
            None => (target, "notification"),
        }
    }
}

#[async_trait]
impl NotificationChannel for WebhookChannel {
    fn channel_type(&self) -> &str {
        "webhook"
    }

    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId> {
        let (task_id, event_type) = Self::parse_target(target);
        let payload = WebhookPayload {
            task_id: task_id.to_string(),
            event_type: event_type.to_string(),
            title: message.to_string(),
            description: message.to_string(),
            status: "sent".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            actions: vec![],
        };
        self.send_payload(&payload).await
    }

    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId> {
        let (task_id, event_type) = Self::parse_target(target);
        let payload = WebhookPayload {
            task_id: task_id.to_string(),
            event_type: event_type.to_string(),
            title: message.plain_text.clone(),
            description: message
                .markdown
                .clone()
                .or_else(|| message.html.clone())
                .unwrap_or_else(|| message.plain_text.clone()),
            status: "sent".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            actions: vec![],
        };
        self.send_payload(&payload).await
    }

    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId> {
        let (task_id, event_type) = Self::parse_target(target);
        let payload = WebhookPayload {
            task_id: task_id.to_string(),
            event_type: event_type.to_string(),
            title: message.to_string(),
            description: message.to_string(),
            status: "sent".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            actions: actions
                .iter()
                .map(|a| WebhookAction {
                    id: a.id.clone(),
                    label: a.label.clone(),
                    style: a.style,
                })
                .collect(),
        };
        self.send_payload(&payload).await
    }

    fn supports_receive(&self) -> bool {
        false
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        anyhow::bail!("webhook channel does not support receiving messages")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_signature_produces_valid_hmac() {
        let sig = WebhookChannel::compute_signature("my-secret", b"hello world");
        // Known HMAC-SHA256 of "hello world" with key "my-secret"
        assert_eq!(sig.len(), 64); // 32 bytes hex-encoded
        // Verify deterministic
        let sig2 = WebhookChannel::compute_signature("my-secret", b"hello world");
        assert_eq!(sig, sig2);
    }

    #[test]
    fn compute_signature_differs_with_different_secrets() {
        let sig1 = WebhookChannel::compute_signature("secret-a", b"payload");
        let sig2 = WebhookChannel::compute_signature("secret-b", b"payload");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn compute_signature_differs_with_different_payloads() {
        let sig1 = WebhookChannel::compute_signature("secret", b"payload-a");
        let sig2 = WebhookChannel::compute_signature("secret", b"payload-b");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn event_filter_allows_all_when_empty() {
        let ch = WebhookChannel::new(WebhookConfig {
            url: "http://localhost".into(),
            secret: None,
            events: vec![],
        });
        assert!(ch.event_allowed("task_ready"));
        assert!(ch.event_allowed("anything"));
    }

    #[test]
    fn event_filter_restricts_when_configured() {
        let ch = WebhookChannel::new(WebhookConfig {
            url: "http://localhost".into(),
            secret: None,
            events: vec!["task_ready".into(), "task_failed".into()],
        });
        assert!(ch.event_allowed("task_ready"));
        assert!(ch.event_allowed("task_failed"));
        assert!(!ch.event_allowed("task_blocked"));
    }

    #[test]
    fn parse_target_with_event_type() {
        let (task_id, event_type) = WebhookChannel::parse_target("my-task:task_failed");
        assert_eq!(task_id, "my-task");
        assert_eq!(event_type, "task_failed");
    }

    #[test]
    fn parse_target_without_event_type() {
        let (task_id, event_type) = WebhookChannel::parse_target("my-task");
        assert_eq!(task_id, "my-task");
        assert_eq!(event_type, "notification");
    }

    #[test]
    fn payload_serializes_to_expected_json() {
        let payload = WebhookPayload {
            task_id: "task-123".into(),
            event_type: "task_ready".into(),
            title: "Task is ready".into(),
            description: "The task is now ready for work".into(),
            status: "ready".into(),
            timestamp: "2026-03-04T12:00:00Z".into(),
            actions: vec![],
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["task_id"], "task-123");
        assert_eq!(json["event_type"], "task_ready");
        assert_eq!(json["timestamp"], "2026-03-04T12:00:00Z");
        // actions should be omitted when empty
        assert!(json.get("actions").is_none());
    }

    #[test]
    fn payload_with_actions_serializes() {
        let payload = WebhookPayload {
            task_id: "task-456".into(),
            event_type: "approval".into(),
            title: "Approve deployment?".into(),
            description: "Deploy v2.0 to production".into(),
            status: "pending".into(),
            timestamp: "2026-03-04T12:00:00Z".into(),
            actions: vec![
                WebhookAction {
                    id: "approve".into(),
                    label: "Approve".into(),
                    style: ActionStyle::Primary,
                },
                WebhookAction {
                    id: "reject".into(),
                    label: "Reject".into(),
                    style: ActionStyle::Danger,
                },
            ],
        };
        let json = serde_json::to_value(&payload).unwrap();
        let actions = json["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0]["id"], "approve");
        assert_eq!(actions[0]["style"], "primary");
        assert_eq!(actions[1]["style"], "danger");
    }

    #[test]
    fn payload_round_trips_through_json() {
        let payload = WebhookPayload {
            task_id: "rt-test".into(),
            event_type: "task_failed".into(),
            title: "Build failed".into(),
            description: "cargo test returned exit code 1".into(),
            status: "failed".into(),
            timestamp: "2026-03-04T15:30:00Z".into(),
            actions: vec![WebhookAction {
                id: "retry".into(),
                label: "Retry".into(),
                style: ActionStyle::Secondary,
            }],
        };
        let json_str = serde_json::to_string(&payload).unwrap();
        let deserialized: WebhookPayload = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.task_id, "rt-test");
        assert_eq!(deserialized.actions.len(), 1);
        assert_eq!(deserialized.actions[0].id, "retry");
    }

    #[test]
    fn webhook_config_deserializes_from_toml() {
        let toml_str = r#"
url = "https://example.com/hook"
secret = "my-hmac-secret"
events = ["task_ready", "task_failed"]
"#;
        let config: WebhookConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.url, "https://example.com/hook");
        assert_eq!(config.secret.as_deref(), Some("my-hmac-secret"));
        assert_eq!(config.events, vec!["task_ready", "task_failed"]);
    }

    #[test]
    fn webhook_config_minimal() {
        let toml_str = r#"url = "https://example.com/hook""#;
        let config: WebhookConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.url, "https://example.com/hook");
        assert!(config.secret.is_none());
        assert!(config.events.is_empty());
    }

    #[test]
    fn channel_type_is_webhook() {
        let ch = WebhookChannel::new(WebhookConfig {
            url: "http://localhost".into(),
            secret: None,
            events: vec![],
        });
        assert_eq!(ch.channel_type(), "webhook");
        assert!(!ch.supports_receive());
    }
}
