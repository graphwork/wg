//! Canonical author-trust resolution — the ONE trust dial.
//!
//! Surfaced by the `e2e_family_team` integration as the seam the isolated sparks
//! could not: the composed flow had to *hand-pass* `wg review check --trust <level>`,
//! mapping known-peer⇒verified / stranger⇒unknown by hand in the scenario script. In
//! production that trust input must be **derived** from the same persisted assertions
//! the WG-Exec provider pool already reads, so the inbound review gate's *depth* and
//! the exec *leash* read **one** dial (CLAUDE.md "auto-wire the four ingest seams";
//! ADR-CS1 D3 — review depth is an output face of the one trust dial, not a new one).
//!
//! ## What "one dial" means here
//!
//! Trust is a [`TrustLevel`] (`graph::TrustLevel`) and is **always the authorizer's
//! local assertion**, never self-certified by the subject (the invariant the WG-Exec
//! [`ProviderRegistry`](crate::providers::ProviderRegistry) and the WG-Fed sigchain
//! both enforce — a `wgid:` proves *who*, never *how-trusted*). There are exactly two
//! homes for such an assertion, and this resolver folds both:
//!
//! 1. **The federation peer registry** (`federation.yaml` peers, `wg peer add
//!    --trust`) — the correspondent/sender's trust. The inbound author of a cross-graph
//!    message is a peer.
//! 2. **The WG-Exec provider pool** (`exec/registry.json`, `wg provider enroll
//!    --trust`) — the SAME map the placement leash reads, loaded through the one
//!    canonical [`ProviderRegistry::load`].
//!
//! An author may carry an opinion in either home (or neither). [`resolve_author_trust`]
//! returns the **most-trusting present opinion** — both are positive "I vouch for this
//! identity at level X" assertions, and absence is *no opinion*, not distrust — and
//! **fails closed to [`TrustLevel::Unknown`]** when neither home vouches. The
//! per-author *revoke* demotion (the review gate's `trust_overrides`, the exec pool's
//! `lower_trust`) is applied **on top** of this baseline by the consuming caller as a
//! strictest-wins fold, so a revoked author's next item still takes the deep path.

use std::path::Path;

use crate::federation::load_federation_config;
use crate::graph::TrustLevel;
use crate::providers::ProviderRegistry;

/// A monotone rank for trust comparison: `Verified(2) > Provisional(1) > Unknown(0)`.
/// Mirrors `providers::trust_rank` (the leash's floor compare) so both planes order
/// trust identically.
pub fn trust_rank(t: TrustLevel) -> u8 {
    match t {
        TrustLevel::Unknown => 0,
        TrustLevel::Provisional => 1,
        TrustLevel::Verified => 2,
    }
}

/// The **more-trusting** of two opinions (`None` = no opinion). Used to fold a peer
/// vouch against an exec-pool vouch about the same identity.
fn most_trusting(a: Option<TrustLevel>, b: Option<TrustLevel>) -> Option<TrustLevel> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if trust_rank(x) >= trust_rank(y) { x } else { y }),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// The **less-trusting** (strictest) of two trust levels. The caller folds a
/// revoke-lowered override against the resolved baseline with this, so a demotion can
/// only tighten, never loosen (ADR-CS3 D4).
pub fn strictest_trust(a: TrustLevel, b: TrustLevel) -> TrustLevel {
    if trust_rank(a) <= trust_rank(b) { a } else { b }
}

/// Normalize a `wgid:`/`did:key:` spelling to the canonical `wgid:` the registries key
/// by; `None` if `s` is not a key address.
fn normalize(s: &str) -> Option<String> {
    crate::identity::keys::pubkey_from_wgid(s)
        .ok()
        .map(|pk| crate::identity::keys::wgid_from_pubkey(&pk))
}

/// The federation peer registry's trust opinion about an author `wgid` (home 1).
/// `Some(level)` iff a peer entry carries this `wgid:`; an enrolled-but-unvouched peer
/// resolves to `Provisional` (TOFU). `None` means "not a peer".
pub fn peer_trust_opinion(workgraph_dir: &Path, wgid: &str) -> Option<TrustLevel> {
    let canon = normalize(wgid)?;
    let cfg = load_federation_config(workgraph_dir).ok()?;
    cfg.peers.values().find_map(|peer| {
        let pw = peer.wgid.as_deref().and_then(normalize)?;
        (pw == canon).then(|| peer.trust.unwrap_or(TrustLevel::Provisional))
    })
}

/// The WG-Exec provider pool's trust opinion about a `wgid` (home 2) — the SAME
/// `exec/registry.json` the placement leash reads. `None` iff not enrolled.
pub fn provider_trust_opinion(workgraph_dir: &Path, wgid: &str) -> Option<TrustLevel> {
    let canon = normalize(wgid).unwrap_or_else(|| wgid.to_string());
    ProviderRegistry::load(workgraph_dir).opinion_of(&canon)
}

/// Resolve the canonical author-trust for a `wgid:` — the most-trusting present opinion
/// across the federation peer registry and the WG-Exec provider pool, **fail-closed to
/// [`TrustLevel::Unknown`]** when neither vouches. This is the single source the
/// inbound review gate reads for depth; it is the same data the exec leash reads, so
/// the two planes share one dial.
pub fn resolve_author_trust(workgraph_dir: &Path, wgid: &str) -> TrustLevel {
    most_trusting(
        peer_trust_opinion(workgraph_dir, wgid),
        provider_trust_opinion(workgraph_dir, wgid),
    )
    .unwrap_or(TrustLevel::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::{FederationConfig, PeerConfig, save_federation_config};
    use crate::providers::ProviderRegistry;
    use tempfile::TempDir;

    // A well-formed wgid for tests (gen a keypair, derive the address).
    fn a_wgid() -> String {
        let kp = crate::identity::keys::gen_ed25519().unwrap();
        crate::identity::keys::wgid_from_pubkey(&kp.public)
    }

    fn enroll_provider(dir: &Path, wgid: &str, trust: TrustLevel) {
        let mut reg = ProviderRegistry::load(dir);
        reg.enroll(wgid, trust, None);
        std::fs::create_dir_all(dir.join("exec")).unwrap();
        std::fs::write(
            crate::providers::registry_path(dir),
            serde_json::to_string_pretty(&reg).unwrap(),
        )
        .unwrap();
    }

    fn add_peer(dir: &Path, name: &str, wgid: &str, trust: Option<TrustLevel>) {
        let mut cfg = load_federation_config(dir).unwrap_or(FederationConfig::default());
        cfg.peers.insert(
            name.to_string(),
            PeerConfig {
                wgid: Some(wgid.to_string()),
                trust,
                ..PeerConfig::default()
            },
        );
        save_federation_config(dir, &cfg).unwrap();
    }

    #[test]
    fn stranger_fails_closed_to_unknown() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        assert_eq!(resolve_author_trust(dir, &a_wgid()), TrustLevel::Unknown);
    }

    #[test]
    fn known_peer_resolves_from_peer_registry_no_flag() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        add_peer(dir, "sara", &w, Some(TrustLevel::Verified));
        assert_eq!(resolve_author_trust(dir, &w), TrustLevel::Verified);
    }

    #[test]
    fn enrolled_but_unvouched_peer_is_provisional_tofu() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        add_peer(dir, "newpeer", &w, None);
        assert_eq!(resolve_author_trust(dir, &w), TrustLevel::Provisional);
    }

    #[test]
    fn exec_pool_trust_is_visible_to_the_canonical_resolver() {
        // The unification: a provider enrolled ONLY in the exec pool is seen by the
        // author-trust resolver at the SAME level the leash reads.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        enroll_provider(dir, &w, TrustLevel::Verified);
        assert_eq!(
            provider_trust_opinion(dir, &w),
            Some(TrustLevel::Verified),
            "exec pool opinion must be readable"
        );
        assert_eq!(
            resolve_author_trust(dir, &w),
            TrustLevel::Verified,
            "canonical resolver must read the same exec-pool dial"
        );
        // And it equals what the leash itself reads (`trust_of`) — one dial.
        assert_eq!(
            ProviderRegistry::load(dir).trust_of(&w),
            TrustLevel::Verified
        );
    }

    #[test]
    fn most_trusting_vouch_wins_across_both_homes() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        // Peer vouches Verified; exec pool only Provisional → the higher vouch wins.
        add_peer(dir, "p", &w, Some(TrustLevel::Verified));
        enroll_provider(dir, &w, TrustLevel::Provisional);
        assert_eq!(resolve_author_trust(dir, &w), TrustLevel::Verified);
    }

    #[test]
    fn strictest_fold_only_tightens() {
        assert_eq!(
            strictest_trust(TrustLevel::Verified, TrustLevel::Unknown),
            TrustLevel::Unknown
        );
        assert_eq!(
            strictest_trust(TrustLevel::Provisional, TrustLevel::Verified),
            TrustLevel::Provisional
        );
    }
}
