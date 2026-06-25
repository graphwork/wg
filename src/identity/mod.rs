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
//!   verify / canonical-encode + BLAKE3 content-addressing + optional sealing.
//! - [`state_safety`] — the S-5 load pipeline (ADR-fed-004 §D6): loaded
//!   `StateSnapshot`s are **untrusted input**, provenance-gated by `trust_level`,
//!   `model_binding`-enforced, with human-in-loop for cross-trust loads.

pub mod envelope;
pub mod freshness;
pub mod keys;
pub mod node;
pub mod sigchain;
pub mod state_safety;
pub mod transport;

use anyhow::{Result, bail};
use serde_json::Value;

// ── Canonical encoding + content-addressing ────────────────────────────────────
//
// All three wire envelopes (and every sigchain link) are content-addressed by
// BLAKE3 of their **canonical** (recursively sorted-key) JSON serialization
// (doc 04 §1.4). Canonicalization is done over `serde_json::Value` so it does not
// depend on serde_json's map ordering: keys are emitted in sorted order at every
// level. The signature covers the canonical bytes with the `sig` field removed.

/// Recursively emit `value` as canonical JSON (object keys sorted, compact).
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                // serde_json renders a string Value with correct JSON escaping.
                out.extend_from_slice(serde_json::to_string(k).unwrap().as_bytes());
                out.push(b':');
                write_canonical(&map[*k], out);
            }
            out.push(b'}');
        }
        Value::Array(arr) => {
            out.push(b'[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(v, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(serde_json::to_string(other).unwrap().as_bytes()),
    }
}

/// BLAKE3 a byte slice into a raw 32-byte digest.
pub fn blake3_32(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Content id of a value: `b3:<hex>` over its canonical serialization (doc 04 §1.4).
pub fn content_cid(value: &Value) -> String {
    format!("b3:{}", hex::encode(blake3_32(&canonical_json(value))))
}

/// The 32-byte digest that a signature covers: the canonical serialization of
/// `value` with any top-level `sig` field removed. Symmetric across sign/verify.
pub fn signing_digest(value: &Value) -> [u8; 32] {
    let mut v = value.clone();
    if let Value::Object(map) = &mut v {
        map.remove("sig");
    }
    blake3_32(&canonical_json(&v))
}

/// WG-Fed wire/format compatibility version.
///
/// Mirrors `WG_AGENCY_COMPAT_VERSION` (`src/agency/mod.rs`) and
/// `WG_PI_PLUGIN_COMPAT_VERSION` (`src/pi_plugin/mod.rs`): a single source of
/// truth peers exchange on first contact, **failing loud** on an incompatible
/// mismatch (ADR-fed-001 §D7, doc 04 §1.5). Per finding S-7 the negotiated
/// parameters are eventually *signed*, not merely exchanged; the spark fixes the
/// constant + the loud-fail rule, and transport-side authentication of the
/// handshake hardens in Wave 4.
pub const WG_FED_COMPAT_VERSION: &str = "0.2.0";

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
        assert!(check_compat("0.2.99").is_ok());
    }

    #[test]
    fn compat_rejects_minor_bump_pre_1_0() {
        // Pre-1.0 the wire format is not frozen, so a minor bump is incompatible.
        let err = check_compat("0.3.0").unwrap_err().to_string();
        assert!(err.contains("FAILED"), "{err}");
        assert!(err.contains("0.3.0"), "{err}");
        assert!(err.contains(WG_FED_COMPAT_VERSION), "{err}");
        // The prior wave's wire (0.1.0) is also incompatible: the Wave-5 rotate_root
        // verification semantics changed, so a 0.1.x peer must fail loud, not
        // silently mis-verify a rotated chain.
        assert!(check_compat("0.1.0").is_err());
    }

    #[test]
    fn compat_rejects_garbage() {
        assert!(check_compat("not-a-version").is_err());
        assert!(check_compat("1").is_err());
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let a = serde_json::json!({"b": 1, "a": {"y": 2, "x": 3}});
        let b = serde_json::json!({"a": {"x": 3, "y": 2}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), br#"{"a":{"x":3,"y":2},"b":1}"#);
    }

    #[test]
    fn signing_digest_ignores_sig_field() {
        let with_sig = serde_json::json!({"from": "alice", "sig": "deadbeef"});
        let other_sig = serde_json::json!({"from": "alice", "sig": "00000000"});
        let no_sig = serde_json::json!({"from": "alice"});
        assert_eq!(signing_digest(&with_sig), signing_digest(&other_sig));
        assert_eq!(signing_digest(&with_sig), signing_digest(&no_sig));
    }

    #[test]
    fn content_cid_is_stable_and_prefixed() {
        let v = serde_json::json!({"x": 1, "y": [1, 2, 3]});
        let cid = content_cid(&v);
        assert!(cid.starts_with("b3:"), "{cid}");
        assert_eq!(content_cid(&v), cid);
        // A change in content changes the cid.
        let v2 = serde_json::json!({"x": 2, "y": [1, 2, 3]});
        assert_ne!(content_cid(&v2), cid);
    }
}
