//! Freshness attestations — the S-3 freeze/eclipse defense (ADR-fed-001 §OQ4,
//! ADR-fed-002 §D5).
//!
//! On the async, store-and-forward path the common case is that nobody is online at
//! the same moment. That opens an **eclipse/freeze** attack: a transport rung that
//! controls a verifier's view can simply *withhold* a `revoke_key` link, so a
//! revoked key keeps looking alive. A signature check alone cannot detect this —
//! the signature is still valid; what is stale is the verifier's knowledge of
//! *current authorization status*.
//!
//! The defense is a small signed, **re-fetchable** attestation the custodian/node
//! periodically emits over its current sigchain head:
//!
//! ```text
//! { v, alg, identity, head, as_of, expires = as_of + Δ, seq, sig }
//! ```
//!
//! A verifier **re-fetches** it before a freshness-gated action and **fails closed
//! on stale** (degrades to read-only, never fail-open). Two independent guards make
//! the freeze ineffective:
//!
//! 1. **Signed `as_of`/`expires`** with a bounded clock-skew tolerance — a verifier
//!    refuses an attestation older than its **policy Δ** for the action class
//!    ([`ActionClass`]). Δ is *tiered*: routine "key still valid" checks tolerate the
//!    email-speed budget (24 h); high-value actions demand a tight, freshly re-fetched
//!    window (15 min).
//! 2. **A monotonic `seq`** — a verifier remembers the highest `seq` it has seen for
//!    an identity and rejects any lower one, so even a perfect clock-skew attack
//!    cannot *replay* an old "still valid" attestation to resurrect a revoked key.
//!
//! Historical verification is explicitly **not** gated: verifying an already-signed,
//! immutable, BLAKE3-addressed artifact against the chain state at its link's
//! position is valid forever (Keybase semantics). Freshness is required only when
//! accepting a **new** action or a **high-value** operation.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::keys::{self, Custodian};
use super::sigchain::{AuthorizedKeys, KeyRole, KeyStatus};
use super::{ALG_ED25519, ENVELOPE_V, content_cid, signing_digest};

/// Default Δ for routine "is this key still valid" checks (24 h — tolerant of the
/// email-speed, both-ends-offline budget). Tunable per ADR-fed-001 §OQ4.
pub const ROUTINE_DELTA_SECS: i64 = 24 * 60 * 60;
/// Default Δ for high-value actions (15 min — a tight, freshly-re-fetched window).
pub const HIGH_VALUE_DELTA_SECS: i64 = 15 * 60;
/// Bounded clock-skew tolerance (±5 min). Widens acceptance slightly; can never
/// extend a revoked key's life beyond `Δ + skew` (the monotonic `seq` closes the
/// residual replay gap).
pub const SKEW_TOLERANCE_SECS: i64 = 5 * 60;

/// The sensitivity class of the action a verifier is about to take. Picks the
/// policy Δ (the maximum attestation age the verifier will accept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// Accepting an ordinary message from a known peer. Δ ≈ 24 h.
    Routine,
    /// Accepting a `rotate_root`, a large-scope delegation, or a cross-trust
    /// `StateSnapshot` load. Δ ≤ 15 min, forced live re-fetch.
    HighValue,
}

impl ActionClass {
    /// The verifier's policy Δ (max acceptable attestation age) for this class.
    pub fn delta_secs(self) -> i64 {
        match self {
            ActionClass::Routine => ROUTINE_DELTA_SECS,
            ActionClass::HighValue => HIGH_VALUE_DELTA_SECS,
        }
    }

    /// Parse a CLI label (`routine` | `high-value` / `high_value` / `highvalue`).
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "routine" => Ok(ActionClass::Routine),
            "high-value" | "highvalue" | "high" => Ok(ActionClass::HighValue),
            other => bail!("unknown action class {other:?} (expected routine|high-value)"),
        }
    }
}

/// A signed `valid-as-of T, expires T+Δ` attestation over an identity's current
/// sigchain head. Re-fetched by a verifier before a freshness-gated action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreshnessAttestation {
    pub v: u16,
    pub alg: String,
    /// The `wgid:` this attestation is about.
    pub identity: String,
    /// The sigchain head CID this attestation vouches is current.
    pub head: String,
    /// RFC3339 instant the attestation was minted.
    pub as_of: String,
    /// RFC3339 instant the attestation expires (`as_of + Δ`).
    pub expires: String,
    /// Monotonic counter — a verifier rejects any attestation with a lower `seq`
    /// than the highest it has seen for this identity (rollback resistance).
    pub seq: u64,
    /// ed25519 signature by an authorized signer (or root), hex.
    #[serde(default)]
    pub sig: String,
}

impl FreshnessAttestation {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("FreshnessAttestation serializes")
    }

    /// Build an attestation valid for `ttl_secs` from `now` (RFC3339). `seq` must be
    /// strictly greater than any previously issued for this identity. A **negative**
    /// `ttl_secs` back-dates `expires` before `as_of`, minting an already-expired
    /// attestation — used to exercise the fail-closed-on-stale path deterministically.
    pub fn build(
        identity: &str,
        head: &str,
        now: chrono::DateTime<chrono::Utc>,
        ttl_secs: i64,
        seq: u64,
    ) -> Self {
        let expires = now + chrono::Duration::seconds(ttl_secs);
        Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            identity: identity.to_string(),
            head: head.to_string(),
            as_of: now.to_rfc3339(),
            expires: expires.to_rfc3339(),
            seq,
            sig: String::new(),
        }
    }

    /// Content id (`b3:<hex>`).
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    /// Sign with `custodian`'s key `signer_kid`.
    pub fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the signature against the identity's authorized signer set, and that
    /// the attestation is *about* that identity. (Offline; no freshness judgement —
    /// see [`check_fresh`].)
    pub fn verify_signature(&self, auth: &AuthorizedKeys) -> Result<()> {
        if self.identity != keys::wgid_from_pubkey(&auth.root_pub) {
            bail!("attestation.identity does not match the verified sigchain root");
        }
        let digest = signing_digest(&self.to_value());
        let sig_bytes = decode_sig(&self.sig)?;
        if keys::verify_sig(&auth.active_root, &digest, &sig_bytes) {
            return Ok(());
        }
        for k in &auth.keys {
            if k.role == KeyRole::Signer && k.status == KeyStatus::Active {
                if let Ok(pk) = decode_pub(&k.public) {
                    if keys::verify_sig(&pk, &digest, &sig_bytes) {
                        return Ok(());
                    }
                }
            }
        }
        bail!("freshness attestation signature does not verify against any authorized key")
    }
}

/// The outcome of a fail-closed freshness check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshVerdict {
    /// The attestation is fresh enough for the action class.
    Fresh { seq: u64, head: String },
    /// The attestation exists but is too old for the action class — **fail closed**.
    Stale { reason: String },
    /// The attestation is a rollback (lower `seq` than already seen) — **fail closed**.
    Rollback { reason: String },
}

impl FreshVerdict {
    pub fn is_fresh(&self) -> bool {
        matches!(self, FreshVerdict::Fresh { .. })
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            FreshVerdict::Fresh { .. } => None,
            FreshVerdict::Stale { reason } | FreshVerdict::Rollback { reason } => Some(reason),
        }
    }
}

/// The fail-closed freshness rule (ADR-fed-001 §OQ4). Given a (signature-verified)
/// attestation, the current time, the action class, and the highest `seq` already
/// seen for this identity, decide whether a freshness-gated action may proceed.
///
/// Refuses (returns `Stale`/`Rollback`, never `Fresh`) if **any** of:
/// - `seq < last_seen` (a replayed/rolled-back attestation);
/// - `now` is past `expires + skew` (the issuer's own validity window elapsed);
/// - the attestation age (`now - as_of`) exceeds the verifier's **policy Δ** for the
///   class, plus skew (the verifier's tighter bar — e.g. 15 min for high-value);
/// - `as_of` is in the future beyond skew (a clock-forward / future-dating attempt).
pub fn check_fresh(
    att: &FreshnessAttestation,
    now: chrono::DateTime<chrono::Utc>,
    class: ActionClass,
    last_seen_seq: Option<u64>,
) -> FreshVerdict {
    let verdict = check_fresh_inner(att, now, class, last_seen_seq);
    // Observability (M20): tally the check and (on a fail-closed verdict) emit a trace
    // event correlated by the attesting identity — freshness failures are exactly the
    // class of event an operator wants alerting on (a withheld revoke / stale peer).
    let fresh = verdict.is_fresh();
    crate::obs::record_freshness(fresh);
    if !fresh {
        tracing::debug!(
            identity = %att.identity,
            class = ?class,
            reason = verdict.reason().unwrap_or("stale"),
            "freshness check failed closed"
        );
    }
    verdict
}

fn check_fresh_inner(
    att: &FreshnessAttestation,
    now: chrono::DateTime<chrono::Utc>,
    class: ActionClass,
    last_seen_seq: Option<u64>,
) -> FreshVerdict {
    // 1. Monotonic seq — rollback resistance, independent of clocks.
    if let Some(prev) = last_seen_seq {
        if att.seq < prev {
            return FreshVerdict::Rollback {
                reason: format!(
                    "attestation seq {} is lower than the highest seen ({prev}) — \
                     refusing a rolled-back attestation (possible revoke withheld)",
                    att.seq
                ),
            };
        }
    }

    let as_of = match chrono::DateTime::parse_from_rfc3339(&att.as_of) {
        Ok(t) => t.with_timezone(&chrono::Utc),
        Err(e) => {
            return FreshVerdict::Stale {
                reason: format!("attestation as_of {:?} is unparseable: {e}", att.as_of),
            };
        }
    };
    let expires = match chrono::DateTime::parse_from_rfc3339(&att.expires) {
        Ok(t) => t.with_timezone(&chrono::Utc),
        Err(e) => {
            return FreshVerdict::Stale {
                reason: format!("attestation expires {:?} is unparseable: {e}", att.expires),
            };
        }
    };

    let skew = chrono::Duration::seconds(SKEW_TOLERANCE_SECS);

    // 2. Future-dating guard (clock-forward attack).
    if as_of > now + skew {
        return FreshVerdict::Stale {
            reason: format!(
                "attestation as_of {} is in the future (now {}) beyond skew tolerance",
                att.as_of,
                now.to_rfc3339()
            ),
        };
    }

    // 3. Issuer's own validity window elapsed (fail closed).
    if now > expires + skew {
        return FreshVerdict::Stale {
            reason: format!(
                "attestation expired at {} (now {}) — failing closed on stale",
                att.expires,
                now.to_rfc3339()
            ),
        };
    }

    // 4. Verifier's policy Δ for this action class (the tighter, class-specific bar).
    let age = now - as_of;
    let policy = chrono::Duration::seconds(class.delta_secs()) + skew;
    if age > policy {
        return FreshVerdict::Stale {
            reason: format!(
                "attestation age {}s exceeds the {:?} policy Δ ({}s) — re-fetch a \
                 fresher attestation before this action (failing closed)",
                age.num_seconds(),
                class,
                class.delta_secs()
            ),
        };
    }

    FreshVerdict::Fresh {
        seq: att.seq,
        head: att.head.clone(),
    }
}

// ── Persistent "highest seq seen" tracker (rollback backstop) ──────────────────

/// Read the highest attestation `seq` a verifier has recorded for `wgid`, or `None`.
/// Stored as a plain integer file under `dir/<sanitized wgid>`.
pub fn load_seen_seq(dir: &std::path::Path, wgid: &str) -> Option<u64> {
    let path = dir.join(super::transport::sanitize(wgid));
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Record `seq` as seen for `wgid` (monotonic — never lowers the stored value).
pub fn record_seen_seq(dir: &std::path::Path, wgid: &str, seq: u64) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let cur = load_seen_seq(dir, wgid).unwrap_or(0);
    let hi = cur.max(seq);
    let path = dir.join(super::transport::sanitize(wgid));
    std::fs::write(&path, hi.to_string()).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn decode_sig(sig_hex: &str) -> Result<[u8; 64]> {
    let b = hex::decode(sig_hex).context("signature is not valid hex")?;
    if b.len() != 64 {
        bail!("signature is {} bytes, expected 64", b.len());
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&b);
    Ok(out)
}

fn decode_pub(pub_hex: &str) -> Result<[u8; 32]> {
    let b = hex::decode(pub_hex).context("public key is not valid hex")?;
    if b.len() != 32 {
        bail!("public key is {} bytes, expected 32", b.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::{Custodian, gen_ed25519, gen_x25519, kid_for, wgid_from_pubkey};
    use crate::identity::sigchain::{KeyEntry, KeyRole, KeyStatus, add_key, genesis, verify};

    struct Minted {
        wgid: String,
        signer_kid: String,
        auth: AuthorizedKeys,
        cust: Custodian,
    }

    fn mint(tag: &str) -> Minted {
        let dir = std::env::temp_dir().join(format!(
            "wg-fresh-ks-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cust = Custodian::with_keystore_dir(tag, dir);
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let signer = gen_ed25519().unwrap();
        let signer_kid = kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let enc = gen_x25519().unwrap();
        let enc_kid = kid_for(&enc.public);
        cust.store_sealing_key(&enc_kid, &enc.secret).unwrap();

        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();
        let signer_entry = KeyEntry {
            kid: signer_kid.clone(),
            public: hex::encode(signer.public),
            role: KeyRole::Signer,
            scope: vec!["event".into(), "state".into()],
            status: KeyStatus::Active,
        };
        let l1 = add_key(&cust, &g, &root.public, &root_kid, signer_entry).unwrap();
        let chain = vec![g, l1];
        let auth = verify(&chain, &wgid).unwrap();
        Minted {
            wgid,
            signer_kid,
            auth,
            cust,
        }
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        // A fixed instant so tests never depend on the wall clock.
        chrono::DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn fresh_attestation_verifies_and_passes_both_classes() {
        let m = mint("fresh");
        let mut att = FreshnessAttestation::build(&m.wgid, "b3:head", now(), ROUTINE_DELTA_SECS, 1);
        att.sign(&m.cust, &m.signer_kid).unwrap();
        att.verify_signature(&m.auth).unwrap();
        // Checked a minute after issue: fresh for both classes.
        let t = now() + chrono::Duration::seconds(60);
        assert!(check_fresh(&att, t, ActionClass::Routine, None).is_fresh());
        assert!(check_fresh(&att, t, ActionClass::HighValue, None).is_fresh());
    }

    #[test]
    fn stale_fails_closed_for_high_value_but_routine_ok() {
        let m = mint("stale");
        // Issued with a 24h TTL, but checked 1 hour later.
        let mut att = FreshnessAttestation::build(&m.wgid, "b3:head", now(), ROUTINE_DELTA_SECS, 1);
        att.sign(&m.cust, &m.signer_kid).unwrap();
        let t = now() + chrono::Duration::minutes(60);
        // High-value Δ is 15 min → a 60-min-old attestation FAILS CLOSED.
        let hv = check_fresh(&att, t, ActionClass::HighValue, None);
        assert!(
            !hv.is_fresh(),
            "high-value must fail closed on a 60-min-old attestation"
        );
        assert!(matches!(hv, FreshVerdict::Stale { .. }));
        // The same attestation is still fine for routine traffic (24h Δ).
        assert!(check_fresh(&att, t, ActionClass::Routine, None).is_fresh());
    }

    #[test]
    fn expired_fails_closed_even_routine() {
        let m = mint("expired");
        // TTL of 0 → expires == as_of → already stale once skew elapses.
        let mut att = FreshnessAttestation::build(&m.wgid, "b3:head", now(), 0, 1);
        att.sign(&m.cust, &m.signer_kid).unwrap();
        let t = now() + chrono::Duration::minutes(10); // past expires + 5min skew
        assert!(matches!(
            check_fresh(&att, t, ActionClass::Routine, None),
            FreshVerdict::Stale { .. }
        ));
    }

    #[test]
    fn rollback_lower_seq_is_rejected() {
        let m = mint("rollback");
        let mut old = FreshnessAttestation::build(&m.wgid, "b3:old", now(), ROUTINE_DELTA_SECS, 1);
        old.sign(&m.cust, &m.signer_kid).unwrap();
        // Verifier has already seen seq=3; a replayed seq=1 must be refused even
        // though it is otherwise perfectly fresh and validly signed.
        let t = now() + chrono::Duration::seconds(30);
        assert!(matches!(
            check_fresh(&old, t, ActionClass::HighValue, Some(3)),
            FreshVerdict::Rollback { .. }
        ));
        // seq equal-or-higher is accepted.
        let mut newer =
            FreshnessAttestation::build(&m.wgid, "b3:new", now(), ROUTINE_DELTA_SECS, 3);
        newer.sign(&m.cust, &m.signer_kid).unwrap();
        assert!(check_fresh(&newer, t, ActionClass::HighValue, Some(3)).is_fresh());
    }

    #[test]
    fn future_dated_attestation_is_rejected() {
        let m = mint("future");
        let future = now() + chrono::Duration::hours(2);
        let mut att =
            FreshnessAttestation::build(&m.wgid, "b3:head", future, ROUTINE_DELTA_SECS, 1);
        att.sign(&m.cust, &m.signer_kid).unwrap();
        assert!(matches!(
            check_fresh(&att, now(), ActionClass::Routine, None),
            FreshVerdict::Stale { .. }
        ));
    }

    #[test]
    fn tampered_attestation_signature_fails() {
        let m = mint("tamper");
        let mut att = FreshnessAttestation::build(&m.wgid, "b3:head", now(), ROUTINE_DELTA_SECS, 1);
        att.sign(&m.cust, &m.signer_kid).unwrap();
        // Flip the head after signing — signature must no longer verify.
        att.head = "b3:evil".into();
        assert!(att.verify_signature(&m.auth).is_err());
    }

    #[test]
    fn seen_seq_tracker_is_monotonic() {
        let dir = std::env::temp_dir().join(format!(
            "wg-fresh-seq-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(load_seen_seq(&dir, "wgid:zX"), None);
        record_seen_seq(&dir, "wgid:zX", 2).unwrap();
        assert_eq!(load_seen_seq(&dir, "wgid:zX"), Some(2));
        // Recording a lower seq never lowers the stored high-water mark.
        record_seen_seq(&dir, "wgid:zX", 1).unwrap();
        assert_eq!(load_seen_seq(&dir, "wgid:zX"), Some(2));
        record_seen_seq(&dir, "wgid:zX", 5).unwrap();
        assert_eq!(load_seen_seq(&dir, "wgid:zX"), Some(5));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
