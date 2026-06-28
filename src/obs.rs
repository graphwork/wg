//! Observability for the federation / execution / content-safety planes (audit M20).
//!
//! Before this module, `src/providers/` and `src/review/` emitted **zero** logs and the
//! relay node had **no `/metrics`** — an operator running a federation deploy saw nothing
//! (audit-testops findings #7/#8/#9). This module is the minimum observability layer the
//! production-readiness audit asks for, in two faces:
//!
//! - **Counters** — process-global, lock-free [`AtomicU64`] tallies of the load-bearing
//!   events an operator monitors: WG-Review verdicts (by disposition), WG-Exec
//!   placements / refusals / accepted-or-rejected results, WG-Fed freshness checks and
//!   failures, and node request volume by response class. They are exposed in Prometheus
//!   text format for the node's `/metrics` endpoint ([`render_prometheus`]) and as a JSON
//!   snapshot for tests and `--json` callers ([`FedMetrics::snapshot`]). The counters live
//!   in the core decision functions (`evaluate_placement`, `review_inbound`,
//!   `check_fresh`, the node router) so **every** caller — CLI, e2e, or a future wired
//!   dispatch path — increments them, not just one entry point.
//!
//! - **Correlation IDs + spans** — [`new_correlation_id`] mints a short process-unique id
//!   per inbound operation; the planes emit [`tracing`] events tagged with it (and with
//!   the natural ids already in scope — task id, content cid, wgid) so a single item is
//!   traceable across review → placement → exec → accept, and a single request is
//!   traceable through the node. Because `tracing` is built with its `log` feature and WG
//!   installs no tracing `Subscriber`, those events surface through the existing
//!   `env_logger` (`RUST_LOG=debug` for the per-decision detail; node request lines are
//!   `info`) with no subscriber wiring.
//!
//! This mirrors the existing [`crate::metrics`] module (cleanup-domain counters) in style
//! — a `static` registry of atomics with a `reset()` for tests — but is scoped to the
//! three federation planes rather than worktree cleanup.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-global counters for the federation / exec / review planes (M20).
///
/// Every field is monotonic and lock-free. The single [`static`](FED_METRICS) instance is
/// reachable via [`metrics()`]; tests call [`FedMetrics::reset`] to take a clean baseline.
#[derive(Debug)]
pub struct FedMetrics {
    // ── WG-Review (content-safety gate) ─────────────────────────────────────────
    /// Total review pipeline runs (`review_inbound*`), any disposition.
    pub review_checks_total: AtomicU64,
    /// Reviews whose final verdict was `accept`.
    pub review_accept_total: AtomicU64,
    /// Reviews whose final verdict was `quarantine`.
    pub review_quarantine_total: AtomicU64,
    /// Reviews whose final verdict was `reject`.
    pub review_reject_total: AtomicU64,

    // ── WG-Exec (execution federation) ──────────────────────────────────────────
    /// Placement evaluations that returned an *eligible* leash decision.
    pub exec_placements_total: AtomicU64,
    /// Placement evaluations refused by the fail-closed filter/leash.
    pub exec_refusals_total: AtomicU64,
    /// Results committed at the accept boundary (attribution + gates cleared).
    pub exec_results_accepted_total: AtomicU64,
    /// Results refused at the accept boundary (attribution / integrity / leash).
    pub exec_results_rejected_total: AtomicU64,

    // ── WG-Fed (identity / freshness) ───────────────────────────────────────────
    /// Freshness attestation checks performed ([`crate::identity::freshness::check_fresh`]).
    pub fed_freshness_checks_total: AtomicU64,
    /// Freshness checks that failed closed (stale / rollback).
    pub fed_freshness_failures_total: AtomicU64,

    // ── Node (relay inbox) ──────────────────────────────────────────────────────
    /// Total HTTP requests the node routed.
    pub node_requests_total: AtomicU64,
    /// Responses with a 2xx status.
    pub node_responses_2xx_total: AtomicU64,
    /// Responses with a 4xx status (client / auth / quota / cid-mismatch).
    pub node_responses_4xx_total: AtomicU64,
    /// Responses with a 5xx status (server error).
    pub node_responses_5xx_total: AtomicU64,
}

impl FedMetrics {
    const fn new() -> Self {
        Self {
            review_checks_total: AtomicU64::new(0),
            review_accept_total: AtomicU64::new(0),
            review_quarantine_total: AtomicU64::new(0),
            review_reject_total: AtomicU64::new(0),
            exec_placements_total: AtomicU64::new(0),
            exec_refusals_total: AtomicU64::new(0),
            exec_results_accepted_total: AtomicU64::new(0),
            exec_results_rejected_total: AtomicU64::new(0),
            fed_freshness_checks_total: AtomicU64::new(0),
            fed_freshness_failures_total: AtomicU64::new(0),
            node_requests_total: AtomicU64::new(0),
            node_responses_2xx_total: AtomicU64::new(0),
            node_responses_4xx_total: AtomicU64::new(0),
            node_responses_5xx_total: AtomicU64::new(0),
        }
    }

    /// Reset all counters to zero. Test-only: lets a test take a clean baseline so it can
    /// assert exact deltas regardless of what other tests in the process recorded.
    pub fn reset(&self) {
        for c in [
            &self.review_checks_total,
            &self.review_accept_total,
            &self.review_quarantine_total,
            &self.review_reject_total,
            &self.exec_placements_total,
            &self.exec_refusals_total,
            &self.exec_results_accepted_total,
            &self.exec_results_rejected_total,
            &self.fed_freshness_checks_total,
            &self.fed_freshness_failures_total,
            &self.node_requests_total,
            &self.node_responses_2xx_total,
            &self.node_responses_4xx_total,
            &self.node_responses_5xx_total,
        ] {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Tally a completed review by its final disposition.
    pub fn record_review_verdict(&self, disposition: &str) {
        self.review_checks_total.fetch_add(1, Ordering::Relaxed);
        let c = match disposition {
            "accept" => &self.review_accept_total,
            "quarantine" => &self.review_quarantine_total,
            "reject" => &self.review_reject_total,
            _ => return,
        };
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// Tally a placement decision: `eligible` → a placement, otherwise a refusal.
    pub fn record_placement(&self, eligible: bool) {
        if eligible {
            self.exec_placements_total.fetch_add(1, Ordering::Relaxed);
        } else {
            self.exec_refusals_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Tally a result at the accept boundary: committed vs refused.
    pub fn record_exec_result(&self, accepted: bool) {
        if accepted {
            self.exec_results_accepted_total
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.exec_results_rejected_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Tally a freshness check: `fresh` → success, otherwise a fail-closed failure.
    pub fn record_freshness(&self, fresh: bool) {
        self.fed_freshness_checks_total
            .fetch_add(1, Ordering::Relaxed);
        if !fresh {
            self.fed_freshness_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Tally a node response by status line; the leading digit selects the class counter.
    pub fn record_node_response(&self, status: &str) {
        self.node_requests_total.fetch_add(1, Ordering::Relaxed);
        let c = match status.as_bytes().first() {
            Some(b'2') => &self.node_responses_2xx_total,
            Some(b'4') => &self.node_responses_4xx_total,
            Some(b'5') => &self.node_responses_5xx_total,
            _ => return,
        };
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// A point-in-time copy of every counter (for JSON / tests).
    pub fn snapshot(&self) -> MetricsSnapshot {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        MetricsSnapshot {
            review_checks_total: g(&self.review_checks_total),
            review_accept_total: g(&self.review_accept_total),
            review_quarantine_total: g(&self.review_quarantine_total),
            review_reject_total: g(&self.review_reject_total),
            exec_placements_total: g(&self.exec_placements_total),
            exec_refusals_total: g(&self.exec_refusals_total),
            exec_results_accepted_total: g(&self.exec_results_accepted_total),
            exec_results_rejected_total: g(&self.exec_results_rejected_total),
            fed_freshness_checks_total: g(&self.fed_freshness_checks_total),
            fed_freshness_failures_total: g(&self.fed_freshness_failures_total),
            node_requests_total: g(&self.node_requests_total),
            node_responses_2xx_total: g(&self.node_responses_2xx_total),
            node_responses_4xx_total: g(&self.node_responses_4xx_total),
            node_responses_5xx_total: g(&self.node_responses_5xx_total),
        }
    }
}

/// A serializable, point-in-time copy of [`FedMetrics`] (one `u64` per counter).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub review_checks_total: u64,
    pub review_accept_total: u64,
    pub review_quarantine_total: u64,
    pub review_reject_total: u64,
    pub exec_placements_total: u64,
    pub exec_refusals_total: u64,
    pub exec_results_accepted_total: u64,
    pub exec_results_rejected_total: u64,
    pub fed_freshness_checks_total: u64,
    pub fed_freshness_failures_total: u64,
    pub node_requests_total: u64,
    pub node_responses_2xx_total: u64,
    pub node_responses_4xx_total: u64,
    pub node_responses_5xx_total: u64,
}

static FED_METRICS: FedMetrics = FedMetrics::new();

/// The process-global federation metrics registry.
pub fn metrics() -> &'static FedMetrics {
    &FED_METRICS
}

// ── Recording helpers ───────────────────────────────────────────────────────────
//
// These centralize the counter bumps so call sites read as a single intent
// (`obs::record_review_verdict("reject")`) and so the disposition→counter mapping lives
// in one place. The richer `tracing` events (with task id / content cid / wgid for
// correlation) stay at the call sites where that context is in scope.

/// Tally a completed review by its final disposition (`accept` / `quarantine` / `reject`).
pub fn record_review_verdict(disposition: &str) {
    metrics().record_review_verdict(disposition);
}

/// Tally a placement decision: `eligible` → a placement, otherwise a refusal.
pub fn record_placement(eligible: bool) {
    metrics().record_placement(eligible);
}

/// Tally a result at the accept boundary: committed vs refused.
pub fn record_exec_result(accepted: bool) {
    metrics().record_exec_result(accepted);
}

/// Tally a freshness check: `fresh` → success, otherwise a fail-closed failure.
pub fn record_freshness(fresh: bool) {
    metrics().record_freshness(fresh);
}

/// Tally a node response by its status line (e.g. `"200 OK"`, `"413 Payload Too Large"`).
/// The leading digit selects the response-class counter; an unparseable status is counted
/// only in the request total.
pub fn record_node_response(status: &str) {
    metrics().record_node_response(status);
}

// ── Correlation IDs ───────────────────────────────────────────────────────────────

static CORR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Mint a short, process-unique correlation id (e.g. `"c1a2b-0"`). Combines the process
/// id with a monotonic counter so two ids from one process never collide and ids from
/// different processes (the two-host case) are distinguishable in a merged log. No wall
/// clock or RNG is used, so it is deterministic given a process and call order.
pub fn new_correlation_id() -> String {
    let n = CORR_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("c{:x}-{:x}", std::process::id(), n)
}

// ── Prometheus exposition ─────────────────────────────────────────────────────────

/// Render every counter in Prometheus text exposition format (v0.0.4) — the body of the
/// node's `GET /metrics`. Each metric carries a `# HELP`/`# TYPE … counter` preamble;
/// the verdict and node-response families use a label (`disposition` / `class`) so they
/// render as one metric family rather than parallel names.
pub fn render_prometheus() -> String {
    render_snapshot(&metrics().snapshot())
}

/// Render a specific [`MetricsSnapshot`] in Prometheus text format. Split out from
/// [`render_prometheus`] so tests can render a hand-built snapshot deterministically
/// (the global counters are bumped concurrently by other tests in the same process).
pub fn render_snapshot(s: &MetricsSnapshot) -> String {
    let mut o = String::new();

    fn scalar(o: &mut String, name: &str, help: &str, val: u64) {
        o.push_str(&format!("# HELP {name} {help}\n"));
        o.push_str(&format!("# TYPE {name} counter\n"));
        o.push_str(&format!("{name} {val}\n"));
    }

    // WG-Review verdicts as one labeled family.
    o.push_str("# HELP wg_review_verdicts_total Content-safety review verdicts by disposition.\n");
    o.push_str("# TYPE wg_review_verdicts_total counter\n");
    o.push_str(&format!(
        "wg_review_verdicts_total{{disposition=\"accept\"}} {}\n",
        s.review_accept_total
    ));
    o.push_str(&format!(
        "wg_review_verdicts_total{{disposition=\"quarantine\"}} {}\n",
        s.review_quarantine_total
    ));
    o.push_str(&format!(
        "wg_review_verdicts_total{{disposition=\"reject\"}} {}\n",
        s.review_reject_total
    ));
    scalar(
        &mut o,
        "wg_review_checks_total",
        "Total content-safety review pipeline runs.",
        s.review_checks_total,
    );

    // WG-Exec.
    scalar(
        &mut o,
        "wg_exec_placements_total",
        "Eligible exec placement decisions.",
        s.exec_placements_total,
    );
    scalar(
        &mut o,
        "wg_exec_refusals_total",
        "Exec placements refused by the fail-closed leash/filter.",
        s.exec_refusals_total,
    );
    scalar(
        &mut o,
        "wg_exec_results_accepted_total",
        "Exec results committed at the accept boundary.",
        s.exec_results_accepted_total,
    );
    scalar(
        &mut o,
        "wg_exec_results_rejected_total",
        "Exec results refused at the accept boundary.",
        s.exec_results_rejected_total,
    );

    // WG-Fed freshness.
    scalar(
        &mut o,
        "wg_fed_freshness_checks_total",
        "Freshness attestation checks performed.",
        s.fed_freshness_checks_total,
    );
    scalar(
        &mut o,
        "wg_fed_freshness_failures_total",
        "Freshness checks that failed closed (stale/rollback).",
        s.fed_freshness_failures_total,
    );

    // Node responses as one labeled family + the request total.
    o.push_str("# HELP wg_node_responses_total Node HTTP responses by status class.\n");
    o.push_str("# TYPE wg_node_responses_total counter\n");
    o.push_str(&format!(
        "wg_node_responses_total{{class=\"2xx\"}} {}\n",
        s.node_responses_2xx_total
    ));
    o.push_str(&format!(
        "wg_node_responses_total{{class=\"4xx\"}} {}\n",
        s.node_responses_4xx_total
    ));
    o.push_str(&format!(
        "wg_node_responses_total{{class=\"5xx\"}} {}\n",
        s.node_responses_5xx_total
    ));
    scalar(
        &mut o,
        "wg_node_requests_total",
        "Total HTTP requests the node routed.",
        s.node_requests_total,
    );

    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_ids_are_unique_and_monotonic() {
        let a = new_correlation_id();
        let b = new_correlation_id();
        assert_ne!(a, b, "two correlation ids must differ");
        assert!(a.starts_with('c'));
    }

    // The recording/render logic is tested on a *local* `FedMetrics` so the assertions are
    // immune to the global static being bumped concurrently by other tests in the process
    // (placement / review / freshness unit tests all increment the global registry).
    #[test]
    fn record_methods_bump_the_right_counters() {
        let m = FedMetrics::new();
        m.record_review_verdict("accept");
        m.record_review_verdict("reject");
        m.record_review_verdict("reject");
        m.record_review_verdict("bogus"); // unknown disposition → checks++ only
        m.record_placement(true);
        m.record_placement(false);
        m.record_exec_result(true);
        m.record_exec_result(false);
        m.record_freshness(true);
        m.record_freshness(false);
        m.record_node_response("200 OK");
        m.record_node_response("413 Payload Too Large");
        m.record_node_response("500 Internal Server Error");

        let s = m.snapshot();
        assert_eq!(s.review_checks_total, 4);
        assert_eq!(s.review_accept_total, 1);
        assert_eq!(s.review_reject_total, 2);
        assert_eq!(s.review_quarantine_total, 0);
        assert_eq!(s.exec_placements_total, 1);
        assert_eq!(s.exec_refusals_total, 1);
        assert_eq!(s.exec_results_accepted_total, 1);
        assert_eq!(s.exec_results_rejected_total, 1);
        assert_eq!(s.fed_freshness_checks_total, 2);
        assert_eq!(s.fed_freshness_failures_total, 1);
        assert_eq!(s.node_requests_total, 3);
        assert_eq!(s.node_responses_2xx_total, 1);
        assert_eq!(s.node_responses_4xx_total, 1);
        assert_eq!(s.node_responses_5xx_total, 1);
    }

    #[test]
    fn reset_zeroes_every_counter() {
        let m = FedMetrics::new();
        m.record_review_verdict("reject");
        m.record_placement(false);
        m.reset();
        assert_eq!(m.snapshot(), MetricsSnapshot::default());
    }

    #[test]
    fn prometheus_render_is_well_formed() {
        let s = MetricsSnapshot {
            review_reject_total: 1,
            review_checks_total: 1,
            node_responses_2xx_total: 1,
            node_requests_total: 1,
            ..Default::default()
        };
        let out = render_snapshot(&s);
        // Labeled families render with their label.
        assert!(out.contains("wg_review_verdicts_total{disposition=\"reject\"} 1"));
        assert!(out.contains("wg_node_responses_total{class=\"2xx\"} 1"));
        // Every metric has a HELP + TYPE preamble.
        assert!(out.contains("# TYPE wg_review_verdicts_total counter"));
        assert!(out.contains("# TYPE wg_exec_refusals_total counter"));
        assert!(out.contains("# HELP wg_fed_freshness_failures_total"));
        // The global entry point is reachable and produces the same families.
        assert!(render_prometheus().contains("# TYPE wg_node_requests_total counter"));
    }
}
