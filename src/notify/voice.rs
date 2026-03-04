//! Voice call notification channel — initiates phone calls via Twilio Voice API.
//!
//! Implements [`NotificationChannel`] for voice calls. Supports:
//! - Outbound: Twilio-initiated calls with TTS (Text-to-Speech) via TwiML `<Say>`
//! - DTMF input: callee presses digits to select actions (e.g. "Press 1 to approve")
//! - Inline TwiML: generates TwiML XML and passes it via the `Twiml` parameter,
//!   avoiding the need for a separate TwiML webhook server
//!
//! Configuration is read from the `[voice]` section of `notify.toml`:
//! ```toml
//! [voice]
//! account_sid = "AC..."
//! auth_token = "..."
//! from = "+15551234567"
//! to = "+15559876543"
//! voice = "Polly.Amy"          # optional, TTS voice name
//! language = "en-US"           # optional, TTS language
//! status_callback = "https://..."  # optional, call status webhook
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::{Action, IncomingMessage, MessageId, NotificationChannel, RichMessage};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Twilio Voice configuration parsed from the `[voice]` section.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct VoiceConfig {
    /// Twilio Account SID.
    pub account_sid: String,
    /// Twilio Auth Token.
    pub auth_token: String,
    /// Twilio phone number to call from (E.164 format).
    pub from: String,
    /// Default recipient phone number (E.164 format).
    pub to: String,
    /// TTS voice name (e.g. "Polly.Amy", "alice"). Defaults to Twilio default.
    #[serde(default)]
    pub voice: Option<String>,
    /// TTS language code (e.g. "en-US"). Defaults to "en-US".
    #[serde(default)]
    pub language: Option<String>,
    /// Optional URL for Twilio to POST call status updates to.
    #[serde(default)]
    pub status_callback: Option<String>,
}

impl VoiceConfig {
    /// Extract from the opaque channel map in [`super::config::NotifyConfig`].
    pub fn from_notify_config(config: &super::config::NotifyConfig) -> Result<Self> {
        let val = config
            .channels
            .get("voice")
            .context("no [voice] section in notify config")?;
        let cfg: Self = val
            .clone()
            .try_into()
            .context("invalid [voice] config")?;
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// TwiML generation
// ---------------------------------------------------------------------------

/// Escape XML special characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build a TwiML document that speaks a message via TTS.
fn twiml_say(message: &str, voice: Option<&str>, language: Option<&str>) -> String {
    let voice_attr = voice
        .map(|v| format!(" voice=\"{}\"", xml_escape(v)))
        .unwrap_or_default();
    let lang_attr = language
        .map(|l| format!(" language=\"{}\"", xml_escape(l)))
        .unwrap_or_default();

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <Response>\
           <Say{voice_attr}{lang_attr}>{}</Say>\
         </Response>",
        xml_escape(message)
    )
}

/// Build a TwiML document that speaks a message and gathers DTMF input.
///
/// Each action is mapped to a digit: action 0 → "Press 1", action 1 → "Press 2", etc.
/// The `<Gather>` verb collects a single digit. If no input is received,
/// the message is repeated once and the call ends.
fn twiml_gather(
    message: &str,
    actions: &[Action],
    voice: Option<&str>,
    language: Option<&str>,
) -> String {
    let voice_attr = voice
        .map(|v| format!(" voice=\"{}\"", xml_escape(v)))
        .unwrap_or_default();
    let lang_attr = language
        .map(|l| format!(" language=\"{}\"", xml_escape(l)))
        .unwrap_or_default();

    let mut prompt = xml_escape(message);
    if !actions.is_empty() {
        prompt.push_str(". ");
        for (i, action) in actions.iter().enumerate() {
            prompt.push_str(&format!(
                "Press {} for {}. ",
                i + 1,
                xml_escape(&action.label)
            ));
        }
    }

    // Gather a single digit; if no input after speaking, repeat once then hang up.
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <Response>\
           <Gather numDigits=\"1\">\
             <Say{voice_attr}{lang_attr}>{prompt}</Say>\
           </Gather>\
           <Say{voice_attr}{lang_attr}>{prompt}</Say>\
         </Response>"
    )
}

/// Map a DTMF digit (1-based) to the corresponding action ID.
pub fn dtmf_to_action(digit: char, actions: &[Action]) -> Option<&str> {
    let idx = digit.to_digit(10)?.checked_sub(1)? as usize;
    actions.get(idx).map(|a| a.id.as_str())
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// A voice call notification channel backed by the Twilio Calls API via `reqwest`.
pub struct VoiceChannel {
    config: VoiceConfig,
    client: reqwest::Client,
}

impl VoiceChannel {
    pub fn new(config: VoiceConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Resolve the target phone number. If empty or "*", uses the configured default.
    fn resolve_recipient<'a>(&'a self, target: &'a str) -> &'a str {
        if target.is_empty() || target == "*" {
            &self.config.to
        } else {
            target
        }
    }

    /// Initiate a Twilio voice call with inline TwiML.
    async fn initiate_call(&self, to: &str, twiml: &str) -> Result<MessageId> {
        let url = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Calls.json",
            self.config.account_sid
        );

        let mut form = vec![
            ("To", to.to_string()),
            ("From", self.config.from.clone()),
            ("Twiml", twiml.to_string()),
        ];

        if let Some(ref cb) = self.config.status_callback {
            form.push(("StatusCallback", cb.clone()));
        }

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
            .form(&form)
            .send()
            .await
            .context("Twilio Voice API request failed")?;

        let status = resp.status();
        let json: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse Twilio Voice API response")?;

        if !status.is_success() {
            let message = json
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("Twilio Voice API error ({}): {}", status, message);
        }

        let sid = json
            .get("sid")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        Ok(MessageId(sid.to_string()))
    }
}

#[async_trait]
impl NotificationChannel for VoiceChannel {
    fn channel_type(&self) -> &str {
        "voice"
    }

    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId> {
        let to = self.resolve_recipient(target);
        let twiml = twiml_say(
            message,
            self.config.voice.as_deref(),
            self.config.language.as_deref(),
        );
        self.initiate_call(to, &twiml).await
    }

    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId> {
        // Voice is audio-only — use the plain text fallback.
        let to = self.resolve_recipient(target);
        let twiml = twiml_say(
            &message.plain_text,
            self.config.voice.as_deref(),
            self.config.language.as_deref(),
        );
        self.initiate_call(to, &twiml).await
    }

    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId> {
        let to = self.resolve_recipient(target);
        let twiml = twiml_gather(
            message,
            actions,
            self.config.voice.as_deref(),
            self.config.language.as_deref(),
        );
        self.initiate_call(to, &twiml).await
    }

    fn supports_receive(&self) -> bool {
        // DTMF input is collected by Twilio and delivered via status callbacks/webhooks.
        // The channel itself doesn't implement a listener loop.
        false
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        anyhow::bail!(
            "Voice call DTMF responses are delivered via Twilio webhooks. \
             Configure a status_callback URL in the [voice] config section."
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

    fn test_config() -> VoiceConfig {
        VoiceConfig {
            account_sid: "AC1234567890abcdef".into(),
            auth_token: "test_auth_token".into(),
            from: "+15551234567".into(),
            to: "+15559876543".into(),
            voice: Some("Polly.Amy".into()),
            language: Some("en-US".into()),
            status_callback: None,
        }
    }

    #[test]
    fn voice_config_from_toml() {
        let toml_str = r#"
[routing]
default = ["voice"]

[voice]
account_sid = "AC0000000000000000"
auth_token = "secret"
from = "+15550001111"
to = "+15552223333"
voice = "alice"
language = "en-GB"
status_callback = "https://example.com/status"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let voice = VoiceConfig::from_notify_config(&config).unwrap();
        assert_eq!(voice.account_sid, "AC0000000000000000");
        assert_eq!(voice.auth_token, "secret");
        assert_eq!(voice.from, "+15550001111");
        assert_eq!(voice.to, "+15552223333");
        assert_eq!(voice.voice.as_deref(), Some("alice"));
        assert_eq!(voice.language.as_deref(), Some("en-GB"));
        assert_eq!(
            voice.status_callback.as_deref(),
            Some("https://example.com/status")
        );
    }

    #[test]
    fn voice_config_minimal() {
        let toml_str = r#"
[routing]
default = ["voice"]

[voice]
account_sid = "AC0000000000000000"
auth_token = "secret"
from = "+15550001111"
to = "+15552223333"
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let voice = VoiceConfig::from_notify_config(&config).unwrap();
        assert!(voice.voice.is_none());
        assert!(voice.language.is_none());
        assert!(voice.status_callback.is_none());
    }

    #[test]
    fn voice_config_missing_section() {
        let config = super::super::config::NotifyConfig::default();
        assert!(VoiceConfig::from_notify_config(&config).is_err());
    }

    #[test]
    fn channel_type_is_voice() {
        let ch = VoiceChannel::new(test_config());
        assert_eq!(ch.channel_type(), "voice");
    }

    #[test]
    fn does_not_support_receive() {
        let ch = VoiceChannel::new(test_config());
        assert!(!ch.supports_receive());
    }

    #[test]
    fn resolve_recipient_default() {
        let ch = VoiceChannel::new(test_config());
        assert_eq!(ch.resolve_recipient("*"), "+15559876543");
        assert_eq!(ch.resolve_recipient(""), "+15559876543");
    }

    #[test]
    fn resolve_recipient_explicit() {
        let ch = VoiceChannel::new(test_config());
        assert_eq!(ch.resolve_recipient("+15550009999"), "+15550009999");
    }

    // -----------------------------------------------------------------------
    // TwiML generation
    // -----------------------------------------------------------------------

    #[test]
    fn twiml_say_basic() {
        let twiml = twiml_say("Hello world", None, None);
        assert!(twiml.starts_with("<?xml version=\"1.0\""));
        assert!(twiml.contains("<Say>Hello world</Say>"));
        assert!(twiml.contains("<Response>"));
        assert!(twiml.contains("</Response>"));
    }

    #[test]
    fn twiml_say_with_voice_and_language() {
        let twiml = twiml_say("Hello", Some("Polly.Amy"), Some("en-US"));
        assert!(twiml.contains("voice=\"Polly.Amy\""));
        assert!(twiml.contains("language=\"en-US\""));
        assert!(twiml.contains("<Say voice=\"Polly.Amy\" language=\"en-US\">Hello</Say>"));
    }

    #[test]
    fn twiml_say_escapes_xml() {
        let twiml = twiml_say("Task <foo> & bar", None, None);
        assert!(twiml.contains("Task &lt;foo&gt; &amp; bar"));
        assert!(!twiml.contains("<foo>"));
    }

    #[test]
    fn twiml_gather_with_actions() {
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
        let twiml = twiml_gather("Deploy ready", &actions, None, None);
        assert!(twiml.contains("<Gather numDigits=\"1\">"));
        assert!(twiml.contains("Press 1 for Approve"));
        assert!(twiml.contains("Press 2 for Reject"));
        assert!(twiml.contains("Deploy ready"));
        // Message is repeated after Gather (no-input fallback)
        let say_count = twiml.matches("<Say>").count();
        assert_eq!(say_count, 2);
    }

    #[test]
    fn twiml_gather_no_actions_just_speaks() {
        let twiml = twiml_gather("Just a message", &[], None, None);
        assert!(twiml.contains("<Gather numDigits=\"1\">"));
        assert!(twiml.contains("Just a message"));
        assert!(!twiml.contains("Press"));
    }

    #[test]
    fn twiml_gather_escapes_action_labels() {
        let actions = vec![Action {
            id: "go".into(),
            label: "Go & Run <fast>".into(),
            style: ActionStyle::Primary,
        }];
        let twiml = twiml_gather("Test", &actions, None, None);
        assert!(twiml.contains("Go &amp; Run &lt;fast&gt;"));
    }

    // -----------------------------------------------------------------------
    // DTMF mapping
    // -----------------------------------------------------------------------

    #[test]
    fn dtmf_to_action_maps_digits() {
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
        assert_eq!(dtmf_to_action('1', &actions), Some("approve"));
        assert_eq!(dtmf_to_action('2', &actions), Some("reject"));
        assert_eq!(dtmf_to_action('3', &actions), None); // out of range
        assert_eq!(dtmf_to_action('0', &actions), None); // 0 - 1 underflows
        assert_eq!(dtmf_to_action('*', &actions), None); // non-digit
    }

    #[test]
    fn xml_escape_all_special_chars() {
        assert_eq!(xml_escape("a & b"), "a &amp; b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(xml_escape("it's"), "it&apos;s");
    }
}
