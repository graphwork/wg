# Human-in-the-Loop Integration: Messaging, Voice, and Notification Channels

Research report for wg — evaluating channels for bidirectional human-agent communication.

## Table of Contents

1. [Channel Comparison Matrix](#channel-comparison-matrix)
2. [Voice Capability Matrix](#voice-capability-matrix)
3. [Rust Crate Availability](#rust-crate-availability)
4. [Detailed Channel Assessments](#detailed-channel-assessments)
5. [Existing Matrix Code Assessment](#existing-matrix-code-assessment)
6. [Unified Abstraction Architecture](#unified-abstraction-architecture)
7. [Configuration Design](#configuration-design)
8. [Recommended Priority Order](#recommended-priority-order)

---

## Channel Comparison Matrix

| Channel | Setup Complexity | Bidirectional | Rich Interaction | Voice | Cost | Privacy | Bot Restrictions | Rust Crate Maturity |
|---------|-----------------|---------------|-----------------|-------|------|---------|------------------|-------------------|
| **Telegram** | Low | Yes | Inline keyboards, callbacks | No (bot API) | Free | Moderate | Minimal | Excellent |
| **Matrix** | Medium | Yes | HTML, reactions, replies | Yes (VoIP) | Free (self-host) | Excellent (E2EE) | None | Good |
| **Slack** | Medium | Yes | Block Kit, modals, buttons | Huddles (no API) | Free tier limited | Low | Moderate | Fair |
| **Discord** | Low-Medium | Yes | Buttons, menus, modals, threads | Yes (voice channels) | Free | Low | Moderate | Excellent |
| **WhatsApp** | High | Structured only | Buttons, templates | No | Per-message | Moderate | **Severe** | Poor |
| **Signal** | High | Hacky | Plain text only | No | Free | Excellent | **No bot API** | None |
| **Email** | Low | Yes (async) | HTML, attachments | No | Free | Moderate | Spam filters | Good |
| **SMS** | Low-Medium | Yes | Plain text (160 char) | No | Per-message | Low | Carrier filtering | Fair |
| **Phone/Voice** | Medium | Yes (real-time) | DTMF, speech | **Yes** | Per-minute | Low | Telecom regs | Fair |
| **WebRTC** | High | Yes (real-time) | Full multimedia | **Yes** | Free (self-host) | Good (P2P) | None | Evolving |
| **Push Notif.** | Medium | No (outbound) | Title + body + actions | No | Free | Moderate | Browser perms | Good |
| **Webhooks** | Low | Outbound* | JSON payloads | No | Free | Depends | None | Trivial |
| **RSS/Atom** | Low | No (pull) | XML | No | Free | Good | None | Good |

---

## Voice Capability Matrix

| Channel | Bot-Initiated Calls | Real-time Audio | TTS | STT | Rust Support |
|---------|--------------------|--------------------|-----|-----|-------------|
| **Twilio Voice** | Yes | Yes | Built-in | Built-in | Fair (HTTP API) |
| **Vonage** | Yes | Yes | Built-in | Built-in | None (HTTP API) |
| **Matrix VoIP** | Partial (Element Call) | Yes | External | External | Partial |
| **Telegram** | No (bots can't call) | N/A | N/A | N/A | N/A |
| **Discord** | Yes (voice channels) | Yes | External | External | Good (songbird) |
| **WebRTC** | Yes (browser) | Yes | External | External | Evolving (webrtc-rs) |

**Verdict:** Twilio Voice is the clear winner for bot-initiated calls. Discord for always-on channels. WebRTC for custom web UI.

---

## Rust Crate Availability

| Platform | Crate | Maturity | Notes |
|----------|-------|----------|-------|
| Telegram | `teloxide` | Excellent | Best Rust bot framework, async, dialogue system |
| Telegram | `frankenstein` | Good | Low-level 1:1 API mapping |
| Matrix | `matrix-sdk` | Good | Official SDK, E2EE, SQLite |
| Slack | `slack-rs-api` | Fair | Covers Web API, BYO HTTP client |
| Discord | `serenity` | Excellent | Batteries-included |
| Discord | `twilight` | Excellent | Modular, for advanced users |
| Discord Voice | `songbird` | Good | Works with serenity |
| WhatsApp | None | N/A | Must use HTTP API directly |
| Signal | None | N/A | signal-cli is Java |
| Email (send) | `lettre` | Excellent | Mature SMTP, async support |
| Email (recv) | `imap` | Fair | Sync only |
| SMS/Voice | `twilio` | Fair | Covers REST API |
| WebRTC | `webrtc` | Evolving | v0.20 rewrite in progress |
| Push | `web-push` | Good | VAPID, RFC8188 |
| Webhooks | `reqwest` | Excellent | Just HTTP POST |
| RSS | `rss`, `atom_syndication` | Good | Mature |

---

## Detailed Channel Assessments

### 1. Telegram

**Setup:** Message @BotFather, get token in 2 minutes. No server infrastructure needed.

**Strengths:**
- Inline keyboards with callback data — perfect for approval buttons (Approve/Reject/Claim)
- Group chats for team notifications
- 30 msg/sec rate limit (generous)
- `teloxide` is best-in-class Rust bot framework with dialogue state machines
- File/image sending for artifacts
- Markdown/HTML formatting

**Weaknesses:**
- No voice calls for bots (can send voice messages as audio files)
- Account tied to phone number
- No E2EE for bot messages

**Best for:** Personal notification channel, approval workflows, quick status checks.

### 2. Matrix (existing integration)

See [Existing Matrix Code Assessment](#existing-matrix-code-assessment) for detailed code review.

**Setup:** Create account on any homeserver (matrix.org, self-hosted). Get access token.

**Strengths:**
- Federated, self-hostable — full data sovereignty
- E2EE support (even for bots)
- VoIP via Element Call / Jitsi integration
- Already partially integrated in wg
- Open protocol, no vendor lock-in
- Rich HTML messages, reactions, threads

**Weaknesses:**
- Heavier SDK dependency (matrix-sdk adds ~60 compile-time deps)
- Homeserver setup is non-trivial for self-hosting
- Smaller user base than Telegram/Slack/Discord

**Best for:** Privacy-conscious users, self-hosted deployments, teams already using Element.

### 3. WhatsApp

**Setup:** Business API requires Meta Business verification. Must use a Cloud API or BSP.

**Strengths:**
- Massive user base (2B+ users)
- Template messages with buttons
- Session messages (24h window after user initiates)

**Weaknesses:**
- **As of Jan 2026: Meta bans general-purpose AI chatbots.** Structured bots for support/notifications are still allowed, but this creates legal risk for wg use cases.
- Per-message pricing ($0.005-$0.08 depending on region and template vs session)
- Must use pre-approved message templates for outbound
- 24-hour session window — humans must reply within 24h or you need a new template message
- No mature Rust crate
- Complex verification process

**Verdict:** **Not recommended.** High cost, severe restrictions, AI bot ban makes this risky. Only consider if users specifically need WhatsApp as a notification-only channel.

### 4. Slack

**Setup:** Create Slack App, configure OAuth scopes, install to workspace.

**Strengths:**
- Block Kit for rich interactive messages (buttons, dropdowns, date pickers, modals)
- Threaded conversations
- Workspace integration (many teams already use Slack)
- Slash commands (`/wg status`)
- Events API + Socket Mode for real-time bidirectional

**Weaknesses:**
- Free plan limits: 90 days message history, 10 app integrations
- Rate limits (1 msg/sec per channel)
- No mature batteries-included Rust crate (slack-rs-api is low-level)
- OAuth flow is complex for initial setup
- Employer-controlled workspace (privacy concern for personal use)

**Best for:** Teams already on Slack. Good Block Kit interactions.

### 5. Discord

**Setup:** Create application in Discord Developer Portal, create bot user, generate invite link.

**Strengths:**
- Excellent bot ecosystem
- Slash commands, buttons, select menus, modals
- Voice channels with programmatic join/leave (via songbird)
- Threads for per-task discussions
- Free, generous limits
- `serenity` and `twilight` are excellent Rust crates
- Can create private servers for project-specific use

**Weaknesses:**
- "Gaming platform" reputation (some orgs won't use it)
- Account required
- Messages stored on Discord servers
- Rate limits on bot actions (around 50/sec global)

**Best for:** Developer teams, voice channel monitoring, rich bot interactions.

### 6. Signal

**Setup:** No official bot API. Must use `signal-cli` (Java) or `signal-cli-rest-api` as a bridge.

**Strengths:**
- Best-in-class privacy (E2EE always on, minimal metadata)
- Open source protocol

**Weaknesses:**
- **No official bot API** — Signal explicitly does not support bots
- Requires running signal-cli (Java) as a subprocess or REST bridge
- No rich interactions (plain text only)
- Account tied to phone number
- No Rust crate whatsoever
- Fragile: signal-cli breaks frequently with protocol updates

**Verdict:** **Not recommended for primary integration.** Could be supported as a hacky notification-only channel via signal-cli REST bridge, but too fragile to rely on.

### 7. Email (SMTP/IMAP)

**Setup:** SMTP credentials for sending, IMAP for receiving. Works with any email provider.

**Strengths:**
- Universal — everyone has email
- Async by nature — good for non-urgent notifications
- HTML formatting, attachments
- `lettre` is a mature, production-ready SMTP crate
- No account creation needed (use existing email)
- Good for daily/weekly digests

**Weaknesses:**
- High latency (minutes to hours for replies)
- Spam filter risk
- IMAP polling for incoming is clunky
- No real-time interaction
- `imap` crate is sync-only

**Best for:** Daily digests, escalation notifications, async approvals.

### 8. SMS (Twilio)

**Setup:** Twilio account + phone number ($1/mo + per-message).

**Strengths:**
- Reaches anyone with a phone, no app needed
- Good for urgent notifications / escalation
- Bidirectional (humans reply to the number)

**Weaknesses:**
- 160 character limit
- Per-message cost ($0.0079 outbound US)
- No rich interaction
- Carrier filtering may block automated messages
- `twilio` crate is basic

**Best for:** Urgent escalation ("Task X is blocked for 2h, needs human input").

### 9. Phone Calls (Twilio Voice)

**Setup:** Twilio account + phone number, TwiML for call scripts.

**Strengths:**
- Most urgent notification possible — phone rings
- TTS built in (Say verb)
- Speech recognition built in (Gather verb)
- DTMF input (press 1 to approve, 2 to reject)
- Conference calls possible

**Weaknesses:**
- Per-minute cost ($0.013+ outbound US)
- Requires TwiML server (webhook endpoint)
- Complex flow design
- Telecom regulations vary by country
- Users may not answer unknown numbers

**Best for:** Critical escalation, "the build is on fire" scenarios.

### 10. WebRTC

**Setup:** Signaling server, STUN/TURN servers, web frontend.

**Strengths:**
- Real-time audio/video in browser
- No phone number needed
- P2P possible (good privacy)
- Full multimedia support

**Weaknesses:**
- High implementation complexity
- Requires web UI component
- `webrtc-rs` is in flux (v0.20 major rewrite)
- Need signaling infrastructure
- Browser-only (no mobile push)

**Best for:** Future web UI integration, not a priority for CLI-first tool.

### 11. Push Notifications

**Setup:** Generate VAPID keys, register service worker in web frontend.

**Strengths:**
- Works across browsers and mobile
- No app installation beyond browser
- Free
- `web-push` crate works well

**Weaknesses:**
- Requires web frontend / PWA
- Outbound only (no replies)
- User must grant permission
- Delivery not guaranteed (browser must be running for desktop)

**Best for:** Complement to a web UI.

### 12. Webhooks

**Setup:** None (just an HTTP endpoint).

**Strengths:**
- Universal integration point
- Can connect to Zapier, n8n, IFTTT, custom systems
- Trivial to implement (just `reqwest::post`)
- JSON payloads with full task data

**Weaknesses:**
- Outbound only (unless paired with inbound webhook endpoint)
- No built-in delivery guarantees
- Requires the receiver to exist

**Best for:** Integration glue. Should be implemented regardless — it's the universal fallback.

### 13. RSS/Atom

**Setup:** Serve a feed file or endpoint.

**Strengths:**
- Universal, standard protocol
- Users choose their own reader
- Zero setup for consumers
- Good for public project status

**Weaknesses:**
- Pull-only, no push
- No interaction
- Polling-based (latency depends on reader)

**Best for:** Public status feeds, low priority.

---

## Existing Matrix Code Assessment

### Overview

The codebase has **two parallel Matrix implementations** (~3,000 lines total):

1. **`src/matrix/`** — Full SDK implementation using `matrix-sdk` (behind `matrix` feature flag)
   - `mod.rs` (577 lines): `MatrixClient` wrapping `matrix-sdk::Client`
   - `listener.rs` (225 lines): `MatrixListener` with room-based command dispatch

2. **`src/matrix_lite/`** — Lightweight HTTP implementation using `reqwest` (behind `matrix-lite` feature flag, **default**)
   - `mod.rs` (608 lines): `MatrixClient` using raw HTTP calls to Matrix CS API
   - `listener.rs` (167 lines): Same listener pattern, lighter dependencies

3. **`src/matrix_commands.rs`** (786 lines): Shared command parser and executor — parses `claim`, `done`, `fail`, `input`, `unclaim`, `status`, `ready`, `help` from Matrix messages and executes against the graph.

4. **`src/commands/matrix.rs`** (244 lines): CLI subcommands — `wg matrix listen`, `wg matrix send`, `wg matrix status`, `wg matrix login`, `wg matrix logout`.

5. **`src/commands/notify.rs`** (411 lines): `wg notify <task>` — sends formatted task notification to Matrix room with HTML.

### State Assessment

**What works well:**
- Clean dual-implementation pattern (feature flags for heavy vs lite)
- Shared command parser (`matrix_commands.rs`) is well-tested (20+ unit tests)
- Command set covers the essential operations (claim/done/fail/input/unclaim/status/ready)
- `notify.rs` produces well-formatted HTML notifications with status emojis and action hints
- Session persistence (access token caching, sync token caching)
- The lite implementation is surprisingly complete — sync, send, receive all work via raw HTTP

**Issues found:**
1. **No trait abstraction** — `MatrixClient` in `matrix/` and `matrix_lite/` are separate structs with identical method names but no shared trait. CLI code uses `#[cfg]` to swap between them. This makes it impossible to write generic code over both implementations.
2. **Listener has a subtle race** — In `matrix_lite/listener.rs`, `sync_once_with_filter` and `rx.recv()` are in `tokio::select!`, but since `sync_once_with_filter` is what populates `rx`, they'll never both be ready simultaneously. The select is effectively sequential. This isn't broken but is misleading.
3. **No reconnection logic** — If the Matrix server goes down, both listeners will print an error and retry after 5s, but there's no exponential backoff or session re-authentication.
4. **`uuid_v4_simple` is not a real UUID** — It XORs PID with nanoseconds. Fine for device IDs but the name is misleading.
5. **The full SDK version pins `matrix-sdk = 0.16`** — matrix-sdk is now at 0.10+ with major API changes from the 0.7+ era. Version 0.16 doesn't exist on crates.io (latest is 0.10.x as of early 2026). This means the full SDK path likely doesn't compile against released crate versions — it may be pointing at a git dependency or is simply broken.
6. **No typing indicators or read receipts** — Would improve UX for interactive sessions.
7. **Command set is limited** — No `retry`, `add`, `show`, `viz` commands from Matrix.

**The user's assessment ("totally messed up") likely refers to:**
- The dual implementation complexity
- Possible compilation issues with the matrix-sdk version
- The lack of a unified trait

**Recommendation:** The lite implementation is solid and should be the foundation. Introduce a `NotificationChannel` trait (see architecture section) that the lite client implements, then add other channels incrementally.

---

## Unified Abstraction Architecture

### Core Trait

```rust
/// A channel that can send messages to humans and optionally receive responses.
#[async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Unique identifier for this channel type
    fn channel_type(&self) -> &str;

    /// Send a plain text message
    async fn send_text(&self, target: &str, message: &str) -> Result<MessageId>;

    /// Send a rich/formatted message (HTML, markdown, or platform-native)
    async fn send_rich(&self, target: &str, message: &RichMessage) -> Result<MessageId>;

    /// Send a message with action buttons (approve/reject/etc.)
    /// Returns None if the channel doesn't support actions.
    async fn send_with_actions(
        &self,
        target: &str,
        message: &str,
        actions: &[Action],
    ) -> Result<MessageId>;

    /// Whether this channel supports receiving messages
    fn supports_receive(&self) -> bool;

    /// Start listening for incoming messages (if supported)
    /// Returns a stream of incoming messages.
    async fn listen(&self) -> Result<Box<dyn Stream<Item = IncomingMessage> + Send>>;
}

/// A message with formatting
pub struct RichMessage {
    pub plain_text: String,
    pub html: Option<String>,
    pub markdown: Option<String>,
}

/// An action button
pub struct Action {
    pub id: String,
    pub label: String,
    pub style: ActionStyle, // Primary, Danger, Secondary
}

pub enum ActionStyle { Primary, Danger, Secondary }

/// Identifies a sent message for threading/replies
pub struct MessageId(pub String);

/// An incoming message from a human
pub struct IncomingMessage {
    pub channel: String,
    pub sender: String,
    pub body: String,
    pub action_id: Option<String>, // If they clicked a button
    pub reply_to: Option<MessageId>,
}
```

### Router Layer

```rust
/// Routes messages to configured channels based on priority and type.
pub struct NotificationRouter {
    channels: Vec<Box<dyn NotificationChannel>>,
    rules: Vec<RoutingRule>,
}

pub struct RoutingRule {
    /// Match criteria
    pub event_type: EventType, // TaskReady, TaskBlocked, TaskFailed, Approval, Urgent
    /// Which channels to send to (in priority order)
    pub channels: Vec<String>,
    /// Escalation: if no response in N seconds, try next channel
    pub escalation_timeout: Option<Duration>,
}
```

### Escalation Chain Example

```
TaskBlocked → Telegram (immediate)
  → no response 30min → SMS
  → no response 1h → Phone call
```

### File Layout

```
src/
  notify/
    mod.rs          -- NotificationChannel trait, RichMessage, Router
    telegram.rs     -- TelegramChannel impl
    matrix.rs       -- MatrixChannel impl (wraps existing lite client)
    slack.rs        -- SlackChannel impl
    discord.rs      -- DiscordChannel impl
    email.rs        -- EmailChannel impl
    sms.rs          -- SmsChannel impl (Twilio)
    voice.rs        -- VoiceChannel impl (Twilio Voice)
    webhook.rs      -- WebhookChannel impl
    push.rs         -- PushChannel impl
```

---

## Configuration Design

### Global notification config (`~/.config/wg/notify.toml`)

```toml
# Default channels for different event types
[routing]
default = ["telegram"]
urgent = ["telegram", "sms"]       # escalation chain
approval = ["telegram"]
digest = ["email"]

# Escalation timeouts (seconds)
[escalation]
approval_timeout = 1800    # 30 min before escalating
urgent_timeout = 3600      # 1h before escalating

# Telegram
[telegram]
bot_token = "123456:ABC-DEF..."
chat_id = "12345678"                # Your user ID or group ID
# Optional: thread_id for group topics

# Matrix (existing config stays in matrix.toml for backward compat)
# [matrix] section here would override matrix.toml

# Slack
[slack]
bot_token = "xoxb-..."
channel = "#wg-notifications"
# Optional: app_token for Socket Mode (bidirectional)
app_token = "xapp-..."

# Discord
[discord]
bot_token = "MTIz..."
channel_id = "123456789"
# Optional: guild_id for slash commands

# Email
[email]
smtp_host = "smtp.gmail.com"
smtp_port = 587
username = "you@gmail.com"
password = "app-password"         # Use app-specific password
from = "wg@yourdomain.com"
to = ["you@gmail.com"]

# SMS (Twilio)
[sms]
account_sid = "AC..."
auth_token = "..."
from = "+15551234567"
to = "+15559876543"

# Voice (Twilio)
[voice]
account_sid = "AC..."           # Can share with SMS
auth_token = "..."
from = "+15551234567"
to = "+15559876543"
twiml_url = "https://your-server/twiml"  # TwiML webhook

# Webhooks
[webhook]
url = "https://your-server/wg-events"
secret = "hmac-secret-for-verification"
events = ["task_done", "task_failed", "task_ready"]
```

### CLI commands

```bash
# Quick setup for common channels
wg config --notify telegram --token "123:ABC" --chat-id "12345"
wg config --notify slack --bot-token "xoxb-..." --channel "#wg"
wg config --notify email --smtp-host "smtp.gmail.com" --to "you@email.com"

# Test a channel
wg notify --test telegram "Hello from wg!"

# Send task notification (existing command, now channel-aware)
wg notify <task-id>                     # Uses default channels
wg notify <task-id> --channel telegram  # Specific channel
wg notify <task-id> --urgent            # Uses urgent escalation chain

# Show notification config
wg config --notify-status
```

---

## Recommended Priority Order

### Tier 1 — Implement First (highest value, lowest effort)

1. **Telegram** — Best ROI. Easy setup, inline keyboards for approvals, excellent Rust crate (`teloxide`). Most users have Telegram. Implement as the primary notification channel.

2. **Webhooks** — Universal glue. Trivial to implement (`reqwest::post`). Enables integration with Zapier, n8n, custom systems. Should always be available.

3. **Matrix (refactor existing)** — Already partially built. Refactor the lite client behind the `NotificationChannel` trait. Fix the issues listed above.

### Tier 2 — Implement Next (good value)

4. **Email** — Universal reach, good for digests and async approvals. `lettre` is mature. Good complement to real-time channels.

5. **Slack** — Many teams use it. Block Kit interactions are powerful. Worth it for team/enterprise use cases.

6. **Discord** — Excellent for developer communities. Voice channels are a unique feature. Good Rust ecosystem.

### Tier 3 — Implement When Needed

7. **SMS (Twilio)** — Escalation channel for urgent notifications. Simple to implement over HTTP.

8. **Phone Calls (Twilio Voice)** — Ultimate escalation. "Your build is on fire" channel. Needs TwiML server.

9. **Push Notifications** — Requires web UI / PWA. Implement when web frontend exists.

### Tier 4 — Deprioritize or Skip

10. **WebRTC** — Too complex for current state. Revisit when there's a web UI.
11. **WhatsApp** — AI bot ban, high cost, complex setup. Not recommended.
12. **Signal** — No official bot API, fragile workarounds. Not recommended.
13. **RSS/Atom** — Nice-to-have, low interaction value.

### Implementation Roadmap

**Phase 1:** Trait + Telegram + Webhooks + Matrix refactor
**Phase 2:** Email + Slack
**Phase 3:** Discord + SMS
**Phase 4:** Voice calls + Push (when web UI exists)

---

## Summary

The wg human-in-the-loop system should be built on a `NotificationChannel` trait that abstracts over all platforms. Start with Telegram (best UX for individuals) and webhooks (universal integration), refactor existing Matrix code behind the trait, then layer on additional channels based on user demand. The escalation chain pattern (Telegram → SMS → Phone) provides robust human reachability for critical paths.

The existing Matrix code is functional but needs a trait abstraction and version fix. The `matrix_lite` implementation is the stronger foundation. The command parser and notification formatter are solid and should be generalized for use across all channels.
