//! Classify an agent failure from the raw JSONL stream written by the claude/codex executors.
//!
//! This is a pure function: no side-effects, no graph I/O. The wrapper invokes
//! `wg classify-failure` which shells out to this logic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use worksgood::dispatch::plan::ExecutorKind;
use worksgood::graph::{FailureClass, FailureReason, FailureSignal};
use worksgood::telemetry::{
    failure_signal_from_envelope, failure_signal_from_evidence, parse_openrouter_error_envelope,
    parse_retry_after_text,
};

/// Maximum bytes to read from the tail of executor streams when scanning for
/// error patterns. Cargo/linker ENOSPC diagnostics can be followed by wrapper
/// bookkeeping, so retain a bounded 64 KiB rather than the historical 4 KiB.
const TAIL_BYTES: u64 = 64 * 1024;

/// Stable terminal projection derived only from machine-readable stream
/// evidence. This is deliberately separate from [`FailureSignal`]: a provider
/// request can finish authoritatively and then encounter a completion-plane
/// blocker, which is not a source/provider failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalStreamState {
    /// An exact agent/turn receipt reports `rawStopReason` or `stopReason` as
    /// `completed`. This proves provider-turn completion, not task acceptance.
    Completed,
    /// A typed completion/finalization refusal was emitted. The candidate must
    /// be retained and the lifecycle parked by finalization authority.
    FinalizationBlocked,
    /// No completion receipt exists and provider evidence is authoritative.
    ProviderFailure,
    /// Exact terminal receipts disagree. Never convert this into semantic or
    /// source failure; retain the candidate for explicit reconciliation.
    Ambiguous,
    /// Neither exact terminal nor typed provider evidence was observed.
    Unknown,
}

impl TerminalStreamState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::FinalizationBlocked => "finalization-blocked",
            Self::ProviderFailure => "provider-failure",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactTerminalReceipt {
    pub line: usize,
    pub event_type: String,
    pub receipt_id: String,
    pub stop_reason: String,
    pub completed: bool,
}

/// Restart-stable explanation of the winning terminal evidence. No wall-clock
/// field is included, so replaying the same bytes produces identical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStreamClassification {
    pub state: TerminalStreamState,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<ExactTerminalReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,
}

impl TerminalStreamClassification {
    fn unknown() -> Self {
        Self {
            state: TerminalStreamState::Unknown,
            reason_code: "no-authoritative-terminal-evidence".into(),
            receipts: Vec::new(),
            finalization_code: None,
            failure_reason: None,
        }
    }
}

/// Deterministic terminal projection when no stream path exists. Only the
/// wrapper's explicit hard-timeout exit is typed; every other streamless exit
/// stays unknown rather than manufacturing semantic evidence.
pub fn terminal_without_stream(
    exit_code: i32,
    executor: ExecutorKind,
    route: Option<String>,
) -> TerminalStreamClassification {
    if exit_code != 124 {
        return TerminalStreamClassification::unknown();
    }
    let signal =
        failure_signal_from_evidence(None, None, None, None, "hard timeout", executor, route)
            .with_reason(FailureReason::HardTimeout, 0.8);
    TerminalStreamClassification {
        state: TerminalStreamState::ProviderFailure,
        reason_code: "provider-failure:hard-timeout".into(),
        receipts: Vec::new(),
        finalization_code: None,
        failure_reason: Some(signal.reason),
    }
}

/// Classify terminal evidence with explicit precedence.
///
/// Exact completed receipts outrank timeout/reset *text heuristics*. Typed
/// finalization blockers outrank all source-failure projection. Conflicting
/// exact receipts fail closed as `Ambiguous`. A structured provider error (or
/// hard-timeout exit) still wins when no authoritative completion exists.
pub fn classify_terminal_from_raw_stream(
    raw_stream: &Path,
    output_log: Option<&Path>,
    exit_code: i32,
    executor: ExecutorKind,
    route: Option<String>,
) -> TerminalStreamClassification {
    let raw = read_tail(raw_stream).unwrap_or_default();
    let output = output_log.and_then(read_tail).unwrap_or_default();
    let receipts = exact_terminal_receipts(&raw);
    let has_completed = receipts.iter().any(|receipt| receipt.completed);
    let has_non_completed = receipts.iter().any(|receipt| !receipt.completed);

    if has_completed && has_non_completed {
        return TerminalStreamClassification {
            state: TerminalStreamState::Ambiguous,
            reason_code: "conflicting-exact-terminal-receipts".into(),
            receipts,
            finalization_code: None,
            failure_reason: None,
        };
    }

    let finalization_code = raw
        .lines()
        .chain(output.lines())
        .filter_map(typed_finalization_code)
        .next_back();
    if let Some(code) = finalization_code {
        return TerminalStreamClassification {
            state: TerminalStreamState::FinalizationBlocked,
            reason_code: format!("typed-finalization-blocked:{code}"),
            receipts,
            finalization_code: Some(code),
            failure_reason: None,
        };
    }

    if has_completed {
        let last_receipt_line = receipts
            .iter()
            .map(|receipt| receipt.line)
            .max()
            .unwrap_or(0);
        // A structured provider error after the last completed turn is not a
        // timeout-text heuristic: it proves the provider failed before an
        // authoritative terminal completion. Errors before the receipt are
        // superseded (for example an internal provider retry that recovered).
        let provider_failure_after_receipt = raw
            .lines()
            .skip(last_receipt_line)
            .filter_map(|line| signal_from_json_line(line.trim(), executor, route.clone()))
            .filter(|signal| {
                signal.reason != FailureReason::Unknown || signal.http_status.is_some()
            })
            .last();
        if let Some(signal) = provider_failure_after_receipt {
            return TerminalStreamClassification {
                state: TerminalStreamState::ProviderFailure,
                reason_code: format!("provider-failure:{}", signal.reason.as_str()),
                receipts,
                finalization_code: None,
                failure_reason: Some(signal.reason),
            };
        }
        return TerminalStreamClassification {
            state: TerminalStreamState::Completed,
            reason_code: "exact-agent-turn-completed".into(),
            receipts,
            finalization_code: None,
            failure_reason: None,
        };
    }

    let signal = classify_failure_signal_without_terminal(
        raw_stream, output_log, exit_code, executor, route,
    );
    if signal.reason != FailureReason::Unknown {
        TerminalStreamClassification {
            state: TerminalStreamState::ProviderFailure,
            reason_code: format!("provider-failure:{}", signal.reason.as_str()),
            receipts,
            finalization_code: None,
            failure_reason: Some(signal.reason),
        }
    } else {
        TerminalStreamClassification::unknown()
    }
}

fn exact_terminal_receipts(raw: &str) -> Vec<ExactTerminalReceipt> {
    let mut receipts: BTreeMap<(String, String), ExactTerminalReceipt> = BTreeMap::new();
    for (index, line) in raw.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let message = match event_type {
            "message_end" | "turn_end" => value.get("message"),
            "agent_end" if value.get("willRetry").and_then(|v| v.as_bool()) != Some(true) => value
                .get("messages")
                .and_then(|messages| messages.as_array())
                .and_then(|messages| {
                    messages.iter().rev().find(|message| {
                        message.get("role").and_then(|v| v.as_str()) == Some("assistant")
                    })
                }),
            _ => None,
        };
        let Some(message) = message else {
            continue;
        };
        let stop_reason = message
            .get("rawStopReason")
            .or_else(|| message.get("stopReason"))
            .and_then(|v| v.as_str());
        let Some(stop_reason) = stop_reason else {
            continue;
        };
        let normalized = normalize_code(stop_reason);
        let completed = normalized == "completed";
        let exact_non_completed = matches!(
            normalized.as_str(),
            "failed" | "error" | "cancelled" | "canceled" | "aborted" | "timeout" | "timedout"
        );
        if !completed && !exact_non_completed {
            continue;
        }
        let receipt_id = message
            .get("responseId")
            .or_else(|| value.get("turnId"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("b3:{}", blake3::hash(message.to_string().as_bytes())));
        // message_end/turn_end/agent_end commonly repeat the same receipt.
        // Deduplicate only identical outcome+id; contradictory outcomes remain.
        receipts
            .entry((receipt_id.clone(), normalized.clone()))
            .or_insert(ExactTerminalReceipt {
                line: index + 1,
                event_type: event_type.to_string(),
                receipt_id,
                stop_reason: stop_reason.to_string(),
                completed,
            });
    }
    let mut receipts: Vec<_> = receipts.into_values().collect();
    receipts.sort_by_key(|receipt| receipt.line);
    receipts
}

fn typed_finalization_code(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let candidates = [
        value.get("code"),
        value.get("state"),
        value.get("kind"),
        value.get("error").and_then(|error| error.get("code")),
        value
            .get("finalization")
            .and_then(|finalization| finalization.get("code")),
        value
            .get("finalization")
            .and_then(|finalization| finalization.get("state")),
    ];
    for candidate in candidates.into_iter().flatten().filter_map(|v| v.as_str()) {
        if let Some(code) = canonical_finalization_code(candidate) {
            return Some(code);
        }
    }
    canonical_finalization_code(event_type)
}

fn canonical_finalization_code(value: &str) -> Option<String> {
    let normalized = normalize_code(value);
    let code = match normalized.as_str() {
        "needsreview" | "completionneedsreview" => "needs-review",
        "landingpending" | "completionlandingpending" => "landing-pending",
        "evidencerefusal"
        | "evidencerefused"
        | "evidenceguardrefusal"
        | "completionevidencerefusal" => "evidence-refusal",
        "guardrefusal" | "guardrefused" | "finalizationguardrefusal" | "completionguardrefusal" => {
            "guard-refusal"
        }
        "finalizationblocked" | "completionfinalizationblocked" => "finalization-blocked",
        _ => return None,
    };
    Some(code.into())
}

fn normalize_code(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Classify an agent failure from the raw JSONL stream and exit code.
///
/// # Arguments
/// - `raw_stream`: path to the `raw_stream.jsonl` produced by the executor wrapper.
///   May not exist if the agent was killed before producing any output.
/// - `exit_code`: the shell exit code of the agent process (124 = hard timeout).
pub fn classify_from_raw_stream(raw_stream: &Path, exit_code: i32) -> FailureClass {
    let signal = classify_signal_from_raw_stream(
        raw_stream,
        None,
        exit_code,
        infer_executor(raw_stream),
        None,
    );
    failure_class_for_signal(
        &signal,
        read_tail(raw_stream).as_deref(),
        raw_stream,
        exit_code,
    )
}

/// Produce the normalized provider-telemetry signal. `output_log` is scanned
/// in addition to the raw stream because some CLI handlers echo the provider
/// body only on stderr.
pub fn classify_signal_from_raw_stream(
    raw_stream: &Path,
    output_log: Option<&Path>,
    exit_code: i32,
    executor: ExecutorKind,
    route: Option<String>,
) -> FailureSignal {
    let terminal = classify_terminal_from_raw_stream(
        raw_stream,
        output_log,
        exit_code,
        executor,
        route.clone(),
    );
    if matches!(
        terminal.state,
        TerminalStreamState::Completed
            | TerminalStreamState::FinalizationBlocked
            | TerminalStreamState::Ambiguous
    ) {
        return failure_signal_from_evidence(None, None, None, None, "", executor, route);
    }
    classify_failure_signal_without_terminal(raw_stream, output_log, exit_code, executor, route)
}

fn classify_failure_signal_without_terminal(
    raw_stream: &Path,
    output_log: Option<&Path>,
    exit_code: i32,
    executor: ExecutorKind,
    route: Option<String>,
) -> FailureSignal {
    if exit_code == 124 {
        return failure_signal_from_evidence(
            None,
            None,
            None,
            None,
            "hard timeout",
            executor,
            route,
        )
        .with_reason(FailureReason::HardTimeout, 0.8);
    }

    let raw = read_tail(raw_stream).unwrap_or_default();
    let output = output_log.and_then(read_tail).unwrap_or_default();
    let combined = if output.is_empty() {
        raw.clone()
    } else {
        format!("{raw}\n{output}")
    };

    if looks_like_disk_exhaustion(&combined) {
        return failure_signal_from_evidence(None, None, None, None, &combined, executor, route)
            .with_reason(FailureReason::Disk, 0.8);
    }

    // Latest structured error wins. This covers raw OpenRouter envelopes and
    // pi `error` / failed `response` events with string or object errors.
    for line in combined.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(signal) = signal_from_json_line(line, executor, route.clone()) {
            if signal.reason != FailureReason::Unknown || signal.http_status.is_some() {
                return signal;
            }
        }
    }

    if let Some(status) = extract_api_error_status(&combined) {
        return failure_signal_from_evidence(
            Some(status as u16),
            extract_error_type(&combined),
            None,
            parse_retry_after_text(&combined),
            &combined,
            executor,
            route,
        );
    }

    failure_signal_from_evidence(
        extract_status_from_message(&combined),
        extract_error_type(&combined),
        None,
        parse_retry_after_text(&combined),
        &combined,
        executor,
        route,
    )
}

trait FailureSignalExt {
    fn with_reason(self, reason: FailureReason, confidence: f32) -> Self;
}

impl FailureSignalExt for FailureSignal {
    fn with_reason(mut self, reason: FailureReason, confidence: f32) -> Self {
        self.reason = reason;
        self.confidence = confidence;
        self
    }
}

fn failure_class_for_signal(
    signal: &FailureSignal,
    text: Option<&str>,
    raw_stream: &Path,
    exit_code: i32,
) -> FailureClass {
    match signal.reason {
        FailureReason::HardTimeout => FailureClass::AgentHardTimeout,
        FailureReason::Disk => FailureClass::ResourceExhaustedDisk,
        FailureReason::RateLimit => FailureClass::ApiError429RateLimit,
        FailureReason::ProviderUnavailable
        | FailureReason::ProviderOverloaded
        | FailureReason::Transient5xx
        | FailureReason::Timeout
            if signal.http_status.is_some_and(|s| s >= 500) =>
        {
            FailureClass::ApiError5xxTransient
        }
        FailureReason::Hard if signal.http_status == Some(400) => FailureClass::ApiError400Document,
        _ if text.is_some_and(looks_like_executor_tool_model_config_failure) => {
            FailureClass::ExecutorConfig
        }
        _ if read_tail(raw_stream).is_none() && exit_code != 0 => FailureClass::WrapperInternal,
        _ => FailureClass::AgentExitNonzero,
    }
}

fn signal_from_json_line(
    line: &str,
    executor: ExecutorKind,
    route: Option<String>,
) -> Option<FailureSignal> {
    // The direct OpenRouter envelope/calibration fixture path. Calling the
    // shared parser here is the no-duplication contract with the native path.
    // stderr may prefix the JSON with a logger label, so also try the suffix
    // beginning at the envelope's opening object.
    let envelope = std::iter::once(line).chain(
        line.find("{\"error\"")
            .filter(|position| *position > 0)
            .map(|position| &line[position..]),
    );
    for envelope in envelope {
        if let Some(parsed) = parse_openrouter_error_envelope(envelope) {
            let signal = failure_signal_from_envelope(envelope, None, executor, route.clone())?;
            if parsed.status.is_some()
                || parsed.error_type.is_some()
                || signal.reason != FailureReason::Unknown
            {
                return Some(signal);
            }
        }
    }

    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let event_type = value.get("type").and_then(|v| v.as_str());
    let is_pi_error = event_type == Some("error")
        || (event_type == Some("response")
            && value.get("success").and_then(|v| v.as_bool()) == Some(false));
    if !is_pi_error {
        return None;
    }
    let error = value.get("error").or_else(|| value.get("message"));
    let message = error
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| error.map(serde_json::Value::to_string))
        .unwrap_or_else(|| "pi request failed".to_string());
    let envelope = error.map(|v| serde_json::json!({"error": v}).to_string());
    let parsed = envelope
        .as_deref()
        .and_then(parse_openrouter_error_envelope);
    let status = value
        .get("status")
        .or_else(|| value.get("code"))
        .and_then(json_u16)
        .or_else(|| parsed.as_ref().and_then(|p| p.status))
        .or_else(|| extract_status_from_message(&message));
    let error_type = value
        .get("error_type")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| parsed.as_ref().and_then(|p| p.error_type.clone()))
        .or_else(|| extract_error_type(&message));
    Some(failure_signal_from_evidence(
        status,
        error_type,
        parsed.as_ref().and_then(|p| p.provider_code.clone()),
        parsed.as_ref().and_then(|p| p.retry_after_secs),
        &message,
        executor,
        route,
    ))
}

fn json_u16(value: &serde_json::Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn extract_status_from_message(text: &str) -> Option<u16> {
    for marker in ["API error ", "HTTP "] {
        let lower = text.to_ascii_lowercase();
        let marker_lower = marker.to_ascii_lowercase();
        if let Some(pos) = lower.find(&marker_lower) {
            let after = &text[pos + marker.len()..];
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(status) = digits.parse() {
                return Some(status);
            }
        }
    }
    None
}

fn extract_error_type(text: &str) -> Option<String> {
    // Do not treat a stream event's generic `type` (for example `result`) as
    // OpenRouter's canonical error_type; nested envelopes are parsed
    // structurally before this fallback.
    let needle = "\"error_type\"";
    let pos = text.find(needle)?;
    let after = &text[pos + needle.len()..];
    let start = after.find('"').map(|pos| pos + 1)?;
    let value = &after[start..];
    let end = value.find('"')?;
    (end > 0).then(|| value[..end].to_string())
}

pub fn infer_executor(raw_stream: &Path) -> ExecutorKind {
    let content = read_tail(raw_stream).unwrap_or_default();
    if content.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .is_some_and(|kind| {
                matches!(
                    kind.as_str(),
                    "session" | "turn_end" | "tool_execution_start" | "response" | "error"
                )
            })
    }) {
        ExecutorKind::Pi
    } else {
        ExecutorKind::Native
    }
}

/// Classify `NoOperationalOutput` (guardrail G4): the agent "talked but
/// didn't act" — exited cleanly / called `wg done`, produced no artifacts,
/// wrote no files outside `log/`, but left a non-empty `output.log`.
///
/// This is the *weak* fallback for tasks without a parsed `## Deliverables`
/// block (the strong signal is G1's `DeliverableMissing` preflight). When the
/// signature matches, the retry path (G3) injects the no-op directive block
/// so the loop breaks instead of repeating meta/observation work.
///
/// # Arguments
/// - `clean_exit_or_done`: exit code 0 OR the agent called `wg done`.
/// - `artifacts_empty`: `task.artifacts` is empty (no `wg artifact` calls).
/// - `has_file_writes`: files were written outside `log/` — true if
///   `git status --porcelain` is non-empty OR `output.log` shows a mutation
///   command (`write_file` / `edit_file` / `wg add` / shell-mutation). The
///   caller may derive this from either signal per the G4 rule.
/// - `output_log_nonempty`: `output.log` has non-whitespace content.
pub fn classify_no_operational_output(
    clean_exit_or_done: bool,
    artifacts_empty: bool,
    has_file_writes: bool,
    output_log_nonempty: bool,
) -> Option<FailureClass> {
    if clean_exit_or_done && artifacts_empty && !has_file_writes && output_log_nonempty {
        Some(FailureClass::NoOperationalOutput)
    } else {
        None
    }
}

/// Scan an `output.log` body for evidence of filesystem mutation — the
/// command tokens the agent shells out to write/edit files. Used by the G4
/// classifier (via the wrapper) to derive `has_file_writes` from the log
/// when `git status` is unavailable or unreliable.
///
/// Matches (case-insensitive, as substrings):
/// - `write_file` / `edit_file` — the executor tool calls.
/// - `wg add` — staging a file for commit.
/// - `wg artifact` — recording an artifact (counts as a write for G4 since
///   it implies the agent produced an output; the `artifacts_empty` signal
///   already gates this, but the log token is a corroborating signal).
/// - shell-mutation commands: `git commit`, `git mv`, `mkdir -p`, `curl`,
///   `wget`, `cp `, `mv `, `tee ` — i.e. the operational verbs an intake
///   task would use to produce its deliverables.
///
/// Returns `true` if ANY mutation token is present.
pub fn output_log_has_mutations(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    const TOKENS: &[&str] = &[
        "write_file",
        "edit_file",
        "wg add",
        "wg artifact",
        "git commit",
        "git mv",
        "mkdir -p",
        "curl ",
        "wget ",
        "cp ",
        "mv ",
        "tee ",
    ];
    TOKENS.iter().any(|t| lower.contains(t))
}

/// Read up to TAIL_BYTES from the end of `path`, returning the string content.
/// Returns None if the file doesn't exist, can't be read, or is empty.
fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let offset = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    if buf.is_empty() { None } else { Some(buf) }
}

/// Extract the integer value of the first `api_error_status` key found in `text`.
/// Handles both `"api_error_status":400` and `"api_error_status": 400` (with space).
fn extract_api_error_status(text: &str) -> Option<u32> {
    let key = "api_error_status";
    let pos = text.find(key)?;
    let after = &text[pos + key.len()..];
    let mut chars = after.chars().peekable();
    // Skip closing quote (if present), then colon, then optional whitespace.
    // Input is typically: `"api_error_status":400` or `api_error_status: 400`.
    // After skipping past `api_error_status`, `after` starts with `":400` or `:400`.
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            break;
        }
        chars.next();
    }
    // read digits
    let digits: String = chars.take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn looks_like_disk_exhaustion(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("no space left on device")
        || lower.contains("os error 28")
        || lower.contains("enospc")
        || lower.contains("disk quota exceeded")
}

/// Detect disk exhaustion in either executor stdout/stderr or its structured
/// raw stream. Dead-agent recovery uses this when the filesystem was too full
/// for the wrapper's final `wg fail` graph write to succeed.
pub fn is_disk_resource_failure(raw_stream: &Path, output_log: &Path) -> bool {
    [raw_stream, output_log]
        .into_iter()
        .any(|path| read_tail(path).is_some_and(|tail| looks_like_disk_exhaustion(&tail)))
}

fn looks_like_executor_tool_model_config_failure(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("param: tools")
        && lower.contains("model")
        && (lower.contains("does not exist")
            || lower.contains("not found")
            || lower.contains("unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_stream(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_classifier_pdf_400_from_real_jsonl() {
        let f = write_stream(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process PDF"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn test_classifier_pdf_400_could_not_process_document() {
        let f = write_stream(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process document"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn classifier_pi_402_and_calibration_envelope_are_credit_exhausted() {
        let pi = write_stream(
            r#"{"type":"error","error":{"code":402,"message":"Insufficient credits","metadata":{"error_type":"payment_required"}}}"#,
        );
        let signal = classify_signal_from_raw_stream(
            pi.path(),
            None,
            1,
            ExecutorKind::Pi,
            Some("pi:openrouter:test/model".into()),
        );
        assert_eq!(signal.reason, FailureReason::CreditExhausted);
        assert_eq!(signal.http_status, Some(402));
        assert_eq!(signal.confidence, 1.0);

        let calibration = write_stream(
            r#"{"error":{"message":"Provider returned error","code":402,"metadata":{"provider_name":"Crucible","raw":"{\"error\":{\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\",\"message\":\"Out of credits. Top up at /dashboard/billing to continue.\"}}"}}}"#,
        );
        let signal =
            classify_signal_from_raw_stream(calibration.path(), None, 1, ExecutorKind::Pi, None);
        assert_eq!(signal.reason, FailureReason::CreditExhausted);
        assert_eq!(signal.error_type.as_deref(), Some("insufficient_quota"));
    }

    #[test]
    fn classifier_structured_statuses_and_retry_after() {
        let cases = [
            (
                r#"{"type":"error","error":{"code":429,"message":"Rate limited","metadata":{"error_type":"rate_limit_exceeded","retry_after":12.5}}}"#,
                FailureReason::RateLimit,
                Some(12.5),
            ),
            (
                r#"{"type":"response","success":false,"error":{"code":401,"message":"Unauthorized","metadata":{"error_type":"authentication"}}}"#,
                FailureReason::Auth,
                None,
            ),
            (
                r#"{"type":"error","error":{"code":529,"message":"Provider overloaded","metadata":{"error_type":"provider_overloaded"}}}"#,
                FailureReason::ProviderOverloaded,
                None,
            ),
        ];
        for (body, expected, retry_after) in cases {
            let stream = write_stream(body);
            let signal =
                classify_signal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
            assert_eq!(signal.reason, expected, "{body}");
            assert_eq!(signal.retry_after_secs, retry_after, "{body}");
        }
    }

    #[test]
    fn test_classifier_429_rate_limit() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":429,"message":"Rate limit exceeded"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError429RateLimit
        );
    }

    #[test]
    fn test_classifier_500_transient() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":500,"message":"Internal server error"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError5xxTransient
        );
    }

    #[test]
    fn test_classifier_503_transient() {
        let f = write_stream(
            r#"{"type":"result","is_error":true,"api_error_status":503,"message":"Service unavailable"}"#,
        );
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError5xxTransient
        );
    }

    #[test]
    fn test_classifier_hard_timeout() {
        // File doesn't matter for exit 124
        let f = write_stream("doesn't matter");
        assert_eq!(
            classify_from_raw_stream(f.path(), 124),
            FailureClass::AgentHardTimeout
        );
    }

    #[test]
    fn test_classifier_generic_exit() {
        let f = write_stream(r#"{"type":"result","subtype":"success","result":"done"}"#);
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::AgentExitNonzero
        );
    }

    #[test]
    fn test_classifier_disk_exhaustion_is_resource_failure() {
        for message in [
            "error: failed to write: No space left on device (os error 28)",
            "link failed: ENOSPC",
            "Disk quota exceeded",
        ] {
            let f = write_stream(message);
            assert_eq!(
                classify_from_raw_stream(f.path(), 1),
                FailureClass::ResourceExhaustedDisk
            );
        }
    }

    #[test]
    fn test_dead_attempt_detects_output_only_disk_failure_after_large_bookkeeping() {
        let raw = write_stream("{\"type\":\"result\",\"subtype\":\"success\"}\n");
        let output = write_stream(&format!(
            "No space left on device (os error 28)\n{}",
            "wrapper bookkeeping\n".repeat(2_000)
        ));
        assert!(is_disk_resource_failure(raw.path(), output.path()));
    }

    #[test]
    fn test_classifier_codex_unavailable_optional_tool_model() {
        let f = write_stream("The model 'gpt-image-2' does not exist.\nparam: tools\n");
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ExecutorConfig
        );
    }

    #[test]
    fn test_classifier_missing_raw_stream() {
        let path = std::path::PathBuf::from("/nonexistent/path/raw_stream.jsonl");
        assert_eq!(
            classify_from_raw_stream(&path, 1),
            FailureClass::WrapperInternal
        );
    }

    #[test]
    fn test_classifier_truncated_jsonl() {
        // Last line is partial JSON — should fall back, not panic
        let f = write_stream(r#"{"type":"result","api_error_status":400,"mes"#);
        // Still extracts the status code from partial JSON
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document
        );
    }

    #[test]
    fn test_classifier_empty_stream_nonzero_exit() {
        let f = write_stream("");
        // Empty stream + non-zero exit → WrapperInternal (no stream data)
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::WrapperInternal
        );
    }

    #[test]
    fn test_extract_api_error_status_with_space() {
        assert_eq!(
            extract_api_error_status(r#""api_error_status": 400"#),
            Some(400)
        );
    }

    #[test]
    fn test_extract_api_error_status_no_space() {
        assert_eq!(
            extract_api_error_status(r#""api_error_status":429"#),
            Some(429)
        );
    }

    #[test]
    fn test_extract_api_error_status_not_found() {
        assert_eq!(extract_api_error_status(r#"{"type":"result"}"#), None);
    }

    #[test]
    fn completed_terminal_receipt_outranks_incidental_timeout_text() {
        // Reduced fixture from provider-backoff-contract: design/tool output
        // discussed timeouts, but the final provider receipt is exact and says
        // the turn completed. Assistant prose is intentionally irrelevant.
        let stream = write_stream(concat!(
            "{\"type\":\"tool_execution_end\",\"toolCallId\":\"t1\",\"result\":\"provider timeout policy\"}\n",
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"all done\"}],\"responseId\":\"resp-1\",\"stopReason\":\"stop\",\"rawStopReason\":\"completed\"}}\n",
            "{\"type\":\"agent_end\",\"messages\":[{\"role\":\"assistant\",\"responseId\":\"resp-1\",\"stopReason\":\"stop\",\"rawStopReason\":\"completed\"}],\"willRetry\":false}\n",
            "{\"type\":\"compaction_end\",\"result\":{\"summary\":\"transport timeout and reset acceptance cases\"}}\n",
        ));

        let terminal =
            classify_terminal_from_raw_stream(stream.path(), None, 124, ExecutorKind::Pi, None);
        assert_eq!(terminal.state, TerminalStreamState::Completed);
        assert_eq!(terminal.reason_code, "exact-agent-turn-completed");
        assert_eq!(
            terminal.receipts.len(),
            1,
            "duplicate receipt must collapse"
        );
        assert_eq!(
            classify_signal_from_raw_stream(stream.path(), None, 124, ExecutorKind::Pi, None,)
                .reason,
            FailureReason::Unknown,
            "completed turn must not become provider timeout telemetry"
        );
    }

    #[test]
    fn completed_receipt_plus_typed_guard_refusal_is_finalization_blocked() {
        let stream = write_stream(concat!(
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"responseId\":\"resp-1\",\"rawStopReason\":\"completed\"}}\n",
            "{\"type\":\"finalization_blocked\",\"code\":\"NeedsReview\",\"message\":\"review budget exhausted\"}\n",
        ));
        let terminal =
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
        assert_eq!(terminal.state, TerminalStreamState::FinalizationBlocked);
        assert_eq!(terminal.finalization_code.as_deref(), Some("needs-review"));
        assert_eq!(
            classify_signal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None,).reason,
            FailureReason::Unknown
        );
    }

    #[test]
    fn genuine_pre_terminal_transport_timeout_remains_provider_failure() {
        let stream = write_stream(
            r#"{"type":"error","error":{"code":408,"message":"request timed out","metadata":{"error_type":"timeout"}}}"#,
        );
        let terminal =
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
        assert_eq!(terminal.state, TerminalStreamState::ProviderFailure);
        assert_eq!(terminal.failure_reason, Some(FailureReason::Timeout));
        assert_eq!(
            classify_signal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None,).reason,
            FailureReason::Timeout
        );
    }

    #[test]
    fn structured_timeout_after_last_completed_turn_is_provider_failure() {
        let stream = write_stream(concat!(
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"responseId\":\"resp-1\",\"rawStopReason\":\"completed\"}}\n",
            "{\"type\":\"error\",\"error\":{\"code\":408,\"message\":\"request timed out\",\"metadata\":{\"error_type\":\"timeout\"}}}\n",
        ));
        let terminal =
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
        assert_eq!(terminal.state, TerminalStreamState::ProviderFailure);
        assert_eq!(terminal.failure_reason, Some(FailureReason::Timeout));
    }

    #[test]
    fn recovered_timeout_before_completed_turn_is_completed() {
        let stream = write_stream(concat!(
            "{\"type\":\"error\",\"error\":{\"code\":408,\"message\":\"request timed out\",\"metadata\":{\"error_type\":\"timeout\"}}}\n",
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"responseId\":\"resp-1\",\"rawStopReason\":\"completed\"}}\n",
        ));
        assert_eq!(
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None,)
                .state,
            TerminalStreamState::Completed
        );
    }

    #[test]
    fn conflicting_exact_terminal_receipts_are_typed_ambiguous() {
        let stream = write_stream(concat!(
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"responseId\":\"resp-1\",\"rawStopReason\":\"completed\"}}\n",
            "{\"type\":\"turn_end\",\"message\":{\"role\":\"assistant\",\"responseId\":\"resp-2\",\"rawStopReason\":\"failed\"}}\n",
        ));
        let first =
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
        let replay =
            classify_terminal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None);
        assert_eq!(first.state, TerminalStreamState::Ambiguous);
        assert_eq!(first.reason_code, "conflicting-exact-terminal-receipts");
        assert_eq!(first.receipts.len(), 2);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&replay).unwrap(),
            "projection must be restart-stable"
        );
        assert_eq!(
            classify_signal_from_raw_stream(stream.path(), None, 1, ExecutorKind::Pi, None,).reason,
            FailureReason::Unknown,
            "ambiguity must not invent source/provider failure"
        );
    }

    #[test]
    fn assistant_prose_alone_never_proves_completion() {
        let stream = write_stream(
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"task completed successfully"}],"stopReason":"stop"}}"#,
        );
        assert_eq!(
            classify_terminal_from_raw_stream(stream.path(), None, 0, ExecutorKind::Pi, None,)
                .state,
            TerminalStreamState::Unknown
        );
    }

    #[test]
    fn classifier_detects_no_operational_output() {
        // Full signature: clean exit, no artifacts, no file writes, non-empty
        // output.log → NoOperationalOutput.
        assert_eq!(
            classify_no_operational_output(true, true, false, true),
            Some(FailureClass::NoOperationalOutput)
        );

        // Non-clean exit (crash/timeout) → not no-op (it's a real failure).
        assert_eq!(
            classify_no_operational_output(false, true, false, true),
            None
        );

        // Artifacts present → agent did produce something → not no-op.
        assert_eq!(
            classify_no_operational_output(true, false, false, true),
            None
        );

        // File writes detected → agent acted → not no-op.
        assert_eq!(classify_no_operational_output(true, true, true, true), None);

        // Empty output.log → crash, not meta work → not no-op.
        assert_eq!(
            classify_no_operational_output(true, true, false, false),
            None
        );
    }

    #[test]
    fn output_log_has_mutations_detects_write_tokens() {
        // Executor tool calls.
        assert!(output_log_has_mutations(
            "Used write_file to create latest.pt"
        ));
        assert!(output_log_has_mutations("edit_file src/foo.rs"));
        // wg add / artifact.
        assert!(output_log_has_mutations("ran: wg add latest.pt"));
        assert!(output_log_has_mutations("wg artifact t1 latest.pt"));
        // Shell-mutation verbs.
        assert!(output_log_has_mutations("git commit -m x"));
        assert!(output_log_has_mutations("mkdir -p seed"));
        assert!(output_log_has_mutations("curl -o x.bin URL"));
        assert!(output_log_has_mutations("cp a b"));

        // Pure meta/observation prose — no mutation tokens.
        assert!(!output_log_has_mutations(
            "Analyzed the task. The checkpoint metadata looks fine. Summary: ready."
        ));
        assert!(!output_log_has_mutations(""));
        // Case-insensitivity.
        assert!(output_log_has_mutations("WRITE_FILE latest.pt"));
    }
}
