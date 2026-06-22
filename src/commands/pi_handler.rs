//! `wg pi-handler` — pi.dev bridge for multi-turn chat, routed THROUGH the
//! `wg-pi-plugin`, not via prompt-munging (`integration-plan-v2.md` §2.1, §4;
//! the plugin-first ADAPT of the old wrapper `pi-impl-p1a-handler`).
//!
//! Peer of `wg codex-handler` / `wg claude-handler` / `wg opencode-handler`.
//! Dispatched by `wg spawn-task` when the session's executor is `pi`.
//!
//! ## Two deployment topologies (`integration-plan-v2.md` §2.1)
//!
//! - **Topology A — RPC + auto-loaded plugin (ship first):** spawn a long-lived
//!   `pi --mode rpc` with the wg-pi-plugin present (installed in
//!   `~/.pi/agent/extensions/`, via `pi -e <plugin>`, or settings `packages`).
//!   stdio is piped (headless ⇒ no terminal takeover, Axis 2 (b)). Each WG inbox
//!   message becomes one JSONL `prompt` command; we read pi's JSONL event stream
//!   until `agent_end` and write the assistant text to the WG outbox.
//! - **Topology B — SDK Node host (default for unattended):** spawn
//!   `node pi-plugin/host/wg-pi-host.mjs` (from `pi-plugin-impl-package`), which
//!   loads the plugin in-process via `DefaultResourceLoader` and bridges the
//!   plugin event bus to WG over stdio. No terminal is ever grabbed.
//!
//! Topology is auto-selected from [`executor_discovery::pi_route_availability`]
//! (prefer A when a `pi` binary is present — smallest delta; else B when the
//! Node host + built bundle are present). `WG_PI_TOPOLOGY=rpc|node` forces one.
//!
//! ## Explicit-model contract
//!
//! Like the opencode handler, this ALWAYS resolves the model explicitly and
//! refuses to start without one — it never lets pi fall back to its own default.
//! The model becomes pi's `--provider <p> --model <m>` pair ([`pi_model_arg`]);
//! credentials are supplied by environment only (`OPENROUTER_API_KEY` /
//! `ANTHROPIC_API_KEY` / …), **never** via `--api-key`.
//!
//! ## LF-only RPC framing
//!
//! pi's RPC records are split on `\n` only — a generic line reader that also
//! breaks on `U+2028`/`U+2029` (Node `readline`) is non-compliant because those
//! bytes occur inside JSON strings (`docs/rpc.md`). We frame with
//! [`std::io::BufRead::read_until`]`(b'\n')`.
//!
//! ## Stdout-is-protocol contract
//!
//! Our stdout is the supervisor protocol stream. Never write diagnostics to it
//! from this file or anything it transitively calls — diagnostics go to stderr
//! or `handler.log`, and replies go to the chat outbox (file-based, like
//! opencode). The child's stdout is captured via a pipe; its stderr inherits.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use worksgood::chat;
use worksgood::executor_discovery::{self, PiNodeHost, PiRouteAvailability};
use worksgood::session_lock::{HandlerKind, SessionLock};

const INBOX_POLL: Duration = Duration::from_millis(200);
/// How long to wait for the FIRST byte of a Node-host turn before giving up.
const NODE_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(180);
/// Once a Node-host turn has produced output, treat this much silence (with no
/// explicit turn-end event) as turn completion. The as-built host
/// (`pi-plugin-impl-package`) emits text deltas but no turn-end marker, so we
/// detect quiescence; an explicit `turn_end`/`agent_end`/`done` event short-
/// circuits this when a future host emits one.
const NODE_IDLE_QUIESCE: Duration = Duration::from_millis(1500);

// --- model argument mapping ---------------------------------------------------

/// The `--provider`/`--model` pair pi expects on its argv. Credentials are
/// supplied by env only (never `--api-key`), so this carries no key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiModelArg {
    /// pi provider name (`openrouter`, `anthropic`, `oai-compat`, …).
    pub provider: String,
    /// The model id pi sends to that provider (e.g. `anthropic/claude-3.5-haiku`).
    pub model: String,
}

/// Convert a WG model spec into pi's `--provider <p> --model <m>` arguments.
///
/// Mirrors `opencode_model_arg`: `pi` is an executor name (kept out of
/// `KNOWN_PROVIDERS`), so a leading `pi:` is stripped here rather than by
/// `parse_model_spec`. A provider-qualified spec maps the provider to pi's
/// native name and strips a redundant `openrouter/` CLI prefix off the model
/// id; a bare `vendor/model` (or CLI-slash `openrouter/vendor/model`) route is
/// an OpenRouter route. Returns `None` when no model resolves (a hard error for
/// the caller) or when a bare single-token alias gives pi no provider to use.
pub fn pi_model_arg(model: Option<&str>) -> Option<PiModelArg> {
    let raw = model?.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip the `pi:` executor prefix when present (defensive — the handler
    // normally receives the normalized inner model already).
    let inner = raw.strip_prefix("pi:").unwrap_or(raw).trim();
    if inner.is_empty() {
        return None;
    }

    let spec = worksgood::config::parse_model_spec(inner);
    let (provider, model_id) = match spec.provider.as_deref() {
        Some(prov) => {
            let native = worksgood::config::provider_to_native_provider(prov);
            let id = if native == "openrouter" {
                spec.model_id
                    .strip_prefix("openrouter/")
                    .unwrap_or(&spec.model_id)
                    .to_string()
            } else {
                spec.model_id.clone()
            };
            (native.to_string(), id)
        }
        None => {
            // No provider prefix: CLI-slash `openrouter/vendor/model`, a bare
            // `vendor/model` OpenRouter route, or a bare single-token alias.
            let id = spec.model_id.as_str();
            if let Some(route) = id.strip_prefix("openrouter/") {
                ("openrouter".to_string(), route.to_string())
            } else if id.contains('/') {
                ("openrouter".to_string(), id.to_string())
            } else {
                // A bare alias gives pi no provider to target — unresolved.
                return None;
            }
        }
    };

    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(PiModelArg {
        provider,
        model: model_id,
    })
}

// --- RPC event parsing --------------------------------------------------------

/// Accumulates one RPC turn's events into a final reply. Text arrives as a
/// stream of `message_update`/`text_delta` events (the documented, schema-stable
/// path); `agent_end` marks the turn idle and may also carry the final
/// messages; a `get_last_assistant_text` `response` carries `data.text`.
#[derive(Debug, Default)]
struct RpcTurnAccumulator {
    /// Concatenated `text_delta` deltas seen so far this turn.
    deltas: String,
    /// Final text recovered from `agent_end` messages or `get_last_assistant_text`.
    final_text: Option<String>,
    /// Set once `agent_end` is seen — the turn is idle.
    ended: bool,
    /// Set when pi reports a failed command / error event.
    error: Option<String>,
}

impl RpcTurnAccumulator {
    /// Ingest one parsed JSONL event. Unknown event types are ignored.
    fn ingest(&mut self, val: &serde_json::Value) {
        let ty = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "message_update" => {
                if let Some(ev) = val.get("assistantMessageEvent")
                    && ev.get("type").and_then(|t| t.as_str()) == Some("text_delta")
                    && let Some(delta) = ev.get("delta").and_then(|d| d.as_str())
                {
                    self.deltas.push_str(delta);
                }
            }
            "agent_end" => {
                self.ended = true;
                // Some builds carry the final assistant text on agent_end; keep
                // it as a fallback for when no deltas streamed.
                if let Some(text) = final_event_assistant_text(val) {
                    self.final_text = Some(text);
                }
            }
            "response" => {
                // `get_last_assistant_text` reply: `{...,"data":{"text":...}}`.
                if let Some(text) = val
                    .get("data")
                    .and_then(|d| d.get("text"))
                    .and_then(|t| t.as_str())
                    && !text.trim().is_empty()
                {
                    self.final_text = Some(text.to_string());
                }
                // A failed command surfaces as success:false (+ optional error).
                if val.get("success").and_then(|s| s.as_bool()) == Some(false) {
                    let msg = val
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("pi reported an unsuccessful command")
                        .to_string();
                    self.error = Some(msg);
                }
            }
            "error" => {
                let msg = val
                    .get("error")
                    .and_then(|e| e.as_str())
                    .or_else(|| val.get("message").and_then(|m| m.as_str()))
                    .unwrap_or("pi emitted an error event")
                    .to_string();
                self.error = Some(msg);
            }
            _ => {}
        }
    }

    /// The reply text: streamed deltas win; else the recovered final text.
    fn reply(&self) -> Option<String> {
        let d = self.deltas.trim();
        if !d.is_empty() {
            return Some(d.to_string());
        }
        self.final_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

/// Parse one framed RPC line into a JSON value, skipping blanks/non-JSON.
fn parse_rpc_line(line: &str) -> Option<serde_json::Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Find the last assistant text in a final event. Prefer an explicit
/// `messages[*].role == "assistant"` payload when present; fall back to an
/// in-order deep scan for older/looser event shapes.
fn final_event_assistant_text(val: &serde_json::Value) -> Option<String> {
    let mut found = None;
    if let Some(messages) = val.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if msg.get("role").and_then(|r| r.as_str()) == Some("assistant")
                && let Some(text) = deep_find_last_text(msg)
            {
                found = Some(text);
            }
        }
    }
    found.or_else(|| deep_find_last_text(val))
}

/// In-order walk collecting the LAST non-empty `"text"` string value in a JSON
/// document. Used as a permissive fallback for pi event shapes that do not carry
/// a conventional `messages[].role` structure.
fn deep_find_last_text(val: &serde_json::Value) -> Option<String> {
    fn walk(node: &serde_json::Value, found: &mut Option<String>) {
        match node {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    if k == "text"
                        && let serde_json::Value::String(s) = v
                        && !s.trim().is_empty()
                    {
                        *found = Some(s.trim().to_string());
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

/// Extract the assistant reply from a complete RPC turn event stream. Test-only
/// over canned JSONL fixtures; the live transport accumulates incrementally via
/// [`RpcTurnAccumulator`] inside `send_turn`.
#[cfg(test)]
fn extract_rpc_reply(stream: &str) -> Option<String> {
    let mut acc = RpcTurnAccumulator::default();
    for line in stream.lines() {
        if let Some(val) = parse_rpc_line(line) {
            acc.ingest(&val);
        }
    }
    acc.reply()
}

// --- transport abstraction ----------------------------------------------------

/// One live turn against a pi backend: send `prompt`, stream accumulated text
/// to `streamer` as it arrives, return the final reply. `streamer` receives the
/// FULL accumulated text each time (so the caller can overwrite `.streaming`).
trait PiTransport {
    fn send_turn(&mut self, prompt: &str, streamer: &mut dyn FnMut(&str)) -> Result<String>;
}

// --- Topology A: long-lived `pi --mode rpc` ----------------------------------

struct RpcTransport {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    req_counter: u64,
    logger: HandlerLogger,
}

impl RpcTransport {
    /// Spawn `pi --mode rpc` with the resolved provider/model and a per-chat
    /// session dir (so a crash/restart resumes the same `--session-id`).
    fn spawn(
        pi_binary: &Path,
        marg: &PiModelArg,
        session_id: &str,
        session_dir: &Path,
        logger: &HandlerLogger,
    ) -> Result<Self> {
        std::fs::create_dir_all(session_dir).ok();
        let args = rpc_spawn_args(marg, session_id, session_dir);
        logger.info(&format!(
            "pi-handler: spawning `{} {}`",
            pi_binary.display(),
            args.join(" ")
        ));
        let mut cmd = Command::new(pi_binary);
        cmd.args(&args);
        // Piped stdio defeats the terminal takeover (Axis 2 (b)); stderr
        // inherits so pi diagnostics reach our stderr / handler.log.
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        // Credentials by env only — never --api-key. The provider key is
        // expected to already be in the environment.
        cmd.env("PI_CODING_AGENT_SESSION_DIR", session_dir);

        let mut child = cmd.spawn().context("spawn `pi --mode rpc`")?;
        let stdin = child.stdin.take().context("pi rpc stdin")?;
        let stdout = child.stdout.take().context("pi rpc stdout")?;
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            req_counter: 0,
            logger: logger.clone(),
        })
    }
}

impl PiTransport for RpcTransport {
    fn send_turn(&mut self, prompt: &str, streamer: &mut dyn FnMut(&str)) -> Result<String> {
        self.req_counter += 1;
        let id = format!("req-{}", self.req_counter);
        let command = serde_json::json!({
            "id": id,
            "type": "prompt",
            "message": prompt,
        });
        // One LF-terminated JSONL command (never \r\n-only).
        let mut line = serde_json::to_string(&command)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .context("write pi rpc prompt command")?;
        self.stdin.flush().context("flush pi rpc stdin")?;

        let mut acc = RpcTurnAccumulator::default();
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            // LF-only framing — `read_until(b'\n')`, NOT a Unicode line reader.
            let n = self
                .reader
                .read_until(b'\n', &mut buf)
                .context("read pi rpc event")?;
            if n == 0 {
                anyhow::bail!("pi rpc stream closed before agent_end");
            }
            let text = String::from_utf8_lossy(&buf);
            if let Some(val) = parse_rpc_line(&text) {
                acc.ingest(&val);
                // Live streaming: push the accumulated text out as it grows.
                if !acc.deltas.is_empty() {
                    streamer(&acc.deltas);
                }
                if let Some(err) = &acc.error {
                    anyhow::bail!("pi rpc error: {}", err);
                }
                if acc.ended {
                    break;
                }
            }
        }

        // If the turn ended with no streamed text and no embedded final text,
        // ask pi explicitly for the last assistant text (docs/rpc.md).
        if acc.reply().is_none() {
            self.logger
                .info("pi-handler: agent_end carried no text; requesting get_last_assistant_text");
            if let Ok(text) = self.request_last_assistant_text(&mut acc) {
                let _ = text;
            }
        }

        acc.reply()
            .ok_or_else(|| anyhow::anyhow!("pi produced no assistant text for this turn"))
    }
}

impl RpcTransport {
    /// Send `{"type":"get_last_assistant_text"}` and fold the response into the
    /// accumulator. Best-effort: a missing/short reply just leaves `acc` as-is.
    fn request_last_assistant_text(&mut self, acc: &mut RpcTurnAccumulator) -> Result<()> {
        self.req_counter += 1;
        let id = format!("req-{}", self.req_counter);
        let mut line = serde_json::to_string(&serde_json::json!({
            "id": id,
            "type": "get_last_assistant_text",
        }))?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        let mut buf: Vec<u8> = Vec::new();
        // Read a bounded number of lines for the response.
        for _ in 0..64 {
            buf.clear();
            let n = self.reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buf);
            if let Some(val) = parse_rpc_line(&text) {
                let is_response = val.get("type").and_then(|t| t.as_str()) == Some("response");
                acc.ingest(&val);
                if is_response {
                    break;
                }
            }
        }
        Ok(())
    }
}

impl Drop for RpcTransport {
    fn drop(&mut self) {
        // Best-effort graceful shutdown, then ensure the child is reaped.
        let _ = self.stdin.write_all(b"{\"type\":\"shutdown\"}\n");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build the argv (excluding the `pi` binary) for a `--mode rpc` spawn. Factored
/// out so tests can assert the model is always present and credentials are never
/// passed via `--api-key`.
fn rpc_spawn_args(marg: &PiModelArg, session_id: &str, session_dir: &Path) -> Vec<String> {
    vec![
        "--mode".to_string(),
        "rpc".to_string(),
        "--provider".to_string(),
        marg.provider.clone(),
        "--model".to_string(),
        marg.model.clone(),
        "--session-id".to_string(),
        session_id.to_string(),
        "--session-dir".to_string(),
        session_dir.to_string_lossy().to_string(),
        "--no-approve".to_string(),
    ]
}

// --- Topology B: `node wg-pi-host.mjs` ---------------------------------------

/// A line received from the Node host's stdout, already JSON-parsed.
enum NodeLine {
    Json(serde_json::Value),
    Eof,
}

struct NodeHostTransport {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<NodeLine>,
}

impl NodeHostTransport {
    /// Spawn `node <host_script>` and wait for its `{"type":"ready"}` line. The
    /// host loads the plugin in-process and bridges its event bus to stdio.
    fn spawn(host: &PiNodeHost, marg: &PiModelArg, logger: &HandlerLogger) -> Result<Self> {
        logger.info(&format!(
            "pi-handler: spawning `{} {}` (Topology B node host)",
            host.node.display(),
            host.host_script.display()
        ));
        let mut cmd = Command::new(&host.node);
        cmd.arg(&host.host_script);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        // The plugin/host reads provider+model + credentials from the
        // environment (never an --api-key flag). Pass the resolved route so a
        // future host can register the matching provider.
        cmd.env("WG_PI_PROVIDER", &marg.provider);
        cmd.env("WG_PI_MODEL", &marg.model);

        let mut child = cmd.spawn().context("spawn `node wg-pi-host.mjs`")?;
        let stdin = child.stdin.take().context("node host stdin")?;
        let stdout = child.stdout.take().context("node host stdout")?;

        // Reader thread: forward each parsed JSON line over a channel so the
        // turn loop can apply idle-quiescence timeouts.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => {
                        let _ = tx.send(NodeLine::Eof);
                        break;
                    }
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buf);
                        if let Some(val) = parse_rpc_line(&text) {
                            if tx.send(NodeLine::Json(val)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(NodeLine::Eof);
                        break;
                    }
                }
            }
        });

        let transport = Self { child, stdin, rx };
        transport.await_ready()?;
        Ok(transport)
    }

    /// Block until the host emits `{"type":"ready"}` (or fails to start).
    fn await_ready(&self) -> Result<()> {
        let deadline = Instant::now() + NODE_FIRST_TOKEN_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("node host did not become ready within timeout");
            }
            match self.rx.recv_timeout(remaining) {
                Ok(NodeLine::Json(val)) => {
                    if val.get("type").and_then(|t| t.as_str()) == Some("ready") {
                        return Ok(());
                    }
                    if val.get("type").and_then(|t| t.as_str()) == Some("error") {
                        anyhow::bail!(
                            "node host error before ready: {}",
                            val.get("error").and_then(|e| e.as_str()).unwrap_or("?")
                        );
                    }
                }
                Ok(NodeLine::Eof) => anyhow::bail!("node host exited before ready"),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    anyhow::bail!("node host did not become ready within timeout")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("node host reader disconnected before ready")
                }
            }
        }
    }
}

impl PiTransport for NodeHostTransport {
    fn send_turn(&mut self, prompt: &str, streamer: &mut dyn FnMut(&str)) -> Result<String> {
        let mut line = serde_json::to_string(&serde_json::json!({
            "type": "prompt",
            "text": prompt,
        }))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .context("write node host prompt")?;
        self.stdin.flush().context("flush node host stdin")?;

        let mut acc = String::new();
        let mut got_output = false;
        loop {
            // Before any output, wait the long first-token timeout; afterwards,
            // short idle-quiesce silence means the turn is complete.
            let wait = if got_output {
                NODE_IDLE_QUIESCE
            } else {
                NODE_FIRST_TOKEN_TIMEOUT
            };
            match self.rx.recv_timeout(wait) {
                Ok(NodeLine::Json(val)) => {
                    let ty = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match ty {
                        "delta" => {
                            if let Some(t) = val.get("text").and_then(|t| t.as_str()) {
                                acc.push_str(t);
                                got_output = true;
                                streamer(&acc);
                            }
                        }
                        // Explicit turn-end markers short-circuit quiescence if a
                        // future host emits them.
                        "turn_end" | "agent_end" | "done" | "idle" => break,
                        "error" => {
                            anyhow::bail!(
                                "node host error: {}",
                                val.get("error").and_then(|e| e.as_str()).unwrap_or("?")
                            );
                        }
                        // `wg:event` and anything else: ignore for reply text.
                        _ => {}
                    }
                }
                Ok(NodeLine::Eof) => {
                    if got_output {
                        break;
                    }
                    anyhow::bail!("node host exited before producing a reply");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if got_output {
                        // Quiescence after output ⇒ turn complete.
                        break;
                    }
                    anyhow::bail!("node host produced no reply within timeout");
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if got_output {
                        break;
                    }
                    anyhow::bail!("node host reader disconnected");
                }
            }
        }

        let reply = acc.trim().to_string();
        if reply.is_empty() {
            anyhow::bail!("node host produced no assistant text for this turn");
        }
        Ok(reply)
    }
}

impl Drop for NodeHostTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- topology selection -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topology {
    /// `pi --mode rpc` with the auto-loaded plugin.
    Rpc,
    /// `node wg-pi-host.mjs` SDK host.
    NodeHost,
}

/// Pick a topology from what's installed and an optional `WG_PI_TOPOLOGY`
/// override (`rpc`/`a` or `node`/`b`). Prefer A (a `pi` binary) when present —
/// the smallest delta and the "ship first" path — else fall to B.
fn select_topology(avail: &PiRouteAvailability, override_env: Option<&str>) -> Result<Topology> {
    let forced = override_env.map(|s| s.trim().to_ascii_lowercase());
    match forced.as_deref() {
        Some("rpc") | Some("a") | Some("pi") => {
            if avail.pi_binary.is_some() {
                return Ok(Topology::Rpc);
            }
            anyhow::bail!(
                "WG_PI_TOPOLOGY requested the RPC topology but no `pi` binary is on PATH"
            );
        }
        Some("node") | Some("b") | Some("host") => {
            if avail.node_host.is_some() {
                return Ok(Topology::NodeHost);
            }
            anyhow::bail!(
                "WG_PI_TOPOLOGY requested the Node-host topology but node + \
                 wg-pi-host.mjs + the built plugin bundle were not all found"
            );
        }
        Some(other) if !other.is_empty() => {
            anyhow::bail!("unknown WG_PI_TOPOLOGY={other:?} (expected `rpc` or `node`)");
        }
        _ => {
            if avail.pi_binary.is_some() {
                Ok(Topology::Rpc)
            } else if avail.node_host.is_some() {
                Ok(Topology::NodeHost)
            } else {
                anyhow::bail!(
                    "no usable pi transport: neither a `pi` binary nor the Node host \
                     (node + wg-pi-host.mjs + dist/index.js) is available. Install pi \
                     or build the wg-pi-plugin bundle (`npm --prefix pi-plugin run build`)."
                )
            }
        }
    }
}

// --- handler entry point ------------------------------------------------------

pub fn run(
    workgraph_dir: &Path,
    chat_ref: &str,
    resume: bool,
    role: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let _ = resume; // pi keeps server-side session state; we resume via --session-dir.

    // Explicit-model contract: refuse to start without a resolved model rather
    // than silently inheriting pi's internal default.
    let marg = pi_model_arg(model).ok_or_else(|| {
        anyhow::anyhow!(
            "pi-handler requires an explicitly resolved model, but model resolution produced \
             none. Pin a model on the chat/task (e.g. `pi:openrouter/anthropic/claude-3.5-haiku`), \
             the active profile, or `[dispatcher].model` — this handler will not fall back to \
             pi's default."
        )
    })?;

    let chat_dir = chat::chat_dir_for_ref(workgraph_dir, chat_ref);
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

    let avail = executor_discovery::pi_route_availability();
    let topology = select_topology(&avail, std::env::var("WG_PI_TOPOLOGY").ok().as_deref())?;
    logger.info(&format!(
        "pi-handler starting: chat_ref={}, role={:?}, model={:?} -> provider={}/model={}, topology={:?}",
        chat_ref, role, model, marg.provider, marg.model, topology
    ));

    let system_prompt = build_handler_system_prompt(workgraph_dir, chat_ref, role);
    let coordinator_id = parse_coordinator_id(chat_ref);

    // Build the transport once; it lives for the whole session (pi RPC and the
    // Node host both maintain conversation state across turns).
    let session_dir = chat_dir.join("pi-sessions");
    let mut transport: Box<dyn PiTransport> = match topology {
        Topology::Rpc => {
            let pi = avail
                .pi_binary
                .as_deref()
                .expect("rpc topology implies a pi binary");
            Box::new(RpcTransport::spawn(
                pi,
                &marg,
                chat_ref,
                &session_dir,
                &logger,
            )?)
        }
        Topology::NodeHost => {
            let host = avail
                .node_host
                .as_ref()
                .expect("node-host topology implies a host triple");
            Box::new(NodeHostTransport::spawn(host, &marg, &logger)?)
        }
    };

    // Cursor: skip inbox messages already answered (matched by request_id in
    // outbox). Same logic as the opencode/codex/claude handlers.
    let mut inbox_cursor: u64 = last_answered_inbox_id(workgraph_dir, chat_ref);
    // Deliver the static system prompt once, at the head of the first turn, only
    // when there is no prior history (a resumed pi session already carries it).
    let mut needs_system_prompt = inbox_cursor == 0;
    logger.info(&format!(
        "pi-handler ready: inbox_cursor={}, coordinator_id={:?}, send_system_prompt={}",
        inbox_cursor, coordinator_id, needs_system_prompt
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
                "pi-handler: processing inbox id={} request_id={} ({} chars)",
                msg.id,
                request_id,
                msg.content.len()
            ));

            let turn = assemble_turn(
                workgraph_dir,
                coordinator_id,
                needs_system_prompt.then_some(system_prompt.as_str()),
                &msg.content,
            );
            needs_system_prompt = false;

            let mut streamer = |accumulated: &str| {
                let _ = chat::write_streaming_ref(workgraph_dir, chat_ref, accumulated);
            };
            let reply = match transport.send_turn(&turn, &mut streamer) {
                Ok(t) => t,
                Err(e) => {
                    logger.error(&format!("pi turn failed: {}", e));
                    format!(
                        "The coordinator encountered an error running pi: {}. Please retry.",
                        e
                    )
                }
            };

            if let Err(e) = chat::append_outbox_ref(workgraph_dir, chat_ref, &reply, &request_id) {
                logger.error(&format!("outbox write failed: {}", e));
            } else {
                logger.info(&format!(
                    "pi-handler: response written ({} chars) for {}",
                    reply.len(),
                    request_id
                ));
            }
            chat::clear_streaming_ref(workgraph_dir, chat_ref);
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

fn build_handler_system_prompt(workgraph_dir: &Path, chat_ref: &str, role: Option<&str>) -> String {
    if chat_ref.starts_with("coordinator-") || role == Some("coordinator") {
        crate::commands::service::coordinator_agent::build_system_prompt(workgraph_dir)
    } else if let Some(r) = role {
        format!("You are acting in the role of: {}.", r)
    } else {
        String::from("You are a WG task agent.")
    }
}

/// Assemble one turn's message. Unlike the stateless opencode replay, pi keeps
/// conversation history in its session, so we send the live graph context plus
/// the new user message — and the static system prompt only on the first turn.
fn assemble_turn(
    workgraph_dir: &Path,
    coordinator_id: Option<u32>,
    system_prompt: Option<&str>,
    latest_user_msg: &str,
) -> String {
    let mut out = String::new();
    if let Some(sp) = system_prompt {
        out.push_str("# System\n");
        out.push_str(sp);
        out.push_str("\n\n");
    }
    if let Some(cid) = coordinator_id
        && let Ok(ctx) = crate::commands::service::coordinator_agent::build_coordinator_context(
            workgraph_dir,
            "1970-01-01T00:00:00Z",
            None,
            cid,
        )
        && !ctx.is_empty()
    {
        out.push_str("# Live graph context\n");
        out.push_str(&ctx);
        out.push_str("\n\n");
    }
    out.push_str("# User\n");
    out.push_str(latest_user_msg);
    out
}

// --- handler-local logger (peer of opencode_handler::HandlerLogger) -----------

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

    // --- model argument mapping (test_pi_model_arg_shapes: three spec shapes) --

    #[test]
    fn test_pi_model_arg_shapes() {
        // Shape 1: pi:openrouter/<vendor>/<model> (executor-qualified CLI slash).
        assert_eq!(
            pi_model_arg(Some("pi:openrouter/anthropic/claude-3.5-haiku")),
            Some(PiModelArg {
                provider: "openrouter".into(),
                model: "anthropic/claude-3.5-haiku".into(),
            })
        );
        // Shape 2: openrouter:<vendor>/<model> (provider-qualified).
        assert_eq!(
            pi_model_arg(Some("openrouter:anthropic/claude-3.5-haiku")),
            Some(PiModelArg {
                provider: "openrouter".into(),
                model: "anthropic/claude-3.5-haiku".into(),
            })
        );
        // Shape 3: claude:opus (a non-openrouter provider maps to pi's native
        // provider name `anthropic`, model id passes through).
        assert_eq!(
            pi_model_arg(Some("claude:opus")),
            Some(PiModelArg {
                provider: "anthropic".into(),
                model: "opus".into(),
            })
        );
        // Unresolved shapes.
        assert_eq!(pi_model_arg(None), None);
        assert_eq!(pi_model_arg(Some("")), None);
        assert_eq!(pi_model_arg(Some("   ")), None);
        // A bare single-token alias gives pi no provider — unresolved.
        assert_eq!(pi_model_arg(Some("opus")), None);
    }

    #[test]
    fn test_pi_model_arg_bare_vendor_route_is_openrouter() {
        // Bare `vendor/model` and CLI-slash `openrouter/...` both normalize to
        // the OpenRouter provider with the bare model id.
        assert_eq!(
            pi_model_arg(Some("minimax/minimax-m3")),
            Some(PiModelArg {
                provider: "openrouter".into(),
                model: "minimax/minimax-m3".into(),
            })
        );
        assert_eq!(
            pi_model_arg(Some("openrouter/minimax/minimax-m3")),
            Some(PiModelArg {
                provider: "openrouter".into(),
                model: "minimax/minimax-m3".into(),
            })
        );
    }

    #[test]
    fn test_rpc_spawn_args_carry_model_never_api_key() {
        let marg = PiModelArg {
            provider: "openrouter".into(),
            model: "anthropic/claude-3.5-haiku".into(),
        };
        let args = rpc_spawn_args(&marg, "coordinator-1", Path::new("/tmp/pi-sessions"));
        // Model + provider are always present and explicit.
        let pidx = args.iter().position(|a| a == "--provider").unwrap();
        assert_eq!(args[pidx + 1], "openrouter");
        let midx = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[midx + 1], "anthropic/claude-3.5-haiku");
        assert!(args.contains(&"--mode".to_string()) && args.contains(&"rpc".to_string()));
        // Credentials by env ONLY — never on the command line.
        assert!(
            !args
                .iter()
                .any(|a| a == "--api-key" || a.contains("api-key")),
            "credentials must never be passed via --api-key: {:?}",
            args
        );
    }

    // --- RPC event parsing (agent_end → last assistant text, canned JSONL) ----

    #[test]
    fn test_parse_agent_end_extracts_streamed_assistant_text() {
        // A canonical turn: an accepted-command response, two streamed text
        // deltas, then agent_end (also carrying the final messages).
        let stream = concat!(
            r#"{"type":"response","id":"req-1","success":true}"#,
            "\n",
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"The answer "}}"#,
            "\n",
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"is 42."}}"#,
            "\n",
            r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"The answer is 42."}]}]}"#,
            "\n",
        );
        assert_eq!(
            extract_rpc_reply(stream).as_deref(),
            Some("The answer is 42.")
        );
    }

    #[test]
    fn test_parse_agent_end_recovers_text_when_no_deltas() {
        // No streamed deltas: the reply must be recovered from the agent_end
        // event's messages (last assistant text in the document).
        let stream = concat!(
            r#"{"type":"response","id":"req-1","success":true}"#,
            "\n",
            r#"{"type":"agent_end","messages":[{"role":"user","content":[{"type":"text","text":"q"}]},{"role":"assistant","content":[{"type":"text","text":"final reply"}]}]}"#,
            "\n",
        );
        assert_eq!(extract_rpc_reply(stream).as_deref(), Some("final reply"));
    }

    #[test]
    fn test_get_last_assistant_text_response_is_used() {
        let stream = concat!(
            r#"{"type":"agent_end"}"#,
            "\n",
            r#"{"type":"response","id":"req-2","success":true,"data":{"text":"recovered text"}}"#,
            "\n",
        );
        assert_eq!(extract_rpc_reply(stream).as_deref(), Some("recovered text"));
    }

    #[test]
    fn test_extract_rpc_reply_none_on_empty_or_garbage() {
        assert_eq!(extract_rpc_reply(""), None);
        assert_eq!(extract_rpc_reply("not json\nstill not json\n"), None);
        // agent_end with no text and no follow-up yields nothing.
        assert_eq!(extract_rpc_reply(r#"{"type":"agent_end"}"#), None);
    }

    #[test]
    fn test_accumulator_flags_error_event() {
        let mut acc = RpcTurnAccumulator::default();
        acc.ingest(&serde_json::json!({"type":"error","error":"boom"}));
        assert_eq!(acc.error.as_deref(), Some("boom"));
        let mut acc2 = RpcTurnAccumulator::default();
        acc2.ingest(
            &serde_json::json!({"type":"response","id":"x","success":false,"error":"no key"}),
        );
        assert_eq!(acc2.error.as_deref(), Some("no key"));
    }

    // --- topology selection ----------------------------------------------------

    fn avail(pi: bool, node: bool) -> PiRouteAvailability {
        PiRouteAvailability {
            pi_binary: pi.then(|| std::path::PathBuf::from("/usr/bin/pi")),
            node_host: node.then(|| PiNodeHost {
                node: "/usr/bin/node".into(),
                host_script: "/p/host/wg-pi-host.mjs".into(),
                plugin_bundle: "/p/dist/index.js".into(),
            }),
        }
    }

    #[test]
    fn test_select_topology_prefers_rpc_then_node() {
        assert_eq!(
            select_topology(&avail(true, true), None).unwrap(),
            Topology::Rpc
        );
        assert_eq!(
            select_topology(&avail(true, false), None).unwrap(),
            Topology::Rpc
        );
        assert_eq!(
            select_topology(&avail(false, true), None).unwrap(),
            Topology::NodeHost
        );
        assert!(select_topology(&avail(false, false), None).is_err());
    }

    #[test]
    fn test_select_topology_honors_override() {
        // Force node even when pi is present.
        assert_eq!(
            select_topology(&avail(true, true), Some("node")).unwrap(),
            Topology::NodeHost
        );
        // Force rpc when only node is present → error.
        assert!(select_topology(&avail(false, true), Some("rpc")).is_err());
        // Unknown value → error.
        assert!(select_topology(&avail(true, true), Some("bogus")).is_err());
    }

    #[test]
    fn test_assemble_turn_includes_system_prompt_only_when_requested() {
        let dir = tempfile::TempDir::new().unwrap();
        let with = assemble_turn(dir.path(), None, Some("SYS"), "hello");
        assert!(with.contains("# System"));
        assert!(with.contains("SYS"));
        assert!(with.contains("hello"));
        let without = assemble_turn(dir.path(), None, None, "hello");
        assert!(!without.contains("# System"));
        assert!(without.contains("hello"));
    }
}
