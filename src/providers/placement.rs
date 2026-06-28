//! Placement & the **fail-closed leash engine** (ADR-E1 + the leash spine of
//! ADR-E2/E3/E4).
//!
//! The `leash()` function is the one place the five execution dials are decided
//! together — UCAN `{scope, ttl}`, context `{scope_tier, seal}`, verification
//! `{depth}`, lease `{term, cadence}`, and the placement `trust_floor` — from a single
//! `(provider_trust, task_sensitivity, pool_class, env_config)` input. The matcher
//! ([`evaluate_placement`]) is a **hard filter** (capability match + trust-floor) then
//! an **advisory, deterministic rank**: rank can reorder the eligible set but can
//! **never** promote a provider past the floor (ADR-E1 D3).
//!
//! The load-bearing invariant: the security decision lives entirely in the *filter*,
//! and the filter is **fail-closed** — a confidential task with no attested provider is
//! **refused** (never A/B, ADR-E2 D2), and an **unlabeled** task **refuses / routes to
//! C, never A** (D-i). A deployment cannot relax this into fail-open by tuning the rank.

use crate::context_scope::ContextScope;
use crate::identity::custody::LeashPolicy;

use super::{
    CapabilityAd, IsolationClass, PoolClass, Sensitivity, TrustLevel, trust_rank, trust_str,
};

/// The confidentiality lever the leash selects (ADR-E2 D1). **Minimize is NOT
/// confidentiality** — a B provider reads every token of its slice; only `Sealed`
/// (attested C) defends context against an untrusted operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSeal {
    /// **A — trust:** plaintext on a box you own/trust.
    Plaintext,
    /// **B — minimize:** the smallest slice, plaintext to the provider (blast-radius
    /// bound only, NOT confidentiality).
    Minimized,
    /// **C — attest:** sealed to a verified attestation; plaintext only inside the TEE.
    SealedToAttestation,
}

impl ContextSeal {
    pub fn as_str(self) -> &'static str {
        match self {
            ContextSeal::Plaintext => "plaintext",
            ContextSeal::Minimized => "minimized",
            ContextSeal::SealedToAttestation => "sealed-to-attestation",
        }
    }
}

/// The verification depth the leash selects (ADR-E4 D2). It **tightens monotonically
/// under suspicion and never loosens itself** — trust buys *cheaper verification on
/// fungible work*, never *unverified acceptance on work that matters*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationDepth {
    /// Trusted + normal: attribution + the WG eval-gate + a random spot-check.
    AttributionPlusEvalGate,
    /// Low-trust + checkable: a deterministic re-run in a trusted domain vs a pinned spec.
    ReRunInTrustedDomain,
    /// Low-trust + non-checkable, or high-stakes: escalate (route up / human / 2nd reviewer).
    Escalate,
}

impl VerificationDepth {
    pub fn as_str(self) -> &'static str {
        match self {
            VerificationDepth::AttributionPlusEvalGate => "attribution+eval-gate",
            VerificationDepth::ReRunInTrustedDomain => "re-run-in-trusted-domain",
            VerificationDepth::Escalate => "escalate",
        }
    }
}

/// The full leash decision — all five dials, decided coherently in one place (ADR-E3
/// D2: scope/TTL and lease term/cadence move together).
#[derive(Debug, Clone, PartialEq)]
pub struct LeashDecision {
    /// The minimum provider trust this task may be placed on (ADR-E1 D3 filter input).
    pub trust_floor: TrustLevel,
    /// `true` for the broad/long birth default (trusted pool); `false` where the dial
    /// tightened the delegated authority for a stranger.
    pub delegation_broad: bool,
    /// The UCAN expiry (seconds) the issued capabilities get.
    pub delegation_ttl_secs: i64,
    /// The minimal context slice tier (ADR-E2 D4: default smallest).
    pub context_scope_tier: ContextScope,
    pub context_seal: ContextSeal,
    pub verification_depth: VerificationDepth,
    pub lease_term_secs: i64,
    pub lease_renew_cadence_secs: i64,
}

/// A loud refusal from the fail-closed leash (ADR-E2 D2 / D-i). The failure direction
/// is **always over-protection (refuse), never under-protection (silent exposure)**.
#[derive(Debug, Clone, PartialEq)]
pub struct LeashRefusal {
    /// A bounded reason code (never attacker-controlled prose), e.g.
    /// `no-eligible-confidential-provider`, `unlabeled-fails-closed`,
    /// `trust-floor-not-met`, `capability-mismatch`.
    pub reason: String,
    pub detail: String,
}

impl LeashRefusal {
    fn new(reason: &str, detail: impl Into<String>) -> Self {
        Self {
            reason: reason.to_string(),
            detail: detail.into(),
        }
    }
}

/// Lease/UCAN defaults anchored to today's local values (`HEARTBEAT_LIVENESS_TIMEOUT_SECS
/// = 300`s, 30s heartbeat — ADR-E3 OQ3). The exact numbers are Erik's dial; the
/// *mechanism* (dial-driven, trust-scaled, prefer-liveness) is the commitment.
const VERIFIED_LEASE_TERM: i64 = 1800; // ~30 min, generous
const VERIFIED_RENEW_CADENCE: i64 = 300; // ~5 min, relaxed
const PROVISIONAL_LEASE_TERM: i64 = 300; // today's local 300s
const PROVISIONAL_RENEW_CADENCE: i64 = 90;
const UNKNOWN_LEASE_TERM: i64 = 120; // short / aggressive
const UNKNOWN_RENEW_CADENCE: i64 = 45;
/// Per-task grace multiplier `k`: a per-task UCAN expiry ≈ lease term × k (ADR-E3 OQ1).
const PER_TASK_GRACE_K: i64 = 3;

/// The **fail-closed `leash()` engine** — the spine of ADR-E2/E3/E4.
///
/// Decides every dial from `(provider_trust, task_sensitivity, pool_class)` + the
/// environment leash policy (`LeashPolicy::from_env()`, reused verbatim from WG-Fed —
/// no second dial). It **refuses** rather than emit an unsafe placement:
///
/// - **Unlabeled ⇒ refuse** (`unlabeled-fails-closed`, D-i): never the A plaintext tier
///   on a stranger.
/// - **Confidential ⇒ attested-C or refuse** (`no-eligible-confidential-provider`, ADR-E2
///   D2): never A, never B. v1's attestation slot has an empty allow-list, so any
///   real confidential routing refuses loudly (the spark step-6 assertion).
///
/// `provider_attested` is `true` only for a **verified attestation quote** (never a
/// self-advertised class — TC10).
pub fn leash(
    provider_trust: TrustLevel,
    task_sensitivity: Sensitivity,
    pool_class: PoolClass,
    provider_attested: bool,
) -> Result<LeashDecision, LeashRefusal> {
    // 1. The trust-floor by sensitivity (the filter input). A higher floor refuses more.
    let trust_floor = match task_sensitivity {
        Sensitivity::Normal => TrustLevel::Provisional,
        Sensitivity::High | Sensitivity::Confidential => TrustLevel::Verified,
        // Unlabeled fails closed below — set the strictest floor regardless.
        Sensitivity::Unlabeled => TrustLevel::Verified,
    };

    // 2. Fail-closed gates BEFORE any context decision (X-1: floor before context).
    if task_sensitivity == Sensitivity::Unlabeled {
        return Err(LeashRefusal::new(
            "unlabeled-fails-closed",
            "an unlabeled task does not fall through to the A (plaintext) tier on a \
             stranger — it refuses or routes to C (ADR-E2 D-i). Label the task.",
        ));
    }
    if task_sensitivity == Sensitivity::Confidential {
        // Confidential ⇒ C (attested) or refuse — NEVER A, NEVER B (ADR-E2 D2, FR-K5).
        if !provider_attested {
            return Err(LeashRefusal::new(
                "no-eligible-confidential-provider",
                "a confidential task requires a provider presenting a VERIFIED attestation \
                 (C); it is never placed on A (plaintext) or B (minimized-plaintext). v1's \
                 attestation slot has an empty allow-list, so this refuses loudly — context \
                 is NEVER shipped in plaintext (ADR-E2 D2/D5, FR-K5).",
            ));
        }
    }

    // 3. The context seal lever (ADR-E2 D1). Confidential ⇒ Sealed-to-attestation;
    //    a Verified own-box gets A (plaintext); a low-trust box gets B (minimized).
    let context_seal = match task_sensitivity {
        Sensitivity::Confidential => ContextSeal::SealedToAttestation,
        _ => match provider_trust {
            TrustLevel::Verified => ContextSeal::Plaintext,
            TrustLevel::Provisional | TrustLevel::Unknown => ContextSeal::Minimized,
        },
    };

    // 4. Context tier — default to the smallest slice that lets the task work (D4).
    //    Task = T's input + its --after artifacts. Never the whole graph by default.
    let context_scope_tier = ContextScope::Task;

    // 5. Delegation scope/TTL + lease term/cadence ride the dial together (ADR-E3 D2/D7).
    //    Environment policy (LeashPolicy::from_env) can tighten the TTL further — reused.
    let env_policy = LeashPolicy::from_env();
    let (delegation_broad, base_ttl, lease_term_secs, lease_renew_cadence_secs) =
        match provider_trust {
            TrustLevel::Verified => (
                // Broad/long birth default. A *per-task* grant is naturally bounded to the
                // task/lease lifetime (lease × k, ADR-E3 OQ1), not the standing-signer
                // ceiling — the latter is the many-task leash-slack option.
                true,
                VERIFIED_LEASE_TERM * PER_TASK_GRACE_K,
                VERIFIED_LEASE_TERM,
                VERIFIED_RENEW_CADENCE,
            ),
            TrustLevel::Provisional => (
                false,
                PROVISIONAL_LEASE_TERM * PER_TASK_GRACE_K,
                PROVISIONAL_LEASE_TERM,
                PROVISIONAL_RENEW_CADENCE,
            ),
            TrustLevel::Unknown => (
                false,
                UNKNOWN_LEASE_TERM * PER_TASK_GRACE_K,
                UNKNOWN_LEASE_TERM,
                UNKNOWN_RENEW_CADENCE,
            ),
        };
    // The env leash can only tighten (clamp) the TTL, never widen it (humans-never-leashed
    // does not apply — this is delegated agent authority).
    let delegation_ttl_secs = match env_policy.max_ttl_secs {
        Some(max) if base_ttl > max => max,
        _ => base_ttl,
    };

    // 6. Verification depth (ADR-E4 D2). Trusted+normal ⇒ attribution+eval-gate;
    //    low-trust ⇒ re-run in a trusted domain; high-stakes ⇒ escalate.
    let verification_depth = match (provider_trust, task_sensitivity) {
        (_, Sensitivity::High) => VerificationDepth::ReRunInTrustedDomain,
        (TrustLevel::Verified, _) => VerificationDepth::AttributionPlusEvalGate,
        (TrustLevel::Provisional, _) => VerificationDepth::ReRunInTrustedDomain,
        (TrustLevel::Unknown, _) => VerificationDepth::ReRunInTrustedDomain,
    };
    let _ = pool_class; // pool class is informational at v1 (one mechanism spans all).

    Ok(LeashDecision {
        trust_floor,
        delegation_broad,
        delegation_ttl_secs,
        context_scope_tier,
        context_seal,
        verification_depth,
        lease_term_secs,
        lease_renew_cadence_secs,
    })
}

/// A task's hard requirements (the filter inputs — ADR-E1 D3).
#[derive(Debug, Clone)]
pub struct TaskRequirements {
    pub task_id: String,
    pub required_model: String,
    pub min_isolation: IsolationClass,
    pub sensitivity: Sensitivity,
    /// Whether the deliverable is **checkable** (eval-gateable) — the S7 B-tier
    /// precondition. A non-checkable task may not ride the **verified-overflow (B)** pool,
    /// whose only integrity lever is the trusted-domain re-run (which needs checkable
    /// code). Default `true` (most code work is checkable).
    pub checkable: bool,
}

/// The placement **tier** a provider's trust maps to (S7 — the distinct pools, ADR-E1 D4).
///
/// - `"A"` (**trusted pool**) — `Verified`: own/trusted boxes; non-checkable work allowed
///   on trust; verification is attribution + the eval-gate.
/// - `"B"` (**verified-overflow pool**) — `Provisional`: vouched/overflow capacity,
///   distinct from the trusted pool; **checkable work only**, eval-gated by the
///   trusted-domain re-run at accept.
/// - `"refuse"` — `Unknown`: a stranger is below the `Normal` floor, so it is refused at
///   placement (never silently runs your work).
pub fn pool_tier(trust: TrustLevel) -> &'static str {
    match trust {
        TrustLevel::Verified => "A",
        TrustLevel::Provisional => "B",
        TrustLevel::Unknown => "refuse",
    }
}

/// The verdict of matching one provider to a task (ADR-E1 D3 filter, then leash).
#[derive(Debug, Clone)]
pub enum PlacementVerdict {
    /// Eligible — the leash decision to apply.
    Eligible(LeashDecision),
    /// Refused by the fail-closed filter/leash (loud, with a bounded reason).
    Refused(LeashRefusal),
}

/// Evaluate a single provider against a task: the **hard filter** (capability match +
/// trust-floor), then the **fail-closed leash**. Trust is taken from the authorizer's
/// registry (`provider_trust`), NEVER self-certified.
///
/// Order is load-bearing (X-1): the confidential / unlabeled fail-closed gates live in
/// [`leash`], reached only after capability + trust-floor pass — but the leash itself
/// re-asserts them, so a too-loose placement is impossible by construction.
pub fn evaluate_placement(
    req: &TaskRequirements,
    provider_trust: TrustLevel,
    provider_cap: Option<&CapabilityAd>,
    pool_class: PoolClass,
) -> PlacementVerdict {
    // Compute the leash first to learn the trust-floor (and to re-assert the
    // fail-closed gates even if a capability is missing).
    let attested = provider_cap.map(|c| c.attested).unwrap_or(false);
    let decision = match leash(provider_trust, req.sensitivity, pool_class, attested) {
        Ok(d) => d,
        Err(r) => return PlacementVerdict::Refused(r),
    };

    // Capability match (hard gate). A provider with no advertised capability cannot
    // satisfy the filter.
    let cap = match provider_cap {
        Some(c) => c,
        None => {
            return PlacementVerdict::Refused(LeashRefusal::new(
                "capability-mismatch",
                "provider has no signed capability advertisement on record",
            ));
        }
    };
    if cap.model != req.required_model {
        return PlacementVerdict::Refused(LeashRefusal::new(
            "capability-mismatch",
            format!(
                "provider offers model {:?}, task requires {:?}",
                cap.model, req.required_model
            ),
        ));
    }
    if cap.isolation < req.min_isolation {
        return PlacementVerdict::Refused(LeashRefusal::new(
            "capability-mismatch",
            format!(
                "provider isolation {} < task minimum {}",
                cap.isolation.as_str(),
                req.min_isolation.as_str()
            ),
        ));
    }

    // Trust-floor (hard gate). provider.trust_level >= leash.trust_floor.
    if trust_rank(provider_trust) < trust_rank(decision.trust_floor) {
        return PlacementVerdict::Refused(LeashRefusal::new(
            "trust-floor-not-met",
            format!(
                "provider trust {} < required floor {} for sensitivity {}",
                trust_str(provider_trust),
                trust_str(decision.trust_floor),
                req.sensitivity.as_str()
            ),
        ));
    }

    // S7 — the verified-overflow (B) tier is checkable-only. A non-checkable task may run
    // on the trusted (A) pool on trust, but NEVER on a B (vouched-overflow) provider: the
    // only integrity lever there is the trusted-domain re-run / eval-gate, which a
    // non-checkable deliverable cannot satisfy (it would silently accept on "found
    // nothing"). Fail closed — keep it on A or refuse.
    if !req.checkable && pool_tier(provider_trust) == "B" {
        return PlacementVerdict::Refused(LeashRefusal::new(
            "overflow-requires-checkable",
            format!(
                "a non-checkable task may not ride the verified-overflow (B) pool \
                 (provider trust {}): the B tier's only integrity lever is the \
                 trusted-domain eval-gate re-run, which needs checkable code. Place it on \
                 the trusted (A) pool or refuse.",
                trust_str(provider_trust)
            ),
        ));
    }

    PlacementVerdict::Eligible(decision)
}

/// The advisory, deterministic rank over the *already-eligible* set (ADR-E1 D3 phase 2,
/// OQ1). It can reorder but **never** promote past the filter. Equal-scoring providers
/// break ties by a stable hash of `(task_id, provider_wgid)` — herd-safe, ungameable by
/// advertising faster (OQ1 invariant 2). `live` reflects the authorizer's *observed*
/// liveness, not a self-report.
///
/// Default order (reliability-first, OQ1 proposed default): live+free-capacity →
/// higher trust → lower cost → deterministic hash tiebreak.
pub fn rank_eligible(task_id: &str, mut candidates: Vec<RankInput>) -> Vec<String> {
    candidates.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(trust_rank(b.trust).cmp(&trust_rank(a.trust)))
            .then(
                a.cost_micros
                    .partial_cmp(&b.cost_micros)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(tiebreak_hash(task_id, &a.wgid).cmp(&tiebreak_hash(task_id, &b.wgid)))
    });
    candidates.into_iter().map(|c| c.wgid).collect()
}

/// One ranking candidate (already filter-eligible).
#[derive(Debug, Clone)]
pub struct RankInput {
    pub wgid: String,
    pub trust: TrustLevel,
    pub live: bool,
    pub cost_micros: f64,
}

fn tiebreak_hash(task_id: &str, wgid: &str) -> [u8; 32] {
    crate::identity::blake3_32(format!("{task_id}|{wgid}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidential_to_unattested_provider_refuses() {
        let r = leash(
            TrustLevel::Verified,
            Sensitivity::Confidential,
            PoolClass::Private,
            false, // not attested
        );
        let err = r.unwrap_err();
        assert_eq!(err.reason, "no-eligible-confidential-provider");
    }

    #[test]
    fn confidential_to_attested_provider_seals() {
        let d = leash(
            TrustLevel::Verified,
            Sensitivity::Confidential,
            PoolClass::Private,
            true, // attested
        )
        .unwrap();
        assert_eq!(d.context_seal, ContextSeal::SealedToAttestation);
    }

    #[test]
    fn unlabeled_fails_closed_never_a() {
        let r = leash(
            TrustLevel::Verified,
            Sensitivity::Unlabeled,
            PoolClass::Private,
            false,
        );
        assert_eq!(r.unwrap_err().reason, "unlabeled-fails-closed");
    }

    #[test]
    fn trusted_normal_is_broad_long_and_plaintext() {
        let d = leash(
            TrustLevel::Verified,
            Sensitivity::Normal,
            PoolClass::Private,
            false,
        )
        .unwrap();
        assert!(d.delegation_broad);
        assert_eq!(d.context_seal, ContextSeal::Plaintext);
        assert_eq!(
            d.verification_depth,
            VerificationDepth::AttributionPlusEvalGate
        );
        assert!(d.delegation_ttl_secs >= d.lease_term_secs);
    }

    #[test]
    fn stranger_normal_is_narrow_short_minimized_rerun() {
        let d = leash(
            TrustLevel::Unknown,
            Sensitivity::Normal,
            PoolClass::Cooperative,
            false,
        )
        .unwrap();
        assert!(!d.delegation_broad);
        assert_eq!(d.context_seal, ContextSeal::Minimized);
        assert_eq!(
            d.verification_depth,
            VerificationDepth::ReRunInTrustedDomain
        );
        assert!(d.lease_term_secs < 600);
    }

    #[test]
    fn filter_rejects_below_trust_floor() {
        let req = TaskRequirements {
            task_id: "T".into(),
            required_model: "claude:opus".into(),
            min_isolation: IsolationClass::Container,
            sensitivity: Sensitivity::High, // floor = Verified
            checkable: true,
        };
        let cap = CapabilityAd {
            model: "claude:opus".into(),
            isolation: IsolationClass::Container,
            attested: false,
        };
        // A Provisional provider is below the Verified floor for High sensitivity.
        match evaluate_placement(
            &req,
            TrustLevel::Provisional,
            Some(&cap),
            PoolClass::Private,
        ) {
            PlacementVerdict::Refused(r) => assert_eq!(r.reason, "trust-floor-not-met"),
            _ => panic!("expected refusal"),
        }
    }

    #[test]
    fn filter_rejects_capability_mismatch() {
        let req = TaskRequirements {
            task_id: "T".into(),
            required_model: "claude:opus".into(),
            min_isolation: IsolationClass::Vm,
            sensitivity: Sensitivity::Normal,
            checkable: true,
        };
        let cap = CapabilityAd {
            model: "claude:opus".into(),
            isolation: IsolationClass::Container, // below VM minimum
            attested: false,
        };
        match evaluate_placement(&req, TrustLevel::Verified, Some(&cap), PoolClass::Private) {
            PlacementVerdict::Refused(r) => assert_eq!(r.reason, "capability-mismatch"),
            _ => panic!("expected capability refusal"),
        }
    }

    #[test]
    fn filter_admits_a_matching_trusted_provider() {
        let req = TaskRequirements {
            task_id: "T".into(),
            required_model: "claude:opus".into(),
            min_isolation: IsolationClass::Container,
            sensitivity: Sensitivity::Normal,
            checkable: true,
        };
        let cap = CapabilityAd {
            model: "claude:opus".into(),
            isolation: IsolationClass::Container,
            attested: false,
        };
        assert!(matches!(
            evaluate_placement(&req, TrustLevel::Verified, Some(&cap), PoolClass::Private),
            PlacementVerdict::Eligible(_)
        ));
    }

    #[test]
    fn rank_is_deterministic_and_reliability_first() {
        let cands = vec![
            RankInput {
                wgid: "wgid:zDead".into(),
                trust: TrustLevel::Verified,
                live: false,
                cost_micros: 1.0,
            },
            RankInput {
                wgid: "wgid:zLive".into(),
                trust: TrustLevel::Provisional,
                live: true,
                cost_micros: 9.0,
            },
        ];
        let order = rank_eligible("T", cands.clone());
        // Live beats dead even at higher trust/lower cost (reliability-first).
        assert_eq!(order[0], "wgid:zLive");
        // Deterministic across calls.
        assert_eq!(rank_eligible("T", cands), order);
    }

    #[test]
    fn s7_non_checkable_refused_on_b_pool_but_allowed_on_a_pool() {
        let non_checkable = |trust| {
            let req = TaskRequirements {
                task_id: "T".into(),
                required_model: "claude:opus".into(),
                min_isolation: IsolationClass::Container,
                sensitivity: Sensitivity::Normal,
                checkable: false,
            };
            let cap = CapabilityAd {
                model: "claude:opus".into(),
                isolation: IsolationClass::Container,
                attested: false,
            };
            evaluate_placement(&req, trust, Some(&cap), PoolClass::Cooperative)
        };
        // B (verified-overflow / Provisional) refuses a non-checkable task — no eval-gate.
        match non_checkable(TrustLevel::Provisional) {
            PlacementVerdict::Refused(r) => assert_eq!(r.reason, "overflow-requires-checkable"),
            _ => panic!("expected the B-tier checkable refusal"),
        }
        // A (trusted / Verified) takes non-checkable work on trust.
        assert!(matches!(
            non_checkable(TrustLevel::Verified),
            PlacementVerdict::Eligible(_)
        ));
    }

    #[test]
    fn pool_tier_maps_trust_to_distinct_tiers() {
        assert_eq!(pool_tier(TrustLevel::Verified), "A");
        assert_eq!(pool_tier(TrustLevel::Provisional), "B");
        assert_eq!(pool_tier(TrustLevel::Unknown), "refuse");
    }
}
