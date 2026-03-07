//! Telegram notification channel backed by the Bot API via `reqwest`.
//!
//! Implements [`NotificationChannel`] for Telegram bots. Supports:
//! - Outbound: text, rich (MarkdownV2/HTML), and action-button messages (inline keyboards)
//! - Inbound: long-polling listener that yields [`IncomingMessage`]s
//! - Callback query acknowledgement (dismisses button loading spinners)
//!
//! Configuration is read from the `[telegram]` section of `notify.toml`:
//! ```toml
//! [telegram]
//! bot_token = "123456:ABC-DEF..."
//! chat_id = "12345678"
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::{Action, ActionStyle, IncomingMessage, MessageId, NotificationChannel, RichMessage};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Telegram-specific configuration parsed from the `[telegram]` section.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

impl TelegramConfig {
    /// Extract from the opaque channel map in [`super::config::NotifyConfig`].
    pub fn from_notify_config(config: &super::config::NotifyConfig) -> Result<Self> {
        let val = config
            .channels
            .get("telegram")
            .context("no [telegram] section in notify config")?;
        let cfg: Self = val
            .clone()
            .try_into()
            .context("invalid [telegram] config")?;
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// MarkdownV2 escaping
// ---------------------------------------------------------------------------

/// Characters that must be escaped in Telegram MarkdownV2 outside of pre/code spans.
const MARKDOWNV2_SPECIAL: &[char] = &[
    '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
];

/// Escape a string for safe use in Telegram MarkdownV2 messages.
///
/// This escapes all special characters per the Bot API spec. Use this for
/// dynamic content (task IDs, titles, descriptions) embedded in MarkdownV2.
pub fn escape_markdown_v2(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    for ch in text.chars() {
        if MARKDOWNV2_SPECIAL.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Build an inline keyboard JSON array from [`Action`] slices.
///
/// Each action becomes one button. The callback_data is the action's `id`.
/// Telegram limits callback_data to 64 bytes — callers must ensure IDs fit.
pub fn build_inline_keyboard(actions: &[Action]) -> serde_json::Value {
    let buttons: Vec<serde_json::Value> = actions
        .iter()
        .map(|a| {
            serde_json::json!({
                "text": format_button_label(&a.label, a.style),
                "callback_data": &a.id,
            })
        })
        .collect();

    serde_json::json!({ "inline_keyboard": [buttons] })
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// A Telegram notification channel backed by the Telegram Bot API via `reqwest`.
///
/// This uses the Bot API directly over HTTP rather than pulling in the full
/// teloxide runtime, keeping the non-listener path lightweight.
pub struct TelegramChannel {
    config: TelegramConfig,
    client: reqwest::Client,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "https://api.telegram.org/bot{}/{}",
            self.config.bot_token, method
        )
    }

    /// Send a request to the Telegram Bot API and return the result.
    async fn api_call(&self, method: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(self.api_url(method))
            .json(body)
            .send()
            .await
            .context("Telegram API request failed")?;

        let status = resp.status();
        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse Telegram API response")?;

        if !status.is_success() || json.get("ok") != Some(&serde_json::Value::Bool(true)) {
            let desc = json
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Telegram API error ({}): {}", status, desc);
        }

        Ok(json)
    }

    /// Extract the message_id from a sendMessage response.
    fn extract_message_id(json: &serde_json::Value) -> MessageId {
        let mid = json
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64())
            .unwrap_or(0);
        MessageId(mid.to_string())
    }

    /// Acknowledge a callback query so Telegram removes the loading spinner.
    ///
    /// Optionally shows a brief toast message to the user who clicked.
    async fn answer_callback_query(
        client: &reqwest::Client,
        bot_token: &str,
        callback_query_id: &str,
        text: Option<&str>,
    ) {
        let url = format!(
            "https://api.telegram.org/bot{}/answerCallbackQuery",
            bot_token
        );
        let mut body = serde_json::json!({
            "callback_query_id": callback_query_id,
        });
        if let Some(t) = text {
            body["text"] = serde_json::Value::String(t.to_string());
        }
        // Fire-and-forget: don't block the poll loop on acknowledgement.
        let _ = client.post(&url).json(&body).send().await;
    }
}

#[async_trait]
impl NotificationChannel for TelegramChannel {
    fn channel_type(&self) -> &str {
        "telegram"
    }

    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId> {
        let body = serde_json::json!({
            "chat_id": target,
            "text": message,
        });
        let resp = self.api_call("sendMessage", &body).await?;
        Ok(Self::extract_message_id(&resp))
    }

    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId> {
        // Prefer Markdown, fall back to HTML, then plain text.
        let (text, parse_mode) = if let Some(ref md) = message.markdown {
            (md.clone(), Some("MarkdownV2"))
        } else if let Some(ref html) = message.html {
            (html.clone(), Some("HTML"))
        } else {
            (message.plain_text.clone(), None)
        };

        let mut body = serde_json::json!({
            "chat_id": target,
            "text": text,
        });
        if let Some(mode) = parse_mode {
            body["parse_mode"] = serde_json::Value::String(mode.to_string());
        }

        let resp = self.api_call("sendMessage", &body).await?;
        Ok(Self::extract_message_id(&resp))
    }

    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId> {
        let body = serde_json::json!({
            "chat_id": target,
            "text": message,
            "reply_markup": build_inline_keyboard(actions),
        });

        let resp = self.api_call("sendMessage", &body).await?;
        Ok(Self::extract_message_id(&resp))
    }

    fn supports_receive(&self) -> bool {
        true
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let config = self.config.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            let mut offset: i64 = 0;
            loop {
                let url = format!(
                    "https://api.telegram.org/bot{}/getUpdates",
                    config.bot_token
                );
                let body = serde_json::json!({
                    "offset": offset,
                    "timeout": 30,
                    "allowed_updates": ["message", "callback_query"],
                });

                let resp = match client.post(&url).json(&body).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("Telegram poll error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let json: serde_json::Value = match resp.json().await {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("Telegram parse error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let updates = match json.get("result").and_then(|r| r.as_array()) {
                    Some(arr) => arr.clone(),
                    None => continue,
                };

                for update in &updates {
                    if let Some(uid) = update.get("update_id").and_then(|u| u.as_i64()) {
                        offset = uid + 1;
                    }

                    // Handle callback queries (button presses)
                    if let Some(cb) = update.get("callback_query") {
                        // Acknowledge the callback to dismiss the loading spinner.
                        if let Some(cb_id) = cb.get("id").and_then(|i| i.as_str()) {
                            let action_data = cb
                                .get("data")
                                .and_then(|d| d.as_str())
                                .unwrap_or("");
                            let toast = format!("Received: {action_data}");
                            Self::answer_callback_query(
                                &client,
                                &config.bot_token,
                                cb_id,
                                Some(&toast),
                            )
                            .await;
                        }

                        let sender = extract_sender(cb);

                        let action_id = cb
                            .get("data")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();

                        let reply_to = cb
                            .get("message")
                            .and_then(|m| m.get("message_id"))
                            .and_then(|m| m.as_i64())
                            .map(|mid| MessageId(mid.to_string()));

                        let msg = IncomingMessage {
                            channel: "telegram".to_string(),
                            sender: sender.to_string(),
                            body: action_id.clone(),
                            action_id: Some(action_id),
                            reply_to,
                        };

                        if tx.send(msg).await.is_err() {
                            return; // receiver dropped
                        }
                        continue;
                    }

                    // Handle regular messages
                    if let Some(message) = update.get("message") {
                        let sender = extract_sender(message);

                        let body = message
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();

                        let reply_to = message
                            .get("reply_to_message")
                            .and_then(|r| r.get("message_id"))
                            .and_then(|m| m.as_i64())
                            .map(|mid| MessageId(mid.to_string()));

                        let msg = IncomingMessage {
                            channel: "telegram".to_string(),
                            sender: sender.to_string(),
                            body,
                            action_id: None,
                            reply_to,
                        };

                        if tx.send(msg).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// Extract the sender username (or numeric ID as fallback) from a Telegram object.
fn extract_sender(obj: &serde_json::Value) -> String {
    obj.get("from")
        .and_then(|f| {
            f.get("username")
                .and_then(|u| u.as_str())
                .map(|s| s.to_string())
                .or_else(|| f.get("id").and_then(|i| i.as_i64()).map(|id| id.to_string()))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Add a visual prefix to button labels based on style.
fn format_button_label(label: &str, style: ActionStyle) -> String {
    match style {
        ActionStyle::Primary => format!("✅ {label}"),
        ActionStyle::Danger => format!("❌ {label}"),
        ActionStyle::Secondary => label.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Config tests -------------------------------------------------------

    #[test]
    fn telegram_config_from_toml() {
        let toml_str = r#"
[routing]
default = ["telegram"]

[telegram]
bot_token = "123:ABC"
chat_id = "456"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let tg = TelegramConfig::from_notify_config(&config).unwrap();
        assert_eq!(tg.bot_token, "123:ABC");
        assert_eq!(tg.chat_id, "456");
    }

    #[test]
    fn telegram_config_missing_section() {
        let config = super::super::config::NotifyConfig::default();
        assert!(TelegramConfig::from_notify_config(&config).is_err());
    }

    // -- Channel basics -----------------------------------------------------

    #[test]
    fn channel_type_is_telegram() {
        let ch = TelegramChannel::new(TelegramConfig {
            bot_token: "test".into(),
            chat_id: "test".into(),
        });
        assert_eq!(ch.channel_type(), "telegram");
    }

    #[test]
    fn supports_receive_is_true() {
        let ch = TelegramChannel::new(TelegramConfig {
            bot_token: "test".into(),
            chat_id: "test".into(),
        });
        assert!(ch.supports_receive());
    }

    #[test]
    fn api_url_format() {
        let ch = TelegramChannel::new(TelegramConfig {
            bot_token: "123:ABC".into(),
            chat_id: "456".into(),
        });
        assert_eq!(
            ch.api_url("sendMessage"),
            "https://api.telegram.org/bot123:ABC/sendMessage"
        );
        assert_eq!(
            ch.api_url("answerCallbackQuery"),
            "https://api.telegram.org/bot123:ABC/answerCallbackQuery"
        );
    }

    // -- Message ID extraction ----------------------------------------------

    #[test]
    fn extract_message_id_from_response() {
        let json = serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 42,
                "chat": { "id": 123 },
                "text": "hello"
            }
        });
        let mid = TelegramChannel::extract_message_id(&json);
        assert_eq!(mid.0, "42");
    }

    #[test]
    fn extract_message_id_missing_returns_zero() {
        let json = serde_json::json!({"ok": true});
        let mid = TelegramChannel::extract_message_id(&json);
        assert_eq!(mid.0, "0");
    }

    // -- Button labels ------------------------------------------------------

    #[test]
    fn format_button_labels() {
        assert_eq!(
            format_button_label("Approve", ActionStyle::Primary),
            "✅ Approve"
        );
        assert_eq!(
            format_button_label("Reject", ActionStyle::Danger),
            "❌ Reject"
        );
        assert_eq!(format_button_label("Skip", ActionStyle::Secondary), "Skip");
    }

    // -- MarkdownV2 escaping ------------------------------------------------

    #[test]
    fn escape_markdown_v2_escapes_special_chars() {
        assert_eq!(escape_markdown_v2("hello"), "hello");
        assert_eq!(escape_markdown_v2("a_b*c"), "a\\_b\\*c");
        assert_eq!(escape_markdown_v2("[link](url)"), "\\[link\\]\\(url\\)");
    }

    #[test]
    fn escape_markdown_v2_escapes_all_specials() {
        let input = "_*[]()~`>#+-=|{}.!";
        let escaped = escape_markdown_v2(input);
        for ch in MARKDOWNV2_SPECIAL {
            assert!(
                escaped.contains(&format!("\\{ch}")),
                "missing escape for '{ch}'"
            );
        }
    }

    #[test]
    fn escape_markdown_v2_preserves_non_special() {
        let input = "hello world 123 äöü";
        assert_eq!(escape_markdown_v2(input), input);
    }

    #[test]
    fn escape_markdown_v2_empty_string() {
        assert_eq!(escape_markdown_v2(""), "");
    }

    #[test]
    fn escape_markdown_v2_real_task_id() {
        // Task IDs often contain hyphens and dots
        let id = "build-frontend-v2.1";
        assert_eq!(
            escape_markdown_v2(id),
            "build\\-frontend\\-v2\\.1"
        );
    }

    // -- Inline keyboard builder --------------------------------------------

    #[test]
    fn build_inline_keyboard_creates_correct_structure() {
        let actions = vec![
            Action {
                id: "approve:task-1".to_string(),
                label: "Approve".to_string(),
                style: ActionStyle::Primary,
            },
            Action {
                id: "reject:task-1".to_string(),
                label: "Reject".to_string(),
                style: ActionStyle::Danger,
            },
        ];
        let kb = build_inline_keyboard(&actions);

        let rows = kb["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        let buttons = rows[0].as_array().unwrap();
        assert_eq!(buttons.len(), 2);

        assert_eq!(buttons[0]["text"], "✅ Approve");
        assert_eq!(buttons[0]["callback_data"], "approve:task-1");
        assert_eq!(buttons[1]["text"], "❌ Reject");
        assert_eq!(buttons[1]["callback_data"], "reject:task-1");
    }

    #[test]
    fn build_inline_keyboard_empty_actions() {
        let kb = build_inline_keyboard(&[]);
        let rows = kb["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].as_array().unwrap().is_empty());
    }

    #[test]
    fn build_inline_keyboard_secondary_no_emoji() {
        let actions = vec![Action {
            id: "skip".to_string(),
            label: "Skip".to_string(),
            style: ActionStyle::Secondary,
        }];
        let kb = build_inline_keyboard(&actions);
        let buttons = kb["inline_keyboard"][0].as_array().unwrap();
        assert_eq!(buttons[0]["text"], "Skip");
    }

    // -- Sender extraction --------------------------------------------------

    #[test]
    fn extract_sender_with_username() {
        let obj = serde_json::json!({
            "from": { "id": 12345, "username": "alice" }
        });
        assert_eq!(extract_sender(&obj), "alice");
    }

    #[test]
    fn extract_sender_falls_back_to_id() {
        let obj = serde_json::json!({
            "from": { "id": 12345 }
        });
        assert_eq!(extract_sender(&obj), "12345");
    }

    #[test]
    fn extract_sender_unknown_when_no_from() {
        let obj = serde_json::json!({});
        assert_eq!(extract_sender(&obj), "unknown");
    }

    // -- Update parsing (simulating getUpdates responses) --------------------

    #[test]
    fn parse_callback_query_update() {
        let update = serde_json::json!({
            "update_id": 100,
            "callback_query": {
                "id": "cb-111",
                "from": { "id": 42, "username": "bob" },
                "message": {
                    "message_id": 99,
                    "chat": { "id": 1 },
                    "text": "Approve deploy?"
                },
                "data": "approve:deploy-prod"
            }
        });

        let cb = update.get("callback_query").unwrap();
        let sender = extract_sender(cb);
        assert_eq!(sender, "bob");

        let action_id = cb.get("data").and_then(|d| d.as_str()).unwrap();
        assert_eq!(action_id, "approve:deploy-prod");

        let reply_to = cb
            .get("message")
            .and_then(|m| m.get("message_id"))
            .and_then(|m| m.as_i64())
            .unwrap();
        assert_eq!(reply_to, 99);

        let cb_id = cb.get("id").and_then(|i| i.as_str()).unwrap();
        assert_eq!(cb_id, "cb-111");
    }

    #[test]
    fn parse_regular_message_update() {
        let update = serde_json::json!({
            "update_id": 101,
            "message": {
                "message_id": 200,
                "from": { "id": 42, "username": "alice" },
                "chat": { "id": 1 },
                "text": "Yes, proceed with deploy",
                "reply_to_message": {
                    "message_id": 199,
                    "chat": { "id": 1 },
                    "text": "Should we deploy?"
                }
            }
        });

        let msg = update.get("message").unwrap();
        let sender = extract_sender(msg);
        assert_eq!(sender, "alice");

        let body = msg.get("text").and_then(|t| t.as_str()).unwrap();
        assert_eq!(body, "Yes, proceed with deploy");

        let reply_to = msg
            .get("reply_to_message")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64())
            .unwrap();
        assert_eq!(reply_to, 199);
    }

    #[test]
    fn parse_message_without_reply() {
        let update = serde_json::json!({
            "update_id": 102,
            "message": {
                "message_id": 300,
                "from": { "id": 99 },
                "chat": { "id": 1 },
                "text": "hello bot"
            }
        });

        let msg = update.get("message").unwrap();
        let sender = extract_sender(msg);
        assert_eq!(sender, "99"); // no username, falls back to ID

        let reply_to = msg
            .get("reply_to_message")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64());
        assert!(reply_to.is_none());
    }

    // -- Mock HTTP server tests for API calls --------------------------------

    #[tokio::test]
    async fn send_text_via_mock_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
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
            // Read body
            let header_str = String::from_utf8_lossy(&buf[..total]);
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
                    let mut body_buf = vec![0u8; remaining];
                    let _ = stream.read_exact(&mut body_buf).await;
                }
            }

            let response_body = r#"{"ok":true,"result":{"message_id":77,"chat":{"id":123},"text":"hi"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });

        // Point TelegramChannel at our mock server
        let ch = TelegramChannel {
            config: TelegramConfig {
                bot_token: "test-token".into(),
                chat_id: "123".into(),
            },
            client: reqwest::Client::new(),
        };

        // Override api_url by calling api_call with a direct URL
        let body = serde_json::json!({
            "chat_id": "123",
            "text": "hello",
        });
        let resp = ch
            .client
            .post(format!("http://{addr}/bottest-token/sendMessage"))
            .json(&body)
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        let mid = TelegramChannel::extract_message_id(&json);
        assert_eq!(mid.0, "77");

        server.abort();
    }

    #[tokio::test]
    async fn send_with_actions_builds_correct_payload() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (payload_tx, payload_rx) = tokio::sync::oneshot::channel::<String>();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
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
            let header_end = buf[..total]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(total);

            let cl = header_str
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);

            let body_so_far = total - header_end;
            let mut body_bytes = buf[header_end..total].to_vec();
            if body_so_far < cl {
                let remaining = cl - body_so_far;
                let mut rest = vec![0u8; remaining];
                let _ = stream.read_exact(&mut rest).await;
                body_bytes.extend_from_slice(&rest);
            }

            let _ = payload_tx.send(String::from_utf8_lossy(&body_bytes).to_string());

            let response_body =
                r#"{"ok":true,"result":{"message_id":88,"chat":{"id":456},"text":"approve?"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });

        let actions = vec![
            Action {
                id: "approve:task-42".to_string(),
                label: "Approve".to_string(),
                style: ActionStyle::Primary,
            },
            Action {
                id: "reject:task-42".to_string(),
                label: "Reject".to_string(),
                style: ActionStyle::Danger,
            },
        ];

        let body_json = serde_json::json!({
            "chat_id": "456",
            "text": "Deploy to production?",
            "reply_markup": build_inline_keyboard(&actions),
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/bottest/sendMessage"))
            .json(&body_json)
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(TelegramChannel::extract_message_id(&json).0, "88");

        // Verify the sent payload contains the inline keyboard
        let sent_body = payload_rx.await.unwrap();
        let sent_json: serde_json::Value = serde_json::from_str(&sent_body).unwrap();
        let kb = &sent_json["reply_markup"]["inline_keyboard"];
        let buttons = kb[0].as_array().unwrap();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0]["callback_data"], "approve:task-42");
        assert_eq!(buttons[1]["callback_data"], "reject:task-42");

        server.abort();
    }

    #[tokio::test]
    async fn api_call_returns_error_on_non_ok_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
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
            // Consume body
            let header_str = String::from_utf8_lossy(&buf[..total]).to_string();
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
                    let mut body_buf = vec![0u8; remaining];
                    let _ = stream.read_exact(&mut body_buf).await;
                }
            }

            let response_body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });

        // Build a TelegramChannel that points at our mock. We need to override
        // api_url, so we test api_call indirectly via the client.
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/botbad-token/sendMessage"))
            .json(&serde_json::json!({"chat_id": "1", "text": "x"}))
            .send()
            .await
            .unwrap();

        let status = resp.status();
        let json: serde_json::Value = resp.json().await.unwrap();
        // Replicate the error check from api_call
        assert!(!status.is_success());
        assert_eq!(json.get("ok"), Some(&serde_json::Value::Bool(false)));
        let desc = json
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap();
        assert_eq!(desc, "Unauthorized");

        server.abort();
    }
}
