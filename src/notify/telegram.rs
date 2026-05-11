//! Telegram notification channel implementation using teloxide.
//!
//! Implements [`NotificationChannel`] for Telegram bots. Supports:
//! - Outbound: text, rich (Markdown), and action-button messages (inline keyboards)
//! - Inbound: long-polling listener that yields [`IncomingMessage`]s
//!
//! # Configuration
//!
//! Two forms are accepted (and may coexist) under the `[telegram]` section
//! of `notify.toml`:
//!
//! **Legacy single-bot** — kept for backwards compatibility:
//!
//! ```toml
//! [telegram]
//! bot_token = "123456:ABC-DEF..."
//! chat_id = "12345678"
//! ```
//!
//! This synthesises one bot keyed `"default"` whose [`NotificationChannel::channel_type`]
//! returns `"telegram"` (so existing routing rules like `default = ["telegram"]`
//! keep working without edits).
//!
//! **Multi-bot** — one bot per persistent named agent (the family-team
//! experiment shape: `@nora_planner_bot`, `@bruno_chef_bot`, etc.):
//!
//! ```toml
//! [telegram.bots.nora]
//! bot_token = "123456:ABC..."
//! chat_id   = "78901234"
//! agent_id  = "nora"        # workgraph agent this bot fronts (optional)
//!
//! [telegram.bots.bruno]
//! bot_token = "654321:XYZ..."
//! chat_id   = "78901234"
//! agent_id  = "bruno"
//! ```
//!
//! Each named bot registers as a distinct channel whose `channel_type()` is
//! `"telegram:<bot_id>"`, so the router can address them independently.
//! Inbound messages are tagged with the receiving bot's qualified type so
//! downstream code (the `awaiting-human` task router — see follow-up PR)
//! can route replies to the right open task.

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::{Action, ActionStyle, IncomingMessage, MessageId, NotificationChannel, RichMessage};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Per-bot configuration. One of these is what each `[telegram.bots.<id>]`
/// table parses into; one is also synthesised from the legacy top-level
/// `[telegram] bot_token` + `chat_id` fields when present.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TelegramBotConfig {
    pub bot_token: String,
    pub chat_id: String,
    /// Workgraph agent id this bot fronts (e.g. `"nora"`). When `None`, the
    /// bot is a shared/group bot — outbound routing falls back to it when
    /// no agent-specific bot matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// Telegram-specific configuration parsed from the `[telegram]` section.
///
/// Holds both legacy single-bot fields (for backwards compat) and a
/// multi-bot map. They may coexist; [`TelegramConfig::all_bots`] resolves
/// them into a single ordered list of `(bot_id, TelegramBotConfig)`.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct TelegramConfig {
    /// Legacy single-bot token. Empty when the config uses only the
    /// multi-bot `bots` map.
    #[serde(default)]
    pub bot_token: String,
    /// Legacy single-bot chat id. Empty when the config uses only the
    /// multi-bot `bots` map.
    #[serde(default)]
    pub chat_id: String,
    /// Multi-bot map. Key is the bot id (free-form identifier used in
    /// `channel_type()` as `"telegram:<bot_id>"`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub bots: HashMap<String, TelegramBotConfig>,
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

    /// Resolve all configured bots into a flat list. The legacy single-bot
    /// fields, when both `bot_token` and `chat_id` are non-empty, contribute
    /// one entry keyed `"default"` (with no agent binding); each entry of
    /// the `bots` map contributes its own entry. Iteration order is:
    /// legacy first (when present), then the named bots in their insertion
    /// order.
    ///
    /// Returns an empty vec when the config has no usable bot — callers
    /// should treat this as "telegram is not configured."
    pub fn all_bots(&self) -> Vec<(String, TelegramBotConfig)> {
        let mut out = Vec::new();
        if !self.bot_token.is_empty() && !self.chat_id.is_empty() {
            out.push((
                "default".to_string(),
                TelegramBotConfig {
                    bot_token: self.bot_token.clone(),
                    chat_id: self.chat_id.clone(),
                    agent_id: None,
                },
            ));
        }
        for (id, cfg) in &self.bots {
            out.push((id.clone(), cfg.clone()));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// A Telegram notification channel backed by the Telegram Bot API via `reqwest`.
///
/// This uses the Bot API directly over HTTP rather than pulling in the full
/// teloxide runtime, keeping the non-listener path lightweight.
///
/// One instance per bot. For multi-bot setups (one bot per persistent named
/// agent), build N `TelegramChannel` instances via
/// [`TelegramChannel::all_from_notify_config`] — each gets its own long-poll
/// task on `listen()` and its own `channel_type()` discriminator.
pub struct TelegramChannel {
    bot_id: String,
    bot: TelegramBotConfig,
    /// Pre-computed channel-type string. The legacy `"default"` bot returns
    /// the bare `"telegram"` so existing routing rules remain valid; named
    /// bots return `"telegram:<bot_id>"` so the router can address them
    /// distinctly.
    channel_type: String,
    client: reqwest::Client,
}

impl TelegramChannel {
    /// Construct a single-bot channel from the legacy `[telegram]` block
    /// (`bot_token` + `chat_id` at the top level).
    ///
    /// Kept for backwards compatibility — existing CLI commands
    /// (`wg telegram listen / send / status`) call this with a
    /// [`TelegramConfig`] populated only from the legacy fields. New code
    /// driving multi-bot setups should prefer [`TelegramChannel::from_bot`]
    /// or [`TelegramChannel::all_from_notify_config`].
    pub fn new(config: TelegramConfig) -> Self {
        Self::from_bot(
            "default".to_string(),
            TelegramBotConfig {
                bot_token: config.bot_token,
                chat_id: config.chat_id,
                agent_id: None,
            },
        )
    }

    /// Construct a channel for a specific bot. The `bot_id` is the key from
    /// the `[telegram.bots.<id>]` table (or the literal `"default"` for the
    /// legacy single-bot case).
    pub fn from_bot(bot_id: String, bot: TelegramBotConfig) -> Self {
        let channel_type = if bot_id == "default" {
            "telegram".to_string()
        } else {
            format!("telegram:{}", bot_id)
        };
        Self {
            bot_id,
            bot,
            channel_type,
            client: reqwest::Client::new(),
        }
    }

    /// Build all configured Telegram channels from a [`super::config::NotifyConfig`].
    ///
    /// Iterates [`TelegramConfig::all_bots`] and constructs one channel per
    /// entry. Returns an empty vec when no `[telegram]` section is present
    /// (callers should treat that as "telegram not configured" rather than an
    /// error — same behaviour as if the section were absent in legacy code).
    pub fn all_from_notify_config(config: &super::config::NotifyConfig) -> Result<Vec<Self>> {
        if !config.channels.contains_key("telegram") {
            return Ok(Vec::new());
        }
        let cfg = TelegramConfig::from_notify_config(config)?;
        Ok(cfg
            .all_bots()
            .into_iter()
            .map(|(id, bot)| Self::from_bot(id, bot))
            .collect())
    }

    /// The bot id (`"default"` for the legacy single-bot, otherwise the user-
    /// supplied key from `[telegram.bots.<id>]`).
    pub fn bot_id(&self) -> &str {
        &self.bot_id
    }

    /// The workgraph agent id this bot fronts, when bound. `None` for the
    /// legacy default bot (and for any named bot configured without an
    /// `agent_id` field).
    pub fn agent_id(&self) -> Option<&str> {
        self.bot.agent_id.as_deref()
    }

    /// The default chat id this bot writes to (the bot's own DM thread or
    /// the configured group chat, depending on the bot's role).
    pub fn chat_id(&self) -> &str {
        &self.bot.chat_id
    }

    /// A redacted preview of the bot token suitable for logs and JSON output —
    /// `"123456...XYZ"`. Avoids exposing the full token while still letting an
    /// operator visually distinguish bots when reading `wg telegram list-bots`.
    pub fn bot_token_preview(&self) -> String {
        let t = &self.bot.bot_token;
        if t.len() <= 10 {
            return "(unset)".to_string();
        }
        format!("{}...{}", &t[..6], &t[t.len().saturating_sub(4)..])
    }

    fn api_url(&self, method: &str) -> String {
        format!(
            "https://api.telegram.org/bot{}/{}",
            self.bot.bot_token, method
        )
    }

    /// Send a request to the Telegram Bot API and return the result.
    pub async fn api_call(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
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
}

#[async_trait]
impl NotificationChannel for TelegramChannel {
    fn channel_type(&self) -> &str {
        &self.channel_type
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
        // Build inline keyboard from actions.
        let buttons: Vec<serde_json::Value> = actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "text": format_button_label(&a.label, a.style),
                    "callback_data": &a.id,
                })
            })
            .collect();

        let body = serde_json::json!({
            "chat_id": target,
            "text": message,
            "reply_markup": {
                "inline_keyboard": [buttons],
            },
        });

        let resp = self.api_call("sendMessage", &body).await?;
        Ok(Self::extract_message_id(&resp))
    }

    fn supports_receive(&self) -> bool {
        true
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let bot = self.bot.clone();
        let client = self.client.clone();
        // Pre-compute the channel-type tag so each IncomingMessage carries
        // the bot identity ("telegram" for the legacy bot, "telegram:<id>"
        // for named ones). The downstream router uses this to decide which
        // open `awaiting-human` task should receive the reply.
        let channel_tag = self.channel_type.clone();

        tokio::spawn(async move {
            let mut offset: i64 = 0;
            loop {
                let url = format!(
                    "https://api.telegram.org/bot{}/getUpdates",
                    bot.bot_token
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
                        let sender = cb
                            .get("from")
                            .and_then(|f| f.get("username"))
                            .and_then(|u| u.as_str())
                            .or_else(|| {
                                cb.get("from")
                                    .and_then(|f| f.get("id"))
                                    .and_then(|i| i.as_i64())
                                    .map(|_| "unknown")
                            })
                            .unwrap_or("unknown");

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
                            channel: channel_tag.clone(),
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
                        let sender = message
                            .get("from")
                            .and_then(|f| f.get("username"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("unknown");

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
                            channel: channel_tag.clone(),
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

    fn legacy_config(token: &str, chat: &str) -> TelegramConfig {
        TelegramConfig {
            bot_token: token.into(),
            chat_id: chat.into(),
            bots: HashMap::new(),
        }
    }

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
        assert!(tg.bots.is_empty(), "legacy form should not populate bots map");
    }

    #[test]
    fn telegram_config_missing_section() {
        let config = super::super::config::NotifyConfig::default();
        assert!(TelegramConfig::from_notify_config(&config).is_err());
    }

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

    #[test]
    fn channel_type_is_telegram() {
        // Legacy single-bot construction returns the bare "telegram" type so
        // existing routing rules (`default = ["telegram"]`) keep matching.
        let ch = TelegramChannel::new(legacy_config("test", "test"));
        assert_eq!(ch.channel_type(), "telegram");
        assert_eq!(ch.bot_id(), "default");
        assert_eq!(ch.agent_id(), None);
    }

    #[test]
    fn supports_receive_is_true() {
        let ch = TelegramChannel::new(legacy_config("test", "test"));
        assert!(ch.supports_receive());
    }

    #[test]
    fn api_url_format() {
        let ch = TelegramChannel::new(legacy_config("123:ABC", "456"));
        assert_eq!(
            ch.api_url("sendMessage"),
            "https://api.telegram.org/bot123:ABC/sendMessage"
        );
    }

    // -----------------------------------------------------------------------
    // Multi-bot tests (R16): per-agent named bots, qualified channel types,
    // backwards-compat with the legacy single-bot form.
    // -----------------------------------------------------------------------

    #[test]
    fn parse_multi_bot_config() {
        // Three bots: legacy single (no agent_id, becomes "default"), plus
        // two named with agent bindings. all_bots() resolves them in order:
        // legacy first, then named.
        let toml_str = r#"
[routing]
default = ["telegram"]

[telegram]
bot_token = "111:AAA"
chat_id = "111"

[telegram.bots.nora]
bot_token = "222:BBB"
chat_id = "222"
agent_id = "nora"

[telegram.bots.bruno]
bot_token = "333:CCC"
chat_id = "333"
agent_id = "bruno"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let tg = TelegramConfig::from_notify_config(&config).unwrap();
        assert_eq!(tg.bot_token, "111:AAA");
        assert_eq!(tg.bots.len(), 2);

        let bots = tg.all_bots();
        assert_eq!(bots.len(), 3, "legacy + 2 named = 3 bots");
        assert_eq!(bots[0].0, "default", "legacy bot comes first");
        assert_eq!(bots[0].1.bot_token, "111:AAA");
        assert_eq!(bots[0].1.agent_id, None);

        // The named bots are in HashMap order — we just check both exist.
        let nora = bots.iter().find(|(id, _)| id == "nora").expect("nora bot");
        assert_eq!(nora.1.bot_token, "222:BBB");
        assert_eq!(nora.1.agent_id.as_deref(), Some("nora"));

        let bruno = bots.iter().find(|(id, _)| id == "bruno").expect("bruno bot");
        assert_eq!(bruno.1.bot_token, "333:CCC");
        assert_eq!(bruno.1.agent_id.as_deref(), Some("bruno"));
    }

    #[test]
    fn parse_multi_bot_only_no_legacy() {
        // Configs that use ONLY the new bots map (no top-level bot_token)
        // must parse cleanly and produce no "default" entry.
        let toml_str = r#"
[telegram.bots.nora]
bot_token = "222:BBB"
chat_id = "222"
agent_id = "nora"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let tg = TelegramConfig::from_notify_config(&config).unwrap();
        assert!(tg.bot_token.is_empty(), "no legacy token");
        assert!(tg.chat_id.is_empty(), "no legacy chat_id");
        assert_eq!(tg.bots.len(), 1);

        let bots = tg.all_bots();
        assert_eq!(bots.len(), 1, "no legacy entry should appear");
        assert_eq!(bots[0].0, "nora");
    }

    #[test]
    fn parse_legacy_only_still_works() {
        // The exact legacy form must keep round-tripping without any new keys.
        let toml_str = r#"
[telegram]
bot_token = "123:ABC"
chat_id = "456"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let tg = TelegramConfig::from_notify_config(&config).unwrap();
        assert!(tg.bots.is_empty());

        let bots = tg.all_bots();
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0].0, "default");
        assert_eq!(bots[0].1.agent_id, None);
    }

    #[test]
    fn named_bot_channel_type_is_qualified() {
        // Named bots return "telegram:<bot_id>" so the router can address
        // them distinctly. This is what makes per-agent routing possible.
        let ch = TelegramChannel::from_bot(
            "nora".to_string(),
            TelegramBotConfig {
                bot_token: "222:BBB".into(),
                chat_id: "222".into(),
                agent_id: Some("nora".into()),
            },
        );
        assert_eq!(ch.channel_type(), "telegram:nora");
        assert_eq!(ch.bot_id(), "nora");
        assert_eq!(ch.agent_id(), Some("nora"));
        assert_eq!(ch.chat_id(), "222");
    }

    #[test]
    fn default_bot_channel_type_is_unqualified() {
        // The "default" bot keeps the bare "telegram" channel_type so existing
        // routing rules and tests don't break — this is the load-bearing
        // backwards-compat invariant.
        let ch = TelegramChannel::from_bot(
            "default".to_string(),
            TelegramBotConfig {
                bot_token: "111:AAA".into(),
                chat_id: "111".into(),
                agent_id: None,
            },
        );
        assert_eq!(ch.channel_type(), "telegram");
    }

    #[test]
    fn all_from_notify_config_builds_one_channel_per_bot() {
        let toml_str = r#"
[telegram]
bot_token = "111:AAA"
chat_id = "111"

[telegram.bots.nora]
bot_token = "222:BBB"
chat_id = "222"
agent_id = "nora"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let channels = TelegramChannel::all_from_notify_config(&config).unwrap();
        assert_eq!(channels.len(), 2);

        let types: Vec<&str> = channels.iter().map(|c| c.channel_type()).collect();
        assert!(types.contains(&"telegram"), "default bot present");
        assert!(types.contains(&"telegram:nora"), "named bot present");
    }

    #[test]
    fn all_from_notify_config_empty_when_no_telegram_section() {
        // A NotifyConfig without `[telegram]` must produce an empty vec, not
        // an error — this lets callers treat "telegram not configured" the
        // same way the legacy code does (skip silently).
        let config = super::super::config::NotifyConfig::default();
        let channels = TelegramChannel::all_from_notify_config(&config).unwrap();
        assert!(channels.is_empty());
    }

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
}
