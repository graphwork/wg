//! Cross-task poison (TC8 / D-iii / audit **B7**) — the worst-ranked threat in the
//! adversarial study, jointly owned by the exec + review planes. Today only **provenance**
//! (`ResultEnvelope.producer`) + **trust-lowering** on a caught defection exist. This
//! module adds the three structural defenses the study names, as small **pure** decisions
//! the CLI layer wires into placement / grant / accept / verify / revoke:
//!
//!   1. **Tier-by-graph-position** ([`classify_position`] + [`position_trust_floor`]) — a
//!      *foundational* task (one with downstream descendants) floors its placement trust at
//!      `Verified` (the A / attested tier). A poisoned foundational artifact propagates to
//!      every descendant, so it must never run on a low-trust provider regardless of its own
//!      sensitivity label. Only a *leaf* (no descendant, minimal blast radius) may use a
//!      lower-trust (B / verified-overflow) provider.
//!
//!   2. **Descendant re-run** ([`crate::graph::WorkGraph::transitive_descendants`] feeds
//!      this) — when an upstream artifact is later found bad, every transitive descendant
//!      that consumed it is enumerated and re-run. (The graph walk lives on `WorkGraph`; the
//!      *re-run action* lives at the CLI seam in `exec_fed_cmd` / `review_cmd`.)
//!
//!   3. **Input re-verification across trust boundaries** ([`inputs_crossing_trust_boundary`])
//!      — a higher-trust task re-verifies inputs produced by a **strictly lower-trust** task
//!      before consuming them. The grant seam refuses to seal a cross-boundary input into the
//!      consumer's context until that input has a recorded integrity verification.
//!
//! Every decision here is **fail-closed / over-protective**: the failure direction is
//! "force a higher tier / demand a re-verify", never "silently place on a stranger".

use super::{TrustLevel, trust_rank};

/// A task's position in the dependency graph (the cross-task-poison blast-radius input).
///
/// `Foundational` ⇔ at least one task transitively depends on this task's output; a poison
/// here propagates downstream. `Leaf` ⇔ nothing depends on it; a poison is self-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphPosition {
    /// Has ≥1 descendant — its output feeds other tasks (high blast radius).
    Foundational,
    /// No descendant — minimal blast radius (eligible for a lower-trust tier).
    Leaf,
}

impl GraphPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            GraphPosition::Foundational => "foundational",
            GraphPosition::Leaf => "leaf",
        }
    }
}

/// Classify a task's [`GraphPosition`] from its transitive-descendant count. A task with any
/// descendant is `Foundational`; one with none is a `Leaf`.
pub fn classify_position(descendant_count: usize) -> GraphPosition {
    if descendant_count > 0 {
        GraphPosition::Foundational
    } else {
        GraphPosition::Leaf
    }
}

/// The **tier-by-graph-position** trust floor (defense 1). A `Foundational` task floors at
/// `Verified` — only the A / attested tier may run it. A `Leaf` imposes no positional floor
/// (`None`), so the sensitivity-derived floor stands. The leash takes the **stricter** of
/// this and the sensitivity floor, so position can only *raise* the bar, never lower it.
pub fn position_trust_floor(position: GraphPosition) -> Option<TrustLevel> {
    match position {
        GraphPosition::Foundational => Some(TrustLevel::Verified),
        GraphPosition::Leaf => None,
    }
}

/// Pick the **stricter** (higher-rank) of two trust floors — the fail-closed fold the leash
/// uses to combine the sensitivity floor with the positional floor.
pub fn stricter_floor(a: TrustLevel, b: TrustLevel) -> TrustLevel {
    if trust_rank(a) >= trust_rank(b) { a } else { b }
}

/// **Input re-verification across trust boundaries** (defense 3). Given a consuming task's
/// trust tier and its upstream inputs as `(upstream_task_id, producing_provider_trust)`,
/// return the upstream task ids that crossed a trust boundary **downward** — i.e. were
/// produced by a *strictly lower-trust* provider than the consumer.
///
/// A higher-trust task MUST re-verify exactly these inputs before consuming them: a
/// lower-trust producer is the cross-task-poison injection point, and the consumer's own
/// (higher) trust does not transfer to bytes a stranger produced. Same-tier or higher-tier
/// inputs are not returned (no downward boundary to defend).
pub fn inputs_crossing_trust_boundary(
    consumer_trust: TrustLevel,
    upstream_producers: &[(String, TrustLevel)],
) -> Vec<String> {
    upstream_producers
        .iter()
        .filter(|(_, producer_trust)| trust_rank(*producer_trust) < trust_rank(consumer_trust))
        .map(|(task_id, _)| task_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_position_is_foundational_iff_has_descendants() {
        assert_eq!(classify_position(0), GraphPosition::Leaf);
        assert_eq!(classify_position(1), GraphPosition::Foundational);
        assert_eq!(classify_position(7), GraphPosition::Foundational);
    }

    #[test]
    fn foundational_floors_at_verified_leaf_imposes_none() {
        assert_eq!(
            position_trust_floor(GraphPosition::Foundational),
            Some(TrustLevel::Verified)
        );
        assert_eq!(position_trust_floor(GraphPosition::Leaf), None);
    }

    #[test]
    fn stricter_floor_picks_higher_rank() {
        assert_eq!(
            stricter_floor(TrustLevel::Provisional, TrustLevel::Verified),
            TrustLevel::Verified
        );
        assert_eq!(
            stricter_floor(TrustLevel::Verified, TrustLevel::Unknown),
            TrustLevel::Verified
        );
        assert_eq!(
            stricter_floor(TrustLevel::Provisional, TrustLevel::Provisional),
            TrustLevel::Provisional
        );
    }

    #[test]
    fn cross_boundary_returns_only_strictly_lower_trust_inputs() {
        let consumer = TrustLevel::Verified;
        let inputs = vec![
            ("u_verified".to_string(), TrustLevel::Verified), // same tier — not a boundary
            ("u_prov".to_string(), TrustLevel::Provisional),  // lower — boundary
            ("u_unknown".to_string(), TrustLevel::Unknown),   // lower — boundary
        ];
        let crossing = inputs_crossing_trust_boundary(consumer, &inputs);
        assert_eq!(
            crossing,
            vec!["u_prov".to_string(), "u_unknown".to_string()]
        );
    }

    #[test]
    fn cross_boundary_empty_when_consumer_is_lowest() {
        // An Unknown consumer cannot be *higher* than any producer ⇒ no downward boundary.
        let inputs = vec![
            ("a".to_string(), TrustLevel::Verified),
            ("b".to_string(), TrustLevel::Provisional),
            ("c".to_string(), TrustLevel::Unknown),
        ];
        assert!(inputs_crossing_trust_boundary(TrustLevel::Unknown, &inputs).is_empty());
    }

    #[test]
    fn cross_boundary_provisional_consumer_only_flags_unknown() {
        let inputs = vec![
            ("a".to_string(), TrustLevel::Verified), // higher — not a downward boundary
            ("b".to_string(), TrustLevel::Provisional), // same — not a boundary
            ("c".to_string(), TrustLevel::Unknown),  // lower — boundary
        ];
        let crossing = inputs_crossing_trust_boundary(TrustLevel::Provisional, &inputs);
        assert_eq!(crossing, vec!["c".to_string()]);
    }
}
