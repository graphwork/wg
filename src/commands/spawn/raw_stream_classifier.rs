//! Classify an agent failure from the raw JSONL stream written by the claude/codex executors.
//!
//! This is a pure function: no side-effects, no graph I/O. The wrapper invokes
//! `wg classify-failure` which shells out to this logic.

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
        FailureReason::Hard
            if signal.http_status == Some(400)
                && text.is_some_and(looks_like_document_error) =>
        {
            FailureClass::ApiError400Document
        }
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

/// Does this stream tail show the API rejecting a DOCUMENT (the 400 that means "your PDF is
/// broken"), as opposed to any other 400?
///
/// `status_reason` maps 400..=499 to `FailureReason::Hard` and a present status outranks the
/// message, so every 4xx without a typed error arrives here as Hard + 400 — a malformed PDF and an
/// exhausted API budget look identical at this point. Only the message tells them apart.
fn looks_like_document_error(text: &str) -> bool {
    ["could not process pdf", "could not process document", "could not process image"]
        .iter()
        .any(|needle| text.to_ascii_lowercase().contains(needle))
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
    fn test_classifier_400_usage_limit_is_not_a_document_error() {
        // A real Anthropic usage-limit payload. `status_reason` maps 400..=499 to
        // FailureReason::Hard, and the message is never consulted because a present status
        // outranks it, so this arrives as Hard + 400 — indistinguishable from a bad PDF unless
        // the document arm asks for document evidence.
        let f = write_stream(
            r#"{"type":"result","terminal_reason":"api_error","subtype":"success","is_error":true,"api_error_status":400,"result":"API Error: 400 You have reached your specified API usage limits. You will regain access on 2026-09-01 at 00:00 UTC."}"#,
        );
        assert_ne!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::ApiError400Document,
            "an exhausted API budget is not a malformed document"
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
        // The status is still extracted from the partial line. The CLASS changes with this commit:
        // a stream that stops mid-key carries no evidence of WHY the request was rejected, so it
        // is no longer reported as a document error. Only a message naming a document earns that.
        assert_eq!(
            classify_from_raw_stream(f.path(), 1),
            FailureClass::AgentExitNonzero
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
