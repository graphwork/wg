//! Unified notification abstraction for routing messages to humans across channels.
//!
//! The core [`NotificationChannel`] trait abstracts over messaging platforms (Telegram,
//! Matrix, Slack, email, SMS, webhooks, etc.). The [`NotificationRouter`] selects
//! channels based on event type and supports escalation chains.

pub mod config;

use std::fmt;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Identifies a sent message for threading/replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageId(pub String);

/// A message with optional rich formatting.
#[derive(Debug, Clone)]
pub struct RichMessage {
    /// Plain text fallback (always required).
    pub plain_text: String,
    /// Optional HTML body.
    pub html: Option<String>,
    /// Optional Markdown body.
    pub markdown: Option<String>,
}

impl RichMessage {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            plain_text: text.into(),
            html: None,
            markdown: None,
        }
    }
}

/// Visual style hint for an action button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStyle {
    Primary,
    Danger,
    Secondary,
}

/// An action button attached to a message.
#[derive(Debug, Clone)]
pub struct Action {
    /// Unique identifier returned when the button is clicked.
    pub id: String,
    /// Human-visible label.
    pub label: String,
    /// Visual style hint.
    pub style: ActionStyle,
}

/// An incoming message from a human.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// Channel type that received this message.
    pub channel: String,
    /// Sender identifier (platform-specific).
    pub sender: String,
    /// Message body text.
    pub body: String,
    /// If the human clicked an action button, its id.
    pub action_id: Option<String>,
    /// If this is a reply, the original message id.
    pub reply_to: Option<MessageId>,
}

/// Classification of notification events for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    TaskReady,
    TaskBlocked,
    TaskFailed,
    Approval,
    Urgent,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskReady => write!(f, "task_ready"),
            Self::TaskBlocked => write!(f, "task_blocked"),
            Self::TaskFailed => write!(f, "task_failed"),
            Self::Approval => write!(f, "approval"),
            Self::Urgent => write!(f, "urgent"),
        }
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A channel that can send messages to humans and optionally receive responses.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Unique identifier for this channel type (e.g. "telegram", "email").
    fn channel_type(&self) -> &str;

    /// Send a plain text message.
    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId>;

    /// Send a rich/formatted message.
    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId>;

    /// Send a message with action buttons (approve/reject/etc.).
    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId>;

    /// Whether this channel supports receiving messages from humans.
    fn supports_receive(&self) -> bool;

    /// Start listening for incoming messages (if supported).
    ///
    /// Returns a receiver that yields incoming messages. Implementations that
    /// don't support receiving should return an error.
    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>>;
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

/// A rule that maps an event type to an ordered list of channels.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingRule {
    /// Which event type this rule matches.
    pub event_type: EventType,
    /// Channel type names in priority order.
    pub channels: Vec<String>,
    /// If set, escalate to the next channel after this duration without a response.
    #[serde(default, with = "option_duration_secs")]
    pub escalation_timeout: Option<Duration>,
}

/// Routes notifications to configured channels based on event type.
pub struct NotificationRouter {
    channels: Vec<Box<dyn NotificationChannel>>,
    rules: Vec<RoutingRule>,
    default_channels: Vec<String>,
}

impl NotificationRouter {
    /// Create a new router with the given channels, rules, and default channel list.
    pub fn new(
        channels: Vec<Box<dyn NotificationChannel>>,
        rules: Vec<RoutingRule>,
        default_channels: Vec<String>,
    ) -> Self {
        Self {
            channels,
            rules,
            default_channels,
        }
    }

    /// Return the ordered list of channel type names for an event.
    pub fn channels_for_event(&self, event: EventType) -> Vec<&str> {
        // Find the first matching rule.
        for rule in &self.rules {
            if rule.event_type == event {
                return rule.channels.iter().map(|s| s.as_str()).collect();
            }
        }
        // Fall back to default.
        self.default_channels.iter().map(|s| s.as_str()).collect()
    }

    /// Return the escalation timeout for an event type (if any).
    pub fn escalation_timeout(&self, event: EventType) -> Option<Duration> {
        self.rules
            .iter()
            .find(|r| r.event_type == event)
            .and_then(|r| r.escalation_timeout)
    }

    /// Look up a channel implementation by type name.
    pub fn get_channel(&self, channel_type: &str) -> Option<&dyn NotificationChannel> {
        self.channels
            .iter()
            .find(|c| c.channel_type() == channel_type)
            .map(|c| c.as_ref())
    }

    /// Send a notification through the first available channel for the event type.
    ///
    /// Returns the channel type used and the resulting message id.
    pub async fn send(
        &self,
        event: EventType,
        target: &str,
        message: &str,
    ) -> Result<(String, MessageId)> {
        let channel_names = self.channels_for_event(event);
        if channel_names.is_empty() {
            anyhow::bail!("no channels configured for event type {event}");
        }

        let mut last_err: Option<anyhow::Error> = None;
        for name in channel_names {
            if let Some(ch) = self.get_channel(name) {
                match ch.send_text(target, message).await {
                    Ok(mid) => return Ok((name.to_string(), mid)),
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no matching channel implementation found")))
    }

    /// Send a rich notification through the first available channel.
    pub async fn send_rich(
        &self,
        event: EventType,
        target: &str,
        message: &RichMessage,
    ) -> Result<(String, MessageId)> {
        let channel_names = self.channels_for_event(event);
        if channel_names.is_empty() {
            anyhow::bail!("no channels configured for event type {event}");
        }

        let mut last_err: Option<anyhow::Error> = None;
        for name in channel_names {
            if let Some(ch) = self.get_channel(name) {
                match ch.send_rich(target, message).await {
                    Ok(mid) => return Ok((name.to_string(), mid)),
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no matching channel implementation found")))
    }

    /// List all registered channel type names.
    pub fn available_channels(&self) -> Vec<&str> {
        self.channels.iter().map(|c| c.channel_type()).collect()
    }

    /// List all routing rules.
    pub fn rules(&self) -> &[RoutingRule] {
        &self.rules
    }
}

// ---------------------------------------------------------------------------
// Serde helper: Option<Duration> as optional seconds
// ---------------------------------------------------------------------------

mod option_duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(
        val: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match val {
            Some(d) => serializer.serialize_u64(d.as_secs()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_secs))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-memory channel for testing.
    struct MockChannel {
        name: String,
        fail: bool,
    }

    #[async_trait]
    impl NotificationChannel for MockChannel {
        fn channel_type(&self) -> &str {
            &self.name
        }

        async fn send_text(&self, _target: &str, message: &str) -> Result<MessageId> {
            if self.fail {
                anyhow::bail!("mock failure");
            }
            Ok(MessageId(format!("{}:{}", self.name, message)))
        }

        async fn send_rich(&self, _target: &str, message: &RichMessage) -> Result<MessageId> {
            if self.fail {
                anyhow::bail!("mock failure");
            }
            Ok(MessageId(format!("{}:{}", self.name, message.plain_text)))
        }

        async fn send_with_actions(
            &self,
            _target: &str,
            message: &str,
            _actions: &[Action],
        ) -> Result<MessageId> {
            if self.fail {
                anyhow::bail!("mock failure");
            }
            Ok(MessageId(format!("{}:action:{}", self.name, message)))
        }

        fn supports_receive(&self) -> bool {
            false
        }

        async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
            anyhow::bail!("not supported")
        }
    }

    fn mock(name: &str, fail: bool) -> Box<dyn NotificationChannel> {
        Box::new(MockChannel {
            name: name.to_string(),
            fail,
        })
    }

    #[test]
    fn channels_for_event_returns_matching_rule() {
        let router = NotificationRouter::new(
            vec![mock("telegram", false), mock("email", false)],
            vec![RoutingRule {
                event_type: EventType::Urgent,
                channels: vec!["telegram".into(), "email".into()],
                escalation_timeout: Some(Duration::from_secs(1800)),
            }],
            vec!["email".into()],
        );

        assert_eq!(
            router.channels_for_event(EventType::Urgent),
            vec!["telegram", "email"]
        );
    }

    #[test]
    fn channels_for_event_falls_back_to_default() {
        let router = NotificationRouter::new(
            vec![mock("telegram", false)],
            vec![],
            vec!["telegram".into()],
        );

        assert_eq!(
            router.channels_for_event(EventType::TaskReady),
            vec!["telegram"]
        );
    }

    #[test]
    fn escalation_timeout_returns_configured_value() {
        let router = NotificationRouter::new(
            vec![],
            vec![RoutingRule {
                event_type: EventType::Approval,
                channels: vec!["telegram".into()],
                escalation_timeout: Some(Duration::from_secs(900)),
            }],
            vec![],
        );

        assert_eq!(
            router.escalation_timeout(EventType::Approval),
            Some(Duration::from_secs(900))
        );
        assert_eq!(router.escalation_timeout(EventType::Urgent), None);
    }

    #[tokio::test]
    async fn send_uses_first_available_channel() {
        let router = NotificationRouter::new(
            vec![mock("telegram", false), mock("email", false)],
            vec![RoutingRule {
                event_type: EventType::TaskFailed,
                channels: vec!["telegram".into(), "email".into()],
                escalation_timeout: None,
            }],
            vec![],
        );

        let (ch, mid) = router
            .send(EventType::TaskFailed, "user1", "build broke")
            .await
            .unwrap();
        assert_eq!(ch, "telegram");
        assert_eq!(mid.0, "telegram:build broke");
    }

    #[tokio::test]
    async fn send_falls_through_on_failure() {
        let router = NotificationRouter::new(
            vec![mock("telegram", true), mock("email", false)],
            vec![RoutingRule {
                event_type: EventType::TaskFailed,
                channels: vec!["telegram".into(), "email".into()],
                escalation_timeout: None,
            }],
            vec![],
        );

        let (ch, mid) = router
            .send(EventType::TaskFailed, "user1", "build broke")
            .await
            .unwrap();
        assert_eq!(ch, "email");
        assert_eq!(mid.0, "email:build broke");
    }

    #[tokio::test]
    async fn send_errors_when_all_channels_fail() {
        let router = NotificationRouter::new(
            vec![mock("telegram", true)],
            vec![RoutingRule {
                event_type: EventType::TaskFailed,
                channels: vec!["telegram".into()],
                escalation_timeout: None,
            }],
            vec![],
        );

        let err = router
            .send(EventType::TaskFailed, "user1", "oops")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mock failure"));
    }

    #[tokio::test]
    async fn send_errors_when_no_channels_configured() {
        let router = NotificationRouter::new(vec![], vec![], vec![]);

        let err = router
            .send(EventType::TaskReady, "user1", "hi")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no channels configured"));
    }

    #[tokio::test]
    async fn send_rich_works() {
        let router = NotificationRouter::new(
            vec![mock("email", false)],
            vec![],
            vec!["email".into()],
        );

        let msg = RichMessage {
            plain_text: "hello".into(),
            html: Some("<b>hello</b>".into()),
            markdown: None,
        };
        let (ch, mid) = router
            .send_rich(EventType::TaskReady, "user1", &msg)
            .await
            .unwrap();
        assert_eq!(ch, "email");
        assert_eq!(mid.0, "email:hello");
    }

    #[test]
    fn available_channels_lists_all() {
        let router = NotificationRouter::new(
            vec![mock("telegram", false), mock("email", false), mock("sms", false)],
            vec![],
            vec![],
        );
        assert_eq!(router.available_channels(), vec!["telegram", "email", "sms"]);
    }

    #[test]
    fn event_type_display() {
        assert_eq!(EventType::TaskReady.to_string(), "task_ready");
        assert_eq!(EventType::Urgent.to_string(), "urgent");
    }

    #[test]
    fn trait_is_object_safe() {
        // This compiles iff NotificationChannel is object-safe.
        fn _assert_object_safe(_: &dyn NotificationChannel) {}
    }
}
