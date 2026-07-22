//! Durable runtime evidence for tmux-owned interactive chat processes.
//!
//! The TUI owns only a `tmux attach` client.  The vendor process lives one
//! boundary deeper, so its lifecycle cannot be reconstructed from the PTY
//! child's status.  This module supplies the deliberately-small inner wrapper
//! and a grow-only, append-safe JSONL ledger under the canonical UUID chat
//! directory.
//!
//! Runtime evidence is never spawn authority.  Callers must first prove the
//! graph task is still present, nonterminal, non-archived, and has the same
//! execution identity.  The helpers here only record that decision and make
//! later diagnosis deterministic.

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const RUNTIME_LEDGER_FILE: &str = "runtime.jsonl";
pub const RUNTIME_STDERR_FILE: &str = "runtime-stderr.tail.log";
pub const MAX_RESTARTS_PER_FAILURE: u32 = 1;
const STDERR_TAIL_BYTES: usize = 16 * 1024;

const ENV_GRAPH_PATH: &str = "WG_CHAT_RUNTIME_GRAPH_PATH";
const ENV_TASK_ID: &str = "WG_CHAT_RUNTIME_TASK_ID";
const ENV_CHAT_REF: &str = "WG_CHAT_RUNTIME_CHAT_REF";
const ENV_UUID: &str = "WG_CHAT_RUNTIME_UUID";
const ENV_TMUX: &str = "WG_CHAT_RUNTIME_TMUX";
const ENV_EXECUTOR: &str = "WG_CHAT_RUNTIME_EXECUTOR";
const ENV_ROUTE: &str = "WG_CHAT_RUNTIME_ROUTE";
const ENV_REASONING: &str = "WG_CHAT_RUNTIME_REASONING";
const ENV_SESSION_DIR: &str = "WG_CHAT_RUNTIME_SESSION_DIR";
const ENV_CHAT_DIR: &str = "WG_CHAT_RUNTIME_CHAT_DIR";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub graph_path: String,
    pub task_id: String,
    pub chat_ref: String,
    pub uuid: String,
    pub tmux_session: String,
    pub executor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeEventKind {
    Start,
    Exit,
    AttachExit,
    Decision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeSource {
    InnerVendor,
    OuterTmuxAttach,
    TmuxServer,
    DaemonAdapter,
    TuiRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryDecision {
    ReattachExisting,
    ExplicitRestart,
    RefusedTerminal,
    BudgetExhausted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub version: u8,
    pub at: String,
    pub kind: RuntimeEventKind,
    pub source: RuntimeSource,
    pub identity: RuntimeIdentity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_dumped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<RecoveryDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
}

impl RuntimeEvent {
    fn new(kind: RuntimeEventKind, source: RuntimeSource, identity: RuntimeIdentity) -> Self {
        Self {
            version: 1,
            at: Utc::now().to_rfc3339(),
            kind,
            source,
            identity,
            argv: Vec::new(),
            wrapper_pid: None,
            inner_pid: None,
            exit_code: None,
            signal: None,
            core_dumped: None,
            stderr_path: None,
            stderr_tail: None,
            reason: None,
            decision: None,
            attempt: None,
        }
    }

    pub fn specific_reason(&self) -> Option<String> {
        match self.kind {
            RuntimeEventKind::Exit => {
                let owner = match self.source {
                    RuntimeSource::InnerVendor => format!("inner {}", self.identity.executor),
                    RuntimeSource::DaemonAdapter => "daemon-supervised adapter".to_string(),
                    _ => "chat process".to_string(),
                };
                if let Some(signal) = self.signal {
                    let mut reason =
                        format!("{owner} terminated by {} ({signal})", signal_name(signal));
                    if self.core_dumped == Some(true) {
                        reason.push_str(", core dumped");
                    }
                    Some(reason)
                } else if let Some(code) = self.exit_code {
                    Some(format!("{owner} exited with code {code}"))
                } else {
                    self.reason
                        .clone()
                        .or_else(|| Some(format!("{owner} exit status unavailable")))
                }
            }
            RuntimeEventKind::AttachExit => self.reason.clone().or_else(|| {
                Some(match self.source {
                    RuntimeSource::TmuxServer => {
                        "tmux server/session disappeared; inner status unavailable".to_string()
                    }
                    _ => "outer tmux attach client exited; inner process may still be healthy"
                        .to_string(),
                })
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeLedger {
    pub events: Vec<RuntimeEvent>,
    pub malformed_records: usize,
}

impl RuntimeLedger {
    pub fn last_start(&self) -> Option<(usize, &RuntimeEvent)> {
        self.events
            .iter()
            .enumerate()
            .rev()
            .find(|(_, event)| event.kind == RuntimeEventKind::Start)
    }

    pub fn last_failure(&self) -> Option<(usize, &RuntimeEvent)> {
        self.events.iter().enumerate().rev().find(|(_, event)| {
            matches!(
                event.kind,
                RuntimeEventKind::Exit | RuntimeEventKind::AttachExit
            )
        })
    }

    pub fn pending_failure(&self) -> Option<&RuntimeEvent> {
        let (failure_idx, failure) = self.last_failure()?;
        let start_idx = self.last_start().map(|(idx, _)| idx);
        (start_idx.is_none_or(|idx| failure_idx > idx)).then_some(failure)
    }

    /// Failure at the TUI-owned vendor boundary. Daemon-adapter exits remain
    /// visible evidence, but can never authorize or block a tmux Pi restart.
    pub fn last_vendor_failure(&self) -> Option<(usize, &RuntimeEvent)> {
        self.events.iter().enumerate().rev().find(|(_, event)| {
            event.kind == RuntimeEventKind::AttachExit
                || (event.kind == RuntimeEventKind::Exit
                    && event.source == RuntimeSource::InnerVendor)
        })
    }

    pub fn pending_vendor_failure(&self) -> Option<&RuntimeEvent> {
        let (failure_idx, failure) = self.last_vendor_failure()?;
        let start_idx = self.last_start().map(|(idx, _)| idx);
        (start_idx.is_none_or(|idx| failure_idx > idx)).then_some(failure)
    }

    pub fn restart_authorized(&self) -> bool {
        let Some((failure_idx, _)) = self.last_vendor_failure() else {
            return false;
        };
        let decision_idx = self
            .events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, event)| {
                (event.kind == RuntimeEventKind::Decision
                    && event.decision == Some(RecoveryDecision::ExplicitRestart))
                .then_some(idx)
            });
        let start_idx = self.last_start().map(|(idx, _)| idx);
        decision_idx
            .is_some_and(|idx| idx > failure_idx && start_idx.is_none_or(|start| start < idx))
    }

    pub fn restart_attempts_for_pending_failure(&self) -> u32 {
        let Some((failure_idx, _)) = self.last_vendor_failure() else {
            return 0;
        };
        self.events[failure_idx + 1..]
            .iter()
            .filter(|event| {
                event.kind == RuntimeEventKind::Decision
                    && event.decision == Some(RecoveryDecision::ExplicitRestart)
            })
            .count() as u32
    }

    pub fn next_attempt(&self) -> u32 {
        self.events
            .iter()
            .filter_map(|event| event.attempt)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    pub fn last_specific_event(&self) -> Option<&RuntimeEvent> {
        self.events.iter().rev().find(|event| {
            matches!(
                event.kind,
                RuntimeEventKind::Exit | RuntimeEventKind::AttachExit
            )
        })
    }

    pub fn last_specific_reason(&self) -> Option<String> {
        self.last_specific_event()
            .and_then(RuntimeEvent::specific_reason)
    }

    pub fn last_decision(&self) -> Option<&RuntimeEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| event.kind == RuntimeEventKind::Decision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryRequest {
    InitialLaunch,
    ReattachExisting { attempt: u32 },
    ExplicitRestart { attempt: u32 },
    NeedsExplicitRestart,
    RefusedIdentityMismatch,
    BudgetExhausted { attempt: u32 },
}

pub fn graph_path_for(workgraph_dir: &Path) -> PathBuf {
    canonical_or_absolute(workgraph_dir).join("graph.jsonl")
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

pub fn runtime_chat_dir(workgraph_dir: &Path, chat_ref: &str) -> Result<PathBuf> {
    let uuid = crate::chat_sessions::resolve_ref(workgraph_dir, chat_ref)
        .with_context(|| format!("resolve canonical UUID for {chat_ref}"))?;
    let live = crate::chat_sessions::chat_dir_for_uuid(workgraph_dir, &uuid);
    if live.exists() {
        return Ok(live);
    }
    let archived = workgraph_dir.join("chat").join(".archive").join(&uuid);
    if archived.exists() {
        return Ok(archived);
    }
    Ok(live)
}

pub fn identity_for_chat(
    workgraph_dir: &Path,
    task_id: &str,
    chat_ref: &str,
    tmux_session: &str,
    executor: &str,
    route: Option<&str>,
    reasoning: Option<&str>,
    session_dir: Option<&Path>,
) -> Result<RuntimeIdentity> {
    let uuid = crate::chat_sessions::resolve_ref(workgraph_dir, chat_ref)
        .with_context(|| format!("resolve exact chat UUID for {chat_ref}"))?;
    Ok(RuntimeIdentity {
        graph_path: graph_path_for(workgraph_dir).display().to_string(),
        task_id: task_id.to_string(),
        chat_ref: chat_ref.to_string(),
        uuid,
        tmux_session: tmux_session.to_string(),
        executor: executor.to_string(),
        route: route.map(redact_text).filter(|value| !value.is_empty()),
        reasoning: reasoning
            .map(str::to_string)
            .filter(|value| !value.is_empty()),
        session_dir: session_dir.map(|path| canonical_or_absolute(path).display().to_string()),
    })
}

pub fn identity_from_spawn(
    session_name: &str,
    command: &str,
    args: &[&str],
    env: &[(String, String)],
) -> Result<RuntimeIdentity> {
    let get = |key: &str| {
        env.iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    };
    let workgraph_dir = get("WG_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("Pi runtime wrapper requires WG_DIR"))?;
    let task_id =
        get("WG_CHAT_ID").ok_or_else(|| anyhow!("Pi runtime wrapper requires WG_CHAT_ID"))?;
    let chat_ref =
        get("WG_CHAT_REF").ok_or_else(|| anyhow!("Pi runtime wrapper requires WG_CHAT_REF"))?;
    let executor = get("WG_EXECUTOR_TYPE").unwrap_or("pi");
    let session_dir = option_value(args, "--session-dir").map(PathBuf::from);
    let _ = command;
    identity_for_chat(
        &workgraph_dir,
        task_id,
        chat_ref,
        session_name,
        executor,
        get("WG_MODEL"),
        get("WG_REASONING"),
        session_dir.as_deref(),
    )
}

pub fn install_identity_env(
    env: &mut Vec<(String, String)>,
    identity: &RuntimeIdentity,
    chat_dir: &Path,
) {
    let fields = [
        (ENV_GRAPH_PATH, identity.graph_path.as_str()),
        (ENV_TASK_ID, identity.task_id.as_str()),
        (ENV_CHAT_REF, identity.chat_ref.as_str()),
        (ENV_UUID, identity.uuid.as_str()),
        (ENV_TMUX, identity.tmux_session.as_str()),
        (ENV_EXECUTOR, identity.executor.as_str()),
        (ENV_ROUTE, identity.route.as_deref().unwrap_or("")),
        (ENV_REASONING, identity.reasoning.as_deref().unwrap_or("")),
        (
            ENV_SESSION_DIR,
            identity.session_dir.as_deref().unwrap_or(""),
        ),
        (ENV_CHAT_DIR, chat_dir.to_str().unwrap_or("")),
    ];
    for (key, value) in fields {
        env.retain(|(existing, _)| existing != key);
        env.push((key.to_string(), value.to_string()));
    }
}

fn identity_from_runtime_env() -> Result<(RuntimeIdentity, PathBuf)> {
    let required =
        |key: &str| std::env::var(key).with_context(|| format!("runtime wrapper missing {key}"));
    let empty_none = |value: String| (!value.is_empty()).then_some(value);
    let identity = RuntimeIdentity {
        graph_path: required(ENV_GRAPH_PATH)?,
        task_id: required(ENV_TASK_ID)?,
        chat_ref: required(ENV_CHAT_REF)?,
        uuid: required(ENV_UUID)?,
        tmux_session: required(ENV_TMUX)?,
        executor: required(ENV_EXECUTOR)?,
        route: empty_none(required(ENV_ROUTE)?),
        reasoning: empty_none(required(ENV_REASONING)?),
        session_dir: empty_none(required(ENV_SESSION_DIR)?),
    };
    let chat_dir = PathBuf::from(required(ENV_CHAT_DIR)?);
    if chat_dir.file_name().and_then(|name| name.to_str()) != Some(identity.uuid.as_str()) {
        bail!(
            "runtime chat directory does not match exact UUID {}",
            identity.uuid
        );
    }
    Ok((identity, chat_dir))
}

pub fn ledger_path(chat_dir: &Path) -> PathBuf {
    chat_dir.join(RUNTIME_LEDGER_FILE)
}

pub fn stderr_path(chat_dir: &Path) -> PathBuf {
    chat_dir.join(RUNTIME_STDERR_FILE)
}

struct LedgerLock {
    file: File,
}

impl LedgerLock {
    fn exclusive(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open runtime ledger lock {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            loop {
                let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
                if rc == 0 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error).context("lock runtime ledger");
                }
            }
        }
        Ok(Self { file })
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

pub fn append_event(chat_dir: &Path, event: &RuntimeEvent) -> Result<()> {
    fs::create_dir_all(chat_dir)
        .with_context(|| format!("create runtime chat directory {}", chat_dir.display()))?;
    let path = ledger_path(chat_dir);
    let lock_path = chat_dir.join("runtime.jsonl.lock");
    let _lock = LedgerLock::exclusive(&lock_path)?;
    let mut bytes = serde_json::to_vec(event).context("serialize runtime event")?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open runtime ledger {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("append runtime ledger {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("sync runtime ledger {}", path.display()))?;
    Ok(())
}

pub fn read_ledger(chat_dir: &Path) -> RuntimeLedger {
    let Ok(contents) = fs::read(ledger_path(chat_dir)) else {
        return RuntimeLedger::default();
    };
    let mut ledger = RuntimeLedger::default();
    for line in contents.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<RuntimeEvent>(line) {
            Ok(event) if event.version == 1 => ledger.events.push(event),
            _ => ledger.malformed_records += 1,
        }
    }
    // Splitting bytes rather than reading UTF-8 text is intentional: a crash
    // may leave half of a multibyte character in the final record. That tail
    // is malformed, but it must not hide every preceding durable event.
    ledger
}

pub fn read_ledger_for_chat(workgraph_dir: &Path, chat_ref: &str) -> RuntimeLedger {
    runtime_chat_dir(workgraph_dir, chat_ref)
        .map(|dir| read_ledger(&dir))
        .unwrap_or_default()
}

pub fn request_recovery(
    chat_dir: &Path,
    expected: &RuntimeIdentity,
    expected_argv: Option<&[String]>,
    session_live: bool,
) -> Result<RecoveryRequest> {
    let ledger = read_ledger(chat_dir);
    let attempt = ledger.next_attempt();
    if session_live {
        if let Some((_, start)) = ledger.last_start() {
            let identity_matches = runtime_identity_compatible(&start.identity, expected);
            let argv_matches = expected_argv.is_none_or(|argv| start.argv == sanitize_argv(argv));
            if !identity_matches || !argv_matches {
                let event = decision_event(
                    expected.clone(),
                    RecoveryDecision::BudgetExhausted,
                    attempt,
                    "refused reattach: live tmux session does not match durable graph/UUID/tmux/route/reasoning/session identity",
                );
                append_event(chat_dir, &event)?;
                return Ok(RecoveryRequest::RefusedIdentityMismatch);
            }
        }
        let last_failure_idx = ledger.last_failure().map(|(idx, _)| idx);
        if let Some((decision_idx, existing)) = ledger
            .events
            .iter()
            .enumerate()
            .rev()
            .find(|(_, event)| event.kind == RuntimeEventKind::Decision)
            && existing.decision == Some(RecoveryDecision::ReattachExisting)
            && last_failure_idx.is_none_or(|failure| decision_idx > failure)
        {
            return Ok(RecoveryRequest::ReattachExisting {
                attempt: existing.attempt.unwrap_or(attempt),
            });
        }
        let event = decision_event(
            expected.clone(),
            RecoveryDecision::ReattachExisting,
            attempt,
            "existing tmux session is the runtime owner; attaching without spawning or writing stdin",
        );
        append_event(chat_dir, &event)?;
        return Ok(RecoveryRequest::ReattachExisting { attempt });
    }
    let Some(_) = ledger.pending_vendor_failure() else {
        return Ok(RecoveryRequest::InitialLaunch);
    };
    let identity_matches = ledger
        .last_start()
        .is_some_and(|(_, start)| runtime_identity_matches(&start.identity, expected));
    let argv_matches = expected_argv.is_none_or(|argv| {
        ledger
            .last_start()
            .is_some_and(|(_, start)| start.argv == sanitize_argv(argv))
    });
    if !identity_matches || !argv_matches {
        let event = decision_event(
            expected.clone(),
            RecoveryDecision::BudgetExhausted,
            attempt,
            "refused recovery: exact graph/UUID/tmux/route/reasoning/session identity changed",
        );
        append_event(chat_dir, &event)?;
        return Ok(RecoveryRequest::RefusedIdentityMismatch);
    }
    let used = ledger.restart_attempts_for_pending_failure();
    if used >= MAX_RESTARTS_PER_FAILURE {
        let event = decision_event(
            expected.clone(),
            RecoveryDecision::BudgetExhausted,
            attempt,
            "refused recovery: bounded restart budget exhausted for this durable exit",
        );
        append_event(chat_dir, &event)?;
        return Ok(RecoveryRequest::BudgetExhausted { attempt });
    }
    let event = decision_event(
        expected.clone(),
        RecoveryDecision::ExplicitRestart,
        attempt,
        "explicit restart approved from authoritative nonterminal graph state; zero stdin bytes will be sent",
    );
    append_event(chat_dir, &event)?;
    Ok(RecoveryRequest::ExplicitRestart { attempt })
}

pub fn pending_failure_requires_explicit(chat_dir: &Path) -> bool {
    let ledger = read_ledger(chat_dir);
    ledger.pending_vendor_failure().is_some() && !ledger.restart_authorized()
}

pub fn append_refused_terminal(
    chat_dir: &Path,
    identity: RuntimeIdentity,
    reason: &str,
) -> Result<RuntimeEvent> {
    let ledger = read_ledger(chat_dir);
    let event = decision_event(
        identity,
        RecoveryDecision::RefusedTerminal,
        ledger.next_attempt(),
        reason,
    );
    append_event(chat_dir, &event)?;
    Ok(event)
}

fn decision_event(
    identity: RuntimeIdentity,
    decision: RecoveryDecision,
    attempt: u32,
    reason: &str,
) -> RuntimeEvent {
    let mut event = RuntimeEvent::new(
        RuntimeEventKind::Decision,
        RuntimeSource::TuiRecovery,
        identity,
    );
    event.decision = Some(decision);
    event.attempt = Some(attempt);
    event.reason = Some(redact_text(reason));
    event
}

pub fn append_attach_exit(
    chat_dir: &Path,
    identity: RuntimeIdentity,
    outer_status: &str,
    tmux_session_live: bool,
) -> Result<RuntimeEvent> {
    let source = if tmux_session_live {
        RuntimeSource::OuterTmuxAttach
    } else {
        RuntimeSource::TmuxServer
    };
    let mut event = RuntimeEvent::new(RuntimeEventKind::AttachExit, source, identity);
    event.reason = Some(if tmux_session_live {
        format!(
            "outer tmux attach client exited ({}) while the inner session remained live",
            redact_text(outer_status)
        )
    } else {
        format!(
            "tmux server/session disappeared after outer attach exit ({}); inner status unavailable",
            redact_text(outer_status)
        )
    });
    append_event(chat_dir, &event)?;
    Ok(event)
}

pub fn append_daemon_adapter_exit(
    workgraph_dir: &Path,
    cid: u32,
    adapter_pid: u32,
    exit_code: Option<i32>,
    signal: Option<i32>,
) -> Result<RuntimeEvent> {
    let chat_ref = crate::chat_id::format_chat_session_ref(cid);
    let chat_dir = runtime_chat_dir(workgraph_dir, &chat_ref)?;
    let tmux = crate::chat_id::chat_tmux_session_for_id(workgraph_dir, cid);
    let identity = identity_for_chat(
        workgraph_dir,
        &crate::chat_id::format_chat_task_id(cid),
        &chat_ref,
        &tmux,
        "daemon-adapter",
        None,
        None,
        None,
    )?;
    let mut event = RuntimeEvent::new(
        RuntimeEventKind::Exit,
        RuntimeSource::DaemonAdapter,
        identity,
    );
    event.inner_pid = Some(adapter_pid);
    event.exit_code = exit_code;
    event.signal = signal;
    event.reason = Some("daemon-supervised adapter exited".to_string());
    append_event(&chat_dir, &event)?;
    Ok(event)
}

pub fn runtime_identity_matches(previous: &RuntimeIdentity, expected: &RuntimeIdentity) -> bool {
    previous == expected
}

/// Reattach validation permits an omitted optional field in the attach plan
/// (the process is already running, so no argv/session-dir is reconstructed),
/// but every supplied field and every mandatory identity component must match.
pub fn runtime_identity_compatible(previous: &RuntimeIdentity, expected: &RuntimeIdentity) -> bool {
    previous.graph_path == expected.graph_path
        && previous.task_id == expected.task_id
        && previous.chat_ref == expected.chat_ref
        && previous.uuid == expected.uuid
        && previous.tmux_session == expected.tmux_session
        && previous.executor == expected.executor
        && expected
            .route
            .as_ref()
            .is_none_or(|route| previous.route.as_ref() == Some(route))
        && expected
            .reasoning
            .as_ref()
            .is_none_or(|reasoning| previous.reasoning.as_ref() == Some(reasoning))
        && expected
            .session_dir
            .as_ref()
            .is_none_or(|session_dir| previous.session_dir.as_ref() == Some(session_dir))
}

/// Run the hidden inner-process wrapper.  The returned code is suitable for
/// the wrapper process itself (`128 + signal` for a signalled child); the
/// ledger retains the exact signal/core fields.
pub fn run_wrapper(command: Vec<OsString>) -> Result<i32> {
    let (identity, chat_dir) = identity_from_runtime_env()?;
    run_wrapper_with(identity, chat_dir, command)
}

fn run_wrapper_with(
    identity: RuntimeIdentity,
    chat_dir: PathBuf,
    command: Vec<OsString>,
) -> Result<i32> {
    if command.is_empty() {
        bail!("chat runtime wrapper needs a command after --");
    }
    let argv: Vec<String> = command
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let sanitized_argv = sanitize_argv(&argv);
    let stderr_file = stderr_path(&chat_dir);

    let mut child_command = Command::new(&command[0]);
    child_command.args(&command[1..]);
    child_command.stdin(Stdio::inherit());
    child_command.stdout(Stdio::inherit());
    child_command.stderr(Stdio::piped());
    let mut child = match child_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let mut event =
                RuntimeEvent::new(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, identity);
            event.wrapper_pid = Some(std::process::id());
            event.reason = Some(redact_text(&format!("inner process spawn failed: {error}")));
            event.stderr_path = Some(stderr_file.display().to_string());
            append_event(&chat_dir, &event)?;
            return Err(error).context("spawn inner chat process");
        }
    };
    let inner_pid = child.id();
    let mut start = RuntimeEvent::new(
        RuntimeEventKind::Start,
        RuntimeSource::InnerVendor,
        identity.clone(),
    );
    start.argv = sanitized_argv;
    start.wrapper_pid = Some(std::process::id());
    start.inner_pid = Some(inner_pid);
    start.stderr_path = Some(stderr_file.display().to_string());
    start.reason = Some("tmux-owned inner vendor process started".to_string());
    if let Err(error) = append_event(&chat_dir, &start) {
        // Never leave an untracked vendor process behind if the durable start
        // record cannot be committed. The wrapper is the tmux command owner,
        // so fail closed before handing the child an interactive lifetime.
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("persist inner chat process start");
    }

    let stderr = child.stderr.take();
    let stderr_thread = std::thread::Builder::new()
        .name(format!("chat-runtime-stderr-{inner_pid}"))
        .spawn(move || capture_stderr_tail(stderr))
        .context("spawn stderr capture thread")?;
    let status = child.wait().context("wait for inner chat process")?;
    let tail_bytes = stderr_thread.join().unwrap_or_default();
    let stderr_tail = redact_text(&String::from_utf8_lossy(&tail_bytes));
    fs::write(&stderr_file, stderr_tail.as_bytes())
        .with_context(|| format!("write sanitized stderr tail {}", stderr_file.display()))?;

    let mut exit = RuntimeEvent::new(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, identity);
    exit.wrapper_pid = Some(std::process::id());
    exit.inner_pid = Some(inner_pid);
    exit.stderr_path = Some(stderr_file.display().to_string());
    exit.stderr_tail = (!stderr_tail.is_empty()).then_some(stderr_tail);
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        exit.exit_code = status.code();
        exit.signal = status.signal();
        exit.core_dumped = Some(status.core_dumped());
    }
    #[cfg(not(unix))]
    {
        exit.exit_code = status.code();
    }
    exit.reason = Some(if let Some(signal) = exit.signal {
        format!(
            "inner process terminated by {} ({signal})",
            signal_name(signal)
        )
    } else {
        format!(
            "inner process exited with code {}",
            exit.exit_code.unwrap_or(-1)
        )
    });
    append_event(&chat_dir, &exit)?;

    Ok(match (exit.exit_code, exit.signal) {
        (Some(code), _) => code,
        (_, Some(signal)) => 128 + signal,
        _ => 1,
    })
}

fn capture_stderr_tail(stderr: Option<impl Read>) -> Vec<u8> {
    let Some(mut stderr) = stderr else {
        return Vec::new();
    };
    let mut terminal = std::io::stderr().lock();
    let mut tail = VecDeque::with_capacity(STDERR_TAIL_BYTES);
    let mut buffer = [0u8; 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let _ = terminal.write_all(&buffer[..count]);
                let _ = terminal.flush();
                for byte in &buffer[..count] {
                    if tail.len() == STDERR_TAIL_BYTES {
                        tail.pop_front();
                    }
                    tail.push_back(*byte);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    tail.into_iter().collect()
}

fn option_value<'a>(args: &'a [&str], option: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == option)
        .map(|window| window[1])
}

pub fn sanitize_argv(argv: &[String]) -> Vec<String> {
    let mut sanitized = Vec::with_capacity(argv.len());
    let mut redact_next = false;
    for arg in argv {
        if redact_next {
            sanitized.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        let lower = arg.to_ascii_lowercase();
        if arg.starts_with('-')
            && let Some((key, _)) = arg.split_once('=')
            && is_secret_flag(&key.to_ascii_lowercase())
        {
            sanitized.push(format!("{key}=[REDACTED]"));
            continue;
        }
        if arg.starts_with('-') && is_secret_flag(&lower) {
            sanitized.push(arg.clone());
            redact_next = true;
            continue;
        }
        sanitized.push(redact_text(arg));
    }
    sanitized
}

fn is_secret_flag(value: &str) -> bool {
    let normalized = value.trim_start_matches('-').replace('_', "-");
    normalized.contains("api-key")
        || normalized.contains("token")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized == "authorization"
        || normalized == "auth"
}

pub fn redact_text(value: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for token in value.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if redact_next {
            if lower == "bearer" {
                output.push(token.to_string());
                continue;
            }
            output.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if lower == "bearer" || lower == "authorization:" {
            output.push(token.to_string());
            redact_next = true;
            continue;
        }
        let redacted = if lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("xoxb-")
        {
            "[REDACTED]".to_string()
        } else if token.contains('?') {
            redact_url_query(token)
        } else if let Some((key, _)) = token.split_once('=') {
            if is_secret_flag(&key.to_ascii_lowercase()) {
                format!("{key}=[REDACTED]")
            } else {
                token.to_string()
            }
        } else {
            token.to_string()
        };
        output.push(redacted);
    }
    output.join(" ")
}

fn redact_url_query(token: &str) -> String {
    let Some((base, query)) = token.split_once('?') else {
        return token.to_string();
    };
    let query = query
        .split('&')
        .map(|pair| {
            if let Some((key, value)) = pair.split_once('=') {
                if is_secret_flag(&key.to_ascii_lowercase()) {
                    format!("{key}=[REDACTED]")
                } else {
                    format!("{key}={value}")
                }
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

pub fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn identity(root: &Path) -> RuntimeIdentity {
        RuntimeIdentity {
            graph_path: root.join("graph.jsonl").display().to_string(),
            task_id: ".chat-8".into(),
            chat_ref: "chat-8".into(),
            uuid: "019f5598-9c36-7e72-bd41-75d7ba727b14".into(),
            tmux_session: "wg-chat-project-chat-8".into(),
            executor: "pi".into(),
            route: Some("pi:openai-codex:gpt-5.6-sol".into()),
            reasoning: Some("xhigh".into()),
            session_dir: Some(root.join("pi-sessions").display().to_string()),
        }
    }

    fn event(kind: RuntimeEventKind, source: RuntimeSource, root: &Path) -> RuntimeEvent {
        RuntimeEvent::new(kind, source, identity(root))
    }

    #[test]
    fn exit_reasons_distinguish_zero_nonzero_and_signals() {
        let root = Path::new("/tmp/chat-runtime-test");
        let mut zero = event(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, root);
        zero.exit_code = Some(0);
        assert_eq!(
            zero.specific_reason().as_deref(),
            Some("inner pi exited with code 0")
        );
        zero.exit_code = Some(23);
        assert_eq!(
            zero.specific_reason().as_deref(),
            Some("inner pi exited with code 23")
        );
        zero.exit_code = None;
        zero.signal = Some(15);
        zero.core_dumped = Some(false);
        assert_eq!(
            zero.specific_reason().as_deref(),
            Some("inner pi terminated by SIGTERM (15)")
        );
        zero.signal = Some(9);
        assert_eq!(
            zero.specific_reason().as_deref(),
            Some("inner pi terminated by SIGKILL (9)")
        );
        zero.source = RuntimeSource::DaemonAdapter;
        zero.signal = None;
        zero.exit_code = Some(2);
        assert_eq!(
            zero.specific_reason().as_deref(),
            Some("daemon-supervised adapter exited with code 2")
        );
    }

    #[test]
    fn attach_client_and_tmux_server_loss_are_not_inner_exit() {
        let root = tempfile::tempdir().unwrap();
        let chat = root.path();
        append_attach_exit(chat, identity(chat), "exit code 1", true).unwrap();
        append_attach_exit(chat, identity(chat), "exit code 1", false).unwrap();
        let ledger = read_ledger(chat);
        assert_eq!(ledger.events[0].source, RuntimeSource::OuterTmuxAttach);
        assert!(
            ledger.events[0]
                .specific_reason()
                .unwrap()
                .contains("inner session remained live")
        );
        assert_eq!(ledger.events[1].source, RuntimeSource::TmuxServer);
        assert!(
            ledger.events[1]
                .specific_reason()
                .unwrap()
                .contains("inner status unavailable")
        );
    }

    #[test]
    fn malformed_and_truncated_records_do_not_hide_last_valid_reason() {
        let root = tempfile::tempdir().unwrap();
        let chat = root.path();
        let mut exit = event(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, chat);
        exit.exit_code = Some(7);
        append_event(chat, &exit).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(ledger_path(chat))
            .unwrap();
        file.write_all(b"not-json\n{\"version\":1\xff").unwrap();
        let ledger = read_ledger(chat);
        assert_eq!(ledger.events.len(), 1);
        assert_eq!(ledger.malformed_records, 2);
        assert_eq!(
            ledger.last_specific_reason().as_deref(),
            Some("inner pi exited with code 7")
        );
    }

    #[test]
    fn concurrent_appenders_leave_complete_json_records() {
        let root = tempfile::tempdir().unwrap();
        let chat = Arc::new(root.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for worker in 0..8 {
            let chat = Arc::clone(&chat);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                for index in 0..25 {
                    let mut record = event(
                        RuntimeEventKind::AttachExit,
                        RuntimeSource::OuterTmuxAttach,
                        &chat,
                    );
                    record.reason = Some(format!("worker-{worker}-record-{index}"));
                    append_event(&chat, &record).unwrap();
                }
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let ledger = read_ledger(&chat);
        assert_eq!(ledger.events.len(), 200);
        assert_eq!(ledger.malformed_records, 0);
    }

    #[test]
    fn argv_and_stderr_redaction_remove_common_secrets() {
        let argv = vec![
            "pi".into(),
            "--api-key".into(),
            "sk-live-secret".into(),
            "--token=github_pat_123".into(),
            "https://host/path?token=abc&model=ok".into(),
        ];
        let sanitized = sanitize_argv(&argv).join(" ");
        assert!(!sanitized.contains("live-secret"));
        assert!(!sanitized.contains("github_pat_123"));
        assert!(!sanitized.contains("token=abc"));
        assert!(sanitized.contains("model=ok"));
        assert_eq!(
            redact_text("failure api_key=supersecret"),
            "failure api_key=[REDACTED]"
        );
        assert_eq!(
            redact_text("request Authorization: Bearer top-secret"),
            "request Authorization: Bearer [REDACTED]"
        );
    }

    #[test]
    fn daemon_adapter_exit_is_visible_but_never_pi_restart_authority() {
        let root = tempfile::tempdir().unwrap();
        let chat = root.path();
        let mut adapter = event(RuntimeEventKind::Exit, RuntimeSource::DaemonAdapter, chat);
        adapter.exit_code = Some(3);
        append_event(chat, &adapter).unwrap();
        let ledger = read_ledger(chat);
        assert_eq!(
            ledger.last_specific_reason().as_deref(),
            Some("daemon-supervised adapter exited with code 3")
        );
        assert!(ledger.pending_failure().is_some());
        assert!(ledger.pending_vendor_failure().is_none());
        assert_eq!(
            request_recovery(chat, &identity(chat), None, false).unwrap(),
            RecoveryRequest::InitialLaunch
        );
    }

    #[test]
    fn explicit_recovery_is_exact_and_bounded_per_failure() {
        let root = tempfile::tempdir().unwrap();
        let chat = root.path();
        let id = identity(chat);
        let argv = vec!["pi".to_string(), "--session-id".into(), "chat-8".into()];
        let mut start = event(RuntimeEventKind::Start, RuntimeSource::InnerVendor, chat);
        start.argv = sanitize_argv(&argv);
        append_event(chat, &start).unwrap();
        let mut exit = event(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, chat);
        exit.signal = Some(9);
        append_event(chat, &exit).unwrap();

        assert_eq!(
            request_recovery(chat, &id, Some(&argv), false).unwrap(),
            RecoveryRequest::ExplicitRestart { attempt: 1 }
        );
        assert_eq!(
            request_recovery(chat, &id, Some(&argv), false).unwrap(),
            RecoveryRequest::BudgetExhausted { attempt: 2 }
        );
        let mut changed = id.clone();
        changed.route = Some("pi:openrouter:other/model".into());
        assert_eq!(
            request_recovery(chat, &changed, Some(&argv), false).unwrap(),
            RecoveryRequest::RefusedIdentityMismatch
        );
        assert_eq!(
            request_recovery(chat, &changed, Some(&argv), true).unwrap(),
            RecoveryRequest::RefusedIdentityMismatch,
            "a live tmux name is not authority to reattach a route-mismatched process"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_records_real_exit_codes_signals_pids_and_sanitized_stderr() {
        let cases = [
            ("exit 0", Some(0), None),
            ("exit 17", Some(17), None),
            ("kill -TERM $$", None, Some(15)),
            ("kill -KILL $$", None, Some(9)),
        ];
        for (script, code, signal) in cases {
            let root = tempfile::tempdir().unwrap();
            let chat = root.path().to_path_buf();
            let command = vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(format!("printf 'api_key=do-not-store\\n' >&2; {script}")),
            ];
            let returned = run_wrapper_with(identity(&chat), chat.clone(), command).unwrap();
            assert_eq!(returned, code.unwrap_or_else(|| 128 + signal.unwrap()));
            let ledger = read_ledger(&chat);
            assert_eq!(ledger.events.len(), 2);
            let start = &ledger.events[0];
            let exit = &ledger.events[1];
            assert_eq!(start.kind, RuntimeEventKind::Start);
            assert!(start.wrapper_pid.is_some());
            assert!(start.inner_pid.is_some());
            assert_eq!(exit.exit_code, code);
            assert_eq!(exit.signal, signal);
            assert_eq!(exit.inner_pid, start.inner_pid);
            assert!(
                !fs::read_to_string(stderr_path(&chat))
                    .unwrap()
                    .contains("do-not-store")
            );
            assert!(
                !fs::read_to_string(ledger_path(&chat))
                    .unwrap()
                    .contains("do-not-store")
            );
        }
    }

    #[test]
    fn start_after_exit_opens_a_new_incident_without_replaying_input() {
        let root = tempfile::tempdir().unwrap();
        let chat = root.path();
        let mut start = event(RuntimeEventKind::Start, RuntimeSource::InnerVendor, chat);
        start.inner_pid = Some(1);
        append_event(chat, &start).unwrap();
        let mut exit = event(RuntimeEventKind::Exit, RuntimeSource::InnerVendor, chat);
        exit.exit_code = Some(1);
        append_event(chat, &exit).unwrap();
        assert!(read_ledger(chat).pending_failure().is_some());
        start.inner_pid = Some(2);
        append_event(chat, &start).unwrap();
        assert!(read_ledger(chat).pending_failure().is_none());
    }
}
