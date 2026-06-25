//! The `review{depth, default_verdict}` face of the WG-Exec `leash()` engine
//! (ADR-CS1 D3).
//!
//! Review depth is **not a new threshold and not a new dial** — it is an additional
//! output face of the one trust dial, keyed on the existing
//! [`crate::graph::TrustLevel`] × [`Sensitivity`]. Two coherence rules are inherited
//! **verbatim** from the dial's existing faces:
//!
//! 1. **Fail-closed on unlabeled.** The gate **cannot** emit "light review" for an
//!    unlabeled sensitivity or a below-floor trust — it routes deep. The failure
//!    direction is **over-review (a false positive)**, never under-review.
//! 2. **Monotonic escalate-on-flag.** The dial only ever tightens under suspicion;
//!    it never loosens itself.
//!
//! And one cross-cutting bound: **sensitivity floors depth** — a high-blast action
//! never gets the light path regardless of author trust (the RA-3 bound, ADR-CS3 D4).
//!
//! For the spark, Review-Wave B builds this face as a **standalone function** rather
//! than a literal `+1` on the not-yet-landed `leash()` engine (the seams are
//! to-be-built — ADR-CS1 context). The matrix, the two coherence rules, and
//! sensitivity-floors-depth are the durable design and are implemented here in full.

use super::{Sensitivity, Verdict};
use crate::graph::TrustLevel;

/// The applied review depth — which passes run and the default verdict on clean
/// (ADR-CS1 D3). Surfaced today by `wg review depth` / `wg review check` (a too-loose
/// route is visible at a glance); the `wg show` / `wg config lint` integration is
/// Review-Wave C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDepth {
    /// The highest pass that runs (1..4). Pass 1 always runs; Pass 2+ on the
    /// suspicious band; Pass 3 (sandbox) is a **spark stub** for IC2/opaque; Pass 4
    /// (human) is the escalation seam.
    pub max_pass: u8,
    /// The default verdict on clean content — `accept` on the light/standard path,
    /// **`quarantine` for the Unknown / unlabeled fail-closed cell**.
    pub default_verdict: Verdict,
    /// The diverse-reviewer quorum size for Pass 2 (ADR-CS2 D4): 1 on the standard
    /// band, **2 on the high band** (strictest-wins). The spark proves the *slot*;
    /// the production N is Review-Wave C.
    pub quorum: usize,
    /// A stable, human-facing label (`"Pass 1 only"`, …) for `wg show`.
    pub label: &'static str,
}

impl ReviewDepth {
    /// Does this depth run the Pass-2 LLM reviewer?
    pub fn runs_pass2(&self) -> bool {
        self.max_pass >= 2
    }

    /// Is this the light path (Pass 1 only, accept-default)? The trust-proportional
    /// "trusted ⇒ light" assertion.
    pub fn is_light(&self) -> bool {
        self.max_pass == 1 && self.default_verdict == Verdict::Accept
    }
}

/// Compute the applied `review.depth` from the author's trust and the (already
/// taint-inferred) sensitivity — the D3 matrix.
///
/// Rows are the author's trust *relative to the consumer*; sensitivity **floors**
/// the depth so a high-blast item never gets the light path regardless of trust.
pub fn review_depth(trust: &TrustLevel, sensitivity: Sensitivity) -> ReviewDepth {
    match (trust, sensitivity) {
        // Verified, low-sensitivity, transparent → the light path.
        (TrustLevel::Verified, Sensitivity::Low) => ReviewDepth {
            max_pass: 1,
            default_verdict: Verdict::Accept,
            quorum: 0,
            label: "Pass 1 only",
        },
        // Verified, high-sensitivity → Pass 1+2; accept on clean, human on soft hit.
        (TrustLevel::Verified, _) => ReviewDepth {
            max_pass: 2,
            default_verdict: Verdict::Accept,
            quorum: 1,
            label: "Pass 1+2 (verified, high-sensitivity)",
        },
        // Provisional (TOFU default for federated peers) → Pass 1+2, human on flag.
        (TrustLevel::Provisional, Sensitivity::Low) => ReviewDepth {
            max_pass: 2,
            default_verdict: Verdict::Accept,
            quorum: 1,
            label: "Pass 1+2 (provisional)",
        },
        (TrustLevel::Provisional, _) => ReviewDepth {
            max_pass: 2,
            default_verdict: Verdict::Accept,
            quorum: 2,
            label: "Pass 1+2 (provisional, high-sensitivity)",
        },
        // Unknown → Pass 1+2(+3), quarantine-by-default; human to release.
        // (Unlabeled sensitivity has already been folded to High upstream, so this
        // also covers the fail-closed cell — it can never be the light path.)
        (TrustLevel::Unknown, _) => ReviewDepth {
            max_pass: 3,
            default_verdict: Verdict::Quarantine,
            quorum: 2,
            label: "Pass 1+2+3, quarantine-by-default (unknown)",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_low_is_the_light_path() {
        let d = review_depth(&TrustLevel::Verified, Sensitivity::Low);
        assert!(d.is_light());
        assert_eq!(d.max_pass, 1);
        assert_eq!(d.default_verdict, Verdict::Accept);
        assert!(!d.runs_pass2());
    }

    #[test]
    fn unknown_is_deep_and_quarantine_default() {
        let d = review_depth(&TrustLevel::Unknown, Sensitivity::Low);
        assert!(!d.is_light(), "unknown must never be the light path");
        assert_eq!(d.default_verdict, Verdict::Quarantine);
        assert!(d.runs_pass2());
        assert_eq!(d.quorum, 2, "high band runs the diverse-reviewer quorum");
    }

    #[test]
    fn sensitivity_floors_depth_for_verified() {
        // Even a Verified author gets Pass 2 when sensitivity is High — the RA-3
        // bound: a high-blast action never gets the light path regardless of trust.
        let d = review_depth(&TrustLevel::Verified, Sensitivity::High);
        assert!(d.runs_pass2());
        assert!(!d.is_light());
    }

    #[test]
    fn unlabeled_folded_to_high_is_never_light() {
        // The caller folds Unlabeled→High before calling; assert the High row for
        // every trust level is non-light.
        for t in [
            TrustLevel::Verified,
            TrustLevel::Provisional,
            TrustLevel::Unknown,
        ] {
            let d = review_depth(&t, Sensitivity::High);
            assert!(!d.is_light(), "{t:?} + high must not be light");
        }
    }
}
