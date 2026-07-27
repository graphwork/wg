//! Shared provider-failure detection and rolling telemetry persistence.
//!
//! Both subprocess handlers and the native HTTP executor feed this module so
//! provider classification cannot drift between execution paths.

use crate::dispatch::plan::ExecutorKind;
use crate::graph::{FailureReason, FailureSignal};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub const MAX_TELEMETRY_RECORDS: usize = 1_000;
pub const TELEMETRY_RETENTION_HOURS: i64 = 24;

/// Provider details decoded from an OpenRouter/OpenAI-compatible error body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ParsedProviderError {
    pub status: Option<u16>,
    pub message: Option<String>,
    pub error_type: Option<String>,
    pub provider_code: Option<Value>,
    pub retry_after_secs: Option<f64>,
    pub provider_name: Option<String>,
    pub upstream_type: Option<String>,
    pub upstream_code: Option<String>,
    pub upstream_message: Option<String>,
}

impl ParsedProviderError {
    pub fn has_provider_detail(&self) -> bool {
        self.provider_name.is_some()
            || self.upstream_type.is_some()
            || self.upstream_code.is_some()
            || self.upstream_message.is_some()
    }
}

fn nonempty_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn status_value(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

/// Parse an OpenRouter error envelope. This pure function is shared by the
/// native executor and the subprocess raw-stream classifier.
pub fn parse_openrouter_error_envelope(body: &str) -> Option<ParsedProviderError> {
    let root: Value = serde_json::from_str(body.trim()).ok()?;
    let error = root.get("error")?;
    let mut error = if let Some(s) = error.as_str() {
        serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.to_string()))
    } else {
        error.clone()
    };
    // A pi event may carry the complete OpenRouter body as an escaped string
    // in its `error` field, yielding one additional `{error:{...}}` layer.
    if let Some(nested) = error.get("error") {
        error = nested.clone();
    }
    let detail = error.as_object()?;
    let metadata = detail.get("metadata").and_then(Value::as_object);

    let status = detail
        .get("status")
        .and_then(status_value)
        .or_else(|| detail.get("code").and_then(status_value))
        .or_else(|| root.get("status").and_then(status_value));
    let message = detail.get("message").and_then(nonempty_string);
    let mut error_type = detail
        .get("error_type")
        .or_else(|| detail.get("type"))
        .and_then(nonempty_string)
        .or_else(|| metadata?.get("error_type").and_then(nonempty_string));
    let provider_code = metadata
        .and_then(|m| m.get("provider_code"))
        .cloned()
        .or_else(|| detail.get("provider_code").cloned());
    let retry_after_secs = metadata
        .and_then(|m| m.get("retry_after"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()));
    let provider_name = metadata
        .and_then(|m| m.get("provider_name").or_else(|| m.get("provider")))
        .and_then(nonempty_string);

    let raw = metadata.and_then(|m| m.get("raw"));
    let raw_value = raw.map(|raw| match raw {
        Value::String(s) => serde_json::from_str::<Value>(s)
            .unwrap_or_else(|_| serde_json::json!({"error": {"message": s}})),
        other => other.clone(),
    });
    let raw_error = raw_value.as_ref().map(|v| v.get("error").unwrap_or(v));
    let upstream_type = raw_error
        .and_then(|e| e.get("type"))
        .and_then(nonempty_string)
        .or_else(|| error_type.clone());
    let upstream_code = raw_error
        .and_then(|e| e.get("code"))
        .and_then(nonempty_string)
        .or_else(|| detail.get("code").and_then(nonempty_string));
    let upstream_message = raw_error
        .and_then(|e| e.get("message"))
        .and_then(nonempty_string);

    // OpenRouter sometimes puts the canonical type only in metadata.raw.
    if error_type.is_none() {
        error_type = upstream_type.clone();
    }

    Some(ParsedProviderError {
        status,
        message,
        error_type,
        provider_code,
        retry_after_secs,
        provider_name,
        upstream_type,
        upstream_code,
        upstream_message,
    })
}

fn substring_reason(message: &str) -> Option<FailureReason> {
    let lower = message.to_ascii_lowercase();
    // Order is intentional: provider messages often contain several generic
    // words (for example, "rate limit due to insufficient credits").
    if [
        "insufficient credits",
        "out of credits",
        "payment required",
        "insufficient_quota",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::CreditExhausted)
    } else if ["rate limit", "rate_limit", "too many requests"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::RateLimit)
    } else if ["overloaded", "capacity"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::ProviderOverloaded)
    } else if ["unavailable", "no provider", "no available model"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::ProviderUnavailable)
    } else if ["unauthorized", "invalid api key", "forbidden"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::Auth)
    } else if ["timed out", "timeout"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(FailureReason::Timeout)
    } else {
        None
    }
}

fn typed_reason(error_type: &str) -> Option<FailureReason> {
    let kind = error_type.to_ascii_lowercase();
    if kind.contains("payment_required") || kind.contains("insufficient_quota") {
        Some(FailureReason::CreditExhausted)
    } else if kind.contains("rate_limit") {
        Some(FailureReason::RateLimit)
    } else if kind.contains("token_limit") {
        Some(FailureReason::QuotaToken)
    } else if kind.contains("authentication") || kind.contains("permission_denied") {
        Some(FailureReason::Auth)
    } else if kind.contains("provider_overloaded") {
        Some(FailureReason::ProviderOverloaded)
    } else if kind.contains("provider_unavailable") {
        Some(FailureReason::ProviderUnavailable)
    } else if kind.contains("timeout") {
        Some(FailureReason::Timeout)
    } else if kind.contains("invalid_request") || kind.contains("context_length") {
        Some(FailureReason::Hard)
    } else if kind == "server" || kind.contains("server_error") {
        Some(FailureReason::Transient5xx)
    } else {
        None
    }
}

fn status_reason(status: u16, error_type: Option<&str>) -> FailureReason {
    if let Some(reason) = error_type.and_then(typed_reason) {
        return reason;
    }
    match status {
        402 => FailureReason::CreditExhausted,
        429 => FailureReason::RateLimit,
        401 | 403 => FailureReason::Auth,
        408 | 504 => FailureReason::Timeout,
        529 => FailureReason::ProviderOverloaded,
        503 => FailureReason::ProviderUnavailable,
        500 | 502 | 505..=528 | 530..=599 => FailureReason::Transient5xx,
        400..=499 => FailureReason::Hard,
        _ => FailureReason::Unknown,
    }
}

/// Extract an echoed `Retry-After:` or `retry_after:` number from prose.
pub fn parse_retry_after_text(text: &str) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    for key in ["retry-after", "retry_after"] {
        if let Some(pos) = lower.find(key) {
            let rest = lower[pos + key.len()..]
                .trim_start_matches(|c: char| c == ':' || c == '=' || c.is_whitespace());
            let number: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(value) = number.parse::<f64>() {
                return Some(value);
            }
        }
    }
    None
}

/// Normalize structured and textual evidence using the confidence ladder.
pub fn failure_signal_from_evidence(
    status: Option<u16>,
    error_type: Option<String>,
    provider_code: Option<Value>,
    retry_after_secs: Option<f64>,
    message: &str,
    executor: ExecutorKind,
    route: Option<String>,
) -> FailureSignal {
    let (reason, confidence) = if let Some(status) = status {
        (
            status_reason(status, error_type.as_deref()),
            if error_type.is_some() { 1.0 } else { 0.8 },
        )
    } else if let Some(reason) = error_type.as_deref().and_then(typed_reason) {
        (reason, 0.5)
    } else if let Some(reason) = substring_reason(message) {
        (reason, 0.5)
    } else {
        (FailureReason::Unknown, 0.2)
    };

    FailureSignal {
        reason,
        confidence,
        http_status: status,
        error_type,
        provider_code,
        retry_after_secs: retry_after_secs.or_else(|| parse_retry_after_text(message)),
        executor,
        route,
        detected_at_ms: Utc::now().timestamp_millis(),
    }
}

pub fn failure_signal_from_envelope(
    body: &str,
    fallback_status: Option<u16>,
    executor: ExecutorKind,
    route: Option<String>,
) -> Option<FailureSignal> {
    let parsed = parse_openrouter_error_envelope(body)?;
    let message = [
        parsed.message.as_deref(),
        parsed.upstream_message.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    Some(failure_signal_from_evidence(
        parsed.status.or(fallback_status),
        parsed.error_type,
        parsed.provider_code,
        parsed.retry_after_secs,
        &message,
        executor,
        route,
    ))
}

pub fn route_bucket(route: Option<&str>) -> String {
    route
        .unwrap_or("unknown")
        .strip_suffix(":free")
        .unwrap_or(route.unwrap_or("unknown"))
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryRecord {
    pub ts: DateTime<Utc>,
    pub task: String,
    pub attempt: u32,
    pub bucket: String,
    #[serde(flatten)]
    pub signal: FailureSignal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_remaining_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_free_tier: Option<bool>,
}

impl TelemetryRecord {
    pub fn new(task: impl Into<String>, attempt: u32, signal: FailureSignal) -> Self {
        let bucket = route_bucket(signal.route.as_deref());
        Self {
            ts: Utc::now(),
            task: task.into(),
            attempt,
            bucket,
            signal,
            credit_remaining_usd: None,
            is_free_tier: None,
        }
    }
}

pub fn telemetry_path(dir: &Path) -> PathBuf {
    dir.join("service").join("provider-telemetry.jsonl")
}

struct TelemetryLock {
    #[cfg(unix)]
    file: File,
}

impl TelemetryLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error()).context("lock provider telemetry");
            }
            Ok(Self { file })
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Ok(Self {})
        }
    }
}

impl Drop for TelemetryLock {
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

pub fn read_records(dir: &Path) -> Result<Vec<TelemetryRecord>> {
    let path = telemetry_path(dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Atomically append and prune the rolling window under a sidecar lock.
/// Duplicate recording sites converge on `(task, attempt, executor, bucket)`.
pub fn append_record_at(dir: &Path, record: TelemetryRecord, now: DateTime<Utc>) -> Result<()> {
    let path = telemetry_path(dir);
    let lock_path = path.with_extension("lock");
    let _lock = TelemetryLock::acquire(&lock_path)?;
    let mut records = read_records(dir)?;
    let cutoff = now - Duration::hours(TELEMETRY_RETENTION_HOURS);
    records.retain(|existing| existing.ts >= cutoff);
    if let Some(existing) = records.iter_mut().find(|existing| {
        existing.task == record.task
            && existing.attempt == record.attempt
            && existing.signal.executor == record.signal.executor
            && existing.bucket == record.bucket
    }) {
        *existing = record;
    } else {
        records.push(record);
    }
    records.sort_by_key(|entry| entry.ts);
    if records.len() > MAX_TELEMETRY_RECORDS {
        records.drain(..records.len() - MAX_TELEMETRY_RECORDS);
    }
    let mut body = records
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    crate::atomic_file::write_atomic(&path, body.as_bytes())?;
    Ok(())
}

pub fn append_record(dir: &Path, record: TelemetryRecord) -> Result<()> {
    append_record_at(dir, record, Utc::now())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowCounters {
    pub last_1m: HashMap<FailureReason, u32>,
    pub last_5m: HashMap<FailureReason, u32>,
    pub last_1h: HashMap<FailureReason, u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub bucket: String,
    pub last_reason: FailureReason,
    pub recent: WindowCounters,
    pub consecutive_rate_limits: u32,
    pub consecutive_credit_exhausted: u32,
    pub last_retry_after_secs: Option<f64>,
    pub credit_remaining_usd: Option<f64>,
    pub cooled_until_ms: Option<i64>,
}

impl ProviderHealth {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            last_reason: FailureReason::Unknown,
            recent: WindowCounters::default(),
            consecutive_rate_limits: 0,
            consecutive_credit_exhausted: 0,
            last_retry_after_secs: None,
            credit_remaining_usd: None,
            cooled_until_ms: None,
        }
    }

    /// Update the cached aggregate. Rate-limit cooling is the maximum of an
    /// observed Retry-After and exponential backoff seeded by consecutive
    /// rate limits (1s, 2s, 4s, ... capped at 5 minutes).
    pub fn observe(&mut self, signal: &FailureSignal, now_ms: i64) {
        self.last_reason = signal.reason;
        if signal.reason == FailureReason::RateLimit {
            self.consecutive_rate_limits = self.consecutive_rate_limits.saturating_add(1);
        } else {
            self.consecutive_rate_limits = 0;
        }
        if signal.reason == FailureReason::CreditExhausted {
            self.consecutive_credit_exhausted = self.consecutive_credit_exhausted.saturating_add(1);
        } else {
            self.consecutive_credit_exhausted = 0;
        }
        self.last_retry_after_secs = signal.retry_after_secs;

        if signal.reason == FailureReason::RateLimit {
            let shift = self.consecutive_rate_limits.saturating_sub(1).min(18);
            let exponential_ms = (1_000_i64 << shift).min(300_000);
            let retry_ms = signal
                .retry_after_secs
                .map(|seconds| (seconds.max(0.0) * 1_000.0) as i64)
                .unwrap_or(0);
            let deadline = now_ms.saturating_add(exponential_ms.max(retry_ms));
            self.cooled_until_ms = Some(self.cooled_until_ms.unwrap_or(0).max(deadline));
        }
    }

    pub fn is_cooled(&self, now_ms: i64) -> bool {
        self.cooled_until_ms.is_some_and(|until| now_ms < until)
    }
}

pub fn provider_health(
    records: &[TelemetryRecord],
    now: DateTime<Utc>,
) -> HashMap<String, ProviderHealth> {
    let mut health = HashMap::new();
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|record| record.ts);
    for record in ordered {
        let entry = health
            .entry(record.bucket.clone())
            .or_insert_with(|| ProviderHealth::new(record.bucket.clone()));
        let age = now - record.ts;
        if age <= Duration::minutes(1) {
            *entry
                .recent
                .last_1m
                .entry(record.signal.reason)
                .or_default() += 1;
        }
        if age <= Duration::minutes(5) {
            *entry
                .recent
                .last_5m
                .entry(record.signal.reason)
                .or_default() += 1;
        }
        if age <= Duration::hours(1) {
            *entry
                .recent
                .last_1h
                .entry(record.signal.reason)
                .or_default() += 1;
        }
        entry.credit_remaining_usd = record.credit_remaining_usd;
        entry.observe(&record.signal, record.signal.detected_at_ms);
    }
    health
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn signal(reason: FailureReason, at: i64) -> FailureSignal {
        FailureSignal {
            reason,
            confidence: 0.8,
            http_status: Some(429),
            executor: ExecutorKind::Pi,
            route: Some("pi:openrouter:z-ai/glm-5.2:free".into()),
            detected_at_ms: at,
            ..Default::default()
        }
    }

    #[test]
    fn shared_openrouter_parser_reads_calibration_fixture() {
        let body = r#"{"error":{"message":"Provider returned error","code":402,"metadata":{"provider_name":"Crucible","raw":"{\"error\":{\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\",\"message\":\"Out of credits.\"}}"}}}"#;
        let parsed = parse_openrouter_error_envelope(body).unwrap();
        assert_eq!(parsed.status, Some(402));
        assert_eq!(parsed.provider_name.as_deref(), Some("Crucible"));
        assert_eq!(parsed.error_type.as_deref(), Some("insufficient_quota"));
        assert_eq!(parsed.upstream_message.as_deref(), Some("Out of credits."));
    }

    #[test]
    fn substring_ladder_prefers_credit_over_rate_limit() {
        let signal = failure_signal_from_evidence(
            None,
            None,
            None,
            None,
            "rate limit caused by insufficient credits",
            ExecutorKind::Pi,
            None,
        );
        assert_eq!(signal.reason, FailureReason::CreditExhausted);
        assert_eq!(signal.confidence, 0.5);
    }

    #[test]
    fn health_cooling_uses_retry_after_and_exponential_max() {
        let mut health = ProviderHealth::new("bucket");
        let mut first = signal(FailureReason::RateLimit, 1_000);
        first.retry_after_secs = Some(10.0);
        health.observe(&first, 1_000);
        assert_eq!(health.cooled_until_ms, Some(11_000));
        let second = signal(FailureReason::RateLimit, 2_000);
        health.observe(&second, 2_000);
        assert_eq!(health.cooled_until_ms, Some(11_000));
        for now in 3_000..12_000 {
            health.observe(&second, now);
        }
        assert!(health.cooled_until_ms.unwrap() > 11_000);
    }

    #[test]
    fn rolling_window_prunes_age_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        append_record_at(
            dir.path(),
            TelemetryRecord {
                ts: now - Duration::hours(25),
                ..TelemetryRecord::new("old", 1, signal(FailureReason::RateLimit, 0))
            },
            now,
        )
        .unwrap();
        for i in 0..1_010 {
            append_record_at(
                dir.path(),
                TelemetryRecord {
                    ts: now + Duration::milliseconds(i as i64),
                    ..TelemetryRecord::new(
                        format!("t-{i}"),
                        1,
                        signal(FailureReason::RateLimit, i as i64),
                    )
                },
                now + Duration::milliseconds(i as i64),
            )
            .unwrap();
        }
        let records = read_records(dir.path()).unwrap();
        assert_eq!(records.len(), MAX_TELEMETRY_RECORDS);
        assert!(records.iter().all(|record| record.task != "old"));
        assert_eq!(records.first().unwrap().task, "t-10");
    }

    #[test]
    fn concurrent_atomic_appends_are_not_corrupted() {
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let mut threads = Vec::new();
        for i in 0..32 {
            let dir = Arc::clone(&dir);
            threads.push(std::thread::spawn(move || {
                append_record(
                    dir.path(),
                    TelemetryRecord::new(
                        format!("task-{i}"),
                        1,
                        signal(FailureReason::RateLimit, i),
                    ),
                )
                .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let records = read_records(dir.path()).unwrap();
        assert_eq!(records.len(), 32);
        let content = std::fs::read_to_string(telemetry_path(dir.path())).unwrap();
        assert!(
            content
                .lines()
                .all(|line| serde_json::from_str::<TelemetryRecord>(line).is_ok())
        );
    }
}
