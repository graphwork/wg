# Human Summoning Mechanisms for Workgraph

This document researches mechanisms for "summoning" humans into the workgraph system when agents or tasks need human input - approval, review, expertise, or decisions.

---

## Table of Contents

1. [Notification Channels](#1-notification-channels)
2. [Integration Patterns](#2-integration-patterns)
3. [Two-way Communication](#3-two-way-communication)
4. [Privacy and Security](#4-privacy-and-security)
5. [Data Model Changes](#5-data-model-changes)
6. [Practical Recommendations](#6-practical-recommendations)

---

## 1. Notification Channels

### 1.1 Desktop Notifications

#### notify-rust (Linux/freedesktop, macOS, Windows)

**Crate**: [notify-rust](https://crates.io/crates/notify-rust)

Cross-platform desktop notification library for Rust.

```rust
use notify_rust::Notification;

fn summon_human(task_id: &str, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    Notification::new()
        .summary(&format!("Workgraph: Task needs attention"))
        .body(&format!("{}\n\nTask: {}", message, task_id))
        .icon("dialog-information")
        .urgency(notify_rust::Urgency::Normal)
        .timeout(notify_rust::Timeout::Milliseconds(10000))
        .action("view", "View Task")
        .action("dismiss", "Dismiss")
        .show()?;
    Ok(())
}
```

**Platform support:**
- **Linux**: Uses D-Bus and freedesktop.org notification spec
- **macOS**: Uses native Notification Center
- **Windows**: Uses Windows toast notifications

**Cargo.toml:**

```toml
[dependencies]
notify-rust = "4"
```

**Pros:**
- Zero external dependencies for the user
- Non-intrusive (doesn't interrupt workflow)
- Cross-platform with single API
- Can include action buttons

**Cons:**
- Only works when user is at computer
- Easy to miss/dismiss
- No guaranteed delivery
- Platform-specific behaviors differ

#### macOS-specific: osascript

For more control on macOS, you can use AppleScript via osascript:

```rust
use std::process::Command;

fn macos_notification(title: &str, message: &str) -> std::io::Result<()> {
    Command::new("osascript")
        .args([
            "-e",
            &format!(
                r#"display notification "{}" with title "{}""#,
                message, title
            ),
        ])
        .output()?;
    Ok(())
}
```

### 1.2 Terminal Bell/Alerts

For users running workgraph in tmux, screen, or terminal emulators.

```rust
fn terminal_bell() {
    // Standard ASCII BEL character
    print!("\x07");
    std::io::stdout().flush().unwrap();
}

fn terminal_urgent_bell() {
    // Multiple bells for emphasis
    for _ in 0..3 {
        print!("\x07");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::io::stdout().flush().unwrap();
}
```

**tmux integration:**

tmux can be configured to show visual alerts when a window receives a bell:

```bash
# .tmux.conf
set -g visual-bell on
set -g bell-action any
```

**Terminal title update:**

```rust
fn update_terminal_title(message: &str) {
    // ANSI escape sequence to set terminal title
    print!("\x1b]0;{}\x07", message);
    std::io::stdout().flush().unwrap();
}

// Example: update_terminal_title("ATTENTION: Task needs review");
```

**Pros:**
- Works over SSH
- Very low latency
- No external dependencies
- Works in any terminal emulator

**Cons:**
- Only effective if user is watching terminal
- Easy to miss
- Very limited information capacity

### 1.3 Email Notifications

#### lettre (SMTP)

**Crate**: [lettre](https://crates.io/crates/lettre)

Full-featured email client for Rust.

```rust
use lettre::{
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
    Message, SmtpTransport, Transport,
};

struct EmailConfig {
    smtp_server: String,
    smtp_port: u16,
    username: String,
    password: String,
    from_address: String,
}

fn send_summons_email(
    config: &EmailConfig,
    to: &str,
    task: &Task,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let email = Message::builder()
        .from(config.from_address.parse()?)
        .to(to.parse()?)
        .subject(format!("[Workgraph] Attention needed: {}", task.title))
        .header(ContentType::TEXT_PLAIN)
        .body(format!(
            r#"A task needs your attention.

Task: {}
ID: {}
Reason: {}

View task: workgraph://task/{}

---
Workgraph Notification System
"#,
            task.title, task.id, reason, task.id
        ))?;

    let creds = Credentials::new(
        config.username.clone(),
        config.password.clone(),
    );

    let mailer = SmtpTransport::relay(&config.smtp_server)?
        .port(config.smtp_port)
        .credentials(creds)
        .build();

    mailer.send(&email)?;
    Ok(())
}
```

**Cargo.toml:**

```toml
[dependencies]
lettre = "0.11"
```

#### SendGrid / Mailgun (API-based)

For production use, API-based email services are more reliable:

```rust
use reqwest::Client;
use serde_json::json;

async fn send_via_sendgrid(
    api_key: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), reqwest::Error> {
    let client = Client::new();

    client.post("https://api.sendgrid.com/v3/mail/send")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&json!({
            "personalizations": [{
                "to": [{"email": to}]
            }],
            "from": {"email": "workgraph@yourdomain.com"},
            "subject": subject,
            "content": [{
                "type": "text/plain",
                "value": body
            }]
        }))
        .send()
        .await?;

    Ok(())
}
```

**Pros:**
- Reliable delivery
- Works when user is away from computer
- Provides record/audit trail
- Can include detailed information
- User can reply to email

**Cons:**
- Not real-time (email delay)
- Can get lost in inbox clutter
- Requires email server/service setup
- May be filtered as spam

### 1.4 SMS/Text Messages

#### Twilio SMS

**Crate**: [twilio](https://crates.io/crates/twilio-rs) (unofficial) or direct API

```rust
use reqwest::Client;
use base64::{Engine as _, engine::general_purpose::STANDARD};

struct TwilioConfig {
    account_sid: String,
    auth_token: String,
    from_number: String, // Twilio phone number
}

async fn send_sms(
    config: &TwilioConfig,
    to: &str,
    message: &str,
) -> Result<(), reqwest::Error> {
    let client = Client::new();

    let auth = STANDARD.encode(format!(
        "{}:{}",
        config.account_sid, config.auth_token
    ));

    client
        .post(&format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
            config.account_sid
        ))
        .header("Authorization", format!("Basic {}", auth))
        .form(&[
            ("To", to),
            ("From", &config.from_number),
            ("Body", message),
        ])
        .send()
        .await?;

    Ok(())
}

// Usage for task summons
async fn summon_via_sms(config: &TwilioConfig, actor: &Actor, task: &Task) {
    if let Some(phone) = &actor.phone {
        let msg = format!(
            "Workgraph: Task '{}' needs your attention. Reply YES to claim or NO to skip.",
            task.title
        );
        send_sms(config, phone, &msg).await.ok();
    }
}
```

#### AWS SNS

```rust
use aws_sdk_sns::{Client, Config};

async fn send_sms_via_sns(
    client: &Client,
    phone: &str,
    message: &str,
) -> Result<(), aws_sdk_sns::Error> {
    client
        .publish()
        .phone_number(phone)
        .message(message)
        .send()
        .await?;
    Ok(())
}
```

**Cargo.toml:**

```toml
[dependencies]
aws-sdk-sns = "1"
aws-config = "1"
```

**Pros:**
- High attention-getting (phones are always with us)
- Works anywhere with cell service
- Good for urgent notifications
- Can enable two-way communication

**Cons:**
- Costs money per message (Twilio: ~$0.0079/SMS)
- Limited message length (160 chars, or ~$0.01+ for longer)
- Can be seen as invasive
- Phone number required
- International numbers are more expensive

### 1.5 Phone Calls

#### Twilio Voice API

The "nuclear option" for truly urgent situations.

```rust
use reqwest::Client;

async fn call_human(
    config: &TwilioConfig,
    to: &str,
    task: &Task,
) -> Result<String, reqwest::Error> {
    let client = Client::new();

    // TwiML instructions for the call
    let twiml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
    <Say voice="alice">
        Attention. This is Workgraph calling about task: {}.
        This task requires your immediate attention.
        Press 1 to claim this task.
        Press 2 to snooze for 30 minutes.
        Press 3 to dismiss.
    </Say>
    <Gather numDigits="1" action="/handle-response/{}">
        <Say>Please make a selection.</Say>
    </Gather>
</Response>"#,
        task.title, task.id
    );

    // You'd host this TwiML at a URL and pass it to Twilio
    let auth = base64::encode(format!(
        "{}:{}",
        config.account_sid, config.auth_token
    ));

    let response = client
        .post(&format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Calls.json",
            config.account_sid
        ))
        .header("Authorization", format!("Basic {}", auth))
        .form(&[
            ("To", to),
            ("From", &config.from_number),
            ("Twiml", &twiml),
        ])
        .send()
        .await?
        .text()
        .await?;

    Ok(response)
}
```

**Handling call responses:**

```rust
// This would be an endpoint in your web server
async fn handle_call_response(
    Path(task_id): Path<String>,
    Form(params): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let digit = params.get("Digits").map(|s| s.as_str());

    let response_twiml = match digit {
        Some("1") => {
            // Claim the task
            claim_task(&task_id, "phone-caller").await;
            r#"<Response><Say>Task claimed. Thank you.</Say></Response>"#
        }
        Some("2") => {
            // Snooze - reschedule notification
            snooze_notification(&task_id, Duration::minutes(30)).await;
            r#"<Response><Say>Snoozed for 30 minutes.</Say></Response>"#
        }
        Some("3") => {
            // Dismiss
            r#"<Response><Say>Dismissed. Goodbye.</Say></Response>"#
        }
        _ => {
            r#"<Response><Say>Invalid selection. Goodbye.</Say></Response>"#
        }
    };

    (
        StatusCode::OK,
        [("Content-Type", "application/xml")],
        response_twiml,
    )
}
```

**Pros:**
- Maximum attention-getting
- Works when person is away from all screens
- Can handle responses via DTMF tones
- Professional text-to-speech available

**Cons:**
- Most expensive option (~$0.02-0.03/minute)
- Highly invasive - use sparingly!
- Requires webhook server for responses
- May annoy users if overused

### 1.6 Chat Platforms

#### Slack Webhooks

The most common integration for team notifications.

```rust
use reqwest::Client;
use serde_json::json;

async fn notify_slack(
    webhook_url: &str,
    task: &Task,
    reason: &str,
    actor_slack_id: Option<&str>,
) -> Result<(), reqwest::Error> {
    let client = Client::new();

    let mention = actor_slack_id
        .map(|id| format!("<@{}> ", id))
        .unwrap_or_default();

    client.post(webhook_url)
        .json(&json!({
            "blocks": [
                {
                    "type": "header",
                    "text": {
                        "type": "plain_text",
                        "text": "Task Needs Attention"
                    }
                },
                {
                    "type": "section",
                    "text": {
                        "type": "mrkdwn",
                        "text": format!(
                            "{}*{}*\n\n{}",
                            mention, task.title, reason
                        )
                    }
                },
                {
                    "type": "actions",
                    "elements": [
                        {
                            "type": "button",
                            "text": {"type": "plain_text", "text": "Claim Task"},
                            "action_id": format!("claim_{}", task.id),
                            "style": "primary"
                        },
                        {
                            "type": "button",
                            "text": {"type": "plain_text", "text": "View Details"},
                            "action_id": format!("view_{}", task.id)
                        },
                        {
                            "type": "button",
                            "text": {"type": "plain_text", "text": "Snooze"},
                            "action_id": format!("snooze_{}", task.id)
                        }
                    ]
                }
            ]
        }))
        .send()
        .await?;

    Ok(())
}
```

#### Slack App (Interactive)

For full two-way communication, you need a Slack app with interactive components:

```rust
// Slack sends POST to your endpoint when button is clicked
async fn handle_slack_interaction(
    Json(payload): Json<SlackInteractionPayload>,
) -> impl IntoResponse {
    let action = &payload.actions[0];
    let action_id = &action.action_id;

    if action_id.starts_with("claim_") {
        let task_id = action_id.strip_prefix("claim_").unwrap();
        let user = &payload.user.username;

        // Claim the task
        claim_task(task_id, user).await?;

        // Update the original message
        json!({
            "response_type": "in_channel",
            "replace_original": true,
            "text": format!("Task claimed by @{}", user)
        })
    } else {
        json!({"response_type": "ephemeral", "text": "Unknown action"})
    }
}
```

#### Discord Webhooks

Similar to Slack, but with Discord's embed format:

```rust
async fn notify_discord(
    webhook_url: &str,
    task: &Task,
    reason: &str,
) -> Result<(), reqwest::Error> {
    let client = Client::new();

    client.post(webhook_url)
        .json(&json!({
            "embeds": [{
                "title": format!("Task Needs Attention: {}", task.title),
                "description": reason,
                "color": 15158332, // Red
                "fields": [
                    {"name": "Task ID", "value": task.id, "inline": true},
                    {"name": "Status", "value": format!("{:?}", task.status), "inline": true}
                ],
                "footer": {"text": "Workgraph Notification"}
            }]
        }))
        .send()
        .await?;

    Ok(())
}
```

#### Discord Bot (Interactive)

For commands and reactions:

```rust
use serenity::prelude::*;
use serenity::model::channel::Message;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content.starts_with("!claim") {
            let parts: Vec<&str> = msg.content.split_whitespace().collect();
            if parts.len() >= 2 {
                let task_id = parts[1];
                let user = &msg.author.name;

                match claim_task(task_id, user).await {
                    Ok(_) => {
                        msg.reply(&ctx.http, format!("Claimed task: {}", task_id))
                            .await.ok();
                    }
                    Err(e) => {
                        msg.reply(&ctx.http, format!("Error: {}", e))
                            .await.ok();
                    }
                }
            }
        }
    }
}
```

**Cargo.toml:**

```toml
[dependencies]
serenity = "0.12"
```

#### Matrix

Open-source, self-hostable alternative:

```rust
use matrix_sdk::{Client, config::SyncSettings, ruma::events::room::message::RoomMessageEventContent};

async fn notify_matrix(
    client: &Client,
    room_id: &str,
    task: &Task,
    reason: &str,
) -> Result<(), matrix_sdk::Error> {
    let room = client.get_room(room_id.try_into()?)?;

    let content = RoomMessageEventContent::text_plain(format!(
        "**Task Needs Attention**\n\n*{}*\n\n{}\n\nTask ID: `{}`",
        task.title, reason, task.id
    ));

    room.send(content).await?;
    Ok(())
}
```

#### Telegram Bot

```rust
use teloxide::prelude::*;

async fn notify_telegram(
    bot: &Bot,
    chat_id: i64,
    task: &Task,
    reason: &str,
) -> Result<(), teloxide::RequestError> {
    let message = format!(
        "*Task Needs Attention*\n\n*{}*\n\n{}\n\n/claim\\_{}\n/view\\_{}",
        escape_markdown(&task.title),
        escape_markdown(reason),
        task.id,
        task.id
    );

    bot.send_message(ChatId(chat_id), message)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .await?;

    Ok(())
}
```

**Cargo.toml:**

```toml
[dependencies]
teloxide = { version = "0.12", features = ["macros"] }
```

**Pros of chat platforms:**
- Users already live in these apps
- Rich formatting and interactivity
- Thread-based discussions possible
- Can mention specific users
- Free or low cost

**Cons:**
- Requires app/bot setup
- Webhook URLs are sensitive credentials
- Different APIs for each platform
- Message limits and rate limits

### 1.7 Push Notifications

#### Mobile Apps

Building a native mobile app is significant effort, but push notifications are highly effective.

**Services:**
- Firebase Cloud Messaging (FCM) - Android and iOS
- Apple Push Notification Service (APNs) - iOS only
- OneSignal - unified API

```rust
// Using Firebase Admin SDK
async fn send_push_notification(
    device_token: &str,
    task: &Task,
    reason: &str,
) -> Result<(), reqwest::Error> {
    let client = Client::new();

    client.post("https://fcm.googleapis.com/fcm/send")
        .header("Authorization", format!("key={}", FCM_SERVER_KEY))
        .json(&json!({
            "to": device_token,
            "notification": {
                "title": "Task Needs Attention",
                "body": format!("{}: {}", task.title, reason),
                "click_action": format!("workgraph://task/{}", task.id)
            },
            "data": {
                "task_id": task.id,
                "action": "summon"
            }
        }))
        .send()
        .await?;

    Ok(())
}
```

#### Progressive Web App (PWA)

PWAs can receive push notifications without a native app:

```javascript
// Service worker registration and push subscription
async function subscribeToPush() {
    const registration = await navigator.serviceWorker.register('/sw.js');
    const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(VAPID_PUBLIC_KEY)
    });

    // Send subscription to backend
    await fetch('/api/push-subscribe', {
        method: 'POST',
        body: JSON.stringify(subscription),
        headers: { 'Content-Type': 'application/json' }
    });
}
```

Backend with web-push crate:

```rust
use web_push::{
    WebPushClient, WebPushMessageBuilder, SubscriptionInfo, VapidSignatureBuilder,
};

async fn send_web_push(
    subscription: &SubscriptionInfo,
    task: &Task,
) -> Result<(), web_push::WebPushError> {
    let client = WebPushClient::new()?;

    let sig_builder = VapidSignatureBuilder::from_pem(
        include_bytes!("../private_key.pem"),
        subscription,
    )?
    .build()?;

    let mut builder = WebPushMessageBuilder::new(subscription)?;
    builder.set_payload(
        web_push::ContentEncoding::Aes128Gcm,
        serde_json::to_string(&json!({
            "title": "Task Needs Attention",
            "body": task.title,
            "data": {"task_id": task.id}
        }))?.as_bytes(),
    );
    builder.set_vapid_signature(sig_builder);

    client.send(builder.build()?).await?;
    Ok(())
}
```

**Cargo.toml:**

```toml
[dependencies]
web-push = "0.10"
```

**Pros:**
- Very high visibility
- Works on mobile without native app (PWA)
- Can include action buttons
- Cross-platform

**Cons:**
- Requires HTTPS and service workers (PWA)
- User must grant permission
- Can be battery-draining if overused
- Setup complexity

---

## 2. Integration Patterns

### 2.1 Requesting Human Attention

How does a task "summon" a human? Several patterns:

#### Pattern A: Explicit Summon Field

Task explicitly declares it needs human input:

```json
{
  "id": "review-architecture",
  "kind": "task",
  "title": "Review architecture proposal",
  "status": "open",
  "needs_human": true,
  "human_reason": "Senior engineer approval required",
  "notify_via": ["slack", "email"],
  "assigned": "erik"
}
```

#### Pattern B: Tag-based Triggers

Certain tags automatically trigger notifications:

```json
{
  "id": "deploy-prod",
  "kind": "task",
  "title": "Deploy to production",
  "tags": ["needs-approval", "urgent"]
}
```

Configuration:

```toml
# .wg/config.toml
[notifications.triggers]
"needs-approval" = ["slack:@reviewers", "email"]
"urgent" = ["sms", "slack:@oncall"]
"security" = ["slack:#security", "email:security@company.com"]
```

#### Pattern C: Agent-initiated Summons

Agents can request human attention through a command:

```bash
# Agent calls this when stuck
wg summon review-architecture --reason "Need decision on database choice" --via slack
```

```rust
// CLI implementation
fn summon_command(
    task_id: &str,
    reason: &str,
    via: Vec<NotificationChannel>,
) -> Result<()> {
    let task = wg.get_task(task_id)?;

    for channel in via {
        match channel {
            NotificationChannel::Slack => notify_slack(task, reason).await?,
            NotificationChannel::Email => notify_email(task, reason).await?,
            NotificationChannel::Sms => notify_sms(task, reason).await?,
            // ...
        }
    }

    // Record the summons
    wg.add_event(task_id, Event::Summoned {
        reason: reason.to_string(),
        via: via.clone(),
        at: Utc::now(),
    })?;

    Ok(())
}
```

### 2.2 Escalation Policies

Multi-tier notification with escalation:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationPolicy {
    pub name: String,
    pub levels: Vec<EscalationLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationLevel {
    pub after_minutes: u32,
    pub channels: Vec<NotificationChannel>,
    pub repeat_every_minutes: Option<u32>,
}

// Example policy
let urgent_policy = EscalationPolicy {
    name: "urgent".to_string(),
    levels: vec![
        EscalationLevel {
            after_minutes: 0,
            channels: vec![
                NotificationChannel::Slack { webhook: "...".into() },
                NotificationChannel::Desktop,
            ],
            repeat_every_minutes: None,
        },
        EscalationLevel {
            after_minutes: 15,
            channels: vec![
                NotificationChannel::Sms { to: "+1...".into() },
            ],
            repeat_every_minutes: Some(15),
        },
        EscalationLevel {
            after_minutes: 60,
            channels: vec![
                NotificationChannel::PhoneCall { to: "+1...".into() },
            ],
            repeat_every_minutes: Some(30),
        },
    ],
};
```

Escalation runner:

```rust
async fn run_escalation(
    task_id: &str,
    policy: &EscalationPolicy,
    started_at: DateTime<Utc>,
) {
    loop {
        let elapsed = Utc::now() - started_at;
        let elapsed_minutes = elapsed.num_minutes() as u32;

        // Check if task has been acknowledged
        if is_acknowledged(task_id).await {
            break;
        }

        // Find applicable escalation level
        let level = policy.levels.iter()
            .filter(|l| l.after_minutes <= elapsed_minutes)
            .last();

        if let Some(level) = level {
            // Check if we should send (based on repeat interval)
            if should_send_at_level(task_id, level, elapsed_minutes).await {
                for channel in &level.channels {
                    send_notification(channel, task_id).await;
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
```

### 2.3 Acknowledgment

Human confirms they've seen the notification:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acknowledgment {
    pub task_id: String,
    pub actor_id: String,
    pub acknowledged_at: DateTime<Utc>,
    pub response: AckResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AckResponse {
    LookingAtIt,      // Human is reviewing
    WillHandle,       // Human claims responsibility
    Snoozed(Duration), // Remind me later
    Delegated(String), // Passing to someone else
    Dismissed,        // Not my problem
}

// CLI command
// wg ack review-architecture --response looking
fn acknowledge_command(task_id: &str, response: AckResponse) -> Result<()> {
    wg.acknowledge(task_id, current_actor(), response)?;

    // Stop escalation if appropriate
    if matches!(response, AckResponse::LookingAtIt | AckResponse::WillHandle) {
        stop_escalation(task_id).await;
    }

    Ok(())
}
```

### 2.4 Complete Example Flow

```
1. Agent hits a blocker while working on task "implement-auth"

2. Agent creates summons:
   $ wg summon implement-auth --reason "Need decision: OAuth vs SAML?" --via slack

3. Slack notification sent to #dev-team:
   "Task Needs Attention: implement-auth
    Need decision: OAuth vs SAML?
    [Claim] [Snooze] [View]"

4. No response after 15 minutes -> SMS sent to assigned human

5. Human receives SMS, opens Slack, clicks "Claim"

6. Workgraph updates:
   - Task acknowledged
   - Escalation stopped
   - Human marked as actively reviewing

7. Human makes decision, adds comment via Slack thread

8. Agent sees comment (via webhook or polling), continues work
```

---

## 3. Two-way Communication

### 3.1 SMS Replies

Using Twilio's webhook for incoming SMS:

```rust
// Twilio sends POST when someone replies to your number
async fn handle_incoming_sms(
    Form(params): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let from = params.get("From").unwrap();
    let body = params.get("Body").unwrap().to_uppercase();

    // Find the most recent summons to this number
    let recent_summons = get_recent_summons_for_phone(from).await;

    let response = match body.trim() {
        "YES" | "CLAIM" | "1" => {
            if let Some(summons) = recent_summons {
                claim_task(&summons.task_id, from).await?;
                "Task claimed. Check Slack for details."
            } else {
                "No pending task found."
            }
        }
        "NO" | "SKIP" | "2" => {
            if let Some(summons) = recent_summons {
                dismiss_summons(&summons.id).await?;
                "Dismissed."
            } else {
                "No pending task found."
            }
        }
        "SNOOZE" | "LATER" | "3" => {
            if let Some(summons) = recent_summons {
                snooze_summons(&summons.id, Duration::minutes(30)).await?;
                "Snoozed for 30 minutes."
            } else {
                "No pending task found."
            }
        }
        _ => {
            // Treat as a comment on the task
            if let Some(summons) = recent_summons {
                add_comment(&summons.task_id, from, &body).await?;
                "Comment added to task."
            } else {
                "Reply YES to claim, NO to skip, or SNOOZE to delay."
            }
        }
    };

    // TwiML response
    (
        StatusCode::OK,
        [("Content-Type", "application/xml")],
        format!(r#"<?xml version="1.0"?><Response><Message>{}</Message></Response>"#, response),
    )
}
```

### 3.2 Slack Thread as Task Discussion

When someone replies in a Slack thread, it becomes a task comment:

```rust
// Slack Events API handler
async fn handle_slack_event(
    Json(event): Json<SlackEvent>,
) -> impl IntoResponse {
    match event {
        SlackEvent::Message { channel, thread_ts, user, text, .. } => {
            // Check if this thread corresponds to a task notification
            if let Some(task_id) = get_task_for_thread(&channel, &thread_ts).await {
                // Add as comment
                let username = resolve_slack_user(&user).await;
                add_comment(&task_id, &username, &text).await?;

                // Optionally notify the agent watching this task
                notify_task_watchers(&task_id, &format!(
                    "New comment from {}: {}", username, text
                )).await;
            }
        }
        _ => {}
    }

    StatusCode::OK
}
```

### 3.3 Voice Response (IVR)

For phone calls, Twilio's TwiML allows gathering DTMF tones:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<Response>
    <Say voice="alice">
        This is Workgraph calling about task: Approve budget for Q3 marketing.
        The agent requests your approval.
    </Say>
    <Gather numDigits="1" action="/voice-response/task-123">
        <Say>
            Press 1 to approve.
            Press 2 to reject.
            Press 3 to request more information.
            Press 9 to snooze for one hour.
        </Say>
    </Gather>
    <Say>No input received. Goodbye.</Say>
</Response>
```

Speech recognition is also possible:

```xml
<Gather input="speech dtmf" action="/voice-response/task-123"
        speechTimeout="auto" language="en-US">
    <Say>You can also say approve, reject, or snooze.</Say>
</Gather>
```

### 3.4 Email Replies

Parsing email replies is complex but possible:

```rust
// Using a webhook service like SendGrid Inbound Parse
async fn handle_inbound_email(
    Form(email): Form<InboundEmail>,
) -> impl IntoResponse {
    // Extract task ID from email subject or a reply-to address
    let task_id = extract_task_id_from_subject(&email.subject)
        .or_else(|| extract_task_id_from_address(&email.to));

    if let Some(task_id) = task_id {
        // Strip quoted text and signatures
        let clean_body = strip_email_reply_artifacts(&email.text);

        // Parse for commands
        let first_line = clean_body.lines().next().unwrap_or("").trim().to_uppercase();

        match first_line.as_str() {
            "APPROVED" | "APPROVE" | "YES" => {
                approve_task(&task_id, &email.from).await?;
            }
            "REJECTED" | "REJECT" | "NO" => {
                reject_task(&task_id, &email.from).await?;
            }
            _ => {
                // Treat entire body as a comment
                add_comment(&task_id, &email.from, &clean_body).await?;
            }
        }
    }

    StatusCode::OK
}
```

---

## 4. Privacy and Security

### 4.1 Rate Limiting

Prevent notification spam:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct RateLimiter {
    limits: Mutex<HashMap<String, Vec<Instant>>>,
    max_per_hour: usize,
    max_per_day: usize,
}

impl RateLimiter {
    fn check(&self, user_id: &str) -> Result<(), RateLimitError> {
        let mut limits = self.limits.lock().unwrap();
        let now = Instant::now();

        let history = limits.entry(user_id.to_string()).or_default();

        // Remove old entries
        history.retain(|t| now.duration_since(*t) < Duration::from_secs(86400));

        // Check hourly limit
        let last_hour = history.iter()
            .filter(|t| now.duration_since(**t) < Duration::from_secs(3600))
            .count();

        if last_hour >= self.max_per_hour {
            return Err(RateLimitError::HourlyLimitExceeded);
        }

        // Check daily limit
        if history.len() >= self.max_per_day {
            return Err(RateLimitError::DailyLimitExceeded);
        }

        // Record this notification
        history.push(now);

        Ok(())
    }
}
```

### 4.2 Quiet Hours

Respect user preferences:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreferences {
    pub quiet_hours: Option<QuietHours>,
    pub preferred_channels: Vec<NotificationChannel>,
    pub emergency_override: bool, // Allow critical notifications during quiet hours
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuietHours {
    pub start: NaiveTime, // e.g., 22:00
    pub end: NaiveTime,   // e.g., 08:00
    pub timezone: String, // e.g., "America/Los_Angeles"
}

fn is_quiet_time(prefs: &NotificationPreferences) -> bool {
    if let Some(quiet) = &prefs.quiet_hours {
        let tz: Tz = quiet.timezone.parse().unwrap();
        let now = Utc::now().with_timezone(&tz).time();

        if quiet.start > quiet.end {
            // Spans midnight (e.g., 22:00 - 08:00)
            now >= quiet.start || now < quiet.end
        } else {
            now >= quiet.start && now < quiet.end
        }
    } else {
        false
    }
}

async fn send_notification_with_prefs(
    user: &Actor,
    task: &Task,
    urgency: Urgency,
) -> Result<()> {
    let prefs = get_notification_preferences(&user.id).await?;

    // Check quiet hours
    if is_quiet_time(&prefs) && urgency != Urgency::Critical {
        if !prefs.emergency_override {
            // Queue for later
            queue_notification(user, task, prefs.quiet_hours.end).await?;
            return Ok(());
        }
    }

    // Check rate limits
    rate_limiter.check(&user.id)?;

    // Send via preferred channels
    for channel in &prefs.preferred_channels {
        send_via_channel(channel, user, task).await?;
    }

    Ok(())
}
```

### 4.3 Authentication for Responses

Verify that responses actually come from the intended recipient:

```rust
// For SMS: verify the phone number matches
async fn verify_sms_sender(from: &str, task_id: &str) -> Result<bool> {
    let task = get_task(task_id).await?;
    let assigned_actor = get_actor(&task.assigned.unwrap()).await?;

    Ok(assigned_actor.phone.as_deref() == Some(from))
}

// For Slack: verify workspace and user
fn verify_slack_request(
    headers: &HeaderMap,
    body: &[u8],
    signing_secret: &str,
) -> bool {
    let timestamp = headers.get("X-Slack-Request-Timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let signature = headers.get("X-Slack-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Check timestamp is recent (prevent replay attacks)
    let ts: i64 = timestamp.parse().unwrap_or(0);
    let now = Utc::now().timestamp();
    if (now - ts).abs() > 300 {
        return false;
    }

    // Verify signature
    let sig_basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));
    let expected = hmac_sha256(signing_secret.as_bytes(), sig_basestring.as_bytes());
    let expected_signature = format!("v0={}", hex::encode(expected));

    constant_time_eq(signature.as_bytes(), expected_signature.as_bytes())
}
```

### 4.4 Secure Credential Storage

```rust
// Use environment variables or secret management
struct NotificationConfig {
    slack_webhook: SecretString,
    twilio_auth_token: SecretString,
    sendgrid_api_key: SecretString,
}

impl NotificationConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            slack_webhook: SecretString::new(env::var("WORKGRAPH_SLACK_WEBHOOK")?),
            twilio_auth_token: SecretString::new(env::var("TWILIO_AUTH_TOKEN")?),
            sendgrid_api_key: SecretString::new(env::var("SENDGRID_API_KEY")?),
        })
    }
}

// Alternatively, read from encrypted config file
// Or integrate with HashiCorp Vault, AWS Secrets Manager, etc.
```

---

## 5. Data Model Changes

### 5.1 Task Extensions

Add notification-related fields to the Task struct:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned: Option<String>,

    // ... existing fields ...

    // NEW: Human summoning fields
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_human: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notify_via: Vec<String>, // ["slack", "email", "sms"]

    #[serde(skip_serializing_if = "Option::is_none")]
    pub escalation_policy: Option<String>, // Reference to policy name

    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<String>, // ISO 8601 timestamp

    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_by: Option<String>, // Actor ID
}

fn is_false(b: &bool) -> bool { !*b }
```

### 5.2 Actor Extensions

Add notification preferences to Actor:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    // ... existing fields ...

    // NEW: Contact information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub slack_id: Option<String>, // For @mentions

    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_id: Option<String>,

    // NEW: Notification preferences
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_prefs: Option<NotificationPrefs>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationPrefs {
    #[serde(default)]
    pub preferred_channels: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_start: Option<String>, // "22:00"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_end: Option<String>, // "08:00"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}
```

### 5.3 New Node Type: Notification Config

Could add a configuration node:

```json
{
  "kind": "config",
  "id": "notifications",
  "slack_webhook": "https://hooks.slack.com/...",
  "default_channels": ["slack", "desktop"],
  "escalation_policies": {
    "urgent": {
      "levels": [
        {"after_minutes": 0, "channels": ["slack", "desktop"]},
        {"after_minutes": 15, "channels": ["sms"]},
        {"after_minutes": 60, "channels": ["phone"]}
      ]
    },
    "normal": {
      "levels": [
        {"after_minutes": 0, "channels": ["slack"]},
        {"after_minutes": 60, "channels": ["email"]}
      ]
    }
  }
}
```

### 5.4 Example JSONL with Summoning

```jsonl
{"kind":"actor","id":"erik","name":"Erik","email":"erik@example.com","phone":"+1555123456","slack_id":"U12345","notification_prefs":{"preferred_channels":["slack","sms"],"quiet_start":"22:00","quiet_end":"08:00","timezone":"America/Los_Angeles"}}
{"kind":"actor","id":"claude","name":"Claude (Agent)","role":"ai-agent"}
{"kind":"task","id":"review-security","title":"Review security audit findings","status":"open","assigned":"erik","needs_human":true,"human_reason":"Senior engineer approval required for security changes","notify_via":["slack","email"],"escalation_policy":"urgent"}
{"kind":"task","id":"implement-fix","title":"Implement security fixes","status":"open","blocked_by":["review-security"],"assigned":"claude"}
```

---

## 6. Practical Recommendations

### 6.1 Simplest Useful Thing: Slack/Discord Webhook

**Effort:** ~2-4 hours
**Value:** High for teams

Implementation steps:

1. Add `wg summon` command:
   ```bash
   wg summon <task-id> --reason "message" --via slack
   ```

2. Read webhook URL from environment:
   ```rust
   let webhook = env::var("WORKGRAPH_SLACK_WEBHOOK")?;
   ```

3. POST to webhook with task info

4. Optionally add `--mention @user` flag

This gives you immediate value with minimal infrastructure.

### 6.2 Coolest Thing: Phone Call with IVR

**Effort:** 1-2 weeks (including webhook server)
**Value:** High for critical notifications, impressive demo

Imagine:

```
*Phone rings*

"This is Workgraph. Task 'Deploy to production' requires your approval.
The deployment includes 3 database migrations and updates to the payment
system. Press 1 to approve. Press 2 to reject. Press 3 to schedule a
review call."

*User presses 1*

"Approved. The deployment will proceed. You'll receive a Slack message
when complete. Thank you."
```

This requires:
- Twilio account and phone number (~$1/month)
- Web server for TwiML endpoints
- Text-to-speech for dynamic task details
- Handling DTMF responses

### 6.3 Best Balance: Multi-channel with Escalation

**Effort:** 2-3 weeks
**Value:** Very high for production use

Architecture:

```
wg summon task-id --reason "reason"
    │
    ▼
┌───────────────────┐
│ Notification      │
│ Queue (tokio)     │
└───────────────────┘
    │
    ▼
┌───────────────────┐
│ Escalation        │
│ Manager           │
│                   │
│ Level 0: Slack    │──▶ [Check acknowledgment]
│ Level 1: SMS      │        │
│ Level 2: Phone    │        ▼
└───────────────────┘    [Stop if acked]
    │
    ▼
┌───────────────────┐
│ Channel Adapters  │
│ - SlackAdapter    │
│ - TwilioAdapter   │
│ - EmailAdapter    │
│ - DesktopAdapter  │
└───────────────────┘
```

### 6.4 Implementation Roadmap

**Phase 1: Foundation (Week 1)**
- Add `needs_human`, `notify_via` fields to Task
- Add contact fields to Actor
- Implement `wg summon` command with Slack webhook
- Implement `wg ack` command

**Phase 2: Multi-channel (Week 2)**
- Add desktop notifications via notify-rust
- Add email via lettre
- Add SMS via Twilio
- Implement channel preference logic

**Phase 3: Escalation (Week 3)**
- Implement escalation policies
- Add background escalation runner
- Add quiet hours support
- Add rate limiting

**Phase 4: Two-way (Week 4+)**
- Add Slack interactive buttons
- Add SMS reply handling
- Add webhook server for responses
- Optionally: phone calls with IVR

### 6.5 Configuration Example

```toml
# .wg/config.toml

[notifications]
default_channels = ["desktop", "slack"]

[notifications.slack]
webhook_url = "${WORKGRAPH_SLACK_WEBHOOK}"
default_channel = "#workgraph"

[notifications.email]
smtp_server = "smtp.gmail.com"
smtp_port = 587
from_address = "workgraph@yourdomain.com"

[notifications.twilio]
account_sid = "${TWILIO_ACCOUNT_SID}"
auth_token = "${TWILIO_AUTH_TOKEN}"
from_number = "+1555000000"

[notifications.escalation.urgent]
levels = [
    { after_minutes = 0, channels = ["desktop", "slack"] },
    { after_minutes = 15, channels = ["sms"] },
    { after_minutes = 60, channels = ["phone"], repeat_every_minutes = 30 }
]

[notifications.escalation.normal]
levels = [
    { after_minutes = 0, channels = ["slack"] },
    { after_minutes = 120, channels = ["email"] }
]

[notifications.rate_limits]
max_per_hour = 10
max_per_day = 50
```

### 6.6 CLI Commands Summary

```bash
# Summon a human for a task
wg summon <task-id> --reason "Need approval" [--via slack,sms] [--escalation urgent]

# Acknowledge a summons (stops escalation)
wg ack <task-id> [--response looking|claiming|snoozed|dismissed]

# Configure notification preferences for an actor
wg actor erik --email "erik@example.com" --phone "+1555123456" --slack-id "U12345"

# Set quiet hours
wg actor erik --quiet-hours "22:00-08:00" --timezone "America/Los_Angeles"

# List pending summons
wg summons [--mine] [--all]

# Test notification channels
wg notify-test --channel slack --to "#test-channel"
```

---

## Summary

Human summoning in workgraph bridges the gap between autonomous agent work and necessary human oversight. The key principles:

1. **Start simple** - Slack webhooks provide 80% of the value with 20% of the effort
2. **Escalate thoughtfully** - Not everything needs a phone call; use escalation policies
3. **Respect humans** - Rate limiting, quiet hours, and preferences matter
4. **Enable two-way communication** - Humans should be able to respond through the same channel
5. **Make it configurable** - Different teams and contexts need different approaches

The recommended implementation order:
1. Slack/Discord webhook (immediate value)
2. Desktop notifications (no cost, good for local dev)
3. Email (reliable fallback)
4. SMS (for urgent matters)
5. Phone calls (nuclear option)

With these mechanisms, workgraph can effectively coordinate work between humans and AI agents, escalating to humans when needed while respecting their time and attention.

---

## References

- [notify-rust crate](https://crates.io/crates/notify-rust)
- [lettre (Rust email)](https://crates.io/crates/lettre)
- [Twilio API Documentation](https://www.twilio.com/docs/usage/api)
- [Slack Webhooks](https://api.slack.com/messaging/webhooks)
- [Slack Block Kit](https://api.slack.com/block-kit)
- [Discord Webhooks](https://discord.com/developers/docs/resources/webhook)
- [SendGrid API](https://docs.sendgrid.com/api-reference/mail-send/mail-send)
- [web-push crate](https://crates.io/crates/web-push)
- [serenity (Discord bot)](https://crates.io/crates/serenity)
- [teloxide (Telegram bot)](https://crates.io/crates/teloxide)
