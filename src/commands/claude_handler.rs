//! `wg claude-handler` — standalone bridge between Claude CLI's
//! stream-json stdio and `chat/<ref>/*.jsonl`.
//!
//! ## Stdout-is-protocol contract
//!
//! Stdout for the handler binary is part of the protocol stream that
//! parent supervisors (the daemon, the TUI's PTY embed, smoke harnesses)
//! parse line-by-line. **Never write diagnostic text to stdout from this
//! file or anything it transitively calls** — config-load chatter,
//! deprecation warnings, debug breadcrumbs and progress notes all belong
//! on stderr or in `handler.log` / `daemon.log`. A single stray
//! `println!` in a transitive call corrupts the chat json-line stream
//! and crashes the next-turn parse silently. The regression lock for
//! this contract lives in
//! `tests/integration_handler_stdout_pristine.rs`.
//!
//! Peer of `wg nex --chat <ref>`: where nex IS a native handler that
//! speaks chat/*.jsonl directly, this handler spawns the `claude` CLI
//! and translates between the two protocols. From the daemon's and
//! TUI's perspective, spawning a claude coordinator is now identical
//! to spawning a native one — both go through `wg spawn-task` which
//! execs into the right handler binary.
//!
//! The bridge preserves Claude's native session identity in the canonical
//! UUID-backed chat directory. A replacement daemon generation launches
//! `claude --resume <session-id>` rather than silently starting a new Claude,
//! while every user-visible turn remains in WG's inbox/outbox projection.
//! Claude stream text, tool calls/results, usage, and failures are translated
//! into that projection; the adapter never falls back to Pi or Codex.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use worksgood::chat;
use worksgood::session_lock::{HandlerKind, SessionLock};

/// Poll interval for new inbox messages when the inbox appears empty.
/// Short enough that chat feels snappy; long enough not to hammer the
/// filesystem.
const INBOX_POLL: Duration = Duration::from_millis(200);

/// Timeout for collecting a single assistant response before we give up
/// and write a timeout message. Should cover tool-heavy turns.
const TURN_TIMEOUT: Duration = Duration::from_secs(300);

/// Entry point wired into `main.rs`.
pub fn run(
    workgraph_dir: &Path,
    chat_ref: &str,
    resume: bool,
    role: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    // `resume` is accepted for argv symmetry. Claude continuity is keyed by
    // the persisted native session id below rather than the caller's journal
    // hint, so a daemon restart cannot accidentally start a new conversation.
    let _ = resume;

    // Resolve through the session registry so aliases
    // (`coordinator-0`, bare `0`, task-agent names) land on the
    // canonical `chat/<uuid>/` dir. A naive join here used to
    // create a `chat/<alias>/` directory that got orphaned from
    // the UUID-backed storage.
    let chat_dir = worksgood::chat::chat_dir_for_ref(workgraph_dir, chat_ref);
    std::fs::create_dir_all(&chat_dir)
        .with_context(|| format!("create chat dir {:?}", chat_dir))?;

    let mut _lock = SessionLock::acquire(&chat_dir, HandlerKind::Adapter).with_context(|| {
        format!(
            "acquire session lock for chat session {:?} — another handler is running",
            chat_ref
        )
    })?;

    let handler_log = chat_dir.join("handler.log");
    let logger = HandlerLogger::open(&handler_log)?;
    logger.info(&format!(
        "claude-handler starting: chat_ref={}, role={:?}, model={:?}",
        chat_ref, role, model
    ));

    // SIGTERM → kernel kills us; lock lingers as stale, next handler
    // picks it up. SIGINT → forwarded to the Claude CLI child so the
    // user's "stop generating" gesture (e.g. Ctrl+C in the TUI
    // pathway through `CoordinatorAgent::interrupt()`) preserves the
    // session instead of killing the whole handler. See
    // `install_sigint_forwarder` below.
    let shutdown = Arc::new(Mutex::new(false));

    // Resolve system prompt. For the coordinator-N convention we build
    // the full coordinator prompt; other sessions get a minimal role
    // line (caller can override via --role, which gets appended).
    let system_prompt = build_handler_system_prompt(workgraph_dir, chat_ref, role);

    // Spawn Claude CLI. The session id is learned from Claude's `system/init`
    // (or result) event and lives beside this chat's canonical inbox/outbox.
    // A replacement handler resumes that exact native conversation.
    let session_id_path = chat_dir.join(".claude-session-id");
    let prior_session_id = std::fs::read_to_string(&session_id_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (mut child, mut stdin, stdout, stderr_path) = spawn_claude_process(
        workgraph_dir,
        &chat_dir,
        &system_prompt,
        model,
        prior_session_id.as_deref(),
        &logger,
    )
    .context("spawn claude CLI")?;

    // Record the child PID in a process-global atomic so the SIGINT
    // handler (installed below, async-signal-safe) can forward to
    // it. Set BEFORE installing the handler to close the race where
    // SIGINT arrives between spawn and handler install.
    CLAUDE_CHILD_PID.store(child.id() as i32, Ordering::SeqCst);
    install_sigint_forwarder();

    // Reader thread: Claude stdout → ResponseEvent channel.
    let (resp_tx, resp_rx) = mpsc::channel::<ResponseEvent>();
    let reader_logger = logger.clone();
    let _reader = thread::Builder::new()
        .name("claude-handler-stdout".into())
        .spawn(move || stdout_reader(stdout, resp_tx, reader_logger))
        .context("spawn stdout reader thread")?;

    // Main loop: poll inbox, format → stdin, collect → outbox.
    // Cursor starts at the highest inbox id that already has a
    // matching outbox response — i.e., everything up to the last
    // ANSWERED turn is skipped, and any pending (un-answered) turns
    // are picked up by this handler. This handles both first-run
    // (no outbox → cursor=0 → process all inbox) and restart
    // (outbox has replies up to id N → cursor=N → process id N+1..).
    let mut inbox_cursor: u64 = last_answered_inbox_id(workgraph_dir, chat_ref);
    let coordinator_id = parse_coordinator_id(chat_ref);
    let mut last_interaction = chrono::Utc::now().to_rfc3339();
    logger.info(&format!(
        "claude-handler ready: inbox_cursor={}, coordinator_id={:?}, handler_log={}",
        inbox_cursor,
        coordinator_id,
        handler_log.display()
    ));

    loop {
        if *shutdown.lock().unwrap_or_else(|e| e.into_inner()) {
            logger.info("claude-handler: shutdown signal received");
            break;
        }

        // Child-alive check: if Claude CLI died, exit non-zero so the
        // spawn-task supervisor (daemon) restarts us.
        if let Some(status) = child.try_wait().unwrap_or(None) {
            logger.warn(&format!(
                "claude-handler: Claude CLI exited with status {:?} — handler exiting for restart",
                status
            ));
            // Draining is handled by Drop on SessionLock.
            return Err(anyhow::anyhow!(
                "Claude CLI exited with status {:?}",
                status
            ));
        }

        // Pull any new inbox messages since our cursor.
        let new_msgs = match chat::read_inbox_since_ref(workgraph_dir, chat_ref, inbox_cursor) {
            Ok(msgs) => msgs,
            Err(e) => {
                logger.warn(&format!("claude-handler: inbox read error: {}", e));
                thread::sleep(INBOX_POLL);
                continue;
            }
        };

        if new_msgs.is_empty() {
            thread::sleep(INBOX_POLL);
            continue;
        }

        for msg in new_msgs {
            inbox_cursor = msg.id.max(inbox_cursor);
            let request_id = if msg.request_id.is_empty() {
                format!("req-{}", msg.id)
            } else {
                msg.request_id.clone()
            };

            logger.info(&format!(
                "claude-handler: processing inbox id={} request_id={} ({} chars)",
                msg.id,
                request_id,
                msg.content.len()
            ));

            // For coordinator sessions, prepend the same graph-state
            // context the daemon's legacy inline path injected — so
            // the coordinator sees recent task events, active agents,
            // and failed-task attention markers every turn.
            let full_content = if let Some(cid) = coordinator_id {
                match crate::commands::service::coordinator_agent::build_coordinator_context(
                    workgraph_dir,
                    &last_interaction,
                    None,
                    cid,
                ) {
                    Ok(ctx) if !ctx.is_empty() => {
                        format!("{}\n\n---\n\nUser message:\n{}", ctx, msg.content)
                    }
                    _ => format!("User message:\n{}", msg.content),
                }
            } else {
                msg.content.clone()
            };

            // Format + write user turn.
            let user_msg = format_stream_json_user_message(&full_content);
            if let Err(e) = stdin
                .write_all(user_msg.as_bytes())
                .and_then(|_| stdin.flush())
            {
                logger.error(&format!("claude-handler: stdin write failed: {}", e));
                let _ = chat::append_outbox_ref(
                    workgraph_dir,
                    chat_ref,
                    "The coordinator encountered an error sending to Claude. Restarting.",
                    &request_id,
                );
                return Err(anyhow::anyhow!("stdin write failed: {}", e));
            }

            // Collect response; stream partial text to `.streaming`.
            let streaming_path = chat::streaming_path_ref(workgraph_dir, chat_ref);
            let collected = collect_response(
                &resp_rx,
                &logger,
                TURN_TIMEOUT,
                Some((&streaming_path, workgraph_dir, chat_ref)),
                Some(&session_id_path),
            );

            match collected {
                Some(resp) if !resp.summary.is_empty() => {
                    logger.info(&format!(
                        "claude-handler: response ready for {} ({} chars summary, {} chars transcript)",
                        request_id,
                        resp.summary.len(),
                        resp.full_response.len(),
                    ));
                    // Store the full interleaved transcript (text + tool boxes)
                    // in `full_response` only when it actually contains more than
                    // the summary — avoids duplicating the same text twice in
                    // the outbox file when a turn had no tool calls.
                    let full_response = if resp.full_response.trim() != resp.summary.trim() {
                        Some(resp.full_response.clone())
                    } else {
                        None
                    };
                    if let Err(e) = chat::append_outbox_full_ref(
                        workgraph_dir,
                        chat_ref,
                        &resp.summary,
                        full_response,
                        &request_id,
                    ) {
                        logger.error(&format!("claude-handler: outbox write failed: {}", e));
                    }
                }
                Some(_) => {
                    logger.warn("claude-handler: empty response");
                    let detail = current_process_failure(&mut child, &stderr_path)
                        .unwrap_or_else(|| "Claude produced no response text".to_string());
                    let _ = chat::append_outbox_ref(
                        workgraph_dir,
                        chat_ref,
                        &format!("Claude chat failed: {detail}. Please retry."),
                        &request_id,
                    );
                }
                None => {
                    logger.warn("claude-handler: response timeout or stream end");
                    let detail = current_process_failure(&mut child, &stderr_path)
                        .unwrap_or_else(|| "response timed out or its stream ended".to_string());
                    let _ = chat::append_outbox_ref(
                        workgraph_dir,
                        chat_ref,
                        &format!("Claude chat failed: {detail}. Please retry."),
                        &request_id,
                    );
                }
            }

            chat::clear_streaming_ref(workgraph_dir, chat_ref);
            last_interaction = chrono::Utc::now().to_rfc3339();
        }
    }

    // Graceful shutdown: ask Claude to exit, then reap. SessionLock
    // drop will remove the lock file.
    let _ = child.kill();
    let _ = child.wait();
    logger.info("claude-handler: exited cleanly");
    Ok(())
}

/// Compute the starting inbox cursor: the highest inbox id for which
/// an outbox reply already exists (matched by `request_id`). Messages
/// with larger ids OR without a matching outbox reply are unprocessed
/// work, so we pick them up.
///
/// First run with a fresh inbox: no outbox → cursor = 0 → we process
/// everything.
///
/// Restart scenario: outbox contains replies for the earlier inbox
/// messages → cursor = id of the last answered one → we skip those
/// and resume from the first un-answered message.
fn last_answered_inbox_id(workgraph_dir: &Path, chat_ref: &str) -> u64 {
    let inbox = match chat::read_inbox_ref(workgraph_dir, chat_ref) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let outbox = match chat::read_outbox_since_ref(workgraph_dir, chat_ref, 0) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let answered_request_ids: std::collections::HashSet<String> =
        outbox.iter().map(|m| m.request_id.clone()).collect();
    inbox
        .iter()
        .filter(|m| answered_request_ids.contains(&m.request_id))
        .map(|m| m.id)
        .max()
        .unwrap_or(0)
}

/// If the chat ref is a coordinator alias (`coordinator-N`), return
/// the numeric id so we can call coordinator-specific helpers like
/// `build_coordinator_context`. Otherwise `None` — the handler runs
/// as a plain chat session with no graph-state injection.
fn parse_coordinator_id(chat_ref: &str) -> Option<u32> {
    ["coordinator-", "chat-", ".coordinator-", ".chat-"]
        .into_iter()
        .find_map(|prefix| {
            chat_ref
                .strip_prefix(prefix)
                .and_then(|suffix| suffix.parse::<u32>().ok())
        })
}

/// Build the system prompt. For `coordinator-N` sessions we load the
/// full coordinator prompt (same as the old inline path). Otherwise a
/// minimal role-specific prompt.
fn build_handler_system_prompt(workgraph_dir: &Path, chat_ref: &str, role: Option<&str>) -> String {
    if parse_coordinator_id(chat_ref).is_some() || role == Some("coordinator") {
        crate::commands::service::coordinator_agent::build_system_prompt(workgraph_dir)
    } else if let Some(r) = role {
        format!("You are acting in the role of: {}.", r)
    } else {
        String::from("You are a WG task agent.")
    }
}

// --- Claude stdio bridging ---------------------------------------------------

/// Spawn `claude` with stream-json stdio. Mirrors the flags the daemon
/// previously used inline.
fn spawn_claude_process(
    workgraph_dir: &Path,
    chat_dir: &Path,
    system_prompt: &str,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    logger: &HandlerLogger,
) -> Result<(Child, ChildStdin, ChildStdout, std::path::PathBuf)> {
    let registry = worksgood::service::executor::ExecutorRegistry::new(workgraph_dir);
    let executor_config = registry
        .load_config("claude")
        .context("load claude executor config")?;
    let command = &executor_config.executor.command;

    let mut cmd = Command::new(command);
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.args([
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
    ]);
    if let Some(session_id) = resume_session_id {
        cmd.args(["--resume", session_id]);
    } else {
        cmd.args(["--system-prompt", system_prompt]);
    }
    cmd.args(["--allowedTools", "Bash(wg:*)"]);

    if let Some(m) = model {
        // Strip provider prefix (e.g., "claude:opus" → "opus") for the CLI, then
        // expand friendly aliases with no CLI shortcut (`fable` → `claude-fable-5`).
        let spec = worksgood::config::parse_model_spec(m);
        let model_arg = worksgood::config::claude_cli_model_arg(&spec.model_id);
        cmd.args(["--model", &model_arg]);
    }

    cmd.current_dir(workgraph_dir.parent().unwrap_or(workgraph_dir));
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());

    // Keep stderr per chat and per current Claude generation. A shared
    // service-level append log made one chat's old authentication failure look
    // like another chat's current turn failure. `handler.log` remains the
    // durable append-only diagnostic; this file is only the exact child tail.
    let stderr_path = chat_dir.join("claude-stderr.log");
    if let Some(parent) = stderr_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&stderr_path)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    cmd.stderr(stderr_file);

    logger.info(&format!(
        "claude-handler: spawning {}{} (model={}, cwd={:?}, stderr={:?})",
        command,
        if resume_session_id.is_some() {
            " --resume <persisted-session>"
        } else {
            ""
        },
        model.unwrap_or("default"),
        workgraph_dir.parent().unwrap_or(workgraph_dir),
        stderr_path
    ));

    let mut child = cmd.spawn().context("spawn claude CLI process")?;
    let stdin = child.stdin.take().context("claude stdin take")?;
    let stdout = child.stdout.take().context("claude stdout take")?;
    Ok((child, stdin, stdout, stderr_path))
}

/// Return a bounded, user-visible failure for the current Claude child only.
/// A short grace closes the normal race where stdout reaches EOF a few
/// milliseconds before `try_wait` observes exit. Authentication/model errors
/// therefore reach the outbox/TUI instead of becoming an empty generic reply.
fn current_process_failure(child: &mut Child, stderr_path: &Path) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => break None,
        }
    }?;
    if status.success() {
        return None;
    }

    let stderr = std::fs::read_to_string(stderr_path).unwrap_or_default();
    let mut lines = stderr.lines().rev().take(12).collect::<Vec<_>>();
    lines.reverse();
    let tail = lines.join("\n");
    if tail.trim().is_empty() {
        Some(format!("Claude CLI exited {status}"))
    } else {
        Some(format!("Claude CLI exited {status}: {}", tail.trim()))
    }
}

/// Format a user message as a stream-json user message.
fn format_stream_json_user_message(content: &str) -> String {
    let msg = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content,
        }
    });
    let mut s = serde_json::to_string(&msg).unwrap_or_default();
    s.push('\n');
    s
}

/// Usage attached to one native Claude turn. Claude emits the same fields on
/// assistant/result events; the collector keeps the latest snapshot so usage
/// is rendered once rather than double-counted.
#[derive(Clone, Debug, Default, PartialEq)]
struct ClaudeUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    total_cost_usd: Option<f64>,
}

impl ClaudeUsage {
    fn from_event(value: &serde_json::Value) -> Option<Self> {
        let usage = value.get("usage").or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("usage"))
        })?;
        Some(Self {
            input_tokens: usage
                .get("input_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            cache_read_input_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            cache_creation_input_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            total_cost_usd: value.get("total_cost_usd").and_then(|value| value.as_f64()),
        })
    }

    fn render(&self) -> String {
        let mut fields = vec![
            format!("{} input", self.input_tokens),
            format!("{} output", self.output_tokens),
        ];
        if self.cache_read_input_tokens > 0 {
            fields.push(format!("{} cache-read", self.cache_read_input_tokens));
        }
        if self.cache_creation_input_tokens > 0 {
            fields.push(format!("{} cache-write", self.cache_creation_input_tokens));
        }
        if let Some(cost) = self.total_cost_usd {
            fields.push(format!("${cost:.4}"));
        }
        format!("[usage: {}]", fields.join(" · "))
    }
}

/// Events emitted by the stdout reader.
enum ResponseEvent {
    Text(String),
    ToolUse { name: String, input: String },
    ToolResult(String),
    SessionStarted(String),
    Usage(ClaudeUsage),
    Failure(String),
    TurnComplete,
    StreamEnd,
}

struct CollectedResponse {
    /// The last text block — what goes into `content` (the one-line
    /// summary in the TUI chat when no full transcript is available).
    summary: String,
    /// The full interleaved transcript: every text block + a formatted
    /// tool-box (`┌─ Name ────\n│ $ cmd\n│ output...\n└─`) for each
    /// tool_use/tool_result pair. Matches the native executor's format
    /// so the TUI chat renderer's tool-box styling kicks in.
    full_response: String,
}

fn tool_result_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|value| value.as_str()) == Some("text") {
                        block
                            .get("text")
                            .and_then(|value| value.as_str())
                            .map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Read Claude stdout line-by-line, parse stream-json, forward to
/// `tx`. Mirrors the daemon's previous inline parser.
fn stdout_reader(stdout: ChildStdout, tx: mpsc::Sender<ResponseEvent>, logger: HandlerLogger) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                logger.warn(&format!("stdout read error: {}", e));
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if let Some(session_id) = val.get("session_id").and_then(|value| value.as_str())
            && !session_id.trim().is_empty()
        {
            let _ = tx.send(ResponseEvent::SessionStarted(session_id.to_string()));
        }
        match msg_type {
            "assistant" => {
                if let Some(usage) = ClaudeUsage::from_event(&val) {
                    let _ = tx.send(ResponseEvent::Usage(usage));
                }
                if let Some(message) = val.get("message") {
                    if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                        for block in content {
                            let block_type =
                                block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match block_type {
                                "text" => {
                                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                        let _ = tx.send(ResponseEvent::Text(text.to_string()));
                                    }
                                }
                                "tool_use" => {
                                    let name = block
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let input = block
                                        .get("input")
                                        .map(|v| serde_json::to_string(v).unwrap_or_default())
                                        .unwrap_or_default();
                                    let _ = tx.send(ResponseEvent::ToolUse { name, input });
                                }
                                _ => {}
                            }
                        }
                    }
                    let stop_reason = message
                        .get("stop_reason")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    if stop_reason == "end_turn" || stop_reason == "stop_sequence" {
                        let _ = tx.send(ResponseEvent::TurnComplete);
                    }
                }
            }
            "user" => {
                if let Some(content) = val
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .or_else(|| val.get("content"))
                    .and_then(|content| content.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|value| value.as_str()) == Some("tool_result")
                            && let Some(content) = block.get("content")
                        {
                            let text = tool_result_text(content);
                            if !text.is_empty() {
                                let _ = tx.send(ResponseEvent::ToolResult(text));
                            }
                        }
                    }
                }
            }
            "tool_use" => {
                let name = val
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input = val
                    .get("input")
                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                    .unwrap_or_default();
                let _ = tx.send(ResponseEvent::ToolUse { name, input });
            }
            "tool_result" => {
                let content_text = val
                    .get("content")
                    .map(tool_result_text)
                    .filter(|text| !text.is_empty())
                    .or_else(|| {
                        val.get("output")
                            .and_then(|output| output.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                if !content_text.is_empty() {
                    let _ = tx.send(ResponseEvent::ToolResult(content_text));
                }
            }
            "result" => {
                if let Some(usage) = ClaudeUsage::from_event(&val) {
                    let _ = tx.send(ResponseEvent::Usage(usage));
                }
                let is_error = val
                    .get("is_error")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                    || val
                        .get("subtype")
                        .and_then(|value| value.as_str())
                        .is_some_and(|subtype| subtype != "success");
                if is_error {
                    let detail = val
                        .get("result")
                        .or_else(|| val.get("error"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("Claude reported an unspecified error")
                        .to_string();
                    let _ = tx.send(ResponseEvent::Failure(detail));
                }
                let _ = tx.send(ResponseEvent::TurnComplete);
            }
            _ => {}
        }
    }
    let _ = tx.send(ResponseEvent::StreamEnd);
}

/// Collect the full assistant response until `TurnComplete`.
/// Streams partial text to the `.streaming` file if given.
fn collect_response(
    rx: &mpsc::Receiver<ResponseEvent>,
    logger: &HandlerLogger,
    timeout: Duration,
    streaming: Option<(&Path, &Path, &str)>,
    session_id_path: Option<&Path>,
) -> Option<CollectedResponse> {
    let deadline = Instant::now() + timeout;
    let mut text_parts: Vec<String> = Vec::new();
    // Interleaved transcript: text blocks verbatim, each tool_use
    // opens a `┌─ Name ────\n│ <input>\n` box that the next
    // tool_result closes with `│ <output>\n└─\n`. Matches the native
    // executor's format so the TUI chat renderer's tool-box styling
    // fires automatically.
    let mut transcript = String::new();
    let mut open_tool: Option<String> = None;
    let mut streaming_text = String::new();
    let mut usage: Option<ClaudeUsage> = None;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            logger.warn("response collection timed out");
            return build_collected(&text_parts, &transcript, usage.as_ref());
        }

        match rx.recv_timeout(remaining) {
            Ok(ResponseEvent::Text(t)) => {
                if let Some((_, wg_dir, chat_ref)) = streaming {
                    streaming_text.push_str(&t);
                    if !t.ends_with('\n') {
                        streaming_text.push('\n');
                    }
                    let _ = chat::write_streaming_ref(wg_dir, chat_ref, &streaming_text);
                }
                text_parts.push(t.clone());
                // Append to the interleaved transcript.
                transcript.push_str(&t);
                if !t.ends_with('\n') {
                    transcript.push('\n');
                }
            }
            Ok(ResponseEvent::ToolUse { name, input }) => {
                // If a previous tool box never saw a tool_result, close it
                // defensively before opening a new one.
                if open_tool.is_some() {
                    transcript.push_str("└─\n");
                }
                let header_rule = "─".repeat(40usize.saturating_sub(name.len() + 4));
                transcript.push_str(&format!("\n┌─ {} {}\n", name, header_rule));
                // For Bash-like tools, Claude's `input` is a JSON string with
                // a `command` field; surface it as `$ cmd` rather than raw JSON.
                let pretty_input = format_tool_input(&name, &input);
                for line in pretty_input.lines() {
                    transcript.push_str(&format!("│ {}\n", line));
                }
                open_tool = Some(name);
            }
            Ok(ResponseEvent::ToolResult(content)) => {
                // Stream the tool output into whichever box is open.
                const MAX_LINES: usize = 15;
                let lines: Vec<&str> = content.lines().collect();
                if lines.is_empty() {
                    transcript.push_str("│ (no output)\n");
                } else if lines.len() > MAX_LINES {
                    for line in &lines[..MAX_LINES] {
                        transcript.push_str(&format!("│ {}\n", line));
                    }
                    transcript
                        .push_str(&format!("│ ... ({} more lines)\n", lines.len() - MAX_LINES));
                } else {
                    for line in &lines {
                        transcript.push_str(&format!("│ {}\n", line));
                    }
                }
                transcript.push_str("└─\n");
                open_tool = None;
            }
            Ok(ResponseEvent::SessionStarted(session_id)) => {
                if let Some(path) = session_id_path {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(error) = worksgood::atomic_file::write_atomic(path, &session_id) {
                        logger.warn(&format!("failed to persist Claude session id: {error}"));
                    } else {
                        logger.info(&format!(
                            "claude-handler: native session id persisted ({session_id})"
                        ));
                    }
                }
            }
            Ok(ResponseEvent::Usage(turn_usage)) => {
                logger.info(&format!(
                    "claude-handler: turn usage input={} output={} cache_read={} cache_write={} cost={}",
                    turn_usage.input_tokens,
                    turn_usage.output_tokens,
                    turn_usage.cache_read_input_tokens,
                    turn_usage.cache_creation_input_tokens,
                    turn_usage
                        .total_cost_usd
                        .map(|cost| format!("{cost:.6}"))
                        .unwrap_or_else(|| "unknown".to_string())
                ));
                usage = Some(turn_usage);
            }
            Ok(ResponseEvent::Failure(detail)) => {
                if open_tool.is_some() {
                    transcript.push_str("└─\n");
                }
                let visible = format!("Claude chat failed: {detail}. Please retry.");
                text_parts.push(visible.clone());
                transcript.push_str(&visible);
                transcript.push('\n');
                return build_collected(&text_parts, &transcript, usage.as_ref());
            }
            Ok(ResponseEvent::TurnComplete) => {
                if text_parts.is_empty() {
                    // The turn completed with only tool calls; keep
                    // waiting for the next turn's text.
                    continue;
                }
                // Defensive close for any dangling open box.
                if open_tool.is_some() {
                    transcript.push_str("└─\n");
                }
                return build_collected(&text_parts, &transcript, usage.as_ref());
            }
            Ok(ResponseEvent::StreamEnd) => {
                logger.warn("stdout stream ended during response collection");
                if open_tool.is_some() {
                    transcript.push_str("└─\n");
                }
                return build_collected(&text_parts, &transcript, usage.as_ref());
            }
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if open_tool.is_some() {
                    transcript.push_str("└─\n");
                }
                return build_collected(&text_parts, &transcript, usage.as_ref());
            }
        }
    }
}

fn build_collected(
    parts: &[String],
    transcript: &str,
    usage: Option<&ClaudeUsage>,
) -> Option<CollectedResponse> {
    let summary = parts.last().cloned().unwrap_or_default();
    if summary.is_empty() {
        return None;
    }
    let mut full_response = transcript.to_string();
    if let Some(usage) = usage {
        if !full_response.ends_with('\n') {
            full_response.push('\n');
        }
        full_response.push('\n');
        full_response.push_str(&usage.render());
        full_response.push('\n');
    }
    Some(CollectedResponse {
        summary,
        full_response,
    })
}

/// Render a tool_use input block into something human-readable.
/// Bash commands become `$ <cmd>`; other tools fall back to a truncated
/// pretty-printed JSON.
fn format_tool_input(name: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if matches!(name, "Bash" | "bash") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(cmd) = val.get("command").and_then(|v| v.as_str())
        {
            return format!("$ {}", cmd);
        }
    }
    // Keep non-bash inputs concise; dump as pretty JSON capped at a
    // few lines so a massive tool arg doesn't balloon the outbox entry.
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(val) => {
            let pretty = serde_json::to_string_pretty(&val).unwrap_or_else(|_| raw.to_string());
            const MAX_LINES: usize = 8;
            let lines: Vec<&str> = pretty.lines().collect();
            if lines.len() > MAX_LINES {
                let mut out = lines[..MAX_LINES].join("\n");
                out.push_str(&format!("\n... ({} more lines)", lines.len() - MAX_LINES));
                out
            } else {
                pretty
            }
        }
        Err(_) => raw.to_string(),
    }
}

// --- SIGINT forwarding -------------------------------------------------------

/// PID of the Claude CLI child process. Set by the handler's main
/// thread before installing the signal handler. The `SIGINT` handler
/// (below) uses it to forward the signal; `libc::kill` is
/// async-signal-safe so this is legal from inside a signal handler.
/// 0 means "no child spawned yet" — the handler ignores SIGINT in
/// that case rather than crashing.
static CLAUDE_CHILD_PID: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn sigint_forwarder(_sig: libc::c_int) {
    // Async-signal-safe: just read the atomic + issue kill.
    let pid = CLAUDE_CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
    }
    // Do NOT exit the handler process — Claude CLI treats SIGINT as
    // "stop generating" and the handler continues processing future
    // inbox messages after the interrupted turn flushes.
}

#[cfg(unix)]
fn install_sigint_forwarder() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_forwarder as *const () as libc::sighandler_t,
        );
    }
}

#[cfg(not(unix))]
fn install_sigint_forwarder() {}

// --- Handler-local logger ----------------------------------------------------

#[derive(Clone)]
struct HandlerLogger {
    inner: Arc<Mutex<HandlerLoggerInner>>,
}

struct HandlerLoggerInner {
    file: std::fs::File,
}

impl HandlerLogger {
    fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open handler log {:?}", path))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(HandlerLoggerInner { file })),
        })
    }

    fn log(&self, level: &str, msg: &str) {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("{} [{}] {}\n", ts, level, msg);
        // Also mirror to stderr so the daemon captures it via its
        // child-stderr pipe (gives operators a single log to tail).
        eprint!("{}", line);
        if let Ok(mut inner) = self.inner.lock() {
            let _ = inner.file.write_all(line.as_bytes());
            let _ = inner.file.flush();
        }
    }

    fn info(&self, msg: &str) {
        self.log("INFO", msg);
    }
    fn warn(&self, msg: &str) {
        self.log("WARN", msg);
    }
    fn error(&self, msg: &str) {
        self.log("ERROR", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tool_input_bash_becomes_shell_prompt() {
        let out = format_tool_input("Bash", r#"{"command":"ls /tmp"}"#);
        assert_eq!(out, "$ ls /tmp");
    }

    #[test]
    fn format_tool_input_non_bash_stays_json() {
        let out = format_tool_input("Read", r#"{"file_path":"/etc/hosts"}"#);
        assert!(out.contains("\"file_path\""));
        assert!(out.contains("/etc/hosts"));
    }

    #[test]
    fn format_tool_input_truncates_huge_json() {
        let big = (0..30)
            .map(|i| format!("\"k{}\":\"v\"", i))
            .collect::<Vec<_>>()
            .join(",");
        let raw = format!("{{{}}}", big);
        let out = format_tool_input("Unknown", &raw);
        assert!(out.contains("more lines"));
    }

    #[test]
    fn build_collected_full_response_has_summary_when_no_tools() {
        let parts = vec!["hello from claude".to_string()];
        let transcript = "hello from claude\n";
        let resp = build_collected(&parts, transcript, None).expect("non-empty");
        assert_eq!(resp.summary, "hello from claude");
        assert_eq!(resp.full_response, "hello from claude\n");
    }

    #[test]
    fn build_collected_renders_native_claude_usage_once() {
        let parts = vec!["done".to_string()];
        let usage = ClaudeUsage {
            input_tokens: 13,
            output_tokens: 5,
            cache_read_input_tokens: 7,
            cache_creation_input_tokens: 2,
            total_cost_usd: Some(0.0123),
        };
        let resp = build_collected(&parts, "done\n", Some(&usage)).expect("non-empty");
        assert_eq!(resp.summary, "done");
        assert_eq!(
            resp.full_response,
            "done\n\n[usage: 13 input · 5 output · 7 cache-read · 2 cache-write · $0.0123]\n"
        );
    }

    #[test]
    fn result_usage_parses_without_changing_native_field_identity() {
        let event = serde_json::json!({
            "type": "result",
            "session_id": "native-session-12",
            "total_cost_usd": 0.0456,
            "usage": {
                "input_tokens": 21,
                "output_tokens": 8,
                "cache_read_input_tokens": 144,
                "cache_creation_input_tokens": 3
            }
        });
        assert_eq!(
            ClaudeUsage::from_event(&event),
            Some(ClaudeUsage {
                input_tokens: 21,
                output_tokens: 8,
                cache_read_input_tokens: 144,
                cache_creation_input_tokens: 3,
                total_cost_usd: Some(0.0456),
            })
        );
    }

    #[test]
    fn canonical_chat_refs_receive_coordinator_contract() {
        for (chat_ref, expected) in [
            ("chat-7", Some(7)),
            ("coordinator-8", Some(8)),
            (".chat-9", Some(9)),
            (".coordinator-10", Some(10)),
            ("worker-11", None),
        ] {
            assert_eq!(parse_coordinator_id(chat_ref), expected, "{chat_ref}");
        }
    }

    #[test]
    fn build_collected_empty_returns_none() {
        let parts: Vec<String> = Vec::new();
        assert!(build_collected(&parts, "", None).is_none());
    }
}
