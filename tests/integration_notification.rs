//! Smoke tests for NotificationChannel trait routing.
//!
//! Verifies that webhook and telegram channels route correctly through the
//! NotificationRouter, and that filter logic (e.g. only failures to telegram)
//! works as expected. Uses mock TCP endpoints for real HTTP assertions.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use workgraph::notify::webhook::{WebhookChannel, WebhookConfig};
use workgraph::notify::{
    EventType, MessageId, NotificationChannel, NotificationRouter, RichMessage, RoutingRule,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal in-memory mock channel for routing tests (no network).
struct MockChannel {
    name: String,
    fail: bool,
}

#[async_trait::async_trait]
impl NotificationChannel for MockChannel {
    fn channel_type(&self) -> &str {
        &self.name
    }

    async fn send_text(&self, _target: &str, message: &str) -> anyhow::Result<MessageId> {
        if self.fail {
            anyhow::bail!("mock failure");
        }
        Ok(MessageId(format!("{}:{}", self.name, message)))
    }

    async fn send_rich(&self, _target: &str, message: &RichMessage) -> anyhow::Result<MessageId> {
        if self.fail {
            anyhow::bail!("mock failure");
        }
        Ok(MessageId(format!("{}:{}", self.name, message.plain_text)))
    }

    async fn send_with_actions(
        &self,
        _target: &str,
        message: &str,
        _actions: &[workgraph::notify::Action],
    ) -> anyhow::Result<MessageId> {
        if self.fail {
            anyhow::bail!("mock failure");
        }
        Ok(MessageId(format!("{}:action:{}", self.name, message)))
    }

    fn supports_receive(&self) -> bool {
        false
    }

    async fn listen(
        &self,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<workgraph::notify::IncomingMessage>> {
        anyhow::bail!("not supported")
    }
}

fn mock(name: &str, fail: bool) -> Box<dyn NotificationChannel> {
    Box::new(MockChannel {
        name: name.to_string(),
        fail,
    })
}

/// Spawn a mock HTTP server that accepts one request and returns 200 OK with
/// the given JSON response body. Returns the server address and a oneshot
/// receiver that yields the raw request body bytes.
async fn mock_http_server(
    response_body: &str,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let resp = response_body.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let mut total = 0;
        loop {
            let n = stream.read(&mut buf[total..]).await.unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if total >= 4 && buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        // Parse content-length and read body
        let header_str = String::from_utf8_lossy(&buf[..total]).to_string();
        let header_end = buf[..total]
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(total);

        if let Some(cl) = header_str
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            let body_so_far = total - header_end;
            if body_so_far < cl {
                let remaining = cl - body_so_far;
                let mut rest = vec![0u8; remaining];
                let _ = stream.read_exact(&mut rest).await;
                let mut body = buf[header_end..total].to_vec();
                body.extend_from_slice(&rest);
                let _ = tx.send(body);
            } else {
                let _ = tx.send(buf[header_end..header_end + cl].to_vec());
            }
        } else {
            let _ = tx.send(buf[header_end..total].to_vec());
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            resp.len(),
            resp
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });

    (addr, rx, handle)
}

// ---------------------------------------------------------------------------
// Routing tests: webhook and telegram channels dispatch correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_sends_task_failed_to_telegram_not_webhook() {
    // Rule: TaskFailed → telegram only. Default → webhook.
    let router = NotificationRouter::new(
        vec![mock("telegram", false), mock("webhook", false)],
        vec![RoutingRule {
            event_type: EventType::TaskFailed,
            channels: vec!["telegram".into()],
            escalation_timeout: None,
        }],
        vec!["webhook".into()],
    );

    // TaskFailed should go to telegram
    let (ch, mid) = router
        .send(EventType::TaskFailed, "user1", "build broke")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");
    assert_eq!(mid.0, "telegram:build broke");

    // TaskReady (no specific rule) should go to default: webhook
    let (ch, _) = router
        .send(EventType::TaskReady, "user1", "task ready")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");
}

#[tokio::test]
async fn router_sends_urgent_to_telegram_and_falls_back() {
    // Urgent → telegram (fails) → webhook (succeeds)
    let router = NotificationRouter::new(
        vec![mock("telegram", true), mock("webhook", false)],
        vec![RoutingRule {
            event_type: EventType::Urgent,
            channels: vec!["telegram".into(), "webhook".into()],
            escalation_timeout: Some(Duration::from_secs(300)),
        }],
        vec![],
    );

    let (ch, mid) = router
        .send(EventType::Urgent, "ops", "server down")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");
    assert_eq!(mid.0, "webhook:server down");
}

// ---------------------------------------------------------------------------
// Filter logic: only certain events route to certain channels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filter_only_failures_to_telegram() {
    // Setup: failures → telegram, everything else → webhook
    let router = NotificationRouter::new(
        vec![mock("telegram", false), mock("webhook", false)],
        vec![RoutingRule {
            event_type: EventType::TaskFailed,
            channels: vec!["telegram".into()],
            escalation_timeout: None,
        }],
        vec!["webhook".into()],
    );

    // TaskFailed → telegram
    let (ch, _) = router
        .send(EventType::TaskFailed, "user1", "test failed")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");

    // TaskReady → webhook (not telegram)
    let (ch, _) = router
        .send(EventType::TaskReady, "user1", "ready")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");

    // TaskBlocked → webhook (not telegram)
    let (ch, _) = router
        .send(EventType::TaskBlocked, "user1", "blocked")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");

    // Approval → webhook (not telegram)
    let (ch, _) = router
        .send(EventType::Approval, "user1", "approve?")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");

    // Question → webhook (not telegram)
    let (ch, _) = router
        .send(EventType::Question, "user1", "question")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");
}

#[tokio::test]
async fn filter_multiple_event_types_to_telegram() {
    // Setup: failures AND urgent → telegram, rest → webhook
    let router = NotificationRouter::new(
        vec![mock("telegram", false), mock("webhook", false)],
        vec![
            RoutingRule {
                event_type: EventType::TaskFailed,
                channels: vec!["telegram".into()],
                escalation_timeout: None,
            },
            RoutingRule {
                event_type: EventType::Urgent,
                channels: vec!["telegram".into()],
                escalation_timeout: Some(Duration::from_secs(600)),
            },
        ],
        vec!["webhook".into()],
    );

    let (ch, _) = router
        .send(EventType::TaskFailed, "u", "fail")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");

    let (ch, _) = router
        .send(EventType::Urgent, "u", "urgent")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");

    let (ch, _) = router
        .send(EventType::TaskReady, "u", "ready")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");
}

// ---------------------------------------------------------------------------
// Webhook channel with mock HTTP endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_channel_sends_json_to_mock_endpoint() {
    let (addr, body_rx, server) = mock_http_server(r#"{"status":"ok"}"#).await;

    let ch = WebhookChannel::new(WebhookConfig {
        url: format!("http://{addr}/webhook"),
        secret: Some("test-secret".into()),
        events: vec![],
        event_urls: Default::default(),
        max_retries: 0,
        initial_backoff_ms: 10,
    });

    let mid = ch
        .send_text("my-task:task_failed", "Build failed")
        .await
        .unwrap();
    assert!(mid.0.starts_with("webhook:"));

    // Verify the body was valid JSON with expected fields
    let body_bytes = body_rx.await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(payload["task_id"], "my-task");
    assert_eq!(payload["event_type"], "task_failed");
    assert_eq!(payload["title"], "Build failed");

    server.abort();
}

#[tokio::test]
async fn webhook_channel_filters_events() {
    let ch = WebhookChannel::new(WebhookConfig {
        url: "http://127.0.0.1:1/should-not-be-called".into(),
        secret: None,
        events: vec!["task_failed".into()],
        event_urls: Default::default(),
        max_retries: 0,
        initial_backoff_ms: 10,
    });

    // Sending a task_ready event should be filtered (returns filtered:... ID)
    let mid = ch.send_text("my-task:task_ready", "Task ready").await.unwrap();
    assert!(
        mid.0.starts_with("filtered:"),
        "Expected filtered message, got: {}",
        mid.0
    );
}

#[tokio::test]
async fn webhook_channel_sends_rich_message() {
    let (addr, body_rx, server) = mock_http_server(r#"{"status":"ok"}"#).await;

    let ch = WebhookChannel::new(WebhookConfig {
        url: format!("http://{addr}/webhook"),
        secret: None,
        events: vec![],
        event_urls: Default::default(),
        max_retries: 0,
        initial_backoff_ms: 10,
    });

    let msg = RichMessage {
        plain_text: "Build failed on main".into(),
        html: Some("<b>Build failed</b> on main".into()),
        markdown: Some("**Build failed** on main".into()),
    };

    let mid = ch.send_rich("task-1:task_failed", &msg).await.unwrap();
    assert!(mid.0.starts_with("webhook:"));

    let body_bytes = body_rx.await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(payload["task_id"], "task-1");
    // Webhook prefers markdown for description
    assert_eq!(payload["description"], "**Build failed** on main");

    server.abort();
}

// ---------------------------------------------------------------------------
// Telegram channel with mock HTTP endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn telegram_channel_sends_text_to_mock_endpoint() {
    let tg_response = r#"{"ok":true,"result":{"message_id":42,"chat":{"id":123},"text":"hello"}}"#;
    let (addr, body_rx, server) = mock_http_server(tg_response).await;

    // We can't easily override TelegramChannel's api_url, so test via direct
    // HTTP call matching the same protocol the channel uses.
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "chat_id": "123",
        "text": "Task failed: build-frontend",
    });

    let resp = client
        .post(format!("http://{addr}/bottest-token/sendMessage"))
        .json(&body)
        .send()
        .await
        .unwrap();

    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["result"]["message_id"], 42);

    // Verify request body
    let req_body: serde_json::Value =
        serde_json::from_slice(&body_rx.await.unwrap()).unwrap();
    assert_eq!(req_body["chat_id"], "123");
    assert!(req_body["text"]
        .as_str()
        .unwrap()
        .contains("Task failed"));

    server.abort();
}

// ---------------------------------------------------------------------------
// Dispatch integration: task events route through the full pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_routes_failed_event_to_telegram_channel() {
    use workgraph::notify::dispatch::{dispatch_event, TaskEvent, TaskEventKind};

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
        task_id: "build-frontend".into(),
        title: "Build Frontend".into(),
        kind: TaskEventKind::Failed,
        detail: Some("exit code 1".into()),
    };

    let result = dispatch_event(&router, "user1", &event).await.unwrap();
    let (ch, mid) = result.unwrap();
    assert_eq!(ch, "telegram");
    // The mock channel returns "telegram:<plain_text>" — verify it contains the task info
    assert!(mid.0.contains("telegram:"));
    assert!(mid.0.contains("build-frontend"));
}

#[tokio::test]
async fn dispatch_routes_ready_event_to_default_webhook() {
    use workgraph::notify::dispatch::{dispatch_event, TaskEvent, TaskEventKind};

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
        task_id: "deploy-prod".into(),
        title: "Deploy Production".into(),
        kind: TaskEventKind::Ready,
        detail: None,
    };

    let result = dispatch_event(&router, "user1", &event).await.unwrap();
    let (ch, _) = result.unwrap();
    assert_eq!(ch, "webhook");
}

#[tokio::test]
async fn dispatch_returns_none_when_no_channels() {
    use workgraph::notify::dispatch::{dispatch_event, TaskEvent, TaskEventKind};

    let router = NotificationRouter::new(vec![], vec![], vec![]);

    let event = TaskEvent {
        task_id: "orphan".into(),
        title: "Orphan".into(),
        kind: TaskEventKind::Ready,
        detail: None,
    };

    let result = dispatch_event(&router, "user1", &event).await.unwrap();
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// Config-driven routing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_driven_routing_dispatches_correctly() {
    use workgraph::notify::config::{EscalationConfig, NotifyConfig, RoutingConfig};

    let config = NotifyConfig {
        routing: RoutingConfig {
            default: vec!["webhook".into()],
            urgent: vec!["telegram".into()],
            approval: vec!["telegram".into()],
            digest: vec![],
        },
        escalation: EscalationConfig {
            approval_timeout: 600,
            urgent_timeout: 1200,
        },
        channels: Default::default(),
    };

    let rules = config.to_routing_rules();
    let router = NotificationRouter::new(
        vec![mock("telegram", false), mock("webhook", false)],
        rules,
        config.default_channels().to_vec(),
    );

    // Urgent → telegram
    let (ch, _) = router
        .send(EventType::Urgent, "u", "alert")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");

    // Approval → telegram
    let (ch, _) = router
        .send(EventType::Approval, "u", "approve?")
        .await
        .unwrap();
    assert_eq!(ch, "telegram");

    // TaskReady → default (webhook)
    let (ch, _) = router
        .send(EventType::TaskReady, "u", "ready")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");

    // TaskFailed → default (webhook) — no specific rule for failures
    let (ch, _) = router
        .send(EventType::TaskFailed, "u", "failed")
        .await
        .unwrap();
    assert_eq!(ch, "webhook");

    // Escalation timeouts are set
    assert_eq!(
        router.escalation_timeout(EventType::Urgent),
        Some(Duration::from_secs(1200))
    );
    assert_eq!(
        router.escalation_timeout(EventType::Approval),
        Some(Duration::from_secs(600))
    );
}

// ---------------------------------------------------------------------------
// Webhook HMAC signature verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_sends_hmac_signature_header() {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured_headers = Arc::new(Mutex::new(String::new()));
    let headers_clone = captured_headers.clone();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let mut total = 0;
        loop {
            let n = stream.read(&mut buf[total..]).await.unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if total >= 4 && buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let header_str = String::from_utf8_lossy(&buf[..total]).to_string();
        *headers_clone.lock().await = header_str.clone();

        // Read remaining body
        if let Some(cl) = header_str
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            let header_end = buf[..total]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(total);
            let body_so_far = total - header_end;
            if body_so_far < cl {
                let remaining = cl - body_so_far;
                let mut rest = vec![0u8; remaining];
                let _ = stream.read_exact(&mut rest).await;
            }
        }

        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;
    });

    let ch = WebhookChannel::new(WebhookConfig {
        url: format!("http://{addr}/hook"),
        secret: Some("my-secret-key".into()),
        events: vec![],
        event_urls: Default::default(),
        max_retries: 0,
        initial_backoff_ms: 10,
    });

    ch.send_text("task-1:task_ready", "Ready").await.unwrap();

    let headers = captured_headers.lock().await.to_lowercase();
    assert!(
        headers.contains("x-webhook-signature: sha256="),
        "Expected HMAC signature header, got: {}",
        headers
    );

    server.abort();
}
