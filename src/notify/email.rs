//! Email notification channel — sends messages via SMTP using `lettre`.
//!
//! Implements [`NotificationChannel`] for email. Supports:
//! - Outbound: plain text, rich (HTML), and action-button messages
//! - Inbound: not supported (email receive requires IMAP polling, a future enhancement)
//!
//! Configuration is read from the `[email]` section of `notify.toml`:
//! ```toml
//! [email]
//! smtp_host = "smtp.example.com"
//! smtp_port = 587            # optional, defaults to 587
//! smtp_user = "user@example.com"
//! smtp_password = "password"
//! from = "wg@example.com"
//! to = ["user@example.com"]
//! use_tls = true             # optional, defaults to true
//! ```

use anyhow::{Context, Result};
use async_trait::async_trait;
use lettre::message::{Mailbox, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::{Action, ActionStyle, IncomingMessage, MessageId, NotificationChannel, RichMessage};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Email-specific configuration parsed from the `[email]` section.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct EmailConfig {
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    pub from: String,
    /// Default recipients (used when target is empty or "*").
    pub to: Vec<String>,
    #[serde(default = "default_use_tls")]
    pub use_tls: bool,
    /// Optional subject prefix for all outgoing emails.
    #[serde(default = "default_subject_prefix")]
    pub subject_prefix: String,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_use_tls() -> bool {
    true
}

fn default_subject_prefix() -> String {
    "[WG]".to_string()
}

impl EmailConfig {
    /// Extract from the opaque channel map in [`super::config::NotifyConfig`].
    pub fn from_notify_config(config: &super::config::NotifyConfig) -> Result<Self> {
        let val = config
            .channels
            .get("email")
            .context("no [email] section in notify config")?;
        let cfg: Self = val.clone().try_into().context("invalid [email] config")?;
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// Channel implementation
// ---------------------------------------------------------------------------

/// An email notification channel backed by SMTP via `lettre`.
pub struct EmailChannel {
    config: EmailConfig,
}

impl EmailChannel {
    pub fn new(config: EmailConfig) -> Self {
        Self { config }
    }

    /// Build an async SMTP transport from config.
    fn build_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(
            self.config.smtp_user.clone(),
            self.config.smtp_password.clone(),
        );

        let transport = if self.config.use_tls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)
                .context("failed to create SMTP STARTTLS transport")?
                .port(self.config.smtp_port)
                .credentials(creds)
                .build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.config.smtp_host)
                .port(self.config.smtp_port)
                .credentials(creds)
                .build()
        };

        Ok(transport)
    }

    /// Parse the `from` address into a Mailbox.
    fn from_mailbox(&self) -> Result<Mailbox> {
        self.config
            .from
            .parse()
            .context("invalid 'from' email address")
    }

    /// Resolve target to recipient addresses. If target is "*" or empty,
    /// uses the default `to` list from config.
    fn resolve_recipients(&self, target: &str) -> Vec<String> {
        if target.is_empty() || target == "*" {
            self.config.to.clone()
        } else {
            target.split(',').map(|s| s.trim().to_string()).collect()
        }
    }

    /// Send an email with both plain text and HTML parts.
    async fn send_email(
        &self,
        recipients: &[String],
        subject: &str,
        plain_body: &str,
        html_body: Option<&str>,
    ) -> Result<MessageId> {
        let from = self.from_mailbox()?;
        let transport = self.build_transport()?;

        let full_subject = format!("{} {}", self.config.subject_prefix, subject);
        let mut message_ids = Vec::new();

        for recipient in recipients {
            let to: Mailbox = recipient
                .parse()
                .with_context(|| format!("invalid recipient address: {recipient}"))?;

            let email = if let Some(html) = html_body {
                Message::builder()
                    .from(from.clone())
                    .to(to)
                    .subject(&full_subject)
                    .multipart(
                        MultiPart::alternative()
                            .singlepart(
                                SinglePart::builder()
                                    .content_type(ContentType::TEXT_PLAIN)
                                    .body(plain_body.to_string()),
                            )
                            .singlepart(
                                SinglePart::builder()
                                    .content_type(ContentType::TEXT_HTML)
                                    .body(html.to_string()),
                            ),
                    )
                    .context("failed to build email message")?
            } else {
                Message::builder()
                    .from(from.clone())
                    .to(to)
                    .subject(&full_subject)
                    .body(plain_body.to_string())
                    .context("failed to build email message")?
            };

            let response = transport.send(email).await.context("SMTP send failed")?;

            // Use the SMTP response code as part of the message id.
            message_ids.push(format!("email:{}", response.code()));
        }

        Ok(MessageId(
            message_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "email:sent".to_string()),
        ))
    }
}

#[async_trait]
impl NotificationChannel for EmailChannel {
    fn channel_type(&self) -> &str {
        "email"
    }

    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId> {
        let recipients = self.resolve_recipients(target);
        if recipients.is_empty() {
            anyhow::bail!("no email recipients configured");
        }

        // Use first line or truncated message as subject.
        let subject = message
            .lines()
            .next()
            .unwrap_or("Notification")
            .chars()
            .take(80)
            .collect::<String>();

        self.send_email(&recipients, &subject, message, None).await
    }

    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId> {
        let recipients = self.resolve_recipients(target);
        if recipients.is_empty() {
            anyhow::bail!("no email recipients configured");
        }

        let subject = message
            .plain_text
            .lines()
            .next()
            .unwrap_or("Notification")
            .chars()
            .take(80)
            .collect::<String>();

        self.send_email(
            &recipients,
            &subject,
            &message.plain_text,
            message.html.as_deref(),
        )
        .await
    }

    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId> {
        let recipients = self.resolve_recipients(target);
        if recipients.is_empty() {
            anyhow::bail!("no email recipients configured");
        }

        // Build an HTML body with action buttons rendered as styled links.
        let mut html = format!(
            "<p>{}</p>\n<p><strong>Actions:</strong></p>\n<ul>\n",
            html_escape(message)
        );
        for action in actions {
            let style = match action.style {
                ActionStyle::Primary => "color: green; font-weight: bold",
                ActionStyle::Danger => "color: red; font-weight: bold",
                ActionStyle::Secondary => "color: gray",
            };
            html.push_str(&format!(
                "<li><span style=\"{}\">[{}]</span> {}</li>\n",
                style,
                html_escape(&action.id),
                html_escape(&action.label),
            ));
        }
        html.push_str("</ul>\n<p><em>Reply to this email with the action ID to respond.</em></p>");

        // Plain text fallback with action list.
        let mut plain = format!("{}\n\nActions:\n", message);
        for action in actions {
            plain.push_str(&format!("  - [{}] {}\n", action.id, action.label));
        }
        plain.push_str("\nReply with the action ID to respond.");

        let subject = message
            .lines()
            .next()
            .unwrap_or("Action Required")
            .chars()
            .take(80)
            .collect::<String>();

        self.send_email(&recipients, &subject, &plain, Some(&html))
            .await
    }

    fn supports_receive(&self) -> bool {
        false // IMAP receive is a future enhancement
    }

    async fn listen(&self) -> Result<tokio::sync::mpsc::Receiver<IncomingMessage>> {
        anyhow::bail!("email channel does not yet support receiving (IMAP not implemented)")
    }
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

    fn test_config() -> EmailConfig {
        EmailConfig {
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            smtp_user: "user@example.com".into(),
            smtp_password: "password".into(),
            from: "wg@example.com".into(),
            to: vec!["user@example.com".into()],
            use_tls: true,
            subject_prefix: "[WG]".into(),
        }
    }

    #[test]
    fn email_config_from_toml() {
        let toml_str = r#"
[routing]
default = ["email"]

[email]
smtp_host = "smtp.example.com"
smtp_port = 465
smtp_user = "bot@example.com"
smtp_password = "secret"
from = "bot@example.com"
to = ["admin@example.com", "dev@example.com"]
use_tls = true
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let email = EmailConfig::from_notify_config(&config).unwrap();
        assert_eq!(email.smtp_host, "smtp.example.com");
        assert_eq!(email.smtp_port, 465);
        assert_eq!(email.to.len(), 2);
        assert!(email.use_tls);
    }

    #[test]
    fn email_config_defaults() {
        let toml_str = r#"
[routing]
default = ["email"]

[email]
smtp_host = "smtp.example.com"
smtp_user = "user@example.com"
smtp_password = "pass"
from = "wg@example.com"
to = ["admin@example.com"]
"#;
        let config: super::super::config::NotifyConfig = toml::from_str(toml_str).unwrap();
        let email = EmailConfig::from_notify_config(&config).unwrap();
        assert_eq!(email.smtp_port, 587);
        assert!(email.use_tls);
        assert_eq!(email.subject_prefix, "[WG]");
    }

    #[test]
    fn email_config_missing_section() {
        let config = super::super::config::NotifyConfig::default();
        assert!(EmailConfig::from_notify_config(&config).is_err());
    }

    #[test]
    fn channel_type_is_email() {
        let ch = EmailChannel::new(test_config());
        assert_eq!(ch.channel_type(), "email");
    }

    #[test]
    fn does_not_support_receive() {
        let ch = EmailChannel::new(test_config());
        assert!(!ch.supports_receive());
    }

    #[test]
    fn resolve_recipients_default() {
        let ch = EmailChannel::new(test_config());
        assert_eq!(ch.resolve_recipients("*"), vec!["user@example.com"]);
        assert_eq!(ch.resolve_recipients(""), vec!["user@example.com"]);
    }

    #[test]
    fn resolve_recipients_explicit() {
        let ch = EmailChannel::new(test_config());
        let r = ch.resolve_recipients("a@b.com, c@d.com");
        assert_eq!(r, vec!["a@b.com", "c@d.com"]);
    }

    #[test]
    fn from_mailbox_valid() {
        let ch = EmailChannel::new(test_config());
        assert!(ch.from_mailbox().is_ok());
    }

    #[test]
    fn html_escape_works() {
        assert_eq!(html_escape("<b>test</b>"), "&lt;b&gt;test&lt;/b&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }
}
