//! Bounded retry-with-backoff for transient lock-acquisition failures.
//!
//! Networked filesystems (notably MooseFS) occasionally surface flock
//! contention as `EIO` (errno 5) rather than the expected `EWOULDBLOCK`,
//! and may also legitimately return `EINTR`. These are transient — the
//! lock typically becomes acquirable a few hundred milliseconds later —
//! but bare callers see them as hard failures and abort `wg add` / `wg
//! log` etc. This module wraps a lock-acquisition closure with bounded
//! exponential backoff + jitter so the wrapper layer above (graph load /
//! save / modify) can keep using a single `?` while transient errors are
//! absorbed.
//!
//! Hard errors (`EACCES`, `ENOENT`, `ENOSPC`, …) are NOT retried — they
//! are not transient and retrying would just delay the inevitable
//! failure.

use std::io;
use std::time::{Duration, Instant};

/// Retry policy for transient lock-acquisition failures.
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Maximum total wall-clock time before giving up.
    pub budget: Duration,
    /// Initial backoff delay, doubled each iteration up to `max_delay`.
    pub initial_delay: Duration,
    /// Multiplicative factor applied to the delay each iteration.
    pub factor: f64,
    /// Cap on per-iteration delay so the factor doesn't grow unbounded.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            budget: Duration::from_millis(env_or_u64("WG_LOCK_RETRY_BUDGET_MS", 5_000)),
            initial_delay: Duration::from_millis(env_or_u64("WG_LOCK_INITIAL_DELAY_MS", 25)),
            factor: 2.0,
            max_delay: Duration::from_millis(500),
        }
    }
}

fn env_or_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default)
}

/// `true` if `err` represents a transient flock failure that we should
/// retry: `EIO` (MooseFS quirk), `EWOULDBLOCK` (contention on the
/// happy-path errno), and `EINTR` (signal during the syscall).
pub fn is_transient_blocking(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EIO) | Some(libc::EWOULDBLOCK) | Some(libc::EINTR)
    ) || err.kind() == io::ErrorKind::WouldBlock
        || err.kind() == io::ErrorKind::Interrupted
}

/// Like [`is_transient_blocking`] but excludes `EWOULDBLOCK` — for
/// callers (such as the non-blocking shared read lock used by
/// `load_graph`) where `EWOULDBLOCK` is the documented non-error signal
/// that "another process holds the exclusive lock; proceed without
/// taking the shared lock". Retrying `EWOULDBLOCK` there would defeat
/// the non-blocking contract.
pub fn is_transient_nonblocking(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EIO) | Some(libc::EINTR)
    ) || err.kind() == io::ErrorKind::Interrupted
}

/// Run `acquire` until it returns `Ok(())` or the policy gives up.
///
/// `is_retriable` classifies io errors: `true` means "transient, retry
/// after backoff"; `false` means "hard error, propagate immediately".
///
/// On budget exhaustion the final error message is augmented with
/// retry-count + elapsed-time, e.g.
///   `"acquire lock failed after 12 retries over 5012ms: <inner>"`.
pub fn retry_acquire<F, R>(
    policy: &RetryPolicy,
    is_retriable: R,
    mut acquire: F,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
    R: Fn(&io::Error) -> bool,
{
    let start = Instant::now();
    let mut delay = policy.initial_delay;
    let mut retries: u32 = 0;
    loop {
        match acquire() {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !is_retriable(&e) {
                    return Err(e);
                }
                let elapsed = start.elapsed();
                if elapsed >= policy.budget {
                    let kind = e.kind();
                    let msg = format!(
                        "acquire lock failed after {} retries over {}ms: {}",
                        retries,
                        elapsed.as_millis(),
                        e
                    );
                    log::warn!("{}", msg);
                    return Err(io::Error::new(kind, msg));
                }
                let sleep_for = jitter(delay).min(policy.budget.saturating_sub(elapsed));
                log::debug!(
                    "lock acquisition transient error ({}); retry {} after {}ms",
                    e,
                    retries + 1,
                    sleep_for.as_millis()
                );
                std::thread::sleep(sleep_for);
                retries += 1;
                let next = delay.mul_f64(policy.factor);
                delay = if next > policy.max_delay {
                    policy.max_delay
                } else {
                    next
                };
            }
        }
    }
}

/// Add positive jitter in `[0, d/4]` to `d` so concurrent retriers
/// desynchronize. We avoid pulling in `rand` for this — `SystemTime`
/// nanos are good enough as a non-cryptographic noise source.
fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.subsec_nanos())
        .unwrap_or(0) as u64;
    let quarter_ms = (d.as_millis() / 4) as u64;
    if quarter_ms == 0 {
        // For very short delays (<4ms) jitter at the microsecond scale.
        let quarter_us = (d.as_micros() / 4) as u64;
        let off = if quarter_us == 0 { 0 } else { nanos % (quarter_us + 1) };
        return d + Duration::from_micros(off);
    }
    let off = nanos % (quarter_ms + 1);
    d + Duration::from_millis(off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            budget: Duration::from_millis(500),
            initial_delay: Duration::from_millis(1),
            factor: 2.0,
            max_delay: Duration::from_millis(10),
        }
    }

    #[test]
    fn retry_succeeds_after_n_eio_failures() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            let n = attempts.get();
            attempts.set(n + 1);
            if n < 3 {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok(), "expected success, got {:?}", result);
        assert_eq!(attempts.get(), 4, "expected 4 attempts (3 EIO + 1 ok)");
    }

    #[test]
    fn retry_succeeds_after_ewouldblock_failures() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            let n = attempts.get();
            attempts.set(n + 1);
            if n < 2 {
                Err(io::Error::from_raw_os_error(libc::EWOULDBLOCK))
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn retry_succeeds_after_eintr_failures() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            let n = attempts.get();
            attempts.set(n + 1);
            if n < 1 {
                Err(io::Error::from_raw_os_error(libc::EINTR))
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn retry_gives_up_after_budget_exhausted() {
        let policy = RetryPolicy {
            budget: Duration::from_millis(40),
            initial_delay: Duration::from_millis(5),
            factor: 1.5,
            max_delay: Duration::from_millis(20),
        };
        let attempts = Cell::new(0u32);
        let start = Instant::now();
        let result = retry_acquire(&policy, is_transient_blocking, || {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(libc::EIO))
        });
        let elapsed = start.elapsed();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("retries"), "msg should include retry count: {}", msg);
        assert!(msg.contains("ms"), "msg should include elapsed ms: {}", msg);
        // Budget enforcement: must not run forever, but at least make
        // multiple attempts.
        assert!(attempts.get() >= 2, "expected >=2 attempts, got {}", attempts.get());
        assert!(
            elapsed < Duration::from_millis(500),
            "retry loop overshot budget by a wide margin: {:?}",
            elapsed
        );
    }

    #[test]
    fn retry_propagates_eacces_immediately() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(libc::EACCES))
        });
        assert!(result.is_err());
        assert_eq!(
            attempts.get(),
            1,
            "EACCES must not be retried; expected exactly 1 attempt"
        );
    }

    #[test]
    fn retry_propagates_enoent_immediately() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "ENOENT must not be retried");
    }

    #[test]
    fn retry_propagates_enospc_immediately() {
        let policy = fast_policy();
        let attempts = Cell::new(0u32);
        let result = retry_acquire(&policy, is_transient_blocking, || {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "ENOSPC must not be retried");
    }

    #[test]
    fn nonblocking_predicate_excludes_ewouldblock() {
        let ewb = io::Error::from_raw_os_error(libc::EWOULDBLOCK);
        let eio = io::Error::from_raw_os_error(libc::EIO);
        let eintr = io::Error::from_raw_os_error(libc::EINTR);
        let eacc = io::Error::from_raw_os_error(libc::EACCES);
        assert!(!is_transient_nonblocking(&ewb));
        assert!(is_transient_nonblocking(&eio));
        assert!(is_transient_nonblocking(&eintr));
        assert!(!is_transient_nonblocking(&eacc));
    }

    #[test]
    fn blocking_predicate_includes_all_three() {
        let ewb = io::Error::from_raw_os_error(libc::EWOULDBLOCK);
        let eio = io::Error::from_raw_os_error(libc::EIO);
        let eintr = io::Error::from_raw_os_error(libc::EINTR);
        let eacc = io::Error::from_raw_os_error(libc::EACCES);
        assert!(is_transient_blocking(&ewb));
        assert!(is_transient_blocking(&eio));
        assert!(is_transient_blocking(&eintr));
        assert!(!is_transient_blocking(&eacc));
    }

    #[test]
    fn jitter_stays_within_25_percent_band() {
        let base = Duration::from_millis(100);
        for _ in 0..50 {
            let j = jitter(base);
            assert!(j >= base, "jitter must not shorten the delay");
            assert!(
                j <= base + Duration::from_millis(25),
                "jitter must stay within +25%, got {:?}",
                j
            );
        }
    }
}
