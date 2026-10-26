//! WG-Fed identity & cryptography (Wave 3 spark PoC).
//!
//! This module is the home for **all** signing/sealing cryptography in WG —
//! the tree carried none before federation (only an unrelated VAPID push key;
//! `docs/federation-study/02-current-state-baseline.md` §2.4). It implements the
//! thinnest slice of the model fixed by the accepted federation ADRs:
//!
//! - **ADR-fed-001** — identity is a self-certifying `wgid:<multibase-ed25519-pubkey>`
//!   backed by an append-only, hash-linked, signed **sigchain** over a three-tier
//!   key hierarchy (root / signer / encryption). The address is the genesis root
//!   pubkey, stable under rotation; verification is *never* central.
//! - **ADR-fed-002** — the wire unit is a `SignedEvent`, carried over an untrusted
//!   store-and-forward transport (the spark uses the simplest rung: a dumb object
//!   store + inbox; see `commands::identity_cmd`).
//! - **ADR-fed-003** — the portable identity **excludes the root private key,
//!   always**; the root lives behind an ssh-agent-style "sign this digest" custody
//!   boundary over `wg secret` and is never returned to a caller. "Download ≠
//!   impersonation" (the spark's headline assertion) falls directly out of this.
//! - **ADR-fed-004** — `StateSnapshot` is a tagged, evolvable, signed loadable-state
//!   envelope (the spark carries one `conv-cache-v1` snapshot).
//!
//! Submodules:
//! - [`keys`] — ed25519/X25519 generation + the custody boundary over `wg secret`.
//! - [`sigchain`] — `genesis` / `add_key` / `revoke_key` / `rotate_root` links +
//!   layered recovery (offline recovery key + M-of-N guardians) + [`sigchain::verify`].
//! - [`envelope`] — `IdentityRecord` / `StateSnapshot` / `SignedEvent`: sign /
//!   verify / canonical-encode + BLAKE3 content-addressing + **per-recipient sealing**
//!   (Wave 6: the `to` set IS the ACL — a hybrid CEK envelope wraps the body key to
//!   each recipient, plus an optional sealed-sender mode that hides `from` from relays).
//! - [`custody`] — Wave 6 UCAN-style capability delegation (ADR-fed-003 §D3):
//!   issue / verify / revoke short-lived, scoped, **attenuating-only** capabilities,
//!   plus the [`custody::LeashPolicy`] authority dial (broad/long by birth,
//!   environment-tightenable — §D2's trust-default amendment).
//! - [`state_safety`] — the S-5 load pipeline (ADR-fed-004 §D6): loaded
//!   `StateSnapshot`s are **untrusted input**, provenance-gated by `trust_level`,
//!   `model_binding`-enforced, with human-in-loop for cross-trust loads.
//!
//! The crypto-free canonical-encoding / content-addressing helpers this wire is
//! built on (`canonical_json`, `blake3_32`, `content_cid`, `signing_digest`) now
//! live in [`crate::content_addr`] — they are shared with the always-built
//! completion-evidence plane and carry no identity semantics.

pub mod custody;
pub mod dedup;
pub mod envelope;
pub mod equivocation;
pub mod freshness;
pub mod keys;
pub mod node;
pub mod sigchain;
pub mod state_safety;
pub mod transport;

use anyhow::{Result, bail};

/// WG-Fed wire/format compatibility version.
///
/// Mirrors `WG_AGENCY_COMPAT_VERSION` (`src/agency/mod.rs`) and
/// `WG_PI_PLUGIN_COMPAT_VERSION` (`src/pi_plugin/mod.rs`): a single source of
/// truth peers exchange on first contact, **failing loud** on an incompatible
/// mismatch (ADR-fed-001 §D7, doc 04 §1.5). Per finding S-7 the negotiated
/// parameters are eventually *signed*, not merely exchanged; the spark fixes the
/// constant + the loud-fail rule, and transport-side authentication of the
/// handshake hardens in Wave 4.
///
/// `0.2.0 → 0.3.0` (Wave 6): the wire gained per-recipient sealed envelopes
/// (`SignedEvent.enc_multi`) + sealed-sender (an anonymized outer `from`) and the
/// off-chain UCAN [`custody::Capability`]. A 0.2.x peer cannot verify a multi-sealed
/// or sealed-sender event (it drops the unknown field → the signature no longer
/// matches → it rejects), so it must fail the handshake loudly rather than mis-handle
/// the new wire (S-7).
///
/// `0.3.0 → 0.4.0` (fed-harden): the federation-hardening pass changed several signed
/// wire forms — the sealed-sender inner now carries a signed `outer_commitment` (M8),
/// the sigchain gained a `SetRecovery` link type + a `recovery_at` field + a recovery
/// window on `RecoverySlot` (M11), and a new signed `RevocationHead` rides the store
/// (M10). All new fields are `#[serde(default)]` (forward-readable), but a 0.3.x peer
/// cannot *produce or verify* them, so it must fail the handshake loudly (S-7).
pub const WG_FED_COMPAT_VERSION: &str = "0.4.0";

/// The only signing/identity algorithm the spark implements.
///
/// Every signed structure carries this `alg` id explicitly (crypto agility,
/// ADR-fed-001 §D7): a future primitive (e.g. ed25519 → ML-DSA) is added as a
/// new method + multicodec prefix without abandoning any identity.
pub const ALG_ED25519: &str = "ed25519";

/// Current wire envelope version emitted by this build (doc 04 §1.4).
pub const ENVELOPE_V: u16 = 1;

/// Parse a `major.minor.patch` compat string into its numeric parts.
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Assert a peer's advertised `WG_FED_COMPAT_VERSION` is compatible with ours.
///
/// Compatibility rule (WG's existing convention, mirroring the agency/pi-plugin
/// handshakes): the **major** version must match exactly. A mismatch is a **loud,
/// hard error** naming expected-vs-found — never a silent downgrade (S-7). During
/// `0.x` the minor is also pinned, since pre-1.0 wire formats are not yet stable.
pub fn check_compat(peer_version: &str) -> Result<()> {
    let ours = parse_semver(WG_FED_COMPAT_VERSION)
        .expect("WG_FED_COMPAT_VERSION is a valid semver constant");
    let theirs = match parse_semver(peer_version) {
        Some(v) => v,
        None => bail!(
            "WG-Fed compat handshake FAILED: peer advertised an unparseable \
             version {peer_version:?}; this build speaks {WG_FED_COMPAT_VERSION}"
        ),
    };
    let compatible = if ours.0 == 0 || theirs.0 == 0 {
        // Pre-1.0: wire format not yet frozen — pin major AND minor.
        ours.0 == theirs.0 && ours.1 == theirs.1
    } else {
        ours.0 == theirs.0
    };
    if !compatible {
        bail!(
            "WG-Fed compat handshake FAILED (loud, per ADR-fed-001 §D7): peer \
             speaks WG_FED_COMPAT_VERSION={peer_version}, this build speaks \
             {WG_FED_COMPAT_VERSION}. Refusing to silently downgrade."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_accepts_same_version() {
        assert!(check_compat(WG_FED_COMPAT_VERSION).is_ok());
    }

    #[test]
    fn compat_accepts_patch_bump() {
        // Same major.minor, different patch is fine even pre-1.0.
        assert!(check_compat("0.4.99").is_ok());
    }

    #[test]
    fn compat_rejects_minor_bump_pre_1_0() {
        // Pre-1.0 the wire format is not frozen, so a minor bump is incompatible.
        let err = check_compat("0.5.0").unwrap_err().to_string();
        assert!(err.contains("FAILED"), "{err}");
        assert!(err.contains("0.5.0"), "{err}");
        assert!(err.contains(WG_FED_COMPAT_VERSION), "{err}");
        // Every prior wave's wire (0.1.0 / 0.2.0 / 0.3.0) is incompatible: Wave 5 changed
        // rotate_root verification, Wave 6 added the sealed-multi / sealed-sender / UCAN
        // wire, and fed-harden changed the sealed-sender inner + sigchain + revocation
        // wire — an older peer must fail loud, not silently mis-handle it.
        assert!(check_compat("0.1.0").is_err());
        assert!(check_compat("0.2.0").is_err());
        assert!(check_compat("0.3.0").is_err());
    }

    #[test]
    fn compat_rejects_garbage() {
        assert!(check_compat("not-a-version").is_err());
        assert!(check_compat("1").is_err());
    }
}
