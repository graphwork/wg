//! Bounded retry for transient provider failures on the **agency one-shot**
//! path (FLIP inference + comparison, Eval, assign/evaluate, and the
//! content-review one-shot).
//!
//! The source dispatcher already recovers from a direct, safely-replayable
//! provider failure (`docs/design-provider-failure-backoff.md`), and the native
//! HTTP client retries a 429/5xx once round-trip locally. The CLI handler
//! (notably `pi`) and the surrounding agency call sites did **not**: a single
//! `429 CONCURRENT_REQUEST_LIMIT_EXCEEDED` immediately degraded a FLIP review
//! to `reviewer_unavailable`. This module closes that gap.
//!
//! Safety invariant: retries only ever wrap a side-effect-free single
//! inference. A retry therefore can never convert an outage into an
//! acceptance — after the bounded budget is exhausted the caller still gets
//! the original error and the existing fail-closed `reviewer_unavailable`
//! outcome. Classification reuses the shared provider taxonomy
//! ([`crate::telemetry::classify_provider_signal_from_text`]), not a parallel
//! one.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::SourceProviderRetryConfig;
use crate::dispatch::plan::ExecutorKind;
use crate::graph::FailureReason;
use crate::telemetry::classify_provider_signal_from_text;

/// Resolved retry policy for the agency one-shot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgencyRetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub base: Duration,
    pub cap: Duration,
    pub budget: Duration,
}

impl AgencyRetryPolicy {
    pub fn from_config(config: &SourceProviderRetryConfig) -> Self {
        Self {
            enabled: config.enabled,
            max_retries: config.max_automatic_retries,
            base: Duration::from_secs(config.base_seconds),
            cap: Duration::from_secs(config.delay_cap_seconds),
            budget: Duration::from_secs(config.recovery_window_seconds),
        }
    }

    /// The maximum number of physical calls this policy can make.
    pub fn max_attempts(&self) -> u32 {
        self.enabled
            .then(|| self.max_retries.saturating_add(1))
            .unwrap_or(1)
    }
}

/// Normalized classification of one agency provider failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgencyFailureClassification {
    pub transient: bool,
    pub reason: FailureReason,
    /// Stable label (`rate-limit`, `transient-5xx`, `hard`, …).
    pub label: String,
    pub http_status: Option<u16>,
    pub retry_after: Option<Duration>,
}

impl AgencyFailureClassification {
    fn terminal(reason: FailureReason, label: impl Into<String>) -> Self {
        Self {
            transient: false,
            reason,
            label: label.into(),
            http_status: None,
            retry_after: None,
        }
    }
}

/// Classify an agency provider error from its full `anyhow` context chain.
pub fn classify_agency_failure(error: &anyhow::Error) -> AgencyFailureClassification {
    classify_agency_failure_text(&format!("{error:#}"))
}

/// Classify a captured provider error message.
pub fn classify_agency_failure_text(text: &str) -> AgencyFailureClassification {
    if text.trim().is_empty() {
        return AgencyFailureClassification::terminal(FailureReason::Unknown, "unknown");
    }
    let signal = classify_provider_signal_from_text(text, ExecutorKind::Pi, None);
    let transient = matches!(
        signal.reason,
        FailureReason::RateLimit
            | FailureReason::ProviderUnavailable
            | FailureReason::ProviderOverloaded
            | FailureReason::Transient5xx
            | FailureReason::Timeout
    ) || signal
        .http_status
        .is_some_and(|status| matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529));
    let retry_after = signal
        .retry_after_secs
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(Duration::from_secs_f64);
    AgencyFailureClassification {
        transient,
        reason: signal.reason,
        label: signal.reason.as_str().to_string(),
        http_status: signal.http_status,
        retry_after,
    }
}

/// Observable retry accounting for one agency call (or a FLIP two-phase pair).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgencyRetryStats {
    /// Physical provider calls performed (>= 1 for any attempted call).
    pub attempts: u32,
    /// Additional calls after the first (0 means the first call succeeded or
    /// failed terminally).
    pub retries: u32,
    /// `success`, or the final failure classification label.
    pub final_classification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    /// True when a transient failure was retried as far as the attempt cap or
    /// time budget allowed and still failed.
    #[serde(default)]
    pub exhausted: bool,
}

impl AgencyRetryStats {
    pub fn throttled(&self) -> bool {
        self.retries > 0
    }

    /// Fold the second half of a FLIP two-phase pair into the first so the
    /// receipt shows the total attempts across both phases.
    pub fn merge(&mut self, other: &AgencyRetryStats) {
        self.attempts = self.attempts.saturating_add(other.attempts);
        self.retries = self.retries.saturating_add(other.retries);
        if other.final_classification != "success" || self.final_classification.is_empty() {
            self.final_classification = other.final_classification.clone();
        }
        self.http_status = other.http_status.or(self.http_status);
        self.retry_after_seconds = other.retry_after_seconds.or(self.retry_after_seconds);
        self.exhausted |= other.exhausted;
    }
}

/// Typed failure returned only when a transient agency failure exhausted the
/// bounded retry budget (or fail-stopped on a terminal classification). The
/// caller downcasts this to surface the attempt count and final classification
/// in the review receipt while still treating the outcome as unavailable.
#[derive(Debug)]
pub struct AgencyRetryFailure {
    pub stats: AgencyRetryStats,
    pub source: anyhow::Error,
}

impl std::fmt::Display for AgencyRetryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "agency provider call failed after {} attempt(s) (classification={}, exhausted={}): {:#}",
            self.stats.attempts, self.stats.final_classification, self.stats.exhausted, self.source
        )
    }
}

impl std::error::Error for AgencyRetryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn exponential_delay(policy: &AgencyRetryPolicy, retries: u32) -> Duration {
    let shift = retries.min(16);
    let scaled = policy.base.saturating_mul(1u32 << shift);
    scaled.min(policy.cap)
}

/// Run `call` under the bounded policy, invoking `sleep` between attempts.
///
/// Returns the final result plus the accounting. On a transient error the
/// delay is `max(exponential, Retry-After)`, and a delay that would cross the
/// total time budget stops retrying rather than running the call early.
pub fn run_agency_retry_with<T, F, S>(
    policy: &AgencyRetryPolicy,
    mut call: F,
    mut sleep: S,
) -> (anyhow::Result<T>, AgencyRetryStats)
where
    F: FnMut() -> anyhow::Result<T>,
    S: FnMut(Duration),
{
    let started = Instant::now();
    let mut stats = AgencyRetryStats {
        final_classification: "unattempted".into(),
        ..AgencyRetryStats::default()
    };

    if !policy.enabled {
        stats.attempts = 1;
        return match call() {
            Ok(value) => {
                stats.final_classification = "success".into();
                (Ok(value), stats)
            }
            Err(error) => {
                stats.final_classification = classify_agency_failure(&error).label;
                (Err(error), stats)
            }
        };
    }

    loop {
        stats.attempts = stats.attempts.saturating_add(1);
        match call() {
            Ok(value) => {
                stats.final_classification = "success".into();
                return (Ok(value), stats);
            }
            Err(error) => {
                let classification = classify_agency_failure(&error);
                stats.final_classification = classification.label.clone();
                stats.http_status = classification.http_status;
                stats.retry_after_seconds = classification
                    .retry_after
                    .map(|duration| duration.as_secs());

                if !classification.transient {
                    return (Err(error), stats);
                }
                if stats.retries >= policy.max_retries {
                    stats.exhausted = true;
                    return (Err(error), stats);
                }

                let mut delay = exponential_delay(policy, stats.retries);
                if let Some(retry_after) = classification.retry_after {
                    delay = delay.max(retry_after);
                }

                let elapsed = started.elapsed();
                if elapsed.saturating_add(delay) > policy.budget {
                    stats.exhausted = true;
                    return (Err(error), stats);
                }

                eprintln!(
                    "[agency-retry] transient provider failure classification={} reason={} status={:?} attempt={}/{} retry_after_secs={:?} delay_ms={} — retrying the exact route",
                    classification.label,
                    classification.reason.as_str(),
                    classification.http_status,
                    stats.attempts,
                    policy.max_retries.saturating_add(1),
                    classification
                        .retry_after
                        .map(|duration| duration.as_secs()),
                    delay.as_millis(),
                );
                sleep(delay);
                stats.retries = stats.retries.saturating_add(1);
            }
        }
    }
}

/// Production entry point: bounded retry with real sleeping.
pub fn run_agency_retry<T, F>(
    policy: &AgencyRetryPolicy,
    call: F,
) -> (anyhow::Result<T>, AgencyRetryStats)
where
    F: FnMut() -> anyhow::Result<T>,
{
    run_agency_retry_with(policy, call, std::thread::sleep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn policy(max_retries: u32) -> AgencyRetryPolicy {
        AgencyRetryPolicy {
            enabled: true,
            max_retries,
            base: Duration::from_millis(10),
            cap: Duration::from_millis(40),
            budget: Duration::from_secs(5),
        }
    }

    #[test]
    fn four_twenty_nine_then_success_retries_and_completes() {
        let calls = Rc::new(Cell::new(0));
        let observed_delays = Rc::new(std::cell::RefCell::new(Vec::new()));
        let calls_ref = calls.clone();
        let delays_ref = observed_delays.clone();
        let (result, stats) = run_agency_retry_with(
            &policy(2),
            move || {
                calls_ref.set(calls_ref.get() + 1);
                if calls_ref.get() == 1 {
                    anyhow::bail!("API error 429: rate limit exceeded")
                }
                Ok("ok")
            },
            move |delay| delays_ref.borrow_mut().push(delay),
        );
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(calls.get(), 2);
        assert_eq!(stats.attempts, 2);
        assert_eq!(stats.retries, 1);
        assert_eq!(stats.final_classification, "success");
        assert!(!stats.exhausted);
        assert!(stats.throttled());
        assert_eq!(
            observed_delays.borrow().as_slice(),
            &[Duration::from_millis(10)]
        );
    }

    #[test]
    fn exhausted_four_twenty_nine_stays_failed_and_never_accepts() {
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &policy(2),
            || anyhow::bail!("API error 429: too many requests"),
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(stats.attempts, 3, "1 initial + 2 bounded retries");
        assert_eq!(stats.retries, 2);
        assert!(stats.exhausted);
        assert_eq!(stats.final_classification, "rate-limit");
        assert_eq!(stats.http_status, Some(429));
    }

    #[test]
    fn transient_five_hundred_retries_then_stays_failed_when_exhausted() {
        let calls = Rc::new(Cell::new(0));
        let calls_ref = calls.clone();
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &policy(1),
            move || {
                calls_ref.set(calls_ref.get() + 1);
                anyhow::bail!("upstream returned HTTP 503 service unavailable")
            },
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 2);
        assert_eq!(stats.attempts, 2);
        assert_eq!(stats.final_classification, "provider-unavailable");
        assert!(stats.exhausted);
    }

    #[test]
    fn non_retryable_bad_request_never_retries() {
        let calls = Rc::new(Cell::new(0));
        let calls_ref = calls.clone();
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &policy(3),
            move || {
                calls_ref.set(calls_ref.get() + 1);
                anyhow::bail!("API error 400: invalid request body")
            },
            |_| panic!("a 400 must not sleep"),
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.retries, 0);
        assert!(!stats.exhausted);
        assert_eq!(stats.final_classification, "hard");
    }

    #[test]
    fn retry_after_is_honored_as_a_lower_bound() {
        let observed_delays = Rc::new(std::cell::RefCell::new(Vec::new()));
        let delays_ref = observed_delays.clone();
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &policy(3),
            || anyhow::bail!("API error 429: rate limit; retry-after: 2"),
            move |delay| delays_ref.borrow_mut().push(delay),
        );
        assert!(result.is_err());
        assert_eq!(
            observed_delays.borrow().as_slice(),
            &[
                Duration::from_secs(2),
                Duration::from_secs(2),
                Duration::from_secs(2)
            ],
            "Retry-After dominates the exponential delay"
        );
        assert_eq!(stats.retry_after_seconds, Some(2));
        assert_eq!(stats.attempts, 4);
    }

    #[test]
    fn retry_after_beyond_the_time_budget_stops_without_sleeping() {
        let slept = Rc::new(Cell::new(false));
        let slept_ref = slept.clone();
        let tight = AgencyRetryPolicy {
            enabled: true,
            max_retries: 3,
            base: Duration::from_millis(1),
            cap: Duration::from_millis(1),
            budget: Duration::from_secs(1),
        };
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &tight,
            || anyhow::bail!("API error 429: rate limit; retry-after: 3600"),
            move |_| slept_ref.set(true),
        );
        assert!(result.is_err());
        assert!(!slept.get(), "must not sleep past the budget");
        assert_eq!(stats.attempts, 1);
        assert!(stats.exhausted);
    }

    #[test]
    fn disabled_policy_makes_exactly_one_call() {
        let calls = Rc::new(Cell::new(0));
        let calls_ref = calls.clone();
        let disabled = AgencyRetryPolicy {
            enabled: false,
            ..policy(3)
        };
        let (result, stats): (anyhow::Result<()>, AgencyRetryStats) = run_agency_retry_with(
            &disabled,
            move || {
                calls_ref.set(calls_ref.get() + 1);
                anyhow::bail!("API error 429: rate limit")
            },
            |_| panic!("disabled policy must not sleep"),
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.final_classification, "rate-limit");
    }

    #[test]
    fn classifies_structured_envelope_and_legacy_status() {
        let envelope = classify_agency_failure_text(
            r#"{"error":{"code":429,"message":"slow down","metadata":{"error_type":"rate_limit_exceeded","retry_after":7}}}"#,
        );
        assert!(envelope.transient);
        assert_eq!(envelope.http_status, Some(429));
        assert_eq!(envelope.retry_after, Some(Duration::from_secs(7)));

        let bare = classify_agency_failure_text("429 CONCURRENT_REQUEST_LIMIT_EXCEEDED");
        assert!(bare.transient);
        assert_eq!(bare.http_status, Some(429));
        assert_eq!(bare.reason, FailureReason::RateLimit);

        let auth = classify_agency_failure_text("API error 401: invalid api key");
        assert!(!auth.transient);
        assert_eq!(auth.reason, FailureReason::Auth);
    }
}
