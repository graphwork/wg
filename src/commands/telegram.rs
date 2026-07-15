//! Telegram commands for WG CLI
//!
//! Provides commands for interacting with Telegram:
//! - `wg telegram listen` - Start the Telegram bot listener
//! - `wg telegram send` - Send a message to the configured chat
//! - `wg telegram status` - Show Telegram configuration status

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use worksgood::notify::NotificationChannel;
use worksgood::notify::config::NotifyConfig;
use worksgood::notify::telegram::{TelegramChannel, TelegramConfig};

/// Run the Telegram listener.
///
/// Starts a long-running process that long-polls for incoming messages via the
/// Telegram Bot API and dispatches WG commands. Listens on **every** configured
/// bot — the legacy top-level `[telegram]` bot *and* each `[telegram.bots.<id>]`
/// named bot (the multi-bot form `wg agency human add` uses to front a
/// per-human onboarding DM). Each inbound message is answered on the same bot
/// and chat that received it, so a confirmation sent to a named bot is actually
/// captured and replied to.
pub fn run_listen(dir: &Path, chat_id: Option<&str>) -> Result<()> {
    let notify = NotifyConfig::load(Some(Path::new(".")))
        .context("Failed to load notification config")?
        .context("No notify.toml found. Create one at ~/.config/workgraph/notify.toml")?;
    let channels = TelegramChannel::all_from_notify_config(&notify)
        .context("Failed to build Telegram channels")?;
    if channels.is_empty() {
        anyhow::bail!(
            "No Telegram bots configured. Add a [telegram] block (legacy) or \
             [telegram.bots.<id>] tables to notify.toml — see `wg telegram list-bots`."
        );
    }

    let chat_override = chat_id.map(|s| s.to_string());

    println!("Starting Telegram listener...");
    if let Ok(cfg) = TelegramConfig::from_notify_config(&notify) {
        println!("{}", bot_banner(&cfg));
    }
    if let Some(ref forced) = chat_override {
        println!("Reply chat override: {}", forced);
    }
    println!("Press Ctrl+C to stop\n");

    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;

    rt.block_on(async move {
        // One long-poll task per bot, all fanned into a single receiver. Each
        // bot's channel is kept (by channel_type) so replies go back through
        // the bot that actually received the message.
        let (tx, mut rx) = tokio::sync::mpsc::channel(128);
        let mut reply_channels: HashMap<String, Arc<TelegramChannel>> = HashMap::new();

        for channel in channels {
            let channel = Arc::new(channel);
            let mut sub = channel.listen().await.with_context(|| {
                format!("Failed to start listener for bot '{}'", channel.bot_id())
            })?;
            reply_channels.insert(channel.channel_type().to_string(), channel.clone());
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(msg) = sub.recv().await {
                    if tx.send(msg).await.is_err() {
                        break; // aggregator dropped
                    }
                }
            });
        }
        drop(tx); // only the per-bot forwarders hold senders now

        let workgraph_dir = dir.to_path_buf();
        while let Some(msg) = rx.recv().await {
            // Reply through the bot that received this message, to the chat it
            // arrived in (or the operator's override), falling back to the
            // bot's configured chat_id.
            let reply_channel = reply_channels.get(&msg.channel);
            let reply_target = resolve_reply_target(
                chat_override.as_deref(),
                msg.chat_id.as_deref(),
                reply_channel.map(|c| c.chat_id()),
            );
            let display_sender = msg.sender_username.as_deref().unwrap_or(&msg.sender);
            let reply_channel = reply_channel.map(|c| c.as_ref());

            // Try to parse as a command
            if let Some(cmd) = worksgood::telegram_commands::parse(&msg.body) {
                println!(
                    "[{}] Command from {} on {}: {}",
                    chrono::Utc::now().format("%H:%M:%S"),
                    display_sender,
                    msg.channel,
                    cmd.description()
                );

                let response =
                    worksgood::telegram_commands::execute(&workgraph_dir, &cmd, &msg.sender);
                send_reply(reply_channel, &reply_target, &response, &msg.channel).await;
            } else if let Some(ref action_id) = msg.action_id {
                // Handle callback button presses
                println!(
                    "[{}] Button press from {} on {}: {}",
                    chrono::Utc::now().format("%H:%M:%S"),
                    display_sender,
                    msg.channel,
                    action_id
                );

                // Action IDs follow the pattern "action:task_id" (e.g. "approve:my-task")
                let response = handle_action(&workgraph_dir, action_id, &msg.sender);
                send_reply(reply_channel, &reply_target, &response, &msg.channel).await;
            } else if let Some(name) = try_confirm_binding(
                &workgraph_dir,
                &msg.sender,
                msg.sender_username.as_deref(),
                &msg.body,
            ) {
                // A bound-but-unconfirmed human replied YES — record it and
                // welcome them. This is the inbound half of the
                // `wg agency human add` onboarding handshake (R21/R22).
                println!(
                    "[{}] {} ({}) confirmed on {} — joined via YES handshake",
                    chrono::Utc::now().format("%H:%M:%S"),
                    name,
                    msg.sender,
                    msg.channel,
                );
                let welcome = format!("Welcome aboard, {}! You're all set. \u{2705}", name);
                send_reply(reply_channel, &reply_target, &welcome, &msg.channel).await;
            } else {
                // Not a command and not a button press: treat as a human's
                // reply to a task they were handed. The router authorizes the
                // sender against their CONFIRMED binding and only then records
                // the reply onto that human's parked task — satisfying its
                // HumanInput wait so the coordinator completes it. (The
                // "awaiting-human task router" formerly deferred at
                // src/notify/telegram.rs:42.)
                //
                // Authorization uses the canonical numeric `msg.sender`, matched
                // against the confirmed binding; replies go back through the bot
                // that received the message (the #49 multi-bot reply contract).
                use crate::commands::service::human_dispatch::InboundReplyOutcome;
                match crate::commands::service::human_dispatch::route_inbound_reply(
                    &workgraph_dir,
                    &msg.channel,
                    &msg.sender,
                    &msg.body,
                ) {
                    InboundReplyOutcome::Recorded(task_id) => {
                        println!(
                            "[{}] Reply from {} recorded on awaiting-human task '{}'",
                            chrono::Utc::now().format("%H:%M:%S"),
                            display_sender,
                            task_id
                        );
                        send_reply(
                            reply_channel,
                            &reply_target,
                            &format!("✓ Recorded your reply on task '{}'.", task_id),
                            &msg.channel,
                        )
                        .await;
                    }
                    InboundReplyOutcome::NoWaitingTask => {
                        println!(
                            "[{}] Message from {} on {} (no awaiting-human task matched): {}",
                            chrono::Utc::now().format("%H:%M:%S"),
                            display_sender,
                            msg.channel,
                            msg.body
                        );
                    }
                    InboundReplyOutcome::Rejected(reason) => {
                        // Security event: an unproven sender tried to answer for
                        // a human. Log the reason server-side; reply with a
                        // generic note that never leaks which humans/tasks exist.
                        eprintln!(
                            "[{}] Rejected reply from {} on {}: {}",
                            chrono::Utc::now().format("%H:%M:%S"),
                            display_sender,
                            msg.channel,
                            reason
                        );
                        send_reply(
                            reply_channel,
                            &reply_target,
                            "Sorry — I couldn't match your message to a task you're set up to answer.",
                            &msg.channel,
                        )
                        .await;
                    }
                }
            }
        }

        Ok(())
    })
}

/// Choose where a listener reply should be sent.
///
/// Precedence: the operator's `--chat-id` override wins; otherwise the chat the
/// message actually arrived in (so a multi-bot listener answers on the chat
/// that received the confirmation, not a single global chat); otherwise the
/// receiving bot's configured `chat_id`. Empty string only if nothing is known.
fn resolve_reply_target(
    override_chat: Option<&str>,
    incoming_chat: Option<&str>,
    channel_default: Option<&str>,
) -> String {
    override_chat
        .or(incoming_chat)
        .or(channel_default)
        .unwrap_or("")
        .to_string()
}

/// Send a listener reply back through the channel that received the message.
///
/// `channel` is the bot that received the inbound message (so the reply goes
/// out on the same token/chat identity); `channel_tag` is used only for the
/// diagnostic when no channel is available. Errors are logged, not propagated,
/// so one failed reply never tears down the listener.
async fn send_reply(
    channel: Option<&TelegramChannel>,
    target: &str,
    text: &str,
    channel_tag: &str,
) {
    match channel {
        Some(ch) => {
            if let Err(e) = ch.send_text(target, text).await {
                eprintln!("Failed to send response via '{}': {e}", ch.bot_id());
            }
        }
        None => eprintln!("No channel to reply on for '{channel_tag}' — dropping response"),
    }
}

/// Try to confirm a human-onboarding binding from an inbound message.
///
/// If the inbound sender (canonical numeric `sender_id`, plus optional
/// `sender_username`) has an unconfirmed Telegram binding (see
/// `wg agency human add`) and `body` is an affirmative `YES`, mark the binding
/// confirmed, persist it, and return the human's name. Otherwise return
/// `None`. Persistence failures are logged and swallowed so the listener keeps
/// running.
fn try_confirm_binding(
    workgraph_dir: &Path,
    sender_id: &str,
    sender_username: Option<&str>,
    body: &str,
) -> Option<String> {
    use worksgood::agency::{TelegramBindingMap, apply_confirmation};

    let agency_dir = workgraph_dir.join("agency");
    let mut bindings = match TelegramBindingMap::load(&agency_dir) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to load Telegram binding map: {e}");
            return None;
        }
    };
    let name = apply_confirmation(
        &mut bindings,
        sender_id,
        sender_username,
        body,
        chrono::Utc::now(),
    )?;
    if let Err(e) = bindings.save(&agency_dir) {
        eprintln!("Failed to persist Telegram binding confirmation: {e}");
        return None;
    }
    Some(name)
}

/// Send a message to the configured Telegram chat.
pub fn run_send(chat_id: Option<&str>, message: &str) -> Result<()> {
    let config = load_telegram_config()?;
    let effective_chat_id = chat_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.chat_id.clone());

    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;

    rt.block_on(async {
        let channel = TelegramChannel::new(config);
        channel
            .send_text(&effective_chat_id, message)
            .await
            .context("Failed to send message")?;
        println!("Message sent to chat {}", effective_chat_id);
        Ok(())
    })
}

/// List all configured Telegram bots — the legacy single-bot from `[telegram]`
/// (if any) plus every entry under `[telegram.bots.<id>]`. Used to verify a
/// multi-bot setup before `wg telegram listen` spawns one long-poll task per
/// bot.
pub fn run_list_bots(json: bool) -> Result<()> {
    // Match the project-local-then-global lookup the other telegram subcommands
    // use (see `load_telegram_config`): try `.workgraph/notify.toml` from CWD
    // first, then fall back to `~/.config/workgraph/notify.toml`.
    let notify = match NotifyConfig::load(Some(Path::new(".")))? {
        Some(c) => c,
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"bots": []}))?
                );
            } else {
                println!("Telegram: not configured (no notify.toml found)");
            }
            return Ok(());
        }
    };

    let channels = TelegramChannel::all_from_notify_config(&notify)?;

    if json {
        let bots: Vec<serde_json::Value> = channels
            .iter()
            .map(|ch| {
                serde_json::json!({
                    "bot_id": ch.bot_id(),
                    "channel_type": ch.channel_type(),
                    "agent_id": ch.agent_id(),
                    "chat_id": ch.chat_id(),
                    "bot_token_preview": ch.bot_token_preview(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"bots": bots}))?
        );
    } else if channels.is_empty() {
        println!("Telegram: no bots configured");
        println!(
            "\nAdd a [telegram] block to ~/.config/workgraph/notify.toml or .workgraph/notify.toml."
        );
        println!(
            "Single-bot (legacy):\n  [telegram]\n  bot_token = \"...\"\n  chat_id = \"...\"\n"
        );
        println!(
            "Multi-bot (one per persistent agent):\n  [telegram.bots.nora]\n  bot_token = \"...\"\n  chat_id = \"...\"\n  agent_id = \"nora\"\n"
        );
    } else {
        println!("{} bot(s) configured:\n", channels.len());
        for ch in &channels {
            let agent = ch.agent_id().unwrap_or("(shared / no agent binding)");
            println!("  bot id:        {}", ch.bot_id());
            println!("  channel type:  {}", ch.channel_type());
            println!("  agent id:      {}", agent);
            println!("  chat id:       {}", ch.chat_id());
            println!("  token preview: {}", ch.bot_token_preview());
            println!();
        }
    }
    Ok(())
}

/// Show Telegram configuration status.
pub fn run_status(json: bool) -> Result<()> {
    match load_telegram_config() {
        Ok(config) => {
            if json {
                let status = serde_json::json!({
                    "configured": true,
                    "chat_id": config.chat_id,
                    "bot_token_prefix": &config.bot_token[..config.bot_token.len().min(6)],
                });
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("Telegram: configured");
                println!(
                    "  Bot token: {}...",
                    &config.bot_token[..config.bot_token.len().min(6)]
                );
                println!("  Chat ID: {}", config.chat_id);
            }
        }
        Err(_) => {
            if json {
                let status = serde_json::json!({ "configured": false });
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("Telegram: not configured");
                println!("\nAdd a [telegram] section to your notify.toml:");
                println!("  ~/.config/workgraph/notify.toml");
                println!("  or .wg/notify.toml");
                println!();
                println!("  [telegram]");
                println!("  bot_token = \"123456:ABC-DEF...\"");
                println!("  chat_id = \"12345678\"");
            }
        }
    }
    Ok(())
}

/// Handle an action button callback.
fn handle_action(workgraph_dir: &Path, action_id: &str, sender: &str) -> String {
    let parts: Vec<&str> = action_id.splitn(2, ':').collect();
    if parts.len() != 2 {
        return format!("Unknown action: {action_id}");
    }

    let (action, task_id) = (parts[0], parts[1]);
    match action {
        "approve" | "claim" => {
            worksgood::matrix_commands::execute_claim(workgraph_dir, task_id, Some(sender))
        }
        "reject" | "fail" => worksgood::matrix_commands::execute_fail(
            workgraph_dir,
            task_id,
            Some("rejected via Telegram"),
        ),
        "done" => worksgood::matrix_commands::execute_done(workgraph_dir, task_id),
        _ => format!("Unknown action: {action}"),
    }
}

/// Poll for replies from the configured Telegram chat.
///
/// Calls the Telegram Bot API getUpdates endpoint and waits for replies
/// from the configured chat_id within the timeout period.
pub fn run_poll(chat_id: Option<&str>, timeout_seconds: u64) -> Result<()> {
    let config = load_telegram_config()?;
    let effective_chat_id = chat_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.chat_id.clone());

    println!("Polling for messages from chat {}...", effective_chat_id);
    println!("Timeout: {} seconds", timeout_seconds);

    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;

    rt.block_on(async {
        let channel = TelegramChannel::new(config);

        // Load last seen update_id
        let offset = load_last_update_id().unwrap_or(0);

        let start_time = std::time::Instant::now();
        let timeout_duration = std::time::Duration::from_secs(timeout_seconds);

        loop {
            if start_time.elapsed() >= timeout_duration {
                println!("Timeout reached - no new messages");
                return Ok(());
            }

            match poll_once(&channel, offset, &effective_chat_id, 10).await {
                Ok(Some((message, new_offset))) => {
                    // Save the new offset
                    if let Err(e) = save_last_update_id(new_offset) {
                        eprintln!("Warning: failed to save update_id: {}", e);
                    }

                    println!("Message from {}: {}", message.sender, message.body);
                    return Ok(());
                }
                Ok(None) => {
                    // No new messages, wait a bit before trying again
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(e) => {
                    eprintln!("Poll error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    })
}

/// Send a message and wait for reply.
///
/// Sends the message and polls for reply at intervals. Times out after
/// configurable max wait. Includes task ID context if provided.
pub fn run_ask(
    message: &str,
    chat_id: Option<&str>,
    timeout_seconds: u64,
    interval_seconds: u64,
    task_id: Option<&str>,
) -> Result<()> {
    let config = load_telegram_config()?;
    let effective_chat_id = chat_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.chat_id.clone());

    // Format message with task context if provided
    let formatted_message = if let Some(task) = task_id {
        format!("[{}] Agent question: {}", task, message)
    } else {
        format!("Agent question: {}", message)
    };

    println!("Sending message and waiting for reply...");
    println!("Message: {}", formatted_message);
    println!(
        "Timeout: {} seconds, polling every {} seconds",
        timeout_seconds, interval_seconds
    );

    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;

    rt.block_on(async {
        let channel = TelegramChannel::new(config);

        // Send the message first
        match channel
            .send_text(&effective_chat_id, &formatted_message)
            .await
        {
            Ok(msg_id) => {
                println!("Message sent (ID: {})", msg_id.0);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to send message: {}", e));
            }
        }

        // Load last seen update_id
        let offset = load_last_update_id().unwrap_or(0);

        let start_time = std::time::Instant::now();
        let timeout_duration = std::time::Duration::from_secs(timeout_seconds);
        let interval_duration = std::time::Duration::from_secs(interval_seconds);

        loop {
            if start_time.elapsed() >= timeout_duration {
                println!("Timeout reached - no reply received");
                return Ok(());
            }

            match poll_once(&channel, offset, &effective_chat_id, 10).await {
                Ok(Some((message, new_offset))) => {
                    // Save the new offset
                    if let Err(e) = save_last_update_id(new_offset) {
                        eprintln!("Warning: failed to save update_id: {}", e);
                    }

                    println!("Reply from {}: {}", message.sender, message.body);
                    return Ok(());
                }
                Ok(None) => {
                    // No new messages, wait for the next polling interval
                    tokio::time::sleep(interval_duration).await;
                }
                Err(e) => {
                    eprintln!("Poll error: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    })
}

/// Poll Telegram once for new messages from a specific chat.
/// Returns the first message and the new offset, or None if no messages.
async fn poll_once(
    channel: &TelegramChannel,
    offset: i64,
    target_chat_id: &str,
    timeout: u32,
) -> Result<Option<(worksgood::notify::IncomingMessage, i64)>> {
    let body = serde_json::json!({
        "offset": offset,
        "timeout": timeout,
        "allowed_updates": ["message", "callback_query"],
    });

    let resp = channel.api_call("getUpdates", &body).await?;

    let updates = resp
        .get("result")
        .and_then(|r| r.as_array())
        .context("Invalid response format")?;

    let mut new_offset = offset;

    for update in updates {
        if let Some(uid) = update.get("update_id").and_then(|u| u.as_i64()) {
            new_offset = uid + 1;
        }

        // Reuse the canonical parser so this path emits the same stable
        // numeric sender and chat identity as the long-poll listener, then
        // keep only messages from the chat we're polling.
        if let Some(msg) = worksgood::notify::telegram::parse_update(update, channel.channel_type())
        {
            if msg.chat_id.as_deref() == Some(target_chat_id) {
                return Ok(Some((msg, new_offset)));
            }
        }
    }

    Ok(None)
}

/// Load the last seen update_id from state file.
fn load_last_update_id() -> Result<i64> {
    let state_file = get_state_file_path()?;
    let content = std::fs::read_to_string(state_file)?;
    let id: i64 = content
        .trim()
        .parse()
        .context("Invalid update_id format in state file")?;
    Ok(id)
}

/// Save the last seen update_id to state file.
fn save_last_update_id(update_id: i64) -> Result<()> {
    let state_file = get_state_file_path()?;

    // Ensure parent directory exists
    if let Some(parent) = state_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(state_file, update_id.to_string())?;
    Ok(())
}

/// Get the path to the update_id state file.
fn get_state_file_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home
        .join(".config")
        .join("workgraph")
        .join("telegram_update_id"))
}

/// Render the startup banner line describing which bot(s) are configured.
///
/// Prefers the legacy single `bot_token` when present (masking it to a
/// preview). When the config uses only the multi-bot `bots` map — the
/// legacy `bot_token` is empty — falls back to summarizing all configured
/// bots by id instead of slicing the empty token, which used to panic
/// (out-of-bounds string slice).
fn bot_banner(config: &TelegramConfig) -> String {
    if config.bot_token.is_empty() {
        let bots = config.all_bots();
        if bots.is_empty() {
            "Bots configured: none".to_string()
        } else {
            let names = bots
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} bots configured: {}", bots.len(), names)
        }
    } else {
        let prefix = config.bot_token.get(..6).unwrap_or(&config.bot_token);
        let suffix_start = config.bot_token.len().saturating_sub(4);
        let suffix = config
            .bot_token
            .get(suffix_start..)
            .unwrap_or(&config.bot_token);
        format!("Bot token: {}...{}", prefix, suffix)
    }
}

/// Load Telegram config from notify.toml.
fn load_telegram_config() -> Result<TelegramConfig> {
    let notify_config = NotifyConfig::load(Some(Path::new(".")))
        .context("Failed to load notification config")?
        .context("No notify.toml found. Create one at ~/.config/workgraph/notify.toml")?;
    TelegramConfig::from_notify_config(&notify_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use worksgood::notify::telegram::TelegramBotConfig;

    #[test]
    fn bot_banner_bots_map_only_does_not_panic() {
        // Config with ONLY the multi-bot map, no legacy [telegram] bot_token —
        // this used to panic on `&config.bot_token[..6]`.
        let mut bots = HashMap::new();
        bots.insert(
            "nora".to_string(),
            TelegramBotConfig {
                bot_token: "111:AAA".to_string(),
                chat_id: "1".to_string(),
                agent_id: Some("nora".to_string()),
            },
        );
        bots.insert(
            "bruno".to_string(),
            TelegramBotConfig {
                bot_token: "222:BBB".to_string(),
                chat_id: "2".to_string(),
                agent_id: Some("bruno".to_string()),
            },
        );
        let config = TelegramConfig {
            bot_token: String::new(),
            chat_id: String::new(),
            bots,
        };

        let banner = bot_banner(&config);
        assert!(banner.contains("2 bots configured"));
        assert!(banner.contains("nora"));
        assert!(banner.contains("bruno"));
    }

    #[test]
    fn bot_banner_legacy_token_masks_preview() {
        let config = TelegramConfig {
            bot_token: "123456:ABCDEF".to_string(),
            chat_id: "1".to_string(),
            bots: HashMap::new(),
        };

        let banner = bot_banner(&config);
        assert_eq!(banner, "Bot token: 123456...CDEF");
    }

    #[test]
    fn bot_banner_empty_config_does_not_panic() {
        let config = TelegramConfig {
            bot_token: String::new(),
            chat_id: String::new(),
            bots: HashMap::new(),
        };

        assert_eq!(bot_banner(&config), "Bots configured: none");
    }

    #[test]
    fn handle_action_approve() {
        // We can't easily test without a graph, but we can verify parsing.
        let result = handle_action(Path::new("/nonexistent"), "approve:my-task", "testuser");
        assert!(result.contains("Error") || result.contains("Claimed"));
    }

    #[test]
    fn handle_action_unknown() {
        let result = handle_action(Path::new("/nonexistent"), "foobar:task", "testuser");
        assert!(result.contains("Unknown action"));
    }

    #[test]
    fn handle_action_malformed() {
        let result = handle_action(Path::new("/nonexistent"), "no-colon", "testuser");
        assert!(result.contains("Unknown action"));
    }

    // -----------------------------------------------------------------------
    // Multi-bot listener contract (Erik's PR #49 gap 2). The listener must
    // consume the configured NAMED bots — not just the legacy top-level token —
    // and route replies back to the bot/chat that received the message. These
    // exercise the actual listener wiring (channel selection + reply routing),
    // not merely the startup banner.
    // -----------------------------------------------------------------------

    #[test]
    fn listener_consumes_named_bots_on_multibot_only_config() {
        // A config with ONLY [telegram.bots.*] — the exact shape `wg agency
        // human add` produces and PR #55 made non-panicking. The OLD listener
        // built `TelegramChannel::new(config)`, which discards `bots` and polls
        // an empty legacy token; a confirmation could never arrive. The
        // listener now sources its channels from `all_from_notify_config`, so
        // the named bot must be present with its qualified channel type.
        let toml_str = r#"
[telegram.bots.nadin]
bot_token = "222:BBB"
chat_id = "78901234"
agent_id = "human-nadin"
"#;
        let notify: NotifyConfig = toml::from_str(toml_str).unwrap();
        let channels = TelegramChannel::all_from_notify_config(&notify).unwrap();

        // Exactly the named bot — no phantom legacy "default" channel.
        assert_eq!(channels.len(), 1, "only the named bot should be polled");
        let ch = &channels[0];
        assert_eq!(ch.bot_id(), "nadin");
        assert_eq!(ch.channel_type(), "telegram:nadin");
        assert_eq!(ch.chat_id(), "78901234");
        assert_eq!(ch.agent_id(), Some("human-nadin"));
    }

    #[test]
    fn listener_routes_reply_to_receiving_bot_and_chat() {
        // Two named bots. A message received on `nadin`'s channel must be
        // answered via `nadin` (its token) on the chat it arrived in — never
        // through the other bot or a single global chat.
        let toml_str = r#"
[telegram.bots.nadin]
bot_token = "222:BBB"
chat_id = "111"
agent_id = "human-nadin"

[telegram.bots.erik]
bot_token = "333:CCC"
chat_id = "222"
agent_id = "human-erik"
"#;
        let notify: NotifyConfig = toml::from_str(toml_str).unwrap();
        let channels = TelegramChannel::all_from_notify_config(&notify).unwrap();
        let by_type: HashMap<String, &TelegramChannel> = channels
            .iter()
            .map(|c| (c.channel_type().to_string(), c))
            .collect();

        // A YES arrives on nadin's channel, in a specific chat.
        let update = serde_json::json!({
            "update_id": 1,
            "message": {
                "message_id": 5,
                "from": { "id": 78901234, "username": "nadin" },
                "chat": { "id": 99999, "type": "private" },
                "text": "YES"
            }
        });
        let msg = worksgood::notify::telegram::parse_update(&update, "telegram:nadin").unwrap();

        // The reply channel is selected by the receiving channel type.
        let reply_channel = by_type.get(&msg.channel).copied();
        assert!(reply_channel.is_some(), "reply routed to receiving bot");
        assert_eq!(reply_channel.unwrap().bot_id(), "nadin");

        // And the reply target is the chat the message arrived in.
        let target = resolve_reply_target(
            None,
            msg.chat_id.as_deref(),
            reply_channel.map(|c| c.chat_id()),
        );
        assert_eq!(target, "99999");
    }

    #[test]
    fn resolve_reply_target_precedence() {
        // Override beats everything.
        assert_eq!(
            resolve_reply_target(Some("override"), Some("incoming"), Some("default")),
            "override"
        );
        // Then the chat the message came in on.
        assert_eq!(
            resolve_reply_target(None, Some("incoming"), Some("default")),
            "incoming"
        );
        // Then the bot's configured chat.
        assert_eq!(resolve_reply_target(None, None, Some("default")), "default");
        // Nothing known → empty.
        assert_eq!(resolve_reply_target(None, None, None), "");
    }
}
