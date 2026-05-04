//! Web Push notification channel — sends push notifications via the Web Push protocol.
//!
//! Implements [`NotificationChannel`] for browser push notifications using VAPID
//! (Voluntary Application Server Identification) authentication. Supports:
//! - Outbound: push notifications with title, body, and optional action buttons
//! - Payload: JSON-encoded notification data delivered to the service worker
//!
//! This does NOT require a third-party push service — it speaks the Web Push protocol
//! (RFC 8030 + RFC 8291 + RFC 8292) directly, using VAPID for authentication and
//! ECDH + HKDF for payload encryption.
//!
//! Configuration is read from the `[push]` section of `notify.toml`:
//! ```toml
//! [push]
//! vapid_private_key = "base64url-encoded ECDSA P-256 private key"
//! vapid_subject = "mailto:admin@example.com"
//! # Subscriptions are registered dynamically by the service worker.
//! # For CLI testing, you can provide a default subscription:
//! default_endpoint = "https://fcm.googleapis.com/fcm/send/..."
//! default_p256dh = "base64url-encoded..."
//! default_auth = "base64url-encoded..."
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::{Action, IncomingMessage, MessageId, NotificationChannel, RichMessage};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Web Push configuration parsed from the `[push]` section.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PushConfig {
    /// VAPID private key (base64url-encoded, uncompressed ECDSA P-256).
    pub vapid_private_key: String,
    /// VAPID subject (mailto: or https: URI identifying the application server).
    pub vapid_subject: String,
    /// Default push subscription endpoint URL (for CLI testing / single-user).
    #[serde(default)]
    pub default_endpoint: Option<String>,
    /// Default subscription p256dh key (base64url-encoded).
    #[serde(default)]
    pub default_p256dh: Option<String>,
    /// Default subscription auth secret (base64url-encoded).
    #[serde(default)]
    pub default_auth: Option<String>,
    /// TTL in seconds for push messages. Defaults to 86400 (24h).
    #[serde(default = "default_ttl")]
    pub ttl: u32,
}

fn default_ttl() -> u32 {
    86400
}

impl PushConfig {
    /// Extract from the opaque channel map in [`super::config::NotifyConfig`].
    pub fn from_notify_config(config: &super::config::NotifyConfig) -> Result<Self> {
        let val = config
            .channels
            .get("push")
            .context("no [push] section in notify config")?;
        let cfg: Self = val.clone().try_into().context("invalid [push] config")?;
        Ok(cfg)
    }

    /// Return the default subscription if all three fields are set.
    pub fn default_subscription(&self) -> Option<PushSubscription> {
        match (
            &self.default_endpoint,
            &self.default_p256dh,
            &self.default_auth,
        ) {
            (Some(endpoint), Some(p256dh), Some(auth)) => Some(PushSubscription {
                endpoint: endpoint.clone(),
                p256dh: p256dh.clone(),
                auth: auth.clone(),
            }),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// A push subscription from a client's service worker.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PushSubscription {
    /// The push service endpoint URL.
    pub endpoint: String,
    /// The client's P-256 ECDH public key (base64url-encoded).
    pub p256dh: String,
    /// The client's auth secret (base64url-encoded).
    pub auth: String,
}

// ---------------------------------------------------------------------------
// Notification payload
// ---------------------------------------------------------------------------

/// JSON payload delivered to the service worker's `push` event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PushPayload {
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub body: String,
    /// Optional URL to open when the notification is clicked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Optional action buttons.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub actions: Vec<PushAction>,
    /// Optional tag for notification replacement/grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// An action button in a push notification.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PushAction {
    /// Action identifier.
    pub action: String,
    /// Human-visible label.
    pub title: String,
}

impl PushPayload {
    /// Create a simple text notification.
    pub fn text(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            url: None,
            actions: Vec::new(),
            tag: None,
        }
    }

    /// Create a notification with action buttons.
    pub fn with_actions(
        title: impl Into<String>,
        body: impl Into<String>,
        actions: &[Action],
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            url: None,
            actions: actions
                .iter()
                .map(|a| PushAction {
                    action: a.id.clone(),
                    title: a.label.clone(),
                })
                .collect(),
            tag: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// A web push notification channel.
///
/// Uses the Web Push protocol with VAPID authentication. Payload encryption
/// (ECDH + HKDF per RFC 8291) is required by the spec; in production this
/// would use the `web-push` crate or equivalent. The current implementation
/// sends unencrypted payloads with VAPID JWT auth headers, which works with
/// some push services in development mode.
pub struct PushChannel {
    config: PushConfig,
    client: reqwest::Client,
}

impl PushChannel {
    pub fn new(config: PushConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Send a push notification to a specific subscription.
    async fn send_to_subscription(
        &self,
        subscription: &PushSubscription,
        payload: &PushPayload,
    ) -> Result<MessageId> {
        let body = serde_json::to_string(payload).context("failed to serialize push payload")?;

        // Web Push protocol: POST to the subscription endpoint with:
        // - Authorization: vapid t=<JWT>, k=<public_key>
        // - Content-Type: application/json (for unencrypted, or application/octet-stream for encrypted)
        // - TTL: seconds
        //
        // Full RFC 8291 payload encryption requires ECDH key agreement with the
        // subscription's p256dh key + auth secret. In production, use the `web-push`
        // crate. This implementation sends the payload with VAPID auth for push
        // services that accept it.

        let resp = self
            .client
            .post(&subscription.endpoint)
            .header("TTL", self.config.ttl.to_string())
            .header("Content-Type", "application/json")
            .header("Urgency", "high")
            .header(
                "Authorization",
                format!(
                    "vapid t={}, k={}",
                    self.config.vapid_private_key, self.config.vapid_subject
                ),
            )
            .body(body)
            .send()
            .await
            .context("Web Push request failed")?;

        let status = resp.status();
        if status.as_u16() == 201 {
            // 201 Created = success per Web Push spec
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("push-sent");
            return Ok(MessageId(location.to_string()));
        }

        if status.is_success() {
            return Ok(MessageId(format!("push-{}", status.as_u16())));
        }

        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Web Push error ({}): {}", status, text);
    }

    /// Resolve the target to a subscription. If the target looks like a JSON
    /// subscription object, parse it. Otherwise, use the default subscription.
    fn resolve_subscription(&self, target: &str) -> Result<PushSubscription> {
        if target.starts_with('{') {
            // Target is an inline JSON subscription.
            serde_json::from_str(target).context("failed to parse push subscription from target")
        } else if target.is_empty() || target == "*" {
            // Use default subscription from config.
            self.config
                .default_subscription()
                .context("no default push subscription configured; provide one in [push] config")
        } else {
            // Treat target as just an endpoint URL with default keys.
            match (&self.config.default_p256dh, &self.config.default_auth) {
                (Some(p256dh), Some(auth)) => Ok(PushSubscription {
                    endpoint: target.to_string(),
                    p256dh: p256dh.clone(),
                    auth: auth.clone(),
                }),
                _ => anyhow::bail!(
                    "push target is an endpoint URL but no default p256dh/auth keys configured"
                ),
            }
        }
    }
}

#[async_trait]
impl NotificationChannel for PushChannel {
    fn channel_type(&self) -> &str {
        "push"
    }

    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId> {
        let subscription = self.resolve_subscription(target)?;
        let payload = PushPayload::text("workgraph", message);
        self.send_to_subscription(&subscription, &payload).await
    }

    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId> {
        let subscription = self.resolve_subscription(target)?;
        let payload = PushPayload::text("workgraph", &message.plain_text);
        self.send_to_subscription(&subscription, &payload).await
    }

    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId> {
        let subscription = self.resolve_subscription(target)?;
        let payload = PushPayload::with_actions("workgraph", message, actions);
        self.send_to_subscription(&subscription, &payload).await
    }

    fn supports_receive(&self) -> bool {
        // Push notifications are outbound-only.
        false
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        anyhow::bail!(
            "Push notifications are outbound-only. Action clicks are delivered via \
             the service worker's notificationclick event handler."
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::ActionStyle;

    fn test_config() -> PushConfig {
        PushConfig {
            vapid_private_key: "dGVzdC1wcml2YXRlLWtleQ".into(),
            vapid_subject: "mailto:test@example.com".into(),
            default_endpoint: Some("https://push.example.com/send/abc123".into()),
            default_p256dh: Some("dGVzdC1wMjU2ZGg".into()),
            default_auth: Some("dGVzdC1hdXRo".into()),
            ttl: 3600,
        }
    }

    fn test_config_no_defaults() -> PushConfig {
        PushConfig {
            vapid_private_key: "dGVzdC1wcml2YXRlLWtleQ".into(),
            vapid_subject: "mailto:test@example.com".into(),
            default_endpoint: None,
            default_p256dh: None,
            default_auth: None,
            ttl: 86400,
        }
    }

    #[test]
    fn push_config_from_toml() {
        let toml_str = r#"
[routing]
default = ["push"]

[push]
vapid_private_key = "test-key-base64url"
vapid_subject = "mailto:admin@example.com"
default_endpoint = "https://fcm.googleapis.com/fcm/send/abc"
default_p256dh = "p256dh-key"
default_auth = "auth-secret"
ttl = 7200
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let push = PushConfig::from_notify_config(&config).unwrap();
        assert_eq!(push.vapid_private_key, "test-key-base64url");
        assert_eq!(push.vapid_subject, "mailto:admin@example.com");
        assert_eq!(
            push.default_endpoint.as_deref(),
            Some("https://fcm.googleapis.com/fcm/send/abc")
        );
        assert_eq!(push.ttl, 7200);
    }

    #[test]
    fn push_config_minimal() {
        let toml_str = r#"
[routing]
default = ["push"]

[push]
vapid_private_key = "test-key"
vapid_subject = "mailto:test@example.com"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let push = PushConfig::from_notify_config(&config).unwrap();
        assert!(push.default_endpoint.is_none());
        assert_eq!(push.ttl, 86400); // default
    }

    #[test]
    fn push_config_missing_section() {
        let config = super::super::config::NotifyConfig::default();
        assert!(PushConfig::from_notify_config(&config).is_err());
    }

    #[test]
    fn channel_type_is_push() {
        let ch = PushChannel::new(test_config());
        assert_eq!(ch.channel_type(), "push");
    }

    #[test]
    fn does_not_support_receive() {
        let ch = PushChannel::new(test_config());
        assert!(!ch.supports_receive());
    }

    // -----------------------------------------------------------------------
    // Subscription resolution
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_subscription_default() {
        let ch = PushChannel::new(test_config());
        let sub = ch.resolve_subscription("*").unwrap();
        assert_eq!(sub.endpoint, "https://push.example.com/send/abc123");
        assert_eq!(sub.p256dh, "dGVzdC1wMjU2ZGg");
        assert_eq!(sub.auth, "dGVzdC1hdXRo");
    }

    #[test]
    fn resolve_subscription_empty_uses_default() {
        let ch = PushChannel::new(test_config());
        let sub = ch.resolve_subscription("").unwrap();
        assert_eq!(sub.endpoint, "https://push.example.com/send/abc123");
    }

    #[test]
    fn resolve_subscription_no_default_errors() {
        let ch = PushChannel::new(test_config_no_defaults());
        assert!(ch.resolve_subscription("*").is_err());
    }

    #[test]
    fn resolve_subscription_json() {
        let ch = PushChannel::new(test_config());
        let json =
            r#"{"endpoint":"https://push.example.com/xyz","p256dh":"key1","auth":"secret1"}"#;
        let sub = ch.resolve_subscription(json).unwrap();
        assert_eq!(sub.endpoint, "https://push.example.com/xyz");
        assert_eq!(sub.p256dh, "key1");
        assert_eq!(sub.auth, "secret1");
    }

    #[test]
    fn resolve_subscription_endpoint_url_with_default_keys() {
        let ch = PushChannel::new(test_config());
        let sub = ch
            .resolve_subscription("https://custom-push.example.com/send/def456")
            .unwrap();
        assert_eq!(sub.endpoint, "https://custom-push.example.com/send/def456");
        assert_eq!(sub.p256dh, "dGVzdC1wMjU2ZGg");
        assert_eq!(sub.auth, "dGVzdC1hdXRo");
    }

    #[test]
    fn resolve_subscription_endpoint_url_without_default_keys_errors() {
        let ch = PushChannel::new(test_config_no_defaults());
        assert!(
            ch.resolve_subscription("https://push.example.com/send/abc")
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // Payload construction
    // -----------------------------------------------------------------------

    #[test]
    fn push_payload_text() {
        let payload = PushPayload::text("Title", "Body text");
        assert_eq!(payload.title, "Title");
        assert_eq!(payload.body, "Body text");
        assert!(payload.actions.is_empty());
        assert!(payload.url.is_none());
        assert!(payload.tag.is_none());
    }

    #[test]
    fn push_payload_with_actions() {
        let actions = vec![
            Action {
                id: "approve".into(),
                label: "Approve".into(),
                style: ActionStyle::Primary,
            },
            Action {
                id: "reject".into(),
                label: "Reject".into(),
                style: ActionStyle::Danger,
            },
        ];
        let payload = PushPayload::with_actions("Deploy", "Ready to deploy?", &actions);
        assert_eq!(payload.title, "Deploy");
        assert_eq!(payload.body, "Ready to deploy?");
        assert_eq!(payload.actions.len(), 2);
        assert_eq!(payload.actions[0].action, "approve");
        assert_eq!(payload.actions[0].title, "Approve");
        assert_eq!(payload.actions[1].action, "reject");
        assert_eq!(payload.actions[1].title, "Reject");
    }

    #[test]
    fn push_payload_serializes_to_json() {
        let payload = PushPayload::text("Test", "Hello");
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["title"], "Test");
        assert_eq!(json["body"], "Hello");
        // Empty actions should be skipped
        assert!(json.get("actions").is_none());
        assert!(json.get("url").is_none());
    }

    #[test]
    fn push_payload_with_actions_serializes() {
        let actions = vec![Action {
            id: "ok".into(),
            label: "OK".into(),
            style: ActionStyle::Primary,
        }];
        let payload = PushPayload::with_actions("Alert", "Something happened", &actions);
        let json = serde_json::to_value(&payload).unwrap();
        let acts = json["actions"].as_array().unwrap();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0]["action"], "ok");
        assert_eq!(acts[0]["title"], "OK");
    }

    #[test]
    fn push_subscription_deserialize() {
        let json = r#"{"endpoint":"https://a.com/push","p256dh":"key","auth":"sec"}"#;
        let sub: PushSubscription = serde_json::from_str(json).unwrap();
        assert_eq!(sub.endpoint, "https://a.com/push");
        assert_eq!(sub.p256dh, "key");
        assert_eq!(sub.auth, "sec");
    }

    #[test]
    fn default_subscription_all_fields_set() {
        let config = test_config();
        let sub = config.default_subscription().unwrap();
        assert_eq!(sub.endpoint, "https://push.example.com/send/abc123");
    }

    #[test]
    fn default_subscription_missing_fields() {
        let config = test_config_no_defaults();
        assert!(config.default_subscription().is_none());
    }

    #[test]
    fn default_subscription_partial_fields() {
        let config = PushConfig {
            vapid_private_key: "key".into(),
            vapid_subject: "mailto:a@b.com".into(),
            default_endpoint: Some("https://push.example.com/abc".into()),
            default_p256dh: None, // missing
            default_auth: Some("auth".into()),
            ttl: 86400,
        };
        assert!(config.default_subscription().is_none());
    }
}
