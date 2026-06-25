//! The sigchain — an append-only, hash-linked, signed log mapping a stable
//! identity to its current authorized key set (ADR-fed-001 §D2).
//!
//! Each link is content-addressed (BLAKE3) and signed by a key the chain
//! authorized *at that link's position*. The address (`wgid:`) is the genesis
//! root pubkey and never changes (§D4). The spark implements the two link types
//! it needs — `genesis` (declare the root) and `add_key` (authorize a signer /
//! encryption key) — and a [`verify`] that replays the chain to derive the
//! authorized key set. The remaining link types are modeled in [`LinkType`] for
//! forward-compatibility but are produced in later waves (rotation/revocation =
//! Wave 5, delegation = Wave 6).
//!
//! **Hydra defense (finding S-4):** `add_key` / `rotate_root` are locked to the
//! root key. A day-to-day signer can never grow the authorized key set, at any
//! authority-dial setting (ADR-fed-003 §D2/§D3).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::keys::{self, Custodian};
use super::{ALG_ED25519, ENVELOPE_V, content_cid, signing_digest};

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

/// The optional genesis recovery slot (ADR-fed-001 §OQ3). The slot *always*
/// exists; whether it is populated is mode-dependent (mandatory node-less,
/// optional node-present, absent for agents). The spark mints agents/node-present
/// identities, so it stays `None` — but the field is fixed at genesis so a
/// recovery quorum can be bound from the first link in later waves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverySlot {
    /// Guardian commitments (hashes/pubkeys); empty in the spark.
    #[serde(default)]
    pub guardians: Vec<String>,
    /// M-of-N threshold.
    pub threshold: u8,
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
    /// add_key/revoke_key: the key being authorized/revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyEntry>,
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
    let mut link = SigchainLink {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        link_type: LinkType::Genesis,
        seq: 0,
        prev: None,
        root_pub: Some(hex::encode(root_pub)),
        recovery,
        key: None,
        signer_kid: root_kid.to_string(),
        signer_pub: hex::encode(root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// Build and sign an `add_key` link authorizing `key`. **Signed by the root**
/// (the hydra lock, S-4): only the root may grow the authorized key set.
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
        key: Some(key),
        signer_kid: root_kid.to_string(),
        signer_pub: hex::encode(root_pub),
        sig: String::new(),
    };
    link.sign_with(custodian)?;
    Ok(link)
}

/// The result of replaying a verified sigchain: the authorized key set.
#[derive(Debug, Clone)]
pub struct AuthorizedKeys {
    /// The genesis root public key (== the `wgid:` body).
    pub root_pub: [u8; 32],
    /// Active non-root keys (signer / encryption).
    pub keys: Vec<KeyEntry>,
    /// CID of the latest (head) link.
    pub head: String,
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

    /// Is `pubkey` an active key authorized to sign events for this identity?
    /// True for the root or any active signer key.
    pub fn authorizes_signing(&self, pubkey: &[u8; 32]) -> bool {
        if pubkey == &self.root_pub {
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

/// Replay and verify a sigchain, returning its authorized key set.
///
/// Verification is a pure local computation rooted at `expected_wgid`'s genesis
/// pubkey — **no network, no central authority** (ADR-fed-001 §D5). Enforces:
/// genesis-first + self-signed root; the address equals the genesis root pubkey;
/// each link hash-links to its predecessor (`prev == prev.cid()`) with a strictly
/// increasing `seq`; each link's signature verifies; and `add_key`/`rotate_root`
/// are signed by the root (the hydra lock).
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

    let mut authorized: Vec<KeyEntry> = Vec::new();
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
            LinkType::AddKey | LinkType::RotateRoot => {
                // Hydra lock (S-4): only the root may grow the key set / rotate.
                if signer_pub != root_pub {
                    bail!(
                        "link {} ({:?}) is not signed by the root — refused (hydra \
                         lock, S-4)",
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
                if let Some(k) = &link.key {
                    for a in authorized.iter_mut() {
                        if a.kid == k.kid {
                            a.status = KeyStatus::Revoked;
                        }
                    }
                }
            }
            LinkType::Genesis => bail!("duplicate genesis link at seq {}", link.seq),
            // Other link types carry no key-set change in the spark.
            _ => {}
        }
        prev_link = link;
    }

    Ok(AuthorizedKeys {
        root_pub,
        keys: authorized,
        head: prev_link.cid(),
    })
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
            key: Some(key),
            signer_kid: signer_kid.clone(),
            signer_pub: hex::encode(signer.public),
            sig: String::new(),
        };
        let digest = signing_digest(&link.to_value());
        link.sig = hex::encode(cust.sign_digest(&signer_kid, &digest).unwrap());
        // The signature is valid, but it is not the root → hydra lock rejects it.
        assert!(verify(&[g, link], &wgid).is_err());
    }
}
