//! Task lifecycle → notification dispatch bridge.
//!
//! Converts WG task events into notification messages and routes them
//! through the [`NotificationRouter`]. This module is the integration point
//! between the coordinator/service layer and the notification system.

use anyhow::Result;

use super::{EventType, MessageId, NotificationRouter, RichMessage};

// ---------------------------------------------------------------------------
// Task event types
// ---------------------------------------------------------------------------

/// A task lifecycle event that may trigger a notification.
#[derive(Debug, Clone)]
pub struct TaskEvent {
    /// The task ID.
    pub task_id: String,
    /// The task title.
    pub title: String,
    /// What happened.
    pub kind: TaskEventKind,
    /// Optional extra context (e.g., failure reason, agent id).
    pub detail: Option<String>,
}

/// Classification of task lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEventKind {
    /// Task became ready (all dependencies met).
    Ready,
    /// Task is blocked (dependency not met or explicitly blocked).
    Blocked,
    /// Task failed.
    Failed,
    /// Task requires human approval.
    ApprovalNeeded,
    /// Urgent: task needs immediate attention.
    Urgent,
}

impl TaskEventKind {
    /// Map to the notification system's [`EventType`].
    pub fn to_event_type(self) -> EventType {
        match self {
            Self::Ready => EventType::TaskReady,
            Self::Blocked => EventType::TaskBlocked,
            Self::Failed => EventType::TaskFailed,
            Self::ApprovalNeeded => EventType::Approval,
            Self::Urgent => EventType::Urgent,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Format a task event into a human-readable notification message.
pub fn format_event(event: &TaskEvent) -> RichMessage {
    let emoji = match event.kind {
        TaskEventKind::Ready => "📋",
        TaskEventKind::Blocked => "🚫",
        TaskEventKind::Failed => "❌",
        TaskEventKind::ApprovalNeeded => "🔐",
        TaskEventKind::Urgent => "🚨",
    };

    let kind_label = match event.kind {
        TaskEventKind::Ready => "ready",
        TaskEventKind::Blocked => "blocked",
        TaskEventKind::Failed => "failed",
        TaskEventKind::ApprovalNeeded => "approval needed",
        TaskEventKind::Urgent => "URGENT",
    };

    let plain = if let Some(ref detail) = event.detail {
        format!(
            "{} [{}] {}: {}\n{}",
            emoji, kind_label, event.task_id, event.title, detail
        )
    } else {
        format!(
            "{} [{}] {}: {}",
            emoji, kind_label, event.task_id, event.title
        )
    };

    let html = if let Some(ref detail) = event.detail {
        format!(
            "<p>{} <b>[{}]</b> <code>{}</code>: {}</p><p>{}</p>",
            emoji,
            kind_label,
            html_escape(&event.task_id),
            html_escape(&event.title),
            html_escape(detail),
        )
    } else {
        format!(
            "<p>{} <b>[{}]</b> <code>{}</code>: {}</p>",
            emoji,
            kind_label,
            html_escape(&event.task_id),
            html_escape(&event.title),
        )
    };

    RichMessage {
        plain_text: plain,
        html: Some(html),
        markdown: None,
    }
}

/// Dispatch a task event through the notification router.
///
/// Returns the channel name and message id on success, or an error if no
/// channel could deliver the message. Returns `Ok(None)` if the router has
/// no channels configured for this event type.
pub async fn dispatch_event(
    router: &NotificationRouter,
    target: &str,
    event: &TaskEvent,
) -> Result<Option<(String, MessageId)>> {
    let event_type = event.kind.to_event_type();
    let channels = router.channels_for_event(event_type);

    if channels.is_empty() {
        return Ok(None);
    }

    let message = format_event(event);
    let (ch, mid) = router.send_rich(event_type, target, &message).await?;
    Ok(Some((ch, mid)))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::tests_common::mock;
    use crate::notify::{NotificationRouter, RoutingRule};

    #[test]
    fn task_event_kind_maps_to_event_type() {
        assert_eq!(TaskEventKind::Ready.to_event_type(), EventType::TaskReady);
        assert_eq!(
            TaskEventKind::Blocked.to_event_type(),
            EventType::TaskBlocked
        );
        assert_eq!(TaskEventKind::Failed.to_event_type(), EventType::TaskFailed);
        assert_eq!(
            TaskEventKind::ApprovalNeeded.to_event_type(),
            EventType::Approval
        );
        assert_eq!(TaskEventKind::Urgent.to_event_type(), EventType::Urgent);
    }

    #[test]
    fn format_event_without_detail() {
        let event = TaskEvent {
            task_id: "build-frontend".into(),
            title: "Build Frontend".into(),
            kind: TaskEventKind::Ready,
            detail: None,
        };
        let msg = format_event(&event);
        assert!(msg.plain_text.contains("build-frontend"));
        assert!(msg.plain_text.contains("Build Frontend"));
        assert!(msg.plain_text.contains("ready"));
        assert!(
            msg.html
                .as_ref()
                .unwrap()
                .contains("<code>build-frontend</code>")
        );
    }

    #[test]
    fn format_event_with_detail() {
        let event = TaskEvent {
            task_id: "deploy-prod".into(),
            title: "Deploy to Production".into(),
            kind: TaskEventKind::Failed,
            detail: Some("Exit code 1: cargo test failed".into()),
        };
        let msg = format_event(&event);
        assert!(msg.plain_text.contains("failed"));
        assert!(msg.plain_text.contains("Exit code 1"));
        assert!(msg.html.as_ref().unwrap().contains("Exit code 1"));
    }

    #[tokio::test]
    async fn dispatch_event_routes_to_correct_channel() {
        let router = NotificationRouter::new(
            vec![mock("telegram", false), mock("webhook", false)],
            vec![RoutingRule {
                event_type: EventType::TaskFailed,
                channels: vec!["telegram".into()],
                escalation_timeout: None,
            }],
            vec!["webhook".into()],
        );

        let event = TaskEvent {
            task_id: "test-task".into(),
            title: "Test Task".into(),
            kind: TaskEventKind::Failed,
            detail: Some("build error".into()),
        };

        let result = dispatch_event(&router, "user1", &event).await.unwrap();
        let (ch, _) = result.unwrap();
        assert_eq!(ch, "telegram");
    }

    #[tokio::test]
    async fn dispatch_event_returns_none_when_no_channels() {
        let router = NotificationRouter::new(vec![], vec![], vec![]);

        let event = TaskEvent {
            task_id: "orphan".into(),
            title: "Orphan Task".into(),
            kind: TaskEventKind::Ready,
            detail: None,
        };

        let result = dispatch_event(&router, "user1", &event).await.unwrap();
        assert!(result.is_none());
    }
}
