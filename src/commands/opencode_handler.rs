//! `wg opencode-handler` — OpenCode CLI bridge for multi-turn chat.
//!
//! Peer of `wg codex-handler` / `wg claude-handler` / `wg nex --chat`.
//! Dispatched by `wg spawn-task` when the session's executor is `opencode`.
//!
//! Architecture: `opencode run --format json <message>` is single-shot — it
//! runs one turn and exits. OpenCode does not expose a stable resumable
//! server-side session id the way codex does, so we keep chat state the
//! robust way: replay the full accumulated conversation (reconstructed from
//! the chat inbox/outbox) into the prompt on every turn. Crashes are a
//! non-event (the next turn restarts fresh from the on-disk transcript).
//!
//! ## Explicit-model contract
//!
//! This handler ALWAYS passes the resolved model to opencode with an explicit
//! `--model` flag. If model resolution produced nothing it fails loudly — it
//! NEVER lets opencode fall back to its own internal default. See
//! [`opencode_run_args`].
//!
//! ## Stdout-is-protocol contract
//!
//! Stdout for this handler binary is the protocol stream parent supervisors
//! parse line-by-line. **Never write diagnostic text to stdout from this file
//! or anything it transitively calls** — diagnostics go to stderr or
//! `handler.log`. See `tests/integration_handler_stdout_pristine.rs`.

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use workgraph::chat;
use workgraph::session_lock::{HandlerKind, SessionLock};

const INBOX_POLL: Duration = Duration::from_millis(200);

pub fn run(
    workgraph_dir: &Path,
    chat_ref: &str,
    resume: bool,
    role: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let _ = resume; // accepted for argv symmetry; opencode is single-shot.

    // Explicit-model contract: refuse to start without a resolved model
    // rather than silently inheriting opencode's internal default.
    if opencode_model_arg(model).is_none() {
        anyhow::bail!(
            "opencode-handler requires an explicitly resolved model, but model resolution \
             produced none. Pin a model on the chat/task (e.g. \
             `opencode:openrouter/stepfun/step-3.7-flash`), the active profile, or \
             `[dispatcher].model` — this handler will not fall back to opencode's default."
        );
    }

    // Route through the session registry so aliases resolve to the
    // UUID-backed storage dir — see `chat::chat_dir_for_ref`.
    let chat_dir = workgraph::chat::chat_dir_for_ref(workgraph_dir, chat_ref);
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
        "opencode-handler starting: chat_ref={}, role={:?}, model={:?}",
        chat_ref, role, model
    ));

    let system_prompt = build_handler_system_prompt(workgraph_dir, chat_ref, role);
    let coordinator_id = parse_coordinator_id(chat_ref);

    // Cursor: skip inbox messages already answered (matched by request_id in
    // outbox). Same logic as codex/claude handlers.
    let mut inbox_cursor: u64 = last_answered_inbox_id(workgraph_dir, chat_ref);
    logger.info(&format!(
        "opencode-handler ready: inbox_cursor={}, coordinator_id={:?}",
        inbox_cursor, coordinator_id
    ));

    loop {
        let new_msgs = match chat::read_inbox_since_ref(workgraph_dir, chat_ref, inbox_cursor) {
            Ok(msgs) => msgs,
            Err(e) => {
                logger.warn(&format!("inbox read error: {}", e));
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
                "opencode-handler: processing inbox id={} request_id={} ({} chars)",
                msg.id,
                request_id,
                msg.content.len()
            ));

            // Full-history replay: opencode keeps no server-side session, so
            // every turn carries the system prompt + graph context + the
            // prior conversation transcript + the new user message.
            let prior_turns = prior_conversation(workgraph_dir, chat_ref, msg.id);
            let prompt = assemble_prompt(
                workgraph_dir,
                coordinator_id,
                &system_prompt,
                &prior_turns,
                &msg.content,
            );

            let streaming_path = chat::streaming_path_ref(workgraph_dir, chat_ref);
            let prompt_file = chat_dir.join("opencode-prompt.txt");
            let reply = match run_opencode_turn(&prompt, model, workgraph_dir, &prompt_file, &logger)
            {
                Ok(t) => t,
                Err(e) => {
                    logger.error(&format!("opencode turn failed: {}", e));
                    format!(
                        "The coordinator encountered an error running opencode: {}. Please retry.",
                        e
                    )
                }
            };

            if let Err(e) = chat::append_outbox_ref(workgraph_dir, chat_ref, &reply, &request_id) {
                logger.error(&format!("outbox write failed: {}", e));
            } else {
                logger.info(&format!(
                    "opencode-handler: response written ({} chars) for {}",
                    reply.len(),
                    request_id
                ));
            }

            chat::clear_streaming_ref(workgraph_dir, chat_ref);
            let _ = &streaming_path;
        }
    }
}

fn parse_coordinator_id(chat_ref: &str) -> Option<u32> {
    chat_ref
        .strip_prefix("coordinator-")
        .and_then(|s| s.parse::<u32>().ok())
}

fn last_answered_inbox_id(workgraph_dir: &Path, chat_ref: &str) -> u64 {
    let inbox = match chat::read_inbox_since_ref(workgraph_dir, chat_ref, 0) {
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

/// Reconstruct the prior conversation (everything strictly before
/// `current_inbox_id`) as alternating User/Assistant turns, matched by
/// `request_id`. Returns "" when there is no prior history.
fn prior_conversation(workgraph_dir: &Path, chat_ref: &str, current_inbox_id: u64) -> String {
    let inbox = chat::read_inbox_since_ref(workgraph_dir, chat_ref, 0).unwrap_or_default();
    let outbox = chat::read_outbox_since_ref(workgraph_dir, chat_ref, 0).unwrap_or_default();
    let mut out = String::new();
    for in_msg in inbox.iter().filter(|m| m.id < current_inbox_id) {
        out.push_str("User: ");
        out.push_str(in_msg.content.trim());
        out.push_str("\n\n");
        if let Some(reply) = outbox
            .iter()
            .find(|o| !o.request_id.is_empty() && o.request_id == in_msg.request_id)
        {
            out.push_str("Assistant: ");
            out.push_str(reply.content.trim());
            out.push_str("\n\n");
        }
    }
    out
}

fn build_handler_system_prompt(workgraph_dir: &Path, chat_ref: &str, role: Option<&str>) -> String {
    if chat_ref.starts_with("coordinator-") || role == Some("coordinator") {
        crate::commands::service::coordinator_agent::build_system_prompt(workgraph_dir)
    } else if let Some(r) = role {
        format!("You are acting in the role of: {}.", r)
    } else {
        String::from("You are a WG task agent.")
    }
}

/// Assemble the single prompt sent to `opencode run`. Because opencode is
/// stateless we always include the system prompt, the live graph context,
/// the prior transcript, and the latest user message.
fn assemble_prompt(
    workgraph_dir: &Path,
    coordinator_id: Option<u32>,
    system_prompt: &str,
    prior_turns: &str,
    latest_user_msg: &str,
) -> String {
    let mut out = String::new();
    out.push_str("# System\n");
    out.push_str(system_prompt);
    out.push_str("\n\n");

    if let Some(cid) = coordinator_id
        && let Ok(ctx) = crate::commands::service::coordinator_agent::build_coordinator_context(
            workgraph_dir,
            "1970-01-01T00:00:00Z",
            None,
            cid,
        )
        && !ctx.is_empty()
    {
        out.push_str(&ctx);
        out.push_str("\n\n");
    }

    if !prior_turns.trim().is_empty() {
        out.push_str("# Conversation so far\n");
        out.push_str(prior_turns);
        out.push('\n');
    }

    out.push_str("# User\n");
    out.push_str(latest_user_msg);
    out.push_str(
        "\n\nRespond to the user. Use `wg` shell tools to inspect the graph when the answer \
         requires live state. Keep your reply concise.",
    );
    out
}

/// Convert a WG model spec into the model string opencode expects on its
/// `--model` flag. OpenRouter routes become `openrouter/<vendor>/<model>`;
/// anything else passes through as the bare model id. Returns `None` when no
/// model is resolved — the caller treats that as a hard error.
fn opencode_model_arg(model: Option<&str>) -> Option<String> {
    let raw = model?.trim();
    if raw.is_empty() {
        return None;
    }
    // The handler normally receives the already-normalized inner model
    // (`openrouter:vendor/model`), but accept a raw executor-qualified route
    // (`opencode:openrouter/vendor/model`) defensively by stripping the
    // external-CLI executor prefix.
    let m = match raw.split_once(':') {
        Some((prefix, rest))
            if !rest.trim().is_empty()
                && workgraph::dispatch::ExecutorKind::from_str(prefix)
                    .is_some_and(|k| k.is_external_cli()) =>
        {
            rest.trim()
        }
        _ => raw,
    };
    // Delegate to the canonical normalizer so the worker handler path and the
    // TUI/interactive PTY path (chat_command::opencode_model_arg) agree on every
    // spelling — including a bare `vendor/model` route such as
    // `minimax/minimax-m3`, which must become `openrouter/minimax/minimax-m3`
    // rather than being passed through (OpenCode can't resolve provider
    // `minimax` and silently falls back to its default model).
    workgraph::chat_command::opencode_model_arg(m)
}

/// Build the argv (excluding the `opencode` binary itself) for one
/// non-interactive run. Factored out so tests can assert the model is always
/// on the command line WITHOUT spawning opencode (test B). Errors if no model
/// resolves — enforcing the explicit-model contract.
fn opencode_run_args(model: Option<&str>, prompt_file: &Path) -> Result<Vec<String>> {
    let model_arg = opencode_model_arg(model).ok_or_else(|| {
        anyhow::anyhow!("opencode requires an explicit --model; model resolution produced none")
    })?;
    Ok(vec![
        "run".to_string(),
        // The positional message MUST come BEFORE `--file`. opencode's `--file`
        // is a *variadic* (array) option: any positional token that follows it
        // is greedily consumed as another file path. With the message trailing
        // `--file`, opencode (>=1.16) treats the message as a missing file and
        // exits 1 with `Error: File not found: <message>`. Keeping the message
        // first leaves `--file` to consume only the real prompt-file path.
        "Respond to the attached WG conversation prompt.".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model_arg,
        "--file".to_string(),
        prompt_file.to_string_lossy().to_string(),
    ])
}

/// Extract the opencode session id from a `run --format json` event stream.
/// Every emitted event (`step_start`, …) carries `sessionID`; we return the
/// first non-empty one. Needed because opencode persists the assistant reply
/// to its session store rather than streaming it on `run` stdout — we recover
/// the reply via `opencode export <sessionID>` (see [`fetch_reply_via_export`]).
fn parse_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(s) = val.get("sessionID").and_then(|v| v.as_str())
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_string());
        }
    }
    None
}

/// Parse `opencode export <session>` JSON and return the assistant's reply:
/// the concatenated NON-synthetic `text` parts of the LAST assistant message.
///
/// opencode (>=1.16) does NOT stream assistant text on `run --format json`
/// stdout — that stream carries only `step_start` events. The actual reply is
/// persisted to opencode's session store and surfaced by `opencode export`,
/// whose shape is `{ "messages": [ { "info": {"role": ...}, "parts": [ {
/// "type": "text", "synthetic": <bool>, "text": ... } ] } ] }`. Synthetic text
/// parts (tool-call echoes, file-attachment dumps, the user-message echo) are
/// skipped so only the model's own prose is returned.
fn extract_export_reply(export_json: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(export_json).ok()?;
    let messages = val.get("messages")?.as_array()?;
    let mut reply: Option<String> = None;
    for msg in messages {
        let role = msg
            .get("info")
            .and_then(|i| i.get("role"))
            .and_then(|r| r.as_str());
        if role != Some("assistant") {
            continue;
        }
        let mut texts: Vec<String> = Vec::new();
        if let Some(parts) = msg.get("parts").and_then(|p| p.as_array()) {
            for part in parts {
                if part.get("type").and_then(|t| t.as_str()) != Some("text") {
                    continue;
                }
                if part
                    .get("synthetic")
                    .and_then(|s| s.as_bool())
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(t) = part.get("text").and_then(|t| t.as_str())
                    && !t.trim().is_empty()
                {
                    texts.push(t.trim().to_string());
                }
            }
        }
        // Keep the LAST assistant message that produced prose.
        if !texts.is_empty() {
            reply = Some(texts.join("\n"));
        }
    }
    reply
}

/// Run `opencode export <session_id>` and extract the assistant reply.
/// Returns `None` (with a logged warning) on any failure so the caller can
/// fall back to parsing the run stdout directly.
fn fetch_reply_via_export(
    session_id: &str,
    cwd: &Path,
    logger: &HandlerLogger,
) -> Option<String> {
    let mut cmd = Command::new("opencode");
    cmd.args(["export", session_id]);
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            logger.warn(&format!("opencode export spawn failed: {}", e));
            return None;
        }
    };
    if !output.status.success() {
        logger.warn(&format!(
            "opencode export exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return None;
    }
    extract_export_reply(&String::from_utf8_lossy(&output.stdout))
}

/// Extract the assistant's reply text from opencode's output. Tries JSON
/// shapes first (`--format json`), then falls back to the raw trimmed stdout
/// so an unexpected schema still yields the model's text rather than nothing.
fn extract_reply(stdout: &str) -> String {
    let trimmed = stdout.trim();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in ["text", "content", "message", "response", "output", "result"] {
            if let Some(s) = val.get(key).and_then(|v| v.as_str())
                && !s.trim().is_empty()
            {
                return s.trim().to_string();
            }
        }
        // `{ "parts": [{ "text": "..." }] }` / `{ "messages": [...] }` shapes:
        // walk the JSON for the last non-empty string under a "text" key.
        if let Some(text) = deep_find_last_text(&val)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
    }
    trimmed.to_string()
}

/// In-order walk collecting the LAST non-empty `"text"` string value in a
/// JSON document — robust to opencode emitting an array of message parts
/// where the final part holds the complete reply.
fn deep_find_last_text(val: &serde_json::Value) -> Option<String> {
    fn walk(node: &serde_json::Value, found: &mut Option<String>) {
        match node {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    if k == "text"
                        && let serde_json::Value::String(s) = v
                        && !s.trim().is_empty()
                    {
                        *found = Some(s.clone());
                    }
                    walk(v, found);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    walk(v, found);
                }
            }
            _ => {}
        }
    }
    let mut found = None;
    walk(val, &mut found);
    found
}

/// Spawn `opencode run` for one turn, feeding the assembled prompt via a
/// file and capturing the reply text. Returns the assistant's reply.
fn run_opencode_turn(
    prompt: &str,
    model: Option<&str>,
    workgraph_dir: &Path,
    prompt_file: &Path,
    logger: &HandlerLogger,
) -> Result<String> {
    // Write the prompt to a per-chat file so a huge transcript never blows the
    // argv length limit, and concurrent chats never clobber each other.
    std::fs::write(prompt_file, prompt)
        .with_context(|| format!("write opencode prompt file {:?}", prompt_file))?;

    let args = opencode_run_args(model, prompt_file)?;
    let cwd = workgraph_dir.parent().unwrap_or(workgraph_dir);

    logger.info(&format!(
        "opencode-handler: spawning `opencode {}` (cwd={:?})",
        args.join(" "),
        cwd
    ));

    let mut cmd = Command::new("opencode");
    cmd.args(&args);
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawn opencode")?;
    let stdout = child.stdout.take().context("opencode stdout take")?;
    let mut stdout_buf = String::new();
    let _ = BufReader::new(stdout).read_to_string(&mut stdout_buf);

    let stderr_output = child
        .stderr
        .take()
        .map(|stderr| {
            let mut buf = String::new();
            let _ = Read::read_to_string(&mut BufReader::new(stderr), &mut buf);
            buf
        })
        .unwrap_or_default();

    let status = child.wait().context("opencode wait")?;
    if !status.success() {
        let stderr_trimmed = stderr_output.trim();
        if stderr_trimmed.is_empty() {
            anyhow::bail!("opencode run exited {}", status);
        } else {
            anyhow::bail!("opencode run exited {}: {}", status, stderr_trimmed);
        }
    }

    // opencode (>=1.16) does NOT stream the assistant text on `run` stdout — it
    // emits only `step_start` events there and persists the reply to its
    // session store. Recover the reply by capturing the session id from the
    // event stream and running `opencode export <sessionID>`. Fall back to
    // parsing the run stdout directly for older/other builds that DO stream the
    // assistant text inline.
    if let Some(session_id) = parse_session_id(&stdout_buf) {
        logger.info(&format!(
            "opencode-handler: retrieving reply via `opencode export {}`",
            session_id
        ));
        if let Some(reply) = fetch_reply_via_export(&session_id, cwd, logger)
            && !reply.trim().is_empty()
        {
            return Ok(reply);
        }
        logger.warn("opencode export yielded no assistant text; falling back to stdout parse");
    }

    let reply = extract_reply(&stdout_buf);
    if reply.trim().is_empty() {
        anyhow::bail!("opencode produced no reply text on stdout");
    }
    Ok(reply)
}

// --- Handler-local logger ----------------------------------------------------

#[derive(Clone)]
struct HandlerLogger {
    inner: std::sync::Arc<std::sync::Mutex<HandlerLoggerInner>>,
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
            inner: std::sync::Arc::new(std::sync::Mutex::new(HandlerLoggerInner { file })),
        })
    }

    fn log(&self, level: &str, msg: &str) {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("{} [{}] {}\n", ts, level, msg);
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
    use std::path::PathBuf;

    // --- Test B: the model is ALWAYS on the command line; None is an error --

    #[test]
    fn test_opencode_run_args_includes_resolved_model_explicitly() {
        let pf = PathBuf::from("/tmp/p.txt");
        let args = opencode_run_args(Some("opencode:openrouter/stepfun/step-3.7-flash"), &pf)
            .expect("model resolves");
        // `--model` flag present with the openrouter slash spelling.
        let idx = args.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(args[idx + 1], "openrouter/stepfun/step-3.7-flash");
        assert!(args.contains(&"run".to_string()));
    }

    // Regression: opencode's `--file` is a variadic option that swallows any
    // trailing positional as a second file path (exit 1, "File not found:
    // <message>"). The positional message MUST precede `--file`.
    #[test]
    fn test_opencode_run_args_message_precedes_file() {
        let pf = PathBuf::from("/tmp/p.txt");
        let args = opencode_run_args(Some("openrouter:stepfun/step-3.7-flash"), &pf).unwrap();
        let msg_idx = args
            .iter()
            .position(|a| a.starts_with("Respond to the attached"))
            .expect("positional message present");
        let file_idx = args.iter().position(|a| a == "--file").expect("--file present");
        assert!(
            msg_idx < file_idx,
            "positional message must come before --file (else --file swallows it): {:?}",
            args
        );
        // The very last argv token must be the prompt-file path, not the message.
        assert_eq!(args.last().map(String::as_str), Some("/tmp/p.txt"));
    }

    #[test]
    fn test_parse_session_id_from_step_start() {
        let stdout = r#"{"type":"step_start","timestamp":1,"sessionID":"ses_abc123","part":{"type":"step-start"}}
"#;
        assert_eq!(parse_session_id(stdout).as_deref(), Some("ses_abc123"));
        assert_eq!(parse_session_id(""), None);
        assert_eq!(parse_session_id("not json\n"), None);
    }

    #[test]
    fn test_extract_export_reply_skips_synthetic_and_user_parts() {
        // Mirrors opencode 1.16 `export` output: synthetic tool/file echoes
        // under user/assistant, then the real assistant reply.
        let export = r#"{
          "info": {"id": "ses_x"},
          "messages": [
            {"info": {"role": "user"}, "parts": [
              {"type": "text", "synthetic": true, "text": "Called the Read tool ..."},
              {"type": "text", "synthetic": false, "text": "\"Respond to the attached prompt.\""}
            ]},
            {"info": {"role": "assistant"}, "parts": [
              {"type": "step-start"},
              {"type": "text", "synthetic": false, "text": "OK"}
            ]}
          ]
        }"#;
        assert_eq!(extract_export_reply(export).as_deref(), Some("OK"));
    }

    #[test]
    fn test_extract_export_reply_last_assistant_message_wins() {
        let export = r#"{"messages": [
            {"info": {"role": "assistant"}, "parts": [{"type":"text","text":"first"}]},
            {"info": {"role": "user"}, "parts": [{"type":"text","text":"middle"}]},
            {"info": {"role": "assistant"}, "parts": [{"type":"text","text":"final answer"}]}
        ]}"#;
        assert_eq!(extract_export_reply(export).as_deref(), Some("final answer"));
        assert_eq!(extract_export_reply("garbage"), None);
        assert_eq!(extract_export_reply(r#"{"messages":[]}"#), None);
    }

    #[test]
    fn test_opencode_run_args_bare_openrouter_model_passed_explicitly() {
        let pf = PathBuf::from("/tmp/p.txt");
        // The inner model as normalized by parse_executor_model_route.
        let args = opencode_run_args(Some("openrouter:minimax/minimax-m2.7"), &pf).unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--model openrouter/minimax/minimax-m2.7"),
            "model must appear explicitly on the command line: {}",
            joined
        );
    }

    #[test]
    fn test_opencode_run_args_errors_when_model_is_none() {
        let pf = PathBuf::from("/tmp/p.txt");
        assert!(
            opencode_run_args(None, &pf).is_err(),
            "no resolved model must be a hard error, never a silent default"
        );
        assert!(opencode_run_args(Some("   "), &pf).is_err());
    }

    #[test]
    fn test_opencode_model_arg_shapes() {
        assert_eq!(
            opencode_model_arg(Some("openrouter:stepfun/step-3.7-flash")).as_deref(),
            Some("openrouter/stepfun/step-3.7-flash")
        );
        assert_eq!(opencode_model_arg(None), None);
        assert_eq!(opencode_model_arg(Some("")), None);
    }

    #[test]
    fn test_opencode_model_arg_minimax_all_spellings() {
        // The worker handler path must agree with the TUI/interactive path on
        // every minimax spelling — all three resolve to the OpenRouter route,
        // never a bare `minimax/minimax-m3` (→ opencode default fallback).
        for spelling in [
            "opencode:openrouter/minimax/minimax-m3",
            "openrouter:minimax/minimax-m3",
            "openrouter/minimax/minimax-m3",
            "minimax/minimax-m3",
        ] {
            assert_eq!(
                opencode_model_arg(Some(spelling)).as_deref(),
                Some("openrouter/minimax/minimax-m3"),
                "spelling {spelling:?} must normalize to the openrouter route"
            );
        }
    }

    #[test]
    fn test_opencode_run_args_bare_minimax_route_gets_openrouter_prefix() {
        let pf = PathBuf::from("/tmp/p.txt");
        let args = opencode_run_args(Some("minimax/minimax-m3"), &pf).unwrap();
        let joined = args.join(" ");
        assert!(
            joined.contains("--model openrouter/minimax/minimax-m3"),
            "bare vendor/model route must be normalized to the openrouter route: {joined}"
        );
    }

    #[test]
    fn test_extract_reply_prefers_json_text_then_falls_back() {
        assert_eq!(extract_reply(r#"{"text":"hello there"}"#), "hello there");
        assert_eq!(
            extract_reply(r#"{"parts":[{"text":"a"},{"text":"final answer"}]}"#),
            "final answer"
        );
        // Non-JSON stdout falls back to the trimmed raw text.
        assert_eq!(extract_reply("  plain reply  "), "plain reply");
    }

    #[test]
    fn test_prior_conversation_empty_when_no_history() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(prior_conversation(dir.path(), "coordinator-1", 1), "");
    }
}
