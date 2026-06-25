//! The three self-describing, versioned, BLAKE3-content-addressed wire envelopes
//! (doc 04 §1.4): [`IdentityRecord`] (the portable public identity — carries **no**
//! private key, ADR-fed-003 §D1), [`StateSnapshot`] (tagged loadable state,
//! ADR-fed-004), and [`SignedEvent`] (the unit of transport, ADR-fed-002).
//!
//! Each is signed by an authorized signer key and verified **offline** against the
//! signer set the sigchain authorizes (ADR-fed-001 §D2/§D5). A forged "from" fails
//! the signature check; a flipped byte breaks the content id. [`SignedEvent`] may
//! additionally be **sealed** per-recipient (X25519 ECDH → HKDF → XChaCha20-Poly1305)
//! so the `to` set *is* the ACL (FR-S3) — sealing is optional in the spark.

use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

use super::keys::{self, Custodian};
use super::sigchain::{AuthorizedKeys, KeyEntry};
use super::{ALG_ED25519, ENVELOPE_V, blake3_32, content_cid, signing_digest};

/// A fetchable delivery/state endpoint advertised in an `IdentityRecord`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    /// `relay` | `node` | `inbox` | `state`.
    pub kind: String,
    pub uri: String,
}

/// A verifiable alias binding (petname / handle). None in the spark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliasProof {
    pub alias: String,
    pub proof: String,
    pub url: String,
}

/// The operational `Agent` fields a pulled federated identity needs to be
/// dispatchable without a schema mismatch (FR-I6, ADR-fed-001 §D6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFields {
    #[serde(default)]
    pub role_id: String,
    #[serde(default)]
    pub trust_level: String,
    #[serde(default)]
    pub executor: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

// ── IdentityRecord ─────────────────────────────────────────────────────────────

/// The public, portable identity (the V2 "downloadable identity"). Contains **no**
/// private key — feeding it to an honest client lets you *read and verify*, never
/// *sign as* (ADR-fed-003 §D1, the spark's headline assertion).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub v: u16,
    pub alg: String,
    /// The `wgid:` address — the genesis root pubkey.
    pub id: String,
    /// CID of the latest sigchain link.
    pub sigchain_head: String,
    /// Authorized non-root keys (signer + encryption).
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(default)]
    pub alias_proofs: Vec<AliasProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_fields: Option<AgentFields>,
    /// ed25519 signature by an authorized signer (or root), hex.
    #[serde(default)]
    pub sig: String,
}

impl IdentityRecord {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("IdentityRecord serializes")
    }

    /// Content id of this record (`b3:<hex>`).
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    /// Sign with `custodian`'s key `signer_kid` (the digest boundary).
    pub fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the record signature against the authorized signer set (offline).
    pub fn verify(&self, auth: &AuthorizedKeys) -> Result<()> {
        if self.id != keys::wgid_from_pubkey(&auth.root_pub) {
            bail!("IdentityRecord.id does not match the verified sigchain root");
        }
        verify_sig_against_authorized(&self.to_value(), &self.sig, auth, "IdentityRecord")
    }
}

// ── StateSnapshot ──────────────────────────────────────────────────────────────

/// Loadable/portable state (the HQ10 / ADR-fed-004 format). The *interface* is
/// stable; `payload_kind` is the evolvable slot — an old reader hitting an unknown
/// kind degrades gracefully (verifies signature + provenance, surfaces "payload
/// unreadable by this client"), never silently corrupts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub v: u16,
    pub alg: String,
    pub identity: String,
    /// Tagged, evolvable: `conv-cache-v1` | `summary-v1` | `opaque-blob-v1` | …
    pub payload_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<Value>,
    /// BLAKE3 of the (possibly-encrypted) payload bytes.
    pub content_cid: String,
    /// CID of the prior snapshot for incremental publish, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    #[serde(default)]
    pub sig: String,
}

impl StateSnapshot {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("StateSnapshot serializes")
    }

    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    pub fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    pub fn verify(&self, auth: &AuthorizedKeys) -> Result<()> {
        if self.identity != keys::wgid_from_pubkey(&auth.root_pub) {
            bail!("StateSnapshot.identity does not match the verified sigchain root");
        }
        verify_sig_against_authorized(&self.to_value(), &self.sig, auth, "StateSnapshot")
    }
}

// ── SignedEvent ────────────────────────────────────────────────────────────────

/// A sealed envelope: X25519 ECDH (ephemeral → recipient static) → HKDF-SHA256 →
/// XChaCha20-Poly1305. The `to` set is realized as the recipient kid here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedBlob {
    pub scheme: String,
    /// kid of the recipient encryption key this blob is sealed to.
    pub recipient_kid: String,
    /// Sender's ephemeral X25519 public key, hex.
    pub ephemeral_pub: String,
    /// 24-byte XChaCha20 nonce, hex.
    pub nonce: String,
    /// AEAD ciphertext (incl. tag), hex.
    pub ciphertext: String,
}

/// A message — the unit of transport (doc 04 §1.4c, ADR-fed-002). Addressed by
/// **pubkey** (`wgid:`), not path. Authenticated offline; a forged `from` fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedEvent {
    pub v: u16,
    pub alg: String,
    /// BLAKE3 content id over the event core (idempotent dedup key, FR-M6).
    #[serde(default)]
    pub id: String,
    pub from: String,
    #[serde(default)]
    pub to: Vec<String>,
    pub created_at: String,
    /// `msg` | `ack` | `task-ref` | `state-head` | `sigchain-link` | `delegation`.
    pub kind: String,
    #[serde(default)]
    pub refs: Vec<Value>,
    /// Plaintext body (mutually exclusive with `enc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Sealed body (mutually exclusive with `body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc: Option<SealedBlob>,
    #[serde(default)]
    pub sig: String,
}

impl SignedEvent {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("SignedEvent serializes")
    }

    /// The content id over the event "core" (everything except `id` and `sig`).
    fn core_id(&self) -> String {
        let mut v = self.to_value();
        if let Value::Object(map) = &mut v {
            map.remove("id");
            map.remove("sig");
        }
        content_cid(&v)
    }

    /// Build a plaintext event from `from` to `to` and stamp its content id.
    pub fn new_plain(from: &str, to: &[String], created_at: &str, kind: &str, body: &str) -> Self {
        let mut ev = Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            id: String::new(),
            from: from.to_string(),
            to: to.to_vec(),
            created_at: created_at.to_string(),
            kind: kind.to_string(),
            refs: Vec::new(),
            body: Some(body.to_string()),
            enc: None,
            sig: String::new(),
        };
        ev.id = ev.core_id();
        ev
    }

    /// Build a sealed event: encrypt `body` to `recipient_enc_pub`, then stamp id.
    pub fn new_sealed(
        from: &str,
        to: &[String],
        created_at: &str,
        kind: &str,
        body: &str,
        recipient_kid: &str,
        recipient_enc_pub: &[u8; 32],
    ) -> Result<Self> {
        let enc = seal(body.as_bytes(), recipient_kid, recipient_enc_pub)?;
        let mut ev = Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            id: String::new(),
            from: from.to_string(),
            to: to.to_vec(),
            created_at: created_at.to_string(),
            kind: kind.to_string(),
            refs: Vec::new(),
            body: None,
            enc: Some(enc),
            sig: String::new(),
        };
        ev.id = ev.core_id();
        Ok(ev)
    }

    pub fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the event: the content id is intact AND the signature verifies
    /// against a key the **sender's** sigchain authorizes for signing. This is the
    /// single check behind "forged from fails" and "download ≠ impersonation".
    pub fn verify(&self, sender_auth: &AuthorizedKeys) -> Result<()> {
        // The claimed `from` must be the sender whose chain we verified.
        if self.from != keys::wgid_from_pubkey(&sender_auth.root_pub) {
            bail!(
                "event.from {:?} does not match the verified sender sigchain root",
                self.from
            );
        }
        // Content-address integrity (dedup key must match the core).
        if self.id != self.core_id() {
            bail!("event id does not match its content (tampered or malformed)");
        }
        verify_sig_against_authorized(&self.to_value(), &self.sig, sender_auth, "SignedEvent")
    }

    /// Open a sealed event with the recipient's custody-held encryption key.
    /// A holder of the ciphertext **without** the recipient enc key cannot.
    pub fn open(&self, custodian: &Custodian) -> Result<String> {
        let blob = self
            .enc
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("event is not sealed"))?;
        let plaintext = open_seal(blob, custodian)?;
        String::from_utf8(plaintext).context("sealed payload is not valid UTF-8")
    }
}

// ── Shared verification + sealing helpers ──────────────────────────────────────

/// Verify `sig_hex` over `value` (minus `sig`) against any key authorized to sign
/// for this identity (root or an active signer). Tries each; succeeds if any does.
fn verify_sig_against_authorized(
    value: &Value,
    sig_hex: &str,
    auth: &AuthorizedKeys,
    what: &str,
) -> Result<()> {
    let digest = signing_digest(value);
    let sig_bytes = decode_sig(sig_hex)?;
    // The active root may sign (after a rotate_root, the old root may not).
    if keys::verify_sig(&auth.active_root, &digest, &sig_bytes) {
        return Ok(());
    }
    // Any active signer may sign.
    for k in &auth.keys {
        if k.role == super::sigchain::KeyRole::Signer
            && k.status == super::sigchain::KeyStatus::Active
        {
            if let Ok(pk) = decode_pub(&k.public) {
                if keys::verify_sig(&pk, &digest, &sig_bytes) {
                    return Ok(());
                }
            }
        }
    }
    bail!(
        "{what} signature does not verify against any key authorized by the \
         sigchain — rejected (forged author or tampered content)"
    )
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

/// HKDF-SHA256 a shared X25519 secret into a 32-byte XChaCha20 key.
fn derive_seal_key(shared: &[u8; 32]) -> Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut okm = [0u8; 32];
    hk.expand(b"wg-fed-seal-v1", &mut okm)
        .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    Ok(okm)
}

const SEAL_SCHEME: &str = "x25519-xchacha20poly1305-v1";

/// Seal `plaintext` to a recipient X25519 public key with a fresh ephemeral key.
fn seal(plaintext: &[u8], recipient_kid: &str, recipient_enc_pub: &[u8; 32]) -> Result<SealedBlob> {
    // Fresh ephemeral keypair (sender side needs no custody).
    let mut eph_secret = [0u8; 32];
    getrandom::getrandom(&mut eph_secret)
        .map_err(|e| anyhow::anyhow!("CSPRNG unavailable: {e}"))?;
    let eph = StaticSecret::from(eph_secret);
    let eph_pub = XPublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&XPublicKey::from(*recipient_enc_pub));
    let key = derive_seal_key(&shared.to_bytes())?;

    let mut nonce_bytes = [0u8; 24];
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|e| anyhow::anyhow!("CSPRNG unavailable: {e}"))?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|e| anyhow::anyhow!("seal failed: {e}"))?;

    Ok(SealedBlob {
        scheme: SEAL_SCHEME.to_string(),
        recipient_kid: recipient_kid.to_string(),
        ephemeral_pub: hex::encode(eph_pub),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    })
}

/// Open a `SealedBlob` using the recipient's custody-held X25519 key.
fn open_seal(blob: &SealedBlob, custodian: &Custodian) -> Result<Vec<u8>> {
    if blob.scheme != SEAL_SCHEME {
        bail!("unknown seal scheme {:?}", blob.scheme);
    }
    let eph_pub = decode_pub(&blob.ephemeral_pub).context("bad ephemeral pub")?;
    // ECDH happens inside the custodian — the static enc secret never leaves.
    let shared = custodian.agree(&blob.recipient_kid, &eph_pub)?;
    let key = derive_seal_key(&shared)?;
    let nonce = hex::decode(&blob.nonce).context("bad nonce hex")?;
    if nonce.len() != 24 {
        bail!("nonce is {} bytes, expected 24", nonce.len());
    }
    let ct = hex::decode(&blob.ciphertext).context("bad ciphertext hex")?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    cipher
        .decrypt(XNonce::from_slice(&nonce), ct.as_ref())
        .map_err(|_| anyhow::anyhow!("unseal failed — wrong key or tampered ciphertext"))
}

/// Content id of arbitrary payload bytes (`b3:<hex>`), for `StateSnapshot.content_cid`.
pub fn payload_cid(bytes: &[u8]) -> String {
    format!("b3:{}", hex::encode(blake3_32(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::{Custodian, gen_ed25519, gen_x25519, kid_for, wgid_from_pubkey};
    use crate::identity::sigchain::{KeyEntry, KeyRole, KeyStatus, add_key, genesis, verify};

    struct Minted {
        wgid: String,
        signer_kid: String,
        enc_kid: String,
        enc_pub: [u8; 32],
        auth: AuthorizedKeys,
    }

    fn mint(name: &str) -> (Custodian, Minted) {
        // Unique, leaked keystore dir per identity — parallel-test-safe, no `$HOME`.
        let tmp = tempfile::tempdir().unwrap();
        let keystore = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let cust = Custodian::with_keystore_dir(name, keystore);
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();

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

        let enc = gen_x25519().unwrap();
        let enc_kid = kid_for(&enc.public);
        cust.store_sealing_key(&enc_kid, &enc.secret).unwrap();
        let l2 = add_key(
            &cust,
            &l1,
            &root.public,
            &root_kid,
            KeyEntry {
                kid: enc_kid.clone(),
                public: hex::encode(enc.public),
                role: KeyRole::Enc,
                scope: vec![],
                status: KeyStatus::Active,
            },
        )
        .unwrap();

        let auth = verify(&[g, l1, l2], &wgid).unwrap();
        (
            cust,
            Minted {
                wgid,
                signer_kid,
                enc_kid,
                enc_pub: enc.public,
                auth,
            },
        )
    }

    #[test]
    fn identity_record_sign_verify() {
        let (cust, m) = mint("alice");
        let mut rec = IdentityRecord {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            id: m.wgid.clone(),
            sigchain_head: m.auth.head.clone(),
            keys: m.auth.keys.clone(),
            endpoints: vec![],
            alias_proofs: vec![],
            agent_fields: None,
            sig: String::new(),
        };
        rec.sign(&cust, &m.signer_kid).unwrap();
        assert!(rec.verify(&m.auth).is_ok());

        // Flip a byte of sigchain_head → verification fails.
        let mut tampered = rec.clone();
        tampered.sigchain_head.push('0');
        assert!(tampered.verify(&m.auth).is_err());
    }

    #[test]
    fn forged_event_from_fails() {
        let (cust_bob, bob) = mint("bob");
        let (_cust_mallory, mallory) = mint("mallory");

        // Genuine event from Bob verifies against Bob's authorized set.
        let mut ev = SignedEvent::new_plain(
            &bob.wgid,
            &[mallory.wgid.clone()],
            "2026-06-25T00:00:00Z",
            "msg",
            "hi",
        );
        ev.sign(&cust_bob, &bob.signer_kid).unwrap();
        assert!(ev.verify(&bob.auth).is_ok());

        // Forge "from Bob": rewrite from→bob on a mallory-authored event. The id
        // and signature no longer match → verification fails.
        let mut forged = SignedEvent::new_plain(
            &mallory.wgid,
            &[bob.wgid.clone()],
            "2026-06-25T00:00:00Z",
            "msg",
            "i am bob",
        );
        forged.sign(&_cust_mallory, &mallory.signer_kid).unwrap();
        forged.from = bob.wgid.clone();
        assert!(forged.verify(&bob.auth).is_err());
    }

    #[test]
    fn sealed_event_only_recipient_opens() {
        let (cust_bob, bob) = mint("bob2");
        let (cust_alice, alice) = mint("alice2");

        let mut ev = SignedEvent::new_sealed(
            &bob.wgid,
            &[alice.wgid.clone()],
            "2026-06-25T00:00:00Z",
            "msg",
            "secret for alice",
            &alice.enc_kid,
            &alice.enc_pub,
        )
        .unwrap();
        ev.sign(&cust_bob, &bob.signer_kid).unwrap();
        assert!(ev.verify(&bob.auth).is_ok());

        // Alice opens it.
        assert_eq!(ev.open(&cust_alice).unwrap(), "secret for alice");
        // Bob (a third party w.r.t. the seal) cannot — he holds no key for
        // alice's enc kid.
        assert!(ev.open(&cust_bob).is_err());
    }
}
