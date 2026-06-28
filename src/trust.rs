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
//!    message is a peer. **This is the author dial** — the only home that can *vouch for
//!    authorship*.
//! 2. **The WG-Exec provider pool** (`exec/registry.json`, `wg provider enroll
//!    --trust`) — the SAME map the placement leash reads, loaded through the one
//!    canonical [`ProviderRegistry::load`]. **This is the provider dial** — it asserts
//!    that a *box* is trustworthy to *run compute*, which is a different question from
//!    whether content the box authors is safe to consume.
//!
//! ## M18 — the two dials are SPLIT, folded fail-closed (min), never most-trusting
//!
//! These two assertions are about **different things** and must not be conflated. The
//! original resolver took the **most-trusting** of the two opinions, so enrolling a box
//! as a `Verified` *provider* auto-granted it `Verified` *author* trust — and that
//! cleared the deep author review (audit M18). [`resolve_author_trust`] now keeps them
//! split:
//!
//! - The **author trust** is sourced from the peer registry and **fails closed to
//!   [`TrustLevel::Unknown`]** when no peer entry vouches (a bare `wg peer add` with no
//!   `--trust` is *Unknown*, not Provisional — it records "I've heard of this peer," not
//!   "I vouch for it").
//! - The **provider dial folds in fail-closed (min): it can only *lower*, never *raise*
//!   the review-depth input.** A box's execution trust cannot upgrade its authorship, so
//!   a `Verified`-provider/unknown-peer identity resolves to `Unknown` and takes the deep
//!   path; a low provider trust on a high peer only tightens review. The exec *leash*
//!   still reads the provider dial directly ([`ProviderRegistry`]), so the split is real:
//!   the leash sees provider trust, the review gate sees author trust.
//!
//! The per-author *revoke* demotion (the review gate's `trust_overrides`, the exec
//! pool's `lower_trust`) is applied **on top** of this baseline by the consuming caller
//! as a strictest-wins fold, so a revoked author's next item still takes the deep path.

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

/// The federation peer registry's trust opinion about an author `wgid` (home 1, the
/// **author dial**). `Some(level)` iff a peer entry carries this `wgid:`; an
/// enrolled-but-unvouched peer (a bare `wg peer add` with no `--trust`) resolves to
/// **[`TrustLevel::Unknown`]** — fail-closed: a peer entry records "I've heard of this
/// identity," NOT "I vouch for it" (M18: bare peer-add must not silently clear the
/// review floor via a TOFU `Provisional`). `None` means "not a peer at all".
pub fn peer_trust_opinion(workgraph_dir: &Path, wgid: &str) -> Option<TrustLevel> {
    let canon = normalize(wgid)?;
    let cfg = load_federation_config(workgraph_dir).ok()?;
    cfg.peers.values().find_map(|peer| {
        let pw = peer.wgid.as_deref().and_then(normalize)?;
        (pw == canon).then(|| peer.trust.unwrap_or(TrustLevel::Unknown))
    })
}

/// The WG-Exec provider pool's trust opinion about a `wgid` (home 2) — the SAME
/// `exec/registry.json` the placement leash reads. `None` iff not enrolled.
pub fn provider_trust_opinion(workgraph_dir: &Path, wgid: &str) -> Option<TrustLevel> {
    let canon = normalize(wgid).unwrap_or_else(|| wgid.to_string());
    ProviderRegistry::load(workgraph_dir).opinion_of(&canon)
}

/// Resolve the canonical **author**-trust for a `wgid:` — the input the inbound review
/// gate reads for depth (M18: SPLIT from provider trust, folded fail-closed).
///
/// The **author dial** (the peer registry) is the *source*: it is the only home that can
/// vouch for authorship, and it **fails closed to [`TrustLevel::Unknown`]** when no peer
/// entry vouches. The **provider dial** (the exec pool) folds in **min (strictest-wins)
/// — it can only *lower*, never *raise*** the result: enrolling a box as a `Verified`
/// provider must NOT auto-clear its author review (the M18 conflation bug), so a
/// provider-only identity resolves to `Unknown` and takes the deep path. The exec leash
/// reads the provider dial directly via [`ProviderRegistry`], so the two planes stay
/// genuinely split — same persisted assertions, two distinct questions.
pub fn resolve_author_trust(workgraph_dir: &Path, wgid: &str) -> TrustLevel {
    // The author dial is the source; absence is fail-closed Unknown.
    let author = peer_trust_opinion(workgraph_dir, wgid).unwrap_or(TrustLevel::Unknown);
    // The provider dial folds in min — it can only tighten, never loosen.
    match provider_trust_opinion(workgraph_dir, wgid) {
        Some(provider) => strictest_trust(author, provider),
        None => author,
    }
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
    fn bare_peer_add_is_unknown_fail_closed() {
        // M18: a bare `wg peer add` (no `--trust`) records the peer with no vouch — it
        // must resolve to Unknown (deep review), NOT a TOFU Provisional.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        add_peer(dir, "newpeer", &w, None);
        assert_eq!(peer_trust_opinion(dir, &w), Some(TrustLevel::Unknown));
        assert_eq!(resolve_author_trust(dir, &w), TrustLevel::Unknown);
    }

    #[test]
    fn verified_provider_does_not_clear_author_review() {
        // M18, the central split: a box enrolled as a Verified *provider* (no peer
        // vouch) is trusted to RUN compute (the leash reads Verified) but is NOT thereby
        // a trusted *author* — author trust stays Unknown so its content takes the deep
        // review path. The provider dial can never raise author trust.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        enroll_provider(dir, &w, TrustLevel::Verified);
        // The provider/leash dial DOES see Verified — that half is unchanged.
        assert_eq!(provider_trust_opinion(dir, &w), Some(TrustLevel::Verified));
        assert_eq!(
            ProviderRegistry::load(dir).trust_of(&w),
            TrustLevel::Verified,
            "the exec leash still reads provider trust directly"
        );
        // But the AUTHOR dial is Unknown — the conflation is gone.
        assert_eq!(
            resolve_author_trust(dir, &w),
            TrustLevel::Unknown,
            "a Verified provider must NOT auto-clear author review (M18)"
        );
    }

    #[test]
    fn provider_dial_folds_in_min_can_only_lower() {
        // The fail-closed min-fold: a Verified peer who is ALSO a Provisional provider
        // resolves to the STRICTER (Provisional) — the provider dial tightens, never
        // loosens. And the reverse (peer absent) cannot be raised by the provider.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        add_peer(dir, "p", &w, Some(TrustLevel::Verified));
        enroll_provider(dir, &w, TrustLevel::Provisional);
        assert_eq!(
            resolve_author_trust(dir, &w),
            TrustLevel::Provisional,
            "min-fold: the lower provider trust tightens the Verified peer"
        );
    }

    #[test]
    fn verified_peer_alone_stays_verified() {
        // The legit light-path case must survive the split: a Verified peer with no
        // provider entry resolves to Verified (no spurious lowering).
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let w = a_wgid();
        add_peer(dir, "sara", &w, Some(TrustLevel::Verified));
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
