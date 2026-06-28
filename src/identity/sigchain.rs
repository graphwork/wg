//! The sigchain — an append-only, hash-linked, signed log mapping a stable
//! identity to its current authorized key set (ADR-fed-001 §D2).
//!
//! Each link is content-addressed (BLAKE3) and signed by a key the chain
//! authorized *at that link's position*. The address (`wgid:`) is the genesis
//! root pubkey and never changes (§D4). [`verify`] replays the chain to derive the
//! authorized key set and the current **active** root. Implemented link types:
//! `genesis` (declare the root), `add_key` (authorize a signer / encryption key),
//! `revoke_key` + `rotate_root` (Wave 5 rotation/recovery, below); `delegate`
//! (UCAN) is Wave 6.
//!
//! **Hydra defense (finding S-4):** `add_key` / `revoke_key` / `rotate_root` are
//! locked to the **active root** key. A day-to-day (delegated) signer can never grow
//! the authorized key set or rotate the root, at any authority-dial setting
//! (ADR-fed-003 §D2/§D3). This is the structural hydra kill.
//!
//! **Wave 5 — rotation & recovery (ADR-fed-003 §D5/§D6).** The `wgid:` address is
//! always the *genesis* root pubkey and never changes (§D4), but the **active**
//! signing root rotates underneath it:
//!
//! - [`rotate_root`] — normal succession: the *current* active root signs in the
//!   next root.
//! - [`revoke_key`] — the durable, content-addressed revocation of an authorized key
//!   (the S-3 freeze defense's first layer; freshness is the second, see
//!   [`super::freshness`]).
//! - **Recovery** (V6) when the active root is lost/compromised — two layered paths,
//!   neither of which is a plain succession:
//!   - [`rotate_root_via_recovery_key`] — the *offline recovery key* registered at
//!     genesis (atproto-style higher-priority override, node default).
//!   - [`rotate_root_via_guardians`] — an *M-of-N guardian quorum* endorses the new
//!     root (the mandatory node-less ceremony that defuses the Fatal A-4 finding).
//!
//! Crucially, recovery still **cannot grow the set behind the owner's back**: a
//! recovery rotate installs a *new root* the recoverer possesses, authorized by a
//! control the mere downloader lacks (the recovery key or the guardian quorum) — it
//! is never a surviving-delegate `add_key` (that path *is* the hydra, and is locked).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::keys::{self, Custodian};
use super::{ALG_ED25519, ENVELOPE_V, blake3_32, canonical_json, content_cid, signing_digest};

/// The kind of a sigchain link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    /// Declare the root key — this link's `root_pub` *is* the `wgid:` body.
    Genesis,
    /// Authorize a signer / device / encryption key with a scope.
    AddKey,
    /// Revoke a previously-authorized key (Wave 5).
    RevokeKey,
    /// Succession: the old root signs the next root (Wave 5).
    RotateRoot,
    /// Issue a UCAN-style capability (Wave 6, ADR-fed-003).
    Delegate,
    /// Publish fetchable inbox/state/relay endpoints.
    SetEndpoints,
    /// Bind a verifiable alias (Wave 6).
    SetAliasProof,
    /// **Root-signed** replacement of the active recovery slot (audit B8): rotate the
    /// offline recovery key and/or its window, or clear it. Makes a compromised recovery
    /// key revocable — the prior genesis recovery slot was immutable.
    SetRecovery,
}

/// The role a non-root key plays (ADR-fed-001 §D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyRole {
    /// ed25519 day-to-day key: signs `SignedEvent`/`StateSnapshot`, never the chain.
    Signer,
    /// X25519 static key: per-recipient confidentiality (the ACL realization).
    Enc,
}

/// Lifecycle status of an authorized key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    Revoked,
}

/// One authorized (non-root) key. Shared by the sigchain and `IdentityRecord`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyEntry {
    /// Short stable key id (`keys::kid_for`).
    pub kid: String,
    /// Raw public key, hex-encoded (ed25519 verifying key or X25519 public key).
    #[serde(rename = "pub")]
    pub public: String,
    pub role: KeyRole,
    #[serde(default)]
    pub scope: Vec<String>,
    pub status: KeyStatus,
}

/// The genesis recovery slot (ADR-fed-001 §OQ3, populated in Wave 5 per
/// ADR-fed-003 §D5). The slot *always* exists; whether it is populated is
/// mode-dependent:
///
/// - **Agents / node-present (default):** may be `None` — recovery anchors to the
///   custodian (the node holds the root and re-issues a signer; FR-S6).
/// - **Node default with owner backstop:** an offline `recovery_key` (the
///   higher-priority override key, §D5).
/// - **Node-less (MANDATORY):** **both** a paper/offline `recovery_key` **and**
///   `guardians` with an `M-of-N` `threshold` — the ceremony that defuses Fatal
///   A-4. [`RecoverySlot::validate_node_less`] enforces this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverySlot {
    /// Guardian ed25519 verifying keys (hex). Each may endorse a recovery
    /// `rotate_root` over the new root; `threshold` of them constitute a quorum.
    #[serde(default)]
    pub guardians: Vec<String>,
    /// M-of-N threshold (the `M`). `0` means "no guardian quorum configured".
    pub threshold: u8,
    /// The offline recovery key's ed25519 verifying key (hex) — the
    /// higher-priority override key held offline by the owner (§D5). `None` when no
    /// recovery key is configured (e.g. an agent anchored purely to its custodian).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
    /// **Recovery window** start (RFC3339), audit B8. When either bound is set, a
    /// recovery-key `rotate_root` is valid only if its asserted `recovery_at` falls
    /// inside `[recovery_not_before, recovery_expires]`; outside the window the offline
    /// recovery key is structurally powerless (the atproto-style time-boxed override the
    /// doc claimed but never implemented). `None`/`None` = the legacy unbounded window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_not_before: Option<String>,
    /// **Recovery window** end (RFC3339), audit B8. See [`RecoverySlot::recovery_not_before`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_expires: Option<String>,
}

impl RecoverySlot {
    /// A node-less human identity MUST embed **both** a paper/offline recovery key
    /// **and** an M-of-N (M ≥ 2) guardian quorum (ADR-fed-003 §D5; memo §5
    /// guardrail: never ship node-less without the recovery ceremony). Genesis
    /// tooling calls this and refuses to mint without it.
    pub fn validate_node_less(&self) -> Result<()> {
        if self.recovery_key.is_none() {
            bail!(
                "node-less genesis requires a paper/offline recovery key (the mandatory \
                 recovery ceremony, ADR-fed-003 §D5 — refusing to mint an unrecoverable \
                 identity, the Fatal A-4 path)"
            );
        }
        if self.threshold < 2 {
            bail!(
                "node-less genesis requires an M-of-N guardian quorum with M >= 2 (got \
                 threshold {}); a single guardian is not a quorum (A-7)",
                self.threshold
            );
        }
        if (self.guardians.len() as u8) < self.threshold {
            bail!(
                "node-less genesis names {} guardians but the threshold is {} — cannot \
                 reach the quorum",
                self.guardians.len(),
                self.threshold
            );
        }
        Ok(())
    }
}

/// One guardian's endorsement of a recovery `rotate_root` (the M-of-N social
/// recovery primitive, ADR-fed-003 §D5/§OQ4). The guardian signs the
/// [`recovery_assertion_digest`] over `(wgid, new_root)` — an assertion it can
/// produce **store-and-forward, offline, without the final link** (guardians need
/// not be online simultaneously, NFR-2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardianEndorsement {
    /// The guardian's ed25519 verifying key (hex). Must be in the genesis guardian
    /// set, and each guardian counts at most once toward the threshold.
    pub guardian_pub: String,
    /// ed25519 signature over `recovery_assertion_digest(wgid, new_root)`, hex.
    pub sig: String,
}

/// How a `rotate_root` link that is **not** a normal succession is authorized — the
/// recovery path (ADR-fed-003 §D5). Absent (`None`) ⇒ plain succession, signed by
/// the current active root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum RecoveryProof {
    /// The offline recovery key registered at genesis signs the rotation directly
    /// (the link's own `sig` is by the recovery key) — atproto-style higher-priority
    /// override, recoverable even against a hostile custodian within the window.
    RecoveryKey,
    /// An M-of-N guardian quorum endorses the new root; the link's own `sig` is by
    /// the **new root** (proof of possession), and the quorum authorizes the
    /// transition (the node-less ceremony).
    Guardians {
        endorsements: Vec<GuardianEndorsement>,
    },
}

/// The 32-byte digest a guardian (or recovery flow) signs to endorse installing
/// `new_root_pub` as the active root of `wgid`. Independent of any single link's
/// content so endorsements can be collected asynchronously and combined later.
pub fn recovery_assertion_digest(wgid: &str, new_root_pub: &[u8; 32]) -> [u8; 32] {
    let v = serde_json::json!({
        "purpose": "wg-fed-recover-v1",
        "identity": wgid,
        "new_root": hex::encode(new_root_pub),
    });
    blake3_32(&canonical_json(&v))
}

/// A reference to a parent identity, cited in a **fork**'s genesis (ADR-fed-003
/// §D4 / ADR-fed-001 §D2). A download onto a new host that wants to be *its own*
/// identity starts a new genesis (new root → new `wgid:`) that cites the identity
/// it forked from — a verifiable *child*, cryptographically **not** the parent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentRef {
    /// The forked-from identity's `wgid:` address.
    pub wgid: String,
    /// The parent's sigchain head CID at fork time (pins the parent's state).
    pub sigchain_head: String,
}

/// A single sigchain link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SigchainLink {
    pub v: u16,
    pub alg: String,
    #[serde(rename = "type")]
    pub link_type: LinkType,
    /// 0 for genesis, then strictly increasing.
    pub seq: u64,
    /// CID of the previous link; `None` for genesis (hash-linked, §D2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    /// genesis only: the root public key, hex-encoded. This is the `wgid:` body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_pub: Option<String>,
    /// genesis only: the optional recovery slot (§OQ3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoverySlot>,
    /// genesis only: a parent citation when this identity is a **fork** (§D4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
    /// add_key: the key being authorized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyEntry>,
    /// revoke_key: the kid of the key being revoked (Wave 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke_kid: Option<String>,
    /// rotate_root: the new active root public key, hex (Wave 5). The `wgid:`
    /// address is unchanged (always the genesis root); this rotates the *active*
    /// signing root underneath it (§D4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_root_pub: Option<String>,
    /// rotate_root recovery authorization (Wave 5). Absent ⇒ a normal succession
    /// signed by the current active root; present ⇒ a recovery path (§D5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_proof: Option<RecoveryProof>,
    /// rotate_root via recovery key: the asserted recovery time (RFC3339), checked
    /// against the active recovery window (audit B8). Required when the active recovery
    /// slot declares a window; signed as part of the link so it is authenticated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_at: Option<String>,
    /// kid of the key that signed this link.
    pub signer_kid: String,
    /// public key of the signer, hex-encoded (cross-checked against the
    /// authorized set during [`verify`]; never trusted on its own).
    pub signer_pub: String,
    /// ed25519 signature over the canonical link (with `sig` removed), hex.
    #[serde(default)]
    pub sig: String,
}

impl SigchainLink {
    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("SigchainLink serializes")
    }

    /// Content id of this link (`b3:<hex>`), the `sigchain_head` when latest.
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    /// Sign this link with `custodian`'s key `signer_kid` (the digest boundary).
    fn sign_with(&mut self, custodian: &Custodian) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        let sig = custodian.sign_digest(&self.signer_kid, &digest)?;
        self.sig = hex::encode(sig);
        Ok(())
    }

    /// Verify this link's signature against an explicit public key.
    fn verify_against(&self, pubkey: &[u8; 32]) -> bool {
        let digest = signing_digest(&self.to_value());
        let sig_bytes = match hex::decode(&self.sig) {
            Ok(b) if b.len() == 64 => {
                let mut s = [0u8; 64];
                s.copy_from_slice(&b);
                s
            }
            _ => return false,
        };
        keys::verify_sig(pubkey, &digest, &sig_bytes)
    }
}

/// Decode a hex pubkey field into raw 32 bytes.
fn pub32(hexed: &str, what: &str) -> Result<[u8; 32]> {
    let b = hex::decode(hexed).map_err(|_| anyhow::anyhow!("{what}: invalid hex"))?;
    if b.len() != 32 {
        bail!("{what}: {} bytes, expected 32", b.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b);
    Ok(out)
}

/// Build and self-sign a `genesis` link declaring `root_pub`.
///
/// `custodian` must hold the root seed under `root_kid` (= `keys::kid_for`).
pub fn genesis(
    custodian: &Custodian,
    root_pub: &[u8; 32],
    root_kid: &str,
    recovery: Option<RecoverySlot>,
) -> Result<SigchainLink> {
    genesis_with_parent(custodian, root_pub, root_kid, recovery, None)
}

/// Build and self-sign a **fork** `genesis` link: a brand-new identity (new root →
/// new `wgid:`) that cites `parent` (ADR-fed-003 §D4). This is the default
/// "download onto host B" semantics — a verifiable child, never the parent.
pub fn genesis_fork(
    custodian: &Custodian,
    root_pub: &[u8; 32],
    root_kid: &str,
    recovery: Option<RecoverySlot>,
    parent: ParentRef,
) -> Result<SigchainLink> {
    genesis_with_parent(custodian, root_pub, root_kid, recovery, Some(parent))
}

fn genesis_with_parent(
    custodian: &Custodian,
    root_pub: &[u8; 32],
    root_kid: &str,
    recovery: Option<RecoverySlot>,
    parent: Option<ParentRef>,
) -> Result<SigchainLink> {
    let mut link = SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::Genesis,
        seq: 0,
        prev: None,
        root_pub: Some(hex::encode(root_pub)),
        recovery,
        parent,
        key: None,
        revoke_kid: None,
        new_root_pub: None,
        recovery_proof: None,
        recovery_at: None,
        signer_kid: root_kid.to_string(),
        signer_pub: hex::encode(root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign an `add_key` link authorizing `key`. **Signed by the active
/// root** (the hydra lock, S-4): only the root may grow the authorized key set.
pub fn add_key(
    custodian: &Custodian,
    prev: &SigchainLink,
    root_pub: &[u8; 32],
    root_kid: &str,
    key: KeyEntry,
) -> Result<SigchainLink> {
    let mut link = SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::AddKey,
        seq: prev.seq + 1,
        prev: Some(prev.cid()),
        root_pub: None,
        recovery: None,
        parent: None,
        key: Some(key),
        revoke_kid: None,
        new_root_pub: None,
        recovery_proof: None,
        recovery_at: None,
        signer_kid: root_kid.to_string(),
        signer_pub: hex::encode(root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign a `revoke_key` link revoking the authorized key `target_kid`
/// (Wave 5, ADR-fed-003 §D6). **Signed by the active root** — revocation is a
/// key-set mutation and a recovery-from-compromise primitive, so like `add_key` it
/// is root-locked; a delegated signer cannot revoke another key.
pub fn revoke_key(
    custodian: &Custodian,
    prev: &SigchainLink,
    active_root_pub: &[u8; 32],
    active_root_kid: &str,
    target_kid: &str,
) -> Result<SigchainLink> {
    let mut link = SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::RevokeKey,
        seq: prev.seq + 1,
        prev: Some(prev.cid()),
        root_pub: None,
        recovery: None,
        parent: None,
        key: None,
        revoke_kid: Some(target_kid.to_string()),
        new_root_pub: None,
        recovery_proof: None,
        recovery_at: None,
        signer_kid: active_root_kid.to_string(),
        signer_pub: hex::encode(active_root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign a **`SetRecovery`** link (audit B8) — the **root-signed** replacement
/// of the active recovery slot. This makes the offline recovery key *revocable*: the
/// legitimate active root can rotate out a compromised recovery key (or retune its
/// window), which the immutable genesis slot could not. Passing `None` clears recovery
/// entirely. Root-locked like `add_key`/`revoke_key` (a delegate cannot touch recovery).
pub fn set_recovery(
    custodian: &Custodian,
    prev: &SigchainLink,
    active_root_pub: &[u8; 32],
    active_root_kid: &str,
    new_recovery: Option<RecoverySlot>,
) -> Result<SigchainLink> {
    let mut link = SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::SetRecovery,
        seq: prev.seq + 1,
        prev: Some(prev.cid()),
        root_pub: None,
        recovery: new_recovery,
        parent: None,
        key: None,
        revoke_kid: None,
        new_root_pub: None,
        recovery_proof: None,
        recovery_at: None,
        signer_kid: active_root_kid.to_string(),
        signer_pub: hex::encode(active_root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign a normal-succession `rotate_root` link: the **current active
/// root** signs in `new_root_pub` (Wave 5, ADR-fed-003 §D5). The `wgid:` address
/// (the genesis root) is unchanged; the *active* signing root rotates underneath.
pub fn rotate_root(
    custodian: &Custodian,
    prev: &SigchainLink,
    active_root_pub: &[u8; 32],
    active_root_kid: &str,
    new_root_pub: &[u8; 32],
) -> Result<SigchainLink> {
    let mut link = rotate_root_skeleton(prev, new_root_pub, None);
    link.signer_kid = active_root_kid.to_string();
    link.signer_pub = hex::encode(active_root_pub);
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign a **recovery** `rotate_root` authorized by the offline recovery
/// key registered at genesis (ADR-fed-003 §D5, the node default). The link is
/// signed by the recovery key directly — a higher-priority override that recovers
/// even against a hostile custodian. `recovery_kid` is the recovery key in custody.
///
/// `recovery_at` (RFC3339) is the asserted recovery time checked against the active
/// recovery **window** (audit B8): it is **required** when the active recovery slot
/// declares a window and ignored otherwise (legacy unbounded recovery passes `None`).
pub fn rotate_root_via_recovery_key(
    custodian: &Custodian,
    prev: &SigchainLink,
    recovery_pub: &[u8; 32],
    recovery_kid: &str,
    new_root_pub: &[u8; 32],
    recovery_at: Option<&str>,
) -> Result<SigchainLink> {
    let mut link = rotate_root_skeleton(prev, new_root_pub, Some(RecoveryProof::RecoveryKey));
    link.recovery_at = recovery_at.map(|s| s.to_string());
    link.signer_kid = recovery_kid.to_string();
    link.signer_pub = hex::encode(recovery_pub);
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign a **recovery** `rotate_root` authorized by an M-of-N guardian
/// quorum (ADR-fed-003 §D5, the node-less ceremony). The link is signed by the
/// **new root** (proof of possession by the recovering owner); `endorsements` are
/// the collected guardian signatures over [`recovery_assertion_digest`].
pub fn rotate_root_via_guardians(
    custodian: &Custodian,
    prev: &SigchainLink,
    new_root_pub: &[u8; 32],
    new_root_kid: &str,
    endorsements: Vec<GuardianEndorsement>,
) -> Result<SigchainLink> {
    let mut link = rotate_root_skeleton(
        prev,
        new_root_pub,
        Some(RecoveryProof::Guardians { endorsements }),
    );
    link.signer_kid = new_root_kid.to_string();
    link.signer_pub = hex::encode(new_root_pub);
    link.sign_with(custodian)?;
    Ok(link)
}

/// Shared skeleton for the three `rotate_root` builders (signer fields filled in by
/// the caller before signing).
fn rotate_root_skeleton(
    prev: &SigchainLink,
    new_root_pub: &[u8; 32],
    recovery_proof: Option<RecoveryProof>,
) -> SigchainLink {
    SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::RotateRoot,
        seq: prev.seq + 1,
        prev: Some(prev.cid()),
        root_pub: None,
        recovery: None,
        parent: None,
        key: None,
        revoke_kid: None,
        new_root_pub: Some(hex::encode(new_root_pub)),
        recovery_proof,
        recovery_at: None,
        signer_kid: String::new(),
        signer_pub: String::new(),
        sig: String::new(),
    }
}

/// The result of replaying a verified sigchain: the authorized key set.
#[derive(Debug, Clone)]
pub struct AuthorizedKeys {
    /// The **genesis** root public key (== the `wgid:` body / address). Immutable
    /// under rotation (§D4) — this is what an address resolves to, forever.
    pub root_pub: [u8; 32],
    /// The **active** signing root after replaying any `rotate_root` links. Equals
    /// `root_pub` for an un-rotated chain. This is the key currently authorized to
    /// sign `add_key`/`revoke_key`/`rotate_root` and to author events (§D5).
    pub active_root: [u8; 32],
    /// Active non-root keys (signer / encryption).
    pub keys: Vec<KeyEntry>,
    /// CID of the latest (head) link.
    pub head: String,
    /// The genesis recovery slot (guardians / threshold / recovery key), for
    /// recovery tooling. `None` if genesis embedded no recovery configuration.
    pub recovery: Option<RecoverySlot>,
}

impl AuthorizedKeys {
    /// The active signer key entry, if any (the spark authorizes exactly one).
    pub fn active_signer(&self) -> Option<&KeyEntry> {
        self.keys
            .iter()
            .find(|k| k.role == KeyRole::Signer && k.status == KeyStatus::Active)
    }

    /// The active encryption key entry, if any.
    pub fn active_enc(&self) -> Option<&KeyEntry> {
        self.keys
            .iter()
            .find(|k| k.role == KeyRole::Enc && k.status == KeyStatus::Active)
    }

    /// Is `pubkey` a key authorized to sign events for this identity right now?
    /// True for the **active** root or any active signer key. (After a
    /// `rotate_root`, the *old* root no longer authorizes new signatures.)
    pub fn authorizes_signing(&self, pubkey: &[u8; 32]) -> bool {
        if pubkey == &self.active_root {
            return true;
        }
        self.keys.iter().any(|k| {
            k.role == KeyRole::Signer
                && k.status == KeyStatus::Active
                && pub32(&k.public, "key")
                    .map(|p| &p == pubkey)
                    .unwrap_or(false)
        })
    }
}

/// Verify an M-of-N guardian quorum endorsing `new_root` for `wgid` against the
/// genesis recovery slot. Each endorsement's `guardian_pub` must be in the slot's
/// guardian set, each guardian counts at most once, every signature must verify
/// over [`recovery_assertion_digest`], and the distinct-guardian count must reach
/// the threshold. (ADR-fed-003 §D5/§OQ4; abuse-resistant per A-7: M ≥ 2 means no
/// lone guardian can recover.)
fn verify_guardian_quorum(
    recovery: &RecoverySlot,
    wgid: &str,
    new_root: &[u8; 32],
    endorsements: &[GuardianEndorsement],
) -> Result<()> {
    if recovery.threshold < 2 {
        bail!("guardian recovery requires an M-of-N quorum with M >= 2 (A-7)");
    }
    let guardian_set: Vec<[u8; 32]> = recovery
        .guardians
        .iter()
        .filter_map(|g| pub32(g, "guardian pub").ok())
        .collect();
    let digest = recovery_assertion_digest(wgid, new_root);
    let mut counted: Vec<[u8; 32]> = Vec::new();
    for e in endorsements {
        let gp = pub32(&e.guardian_pub, "endorsement guardian_pub")?;
        if !guardian_set.contains(&gp) {
            bail!("guardian endorsement from a key not in the genesis guardian set");
        }
        if counted.contains(&gp) {
            continue; // a guardian counts at most once toward the threshold
        }
        let sig = decode_sig64(&e.sig)?;
        if !keys::verify_sig(&gp, &digest, &sig) {
            bail!("guardian endorsement signature does not verify over the recovery assertion");
        }
        counted.push(gp);
    }
    if (counted.len() as u8) < recovery.threshold {
        bail!(
            "guardian quorum not reached: {} valid distinct endorsements, need {} (M-of-N)",
            counted.len(),
            recovery.threshold
        );
    }
    Ok(())
}

/// Decode a 64-byte hex signature.
fn decode_sig64(sig_hex: &str) -> Result<[u8; 64]> {
    let b = hex::decode(sig_hex).map_err(|_| anyhow::anyhow!("signature is not valid hex"))?;
    if b.len() != 64 {
        bail!("signature is {} bytes, expected 64", b.len());
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&b);
    Ok(out)
}

/// Replay and verify a sigchain, returning its authorized key set.
///
/// Verification is a pure local computation rooted at `expected_wgid`'s genesis
/// pubkey — **no network, no central authority** (ADR-fed-001 §D5). Enforces:
/// genesis-first + self-signed root; the address equals the genesis root pubkey;
/// each link hash-links to its predecessor (`prev == prev.cid()`) with a strictly
/// increasing `seq`; each link's signature verifies; `add_key`/`revoke_key` and a
/// *succession* `rotate_root` are signed by the **active** root (the hydra lock,
/// S-4); and a **recovery** `rotate_root` is authorized only by the registered
/// offline recovery key or an M-of-N guardian quorum (§D5). Replays `rotate_root`
/// to track the active signing root underneath the stable genesis address (§D4).
pub fn verify(links: &[SigchainLink], expected_wgid: &str) -> Result<AuthorizedKeys> {
    if links.is_empty() {
        bail!("empty sigchain");
    }
    let g = &links[0];
    if g.link_type != LinkType::Genesis {
        bail!("first sigchain link is not genesis");
    }
    if g.seq != 0 || g.prev.is_some() {
        bail!("malformed genesis link (seq/prev)");
    }
    let root_hex = g
        .root_pub
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("genesis link missing root_pub"))?;
    let root_pub = pub32(root_hex, "genesis root_pub")?;

    // The address IS the genesis root pubkey (self-certifying, §D1/§D4).
    let addr_pub = keys::pubkey_from_wgid(expected_wgid)?;
    if addr_pub != root_pub {
        bail!("sigchain genesis root does not match the wgid address");
    }
    // Genesis is self-signed by the root.
    if pub32(&g.signer_pub, "genesis signer_pub")? != root_pub {
        bail!("genesis must be self-signed by the root key");
    }
    if !g.verify_against(&root_pub) {
        bail!("genesis link signature is invalid");
    }

    // The **active** recovery slot starts at genesis and is replaced by any root-signed
    // `SetRecovery` link (audit B8) — so a compromised recovery key can be rotated out.
    let mut active_recovery = g.recovery.clone();
    let mut authorized: Vec<KeyEntry> = Vec::new();
    // The active signing root starts at genesis and rotates underneath the address.
    let mut active_root = root_pub;
    let mut prev_link = g;

    for link in &links[1..] {
        // Hash-link integrity.
        match &link.prev {
            Some(p) if *p == prev_link.cid() => {}
            _ => bail!(
                "sigchain link {} does not hash-link to its predecessor",
                link.seq
            ),
        }
        if link.seq != prev_link.seq + 1 {
            bail!("sigchain seq is not strictly increasing at {}", link.seq);
        }
        let signer_pub = pub32(&link.signer_pub, "link signer_pub")?;
        if !link.verify_against(&signer_pub) {
            bail!("sigchain link {} signature is invalid", link.seq);
        }
        match link.link_type {
            // Hydra lock (S-4): only the active root mutates the key set OR the recovery
            // slot. `SetRecovery` joins the root-locked set (audit B8) so a delegate can
            // never rotate the recovery key behind the owner's back.
            LinkType::AddKey | LinkType::RevokeKey | LinkType::SetRecovery => {
                if signer_pub != active_root {
                    bail!(
                        "link {} ({:?}) is not signed by the active root — refused \
                         (hydra lock, S-4: a delegated signer cannot grow or shrink \
                         the authorized key set or the recovery slot)",
                        link.seq,
                        link.link_type
                    );
                }
            }
            _ => {}
        }
        match link.link_type {
            LinkType::AddKey => {
                let key = link
                    .key
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("add_key link {} missing key", link.seq))?;
                // Sanity: the kid matches the public key it carries.
                let kp = pub32(&key.public, "add_key key.pub")?;
                if keys::kid_for(&kp) != key.kid {
                    bail!(
                        "add_key link {}: kid does not match its public key",
                        link.seq
                    );
                }
                authorized.push(key);
            }
            LinkType::RevokeKey => {
                let target = link
                    .revoke_kid
                    .clone()
                    .or_else(|| link.key.as_ref().map(|k| k.kid.clone()))
                    .ok_or_else(|| {
                        anyhow::anyhow!("revoke_key link {} names no key to revoke", link.seq)
                    })?;
                let mut hit = false;
                for a in authorized.iter_mut() {
                    if a.kid == target {
                        a.status = KeyStatus::Revoked;
                        hit = true;
                    }
                }
                if !hit {
                    bail!(
                        "revoke_key link {} targets {target:?}, not an authorized key",
                        link.seq
                    );
                }
            }
            LinkType::RotateRoot => {
                let new_root = pub32(
                    link.new_root_pub.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("rotate_root link {} missing new_root_pub", link.seq)
                    })?,
                    "rotate_root new_root_pub",
                )?;
                match &link.recovery_proof {
                    // Normal succession — the current active root signs in the next.
                    None => {
                        if signer_pub != active_root {
                            bail!(
                                "rotate_root link {} (succession) is not signed by the \
                                 active root — refused (hydra lock, S-4)",
                                link.seq
                            );
                        }
                    }
                    // Recovery via the offline recovery key (higher-priority override).
                    Some(RecoveryProof::RecoveryKey) => {
                        let rec = active_recovery.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "rotate_root link {} claims recovery-key authorization but the \
                                 active recovery slot registers no recovery key (it may have been \
                                 rotated out by a SetRecovery link, audit B8)",
                                link.seq
                            )
                        })?;
                        let rk_hex = rec.recovery_key.as_deref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "rotate_root link {} claims recovery-key authorization but the \
                                 active recovery slot registers no recovery key",
                                link.seq
                            )
                        })?;
                        let rk = pub32(rk_hex, "active recovery_key")?;
                        if signer_pub != rk {
                            bail!(
                                "rotate_root link {} (recovery) is not signed by the \
                                 registered offline recovery key",
                                link.seq
                            );
                        }
                        // Recovery WINDOW enforcement (audit B8): when the active slot
                        // declares a window, the link's asserted `recovery_at` must be
                        // present and inside it — outside, the recovery key is powerless.
                        check_recovery_window(rec, link)?;
                    }
                    // Recovery via M-of-N guardian quorum (the node-less ceremony).
                    Some(RecoveryProof::Guardians { endorsements }) => {
                        let rec = active_recovery.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "rotate_root link {} claims guardian recovery but the active \
                                 recovery slot embeds no guardian set",
                                link.seq
                            )
                        })?;
                        // The link is signed by the new root (proof of possession);
                        // the quorum authorizes the transition.
                        if signer_pub != new_root {
                            bail!(
                                "guardian-recovery rotate_root link {} must be signed by the \
                                 new root (proof of possession)",
                                link.seq
                            );
                        }
                        verify_guardian_quorum(rec, expected_wgid, &new_root, endorsements)
                            .with_context_seq(link.seq)?;
                    }
                }
                active_root = new_root;
            }
            // SetRecovery (audit B8): replace the active recovery slot. Already
            // root-locked above; `None` clears recovery. The new window/key takes effect
            // for every subsequent recovery-key rotate_root.
            LinkType::SetRecovery => {
                active_recovery = link.recovery.clone();
            }
            LinkType::Genesis => bail!("duplicate genesis link at seq {}", link.seq),
            // Other link types carry no key-set change yet (delegate = Wave 6).
            _ => {}
        }
        prev_link = link;
    }

    Ok(AuthorizedKeys {
        root_pub,
        active_root,
        keys: authorized,
        head: prev_link.cid(),
        recovery: active_recovery,
    })
}

/// Enforce the recovery **window** for a recovery-key `rotate_root` (audit B8). When the
/// active recovery slot declares either bound, the link's signed `recovery_at` must be
/// present and fall inside `[recovery_not_before, recovery_expires]`; outside it (or
/// missing when a window is set) the recovery is refused. A slot with no window bounds is
/// the legacy unbounded behavior (any `recovery_at`, or none, is accepted).
fn check_recovery_window(rec: &RecoverySlot, link: &SigchainLink) -> Result<()> {
    let has_window = rec.recovery_not_before.is_some() || rec.recovery_expires.is_some();
    if !has_window {
        return Ok(());
    }
    let at_str = link.recovery_at.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "rotate_root link {} via recovery key declares no recovery_at but the active \
             recovery slot is time-boxed — refused (audit B8 window enforcement)",
            link.seq
        )
    })?;
    let at = parse_ts(at_str, "recovery_at")?;
    if let Some(nbf) = rec.recovery_not_before.as_deref() {
        if at < parse_ts(nbf, "recovery_not_before")? {
            bail!(
                "rotate_root link {} recovery_at {at_str} is before the recovery window opens \
                 ({nbf}) — refused (audit B8)",
                link.seq
            );
        }
    }
    if let Some(exp) = rec.recovery_expires.as_deref() {
        if at > parse_ts(exp, "recovery_expires")? {
            bail!(
                "rotate_root link {} recovery_at {at_str} is after the recovery window closed \
                 ({exp}) — the offline recovery key is expired/powerless (audit B8)",
                link.seq
            );
        }
    }
    Ok(())
}

/// Parse an RFC3339 instant (recovery-window bounds, audit B8).
fn parse_ts(s: &str, what: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| anyhow::anyhow!("{what} {s:?} is not RFC3339: {e}"))
}

/// Tiny helper so the guardian-quorum error names the offending link seq.
trait WithSeq<T> {
    fn with_context_seq(self, seq: u64) -> Result<T>;
}
impl<T> WithSeq<T> for Result<T> {
    fn with_context_seq(self, seq: u64) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("rotate_root link {seq}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::{gen_ed25519, gen_x25519, kid_for, wgid_from_pubkey};

    /// A unique, leaked keystore dir for a test (parallel-safe — no `$HOME`).
    fn scratch_keystore() -> std::path::PathBuf {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        path
    }

    /// Mint a minimal genesis + signer + enc chain into a scratch custodian.
    fn mint(name: &str) -> (Vec<SigchainLink>, String, [u8; 32]) {
        let cust = Custodian::with_keystore_dir(name, scratch_keystore());
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);

        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();

        let signer = gen_ed25519().unwrap();
        let signer_kid = kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let signer_key = KeyEntry {
            kid: signer_kid,
            public: hex::encode(signer.public),
            role: KeyRole::Signer,
            scope: vec!["event".into()],
            status: KeyStatus::Active,
        };
        let l1 = add_key(&cust, &g, &root.public, &root_kid, signer_key).unwrap();

        let enc = gen_x25519().unwrap();
        let enc_kid = kid_for(&enc.public);
        cust.store_sealing_key(&enc_kid, &enc.secret).unwrap();
        let enc_key = KeyEntry {
            kid: enc_kid,
            public: hex::encode(enc.public),
            role: KeyRole::Enc,
            scope: vec![],
            status: KeyStatus::Active,
        };
        let l2 = add_key(&cust, &l1, &root.public, &root_kid, enc_key).unwrap();

        (vec![g, l1, l2], wgid, signer.public)
    }

    #[test]
    fn genesis_and_add_key_verify() {
        let (chain, wgid, signer_pub) = mint("alice");
        let auth = verify(&chain, &wgid).unwrap();
        assert_eq!(auth.root_pub, keys::pubkey_from_wgid(&wgid).unwrap());
        assert!(auth.active_signer().is_some());
        assert!(auth.active_enc().is_some());
        assert!(auth.authorizes_signing(&signer_pub));
        assert_eq!(auth.head, chain.last().unwrap().cid());
    }

    #[test]
    fn wrong_address_is_rejected() {
        let (chain, _wgid, _) = mint("bob");
        let other = wgid_from_pubkey(&gen_ed25519().unwrap().public);
        assert!(verify(&chain, &other).is_err());
    }

    #[test]
    fn tampered_link_breaks_verification() {
        let (mut chain, wgid, _) = mint("carol");
        // Flip the authorized signer's public key — signature no longer matches.
        if let Some(k) = chain[1].key.as_mut() {
            let mut p = hex::decode(&k.public).unwrap();
            p[0] ^= 0xff;
            k.public = hex::encode(p);
        }
        assert!(verify(&chain, &wgid).is_err());
    }

    #[test]
    fn add_key_not_signed_by_root_is_rejected() {
        // Re-sign an add_key link with the signer key instead of the root.
        let cust = Custodian::with_keystore_dir("dave", scratch_keystore());
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();

        let signer = gen_ed25519().unwrap();
        let signer_kid = kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let key = KeyEntry {
            kid: signer_kid.clone(),
            public: hex::encode(signer.public),
            role: KeyRole::Signer,
            scope: vec![],
            status: KeyStatus::Active,
        };
        // Build the add_key but sign it with the SIGNER key (not the root).
        let mut link = SigchainLink {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            link_type: LinkType::AddKey,
            seq: 1,
            prev: Some(g.cid()),
            root_pub: None,
            recovery: None,
            parent: None,
            key: Some(key),
            revoke_kid: None,
            new_root_pub: None,
            recovery_proof: None,
            recovery_at: None,
            signer_kid: signer_kid.clone(),
            signer_pub: hex::encode(signer.public),
            sig: String::new(),
        };
        let digest = signing_digest(&link.to_value());
        link.sig = hex::encode(cust.sign_digest(&signer_kid, &digest).unwrap());
        // The signature is valid, but it is not the root → hydra lock rejects it.
        assert!(verify(&[g, link], &wgid).is_err());
    }

    // ── Wave 5: rotation / revocation / recovery / fork ─────────────────────────

    /// A richer mint that hands back the custodian + root keypair so a test can
    /// append rotate/revoke/recovery links. Optionally embeds a recovery slot.
    struct Full {
        cust: Custodian,
        root: crate::identity::keys::Ed25519Keypair,
        root_kid: String,
        wgid: String,
        signer_kid: String,
        signer_pub: [u8; 32],
        chain: Vec<SigchainLink>,
    }

    fn mint_full(name: &str, recovery: Option<RecoverySlot>) -> Full {
        let cust = Custodian::with_keystore_dir(name, scratch_keystore());
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, recovery).unwrap();
        let signer = gen_ed25519().unwrap();
        let signer_kid = kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let l1 = add_key(
            &cust,
            &g,
            &root.public,
            &root_kid,
            KeyEntry {
                kid: signer_kid.clone(),
                public: hex::encode(signer.public),
                role: KeyRole::Signer,
                scope: vec!["event".into()],
                status: KeyStatus::Active,
            },
        )
        .unwrap();
        Full {
            cust,
            root,
            root_kid,
            wgid,
            signer_kid,
            signer_pub: signer.public,
            chain: vec![g, l1],
        }
    }

    #[test]
    fn rotate_root_succession_keeps_address_and_moves_active_root() {
        let mut f = mint_full("rot", None);
        let old_active = verify(&f.chain, &f.wgid).unwrap().active_root;
        assert_eq!(old_active, f.root.public, "active root starts at genesis");

        // Mint a new root, store it, append a succession rotate_root signed by the old.
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();
        let rot = rotate_root(
            &f.cust,
            f.chain.last().unwrap(),
            &f.root.public,
            &f.root_kid,
            &new_root.public,
        )
        .unwrap();
        f.chain.push(rot);

        let auth = verify(&f.chain, &f.wgid).unwrap();
        // Address (genesis root) is unchanged; active root rotated.
        assert_eq!(auth.root_pub, f.root.public);
        assert_eq!(auth.active_root, new_root.public);
        assert_eq!(keys::wgid_from_pubkey(&auth.root_pub), f.wgid);
        // The OLD root no longer authorizes signing; the new one does.
        assert!(!auth.authorizes_signing(&f.root.public));
        assert!(auth.authorizes_signing(&new_root.public));
        // After rotation, only the NEW root may add_key (hydra lock follows the rotate).
        let next = gen_ed25519().unwrap();
        let next_kid = kid_for(&next.public);
        let good = add_key(
            &f.cust,
            f.chain.last().unwrap(),
            &new_root.public,
            &new_kid,
            KeyEntry {
                kid: next_kid,
                public: hex::encode(next.public),
                role: KeyRole::Signer,
                scope: vec![],
                status: KeyStatus::Active,
            },
        )
        .unwrap();
        let mut grown = f.chain.clone();
        grown.push(good);
        assert!(verify(&grown, &f.wgid).is_ok());
    }

    #[test]
    fn rotate_root_signed_by_old_root_after_rotation_is_rejected() {
        let mut f = mint_full("rot2", None);
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();
        let rot = rotate_root(
            &f.cust,
            f.chain.last().unwrap(),
            &f.root.public,
            &f.root_kid,
            &new_root.public,
        )
        .unwrap();
        f.chain.push(rot);
        // The OLD root tries to add_key after it was rotated out → rejected.
        let bad = add_key(
            &f.cust,
            f.chain.last().unwrap(),
            &f.root.public, // old root!
            &f.root_kid,
            KeyEntry {
                kid: "deadbeef".into(),
                public: hex::encode(gen_ed25519().unwrap().public),
                role: KeyRole::Signer,
                scope: vec![],
                status: KeyStatus::Active,
            },
        )
        .unwrap();
        f.chain.push(bad);
        assert!(verify(&f.chain, &f.wgid).is_err());
    }

    #[test]
    fn revoke_key_removes_from_active_set() {
        let mut f = mint_full("rev", None);
        // Before: the signer is active and authorizes signing.
        let auth = verify(&f.chain, &f.wgid).unwrap();
        assert!(auth.authorizes_signing(&f.signer_pub));
        // Revoke the signer, signed by the active root.
        let rk = revoke_key(
            &f.cust,
            f.chain.last().unwrap(),
            &f.root.public,
            &f.root_kid,
            &f.signer_kid,
        )
        .unwrap();
        f.chain.push(rk);
        let auth2 = verify(&f.chain, &f.wgid).unwrap();
        assert!(
            !auth2.authorizes_signing(&f.signer_pub),
            "a revoked signer must no longer authorize signing"
        );
        assert!(auth2.active_signer().is_none());
    }

    #[test]
    fn revoke_key_not_signed_by_root_is_rejected() {
        let f = mint_full("rev2", None);
        // Build a revoke_key signed by the SIGNER (a delegate), not the root.
        let mut link = SigchainLink {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            link_type: LinkType::RevokeKey,
            seq: 2,
            prev: Some(f.chain.last().unwrap().cid()),
            root_pub: None,
            recovery: None,
            parent: None,
            key: None,
            revoke_kid: Some(f.signer_kid.clone()),
            new_root_pub: None,
            recovery_proof: None,
            recovery_at: None,
            signer_kid: f.signer_kid.clone(),
            signer_pub: hex::encode(f.signer_pub),
            sig: String::new(),
        };
        let digest = signing_digest(&link.to_value());
        link.sig = hex::encode(f.cust.sign_digest(&f.signer_kid, &digest).unwrap());
        let mut chain = f.chain.clone();
        chain.push(link);
        assert!(
            verify(&chain, &f.wgid).is_err(),
            "delegate revoke = hydra, must reject"
        );
    }

    fn recovery_slot_with(
        recovery_key: Option<[u8; 32]>,
        guardians: &[[u8; 32]],
        threshold: u8,
    ) -> RecoverySlot {
        RecoverySlot {
            guardians: guardians.iter().map(hex::encode).collect(),
            threshold,
            recovery_key: recovery_key.map(hex::encode),
            recovery_not_before: None,
            recovery_expires: None,
        }
    }

    #[test]
    fn recover_via_offline_recovery_key() {
        // Genesis embeds an offline recovery key (the node-default owner backstop).
        let rkey = gen_ed25519().unwrap();
        let slot = recovery_slot_with(Some(rkey.public), &[], 0);
        let mut f = mint_full("recA", Some(slot));
        // The active root is "lost" — recover with the offline recovery key.
        let rkey_kid = kid_for(&rkey.public);
        f.cust.store_signing_key(&rkey_kid, &rkey.seed).unwrap();
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();
        let rot = rotate_root_via_recovery_key(
            &f.cust,
            f.chain.last().unwrap(),
            &rkey.public,
            &rkey_kid,
            &new_root.public,
            None,
        )
        .unwrap();
        f.chain.push(rot);
        let auth = verify(&f.chain, &f.wgid).unwrap();
        assert_eq!(
            auth.root_pub, f.root.public,
            "address unchanged after recovery"
        );
        assert_eq!(
            auth.active_root, new_root.public,
            "recovered to the new root"
        );
    }

    #[test]
    fn recovery_key_path_rejected_when_signer_is_not_the_registered_key() {
        let rkey = gen_ed25519().unwrap();
        let slot = recovery_slot_with(Some(rkey.public), &[], 0);
        let mut f = mint_full("recA2", Some(slot));
        // An IMPOSTER key (not the registered recovery key) claims the recovery path.
        let imposter = gen_ed25519().unwrap();
        let imp_kid = kid_for(&imposter.public);
        f.cust.store_signing_key(&imp_kid, &imposter.seed).unwrap();
        let new_root = gen_ed25519().unwrap();
        let rot = rotate_root_via_recovery_key(
            &f.cust,
            f.chain.last().unwrap(),
            &imposter.public,
            &imp_kid,
            &new_root.public,
            None,
        )
        .unwrap();
        f.chain.push(rot);
        assert!(verify(&f.chain, &f.wgid).is_err());
    }

    #[test]
    fn recover_via_guardian_quorum() {
        // 2-of-3 guardians (the node-less ceremony default).
        let gks: Vec<_> = (0..3).map(|_| gen_ed25519().unwrap()).collect();
        let gpubs: Vec<[u8; 32]> = gks.iter().map(|k| k.public).collect();
        let rkey = gen_ed25519().unwrap();
        let slot = recovery_slot_with(Some(rkey.public), &gpubs, 2);
        let mut f = mint_full("recB", Some(slot));

        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();

        // 2 of the 3 guardians endorse the new root (async, offline-collected).
        let digest = recovery_assertion_digest(&f.wgid, &new_root.public);
        let endorsements: Vec<GuardianEndorsement> = gks
            .iter()
            .take(2)
            .map(|gk| {
                let sk = ed25519_dalek::SigningKey::from_bytes(&gk.seed);
                let sig = ed25519_dalek::Signer::sign(&sk, &digest);
                GuardianEndorsement {
                    guardian_pub: hex::encode(gk.public),
                    sig: hex::encode(sig.to_bytes()),
                }
            })
            .collect();
        let rot = rotate_root_via_guardians(
            &f.cust,
            f.chain.last().unwrap(),
            &new_root.public,
            &new_kid,
            endorsements,
        )
        .unwrap();
        f.chain.push(rot);
        let auth = verify(&f.chain, &f.wgid).unwrap();
        assert_eq!(auth.active_root, new_root.public);
    }

    #[test]
    fn guardian_quorum_below_threshold_is_rejected() {
        let gks: Vec<_> = (0..3).map(|_| gen_ed25519().unwrap()).collect();
        let gpubs: Vec<[u8; 32]> = gks.iter().map(|k| k.public).collect();
        let slot = recovery_slot_with(None, &gpubs, 2);
        let mut f = mint_full("recC", Some(slot));
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();
        // Only ONE guardian endorses (below the 2-of-3 threshold).
        let digest = recovery_assertion_digest(&f.wgid, &new_root.public);
        let sk = ed25519_dalek::SigningKey::from_bytes(&gks[0].seed);
        let sig = ed25519_dalek::Signer::sign(&sk, &digest);
        let rot = rotate_root_via_guardians(
            &f.cust,
            f.chain.last().unwrap(),
            &new_root.public,
            &new_kid,
            vec![GuardianEndorsement {
                guardian_pub: hex::encode(gks[0].public),
                sig: hex::encode(sig.to_bytes()),
            }],
        )
        .unwrap();
        f.chain.push(rot);
        assert!(verify(&f.chain, &f.wgid).is_err(), "1-of-3 is not a quorum");
    }

    #[test]
    fn guardian_outside_the_set_does_not_count() {
        let gks: Vec<_> = (0..3).map(|_| gen_ed25519().unwrap()).collect();
        let gpubs: Vec<[u8; 32]> = gks.iter().map(|k| k.public).collect();
        let slot = recovery_slot_with(None, &gpubs, 2);
        let mut f = mint_full("recD", Some(slot));
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();
        let digest = recovery_assertion_digest(&f.wgid, &new_root.public);
        // One genuine guardian + one OUTSIDER (not in the genesis set).
        let outsider = gen_ed25519().unwrap();
        let mk = |kp: &crate::identity::keys::Ed25519Keypair| {
            let sk = ed25519_dalek::SigningKey::from_bytes(&kp.seed);
            GuardianEndorsement {
                guardian_pub: hex::encode(kp.public),
                sig: hex::encode(ed25519_dalek::Signer::sign(&sk, &digest).to_bytes()),
            }
        };
        let rot = rotate_root_via_guardians(
            &f.cust,
            f.chain.last().unwrap(),
            &new_root.public,
            &new_kid,
            vec![mk(&gks[0]), mk(&outsider)],
        )
        .unwrap();
        f.chain.push(rot);
        assert!(
            verify(&f.chain, &f.wgid).is_err(),
            "an outsider cannot fill the quorum"
        );
    }

    #[test]
    fn fork_genesis_cites_parent_and_is_a_distinct_identity() {
        // Parent identity.
        let parent = mint_full("forkP", None);
        let parent_auth = verify(&parent.chain, &parent.wgid).unwrap();

        // A downloader forks: a NEW root → NEW wgid, genesis cites the parent.
        let child_cust = Custodian::with_keystore_dir("forkC", scratch_keystore());
        let child_root = gen_ed25519().unwrap();
        let child_kid = kid_for(&child_root.public);
        child_cust
            .store_signing_key(&child_kid, &child_root.seed)
            .unwrap();
        let child_wgid = wgid_from_pubkey(&child_root.public);
        let pref = ParentRef {
            wgid: parent.wgid.clone(),
            sigchain_head: parent_auth.head.clone(),
        };
        let g = genesis_fork(
            &child_cust,
            &child_root.public,
            &child_kid,
            None,
            pref.clone(),
        )
        .unwrap();

        // The fork is its own identity — a different wgid, verifiable on its own.
        assert_ne!(child_wgid, parent.wgid);
        let auth = verify(&[g.clone()], &child_wgid).unwrap();
        assert_eq!(auth.root_pub, child_root.public);
        // The parent citation is preserved and verifiable.
        assert_eq!(g.parent.as_ref().unwrap().wgid, parent.wgid);
        // A fork's genesis does NOT verify under the PARENT's address (not the same self).
        assert!(verify(&[g], &parent.wgid).is_err());
    }

    // ── M11/B8: windowed + revocable recovery ───────────────────────────────────

    fn windowed_slot(recovery_key: [u8; 32], nbf: Option<&str>, exp: Option<&str>) -> RecoverySlot {
        RecoverySlot {
            guardians: vec![],
            threshold: 0,
            recovery_key: Some(hex::encode(recovery_key)),
            recovery_not_before: nbf.map(|s| s.to_string()),
            recovery_expires: exp.map(|s| s.to_string()),
        }
    }

    #[test]
    fn recovery_key_outside_window_is_rejected_within_is_accepted() {
        // B8: a time-boxed recovery key may only rotate within its declared window.
        let rkey = gen_ed25519().unwrap();
        let slot = windowed_slot(
            rkey.public,
            Some("2026-06-01T00:00:00Z"),
            Some("2026-07-01T00:00:00Z"),
        );
        let mut f = mint_full("recWin", Some(slot));
        let rkey_kid = kid_for(&rkey.public);
        f.cust.store_signing_key(&rkey_kid, &rkey.seed).unwrap();
        let new_root = gen_ed25519().unwrap();
        let new_kid = kid_for(&new_root.public);
        f.cust.store_signing_key(&new_kid, &new_root.seed).unwrap();

        let build = |at: Option<&str>| {
            rotate_root_via_recovery_key(
                &f.cust,
                f.chain.last().unwrap(),
                &rkey.public,
                &rkey_kid,
                &new_root.public,
                at,
            )
            .unwrap()
        };

        // recovery_at AFTER the window closes → refused (the key is expired/powerless).
        let mut after = f.chain.clone();
        after.push(build(Some("2026-08-15T00:00:00Z")));
        assert!(
            verify(&after, &f.wgid).is_err(),
            "recovery after the window must be refused"
        );
        // recovery_at BEFORE the window opens → refused.
        let mut before = f.chain.clone();
        before.push(build(Some("2026-01-01T00:00:00Z")));
        assert!(verify(&before, &f.wgid).is_err());
        // No recovery_at at all when a window is set → refused (fail-closed).
        let mut missing = f.chain.clone();
        missing.push(build(None));
        assert!(verify(&missing, &f.wgid).is_err());

        // recovery_at WITHIN the window → accepted.
        f.chain.push(build(Some("2026-06-15T00:00:00Z")));
        let auth = verify(&f.chain, &f.wgid).unwrap();
        assert_eq!(
            auth.active_root, new_root.public,
            "in-window recovery rotates the root"
        );
        assert_eq!(auth.root_pub, f.root.public, "address unchanged");
    }

    #[test]
    fn set_recovery_rotates_out_a_compromised_recovery_key() {
        // B8: the root can SetRecovery to replace a compromised recovery key — the old
        // key becomes powerless, the new one works, and a delegate cannot SetRecovery.
        let old_rkey = gen_ed25519().unwrap();
        let slot = windowed_slot(old_rkey.public, None, None);
        let mut f = mint_full("recRevoke", Some(slot));
        let old_kid = kid_for(&old_rkey.public);
        f.cust.store_signing_key(&old_kid, &old_rkey.seed).unwrap();

        // Root rotates out the old recovery key, installing a NEW one (root-signed).
        let new_rkey = gen_ed25519().unwrap();
        let new_slot = windowed_slot(new_rkey.public, None, None);
        let sr = set_recovery(
            &f.cust,
            f.chain.last().unwrap(),
            &f.root.public,
            &f.root_kid,
            Some(new_slot),
        )
        .unwrap();
        f.chain.push(sr);
        let auth = verify(&f.chain, &f.wgid).unwrap();
        assert_eq!(
            auth.recovery.as_ref().unwrap().recovery_key,
            Some(hex::encode(new_rkey.public)),
            "the active recovery key is the rotated-in one"
        );

        let target = gen_ed25519().unwrap();
        let target_kid = kid_for(&target.public);
        f.cust.store_signing_key(&target_kid, &target.seed).unwrap();

        // The OLD (rotated-out) recovery key can NO LONGER recover.
        let mut bad = f.chain.clone();
        bad.push(
            rotate_root_via_recovery_key(
                &f.cust,
                f.chain.last().unwrap(),
                &old_rkey.public,
                &old_kid,
                &target.public,
                None,
            )
            .unwrap(),
        );
        assert!(
            verify(&bad, &f.wgid).is_err(),
            "a SetRecovery-rotated-out recovery key must be powerless"
        );

        // The NEW recovery key CAN recover.
        let new_rkey_kid = kid_for(&new_rkey.public);
        f.cust
            .store_signing_key(&new_rkey_kid, &new_rkey.seed)
            .unwrap();
        f.chain.push(
            rotate_root_via_recovery_key(
                &f.cust,
                f.chain.last().unwrap(),
                &new_rkey.public,
                &new_rkey_kid,
                &target.public,
                None,
            )
            .unwrap(),
        );
        assert_eq!(
            verify(&f.chain, &f.wgid).unwrap().active_root,
            target.public
        );

        // A DELEGATE cannot SetRecovery (root-locked, hydra). Build one signed by the
        // signer key instead of the active root → refused.
        let mut delegate_sr = set_recovery(
            &f.cust,
            f.chain.last().unwrap(),
            &target.public,
            &target_kid,
            None,
        )
        .unwrap();
        delegate_sr.signer_kid = f.signer_kid.clone();
        delegate_sr.signer_pub = hex::encode(f.signer_pub);
        let digest = signing_digest(&delegate_sr.to_value());
        delegate_sr.sig = hex::encode(f.cust.sign_digest(&f.signer_kid, &digest).unwrap());
        let mut chain_del = f.chain.clone();
        chain_del.push(delegate_sr);
        assert!(
            verify(&chain_del, &f.wgid).is_err(),
            "a delegate cannot SetRecovery (hydra lock)"
        );
    }

    #[test]
    fn node_less_recovery_slot_validation() {
        // Missing recovery key → refused.
        let only_guardians = RecoverySlot {
            guardians: vec![hex::encode(gen_ed25519().unwrap().public); 3],
            threshold: 2,
            recovery_key: None,
            recovery_not_before: None,
            recovery_expires: None,
        };
        assert!(only_guardians.validate_node_less().is_err());
        // M < 2 → refused.
        let lone = RecoverySlot {
            guardians: vec![hex::encode(gen_ed25519().unwrap().public)],
            threshold: 1,
            recovery_key: Some(hex::encode(gen_ed25519().unwrap().public)),
            recovery_not_before: None,
            recovery_expires: None,
        };
        assert!(lone.validate_node_less().is_err());
        // Both present, M ≥ 2, enough guardians → ok.
        let ok = RecoverySlot {
            guardians: vec![hex::encode(gen_ed25519().unwrap().public); 3],
            threshold: 2,
            recovery_key: Some(hex::encode(gen_ed25519().unwrap().public)),
            recovery_not_before: None,
            recovery_expires: None,
        };
        assert!(ok.validate_node_less().is_ok());
    }
}
