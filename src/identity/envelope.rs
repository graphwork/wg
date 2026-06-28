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

/// The placeholder outer `from` of a **sealed-sender** event: the relay/node sees
/// only `anon → to`, never the real author (FR-S4). The true sender + its signature
/// live *inside* the sealed payload and are recovered only by a recipient (HQ4).
pub const ANON_SENDER: &str = "wgid:anon";

const SEAL_CEK_SCHEME: &str = "x25519-hkdf-xchacha20poly1305-cek-v1";

/// One recipient's wrapping of the content-encryption key (CEK). The presence of a
/// wrap for a recipient kid is exactly what puts that recipient in the ACL — only a
/// holder of the matching encryption key can unwrap the CEK (Wave 6, HQ4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipientWrap {
    /// kid of the recipient encryption key this wrap is sealed to.
    pub recipient_kid: String,
    /// Sender's per-recipient ephemeral X25519 public key, hex.
    pub ephemeral_pub: String,
    /// 24-byte XChaCha20 nonce for the wrap, hex.
    pub nonce: String,
    /// AEAD-wrapped CEK (incl. tag), hex.
    pub wrapped_cek: String,
}

/// A **per-recipient sealed envelope** — the realization of *encryption = ACL*
/// (ADR-fed-003 §HQ4, the `federation.rs` `AccessPolicy` hook). The body is encrypted
/// **once** under a random content-encryption key (CEK); that CEK is then wrapped to
/// each recipient via X25519 ECDH. The set of `recipients` **is** the access-control
/// list: every member can unwrap the CEK and decrypt; a third party holding the
/// ciphertext but no listed encryption key cannot unwrap any CEK and is locked out.
/// Static recipient keys (no forward secrecy) on the offline path — FS does not
/// compose with send-to-offline (S-6); rotation caps the static-key exposure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedEnvelope {
    pub scheme: String,
    /// One wrap per recipient — the ACL.
    pub recipients: Vec<RecipientWrap>,
    /// 24-byte XChaCha20 nonce for the body, hex.
    pub body_nonce: String,
    /// AEAD body ciphertext under the CEK (incl. tag), hex.
    pub body_ct: String,
    /// When true, the encrypted body is an inner [`SenderSealed`] payload carrying the
    /// real author + its signature — the outer `from` is [`ANON_SENDER`] so a relay
    /// learns nothing about the sender (sealed-sender, FR-S4).
    #[serde(default)]
    pub sealed_sender: bool,
}

/// The inner, encrypted-under-the-CEK payload of a **sealed-sender** event: the real
/// author and a signature only a recipient (after unsealing) can recover and verify.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SenderSealed {
    pub from: String,
    pub kind: String,
    pub created_at: String,
    #[serde(default)]
    pub to: Vec<String>,
    pub body: String,
    /// BLAKE3 commitment over the **outer** routing metadata (`to`, `created_at`,
    /// `kind`, `refs`) this inner payload was sealed for (audit C3). Because it lives
    /// inside the *signed* inner, a relay/MITM cannot tamper with the outer envelope
    /// of a sealed-sender event without breaking the commitment — `open` re-derives it
    /// from the visible outer fields and refuses a mismatch. (The non-sealed-sender
    /// paths already authenticate the outer metadata via the outer signature.)
    #[serde(default)]
    pub outer_commitment: String,
    #[serde(default)]
    pub sig: String,
}

/// A BLAKE3 commitment (`b3:<hex>`) over a sealed-sender event's relay-visible **outer
/// routing metadata** (`to`, `created_at`, `kind`, `refs`). Stored inside the signed
/// inner payload and re-checked on open (audit C3). It deliberately excludes the
/// envelope `id` (derived from the core, including the seal, so it cannot be folded
/// back in without a cycle) and `from` (always [`ANON_SENDER`] on the wire); the
/// authenticated `from` is the inner one.
fn sealed_sender_outer_commitment(
    to: &[String],
    created_at: &str,
    kind: &str,
    refs: &[Value],
) -> String {
    let v = serde_json::json!({
        "to": to,
        "created_at": created_at,
        "kind": kind,
        "refs": refs,
    });
    content_cid(&v)
}

impl SenderSealed {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("SenderSealed serializes")
    }

    /// Content id (`b3:<hex>`) of the inner signed payload. The recipient's only
    /// **authenticated** handle on a sealed-sender event (the outer `id` is unsigned),
    /// so it is the dedup key for the replay backstop (audit M9, see [`super::dedup`]).
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the inner author signature against the (now-revealed) sender's authorized
    /// signer set. This is the sealed-sender authentication leg: a relay could not
    /// forge it, and only after unsealing does a recipient learn whom to verify.
    pub fn verify(&self, sender_auth: &AuthorizedKeys) -> Result<()> {
        if self.from != keys::wgid_from_pubkey(&sender_auth.root_pub) {
            bail!("sealed-sender inner.from does not match the verified sender sigchain root");
        }
        verify_sig_against_authorized(&self.to_value(), &self.sig, sender_auth, "SenderSealed")
    }
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
    /// Plaintext body (mutually exclusive with `enc`/`enc_multi`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Single-recipient sealed body (Wave 3/4; mutually exclusive with `body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc: Option<SealedBlob>,
    /// Per-recipient sealed envelope (Wave 6 — the `to` set IS the ACL; mutually
    /// exclusive with `body`). Carries the optional sealed-sender mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc_multi: Option<SealedEnvelope>,
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
            enc_multi: None,
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
            enc_multi: None,
            sig: String::new(),
        };
        ev.id = ev.core_id();
        Ok(ev)
    }

    /// Build a **per-recipient sealed** event (Wave 6): the body is encrypted once
    /// under a fresh CEK and that CEK is wrapped to every recipient in `recipients`
    /// (`(enc_kid, enc_pub)` pairs). The recipient set IS the ACL — each can decrypt,
    /// a third party cannot. `from` is the real sender (authenticated by the outer
    /// signature as usual); for sender anonymity use [`SignedEvent::new_sealed_sender`].
    pub fn new_sealed_multi(
        from: &str,
        to: &[String],
        created_at: &str,
        kind: &str,
        body: &str,
        recipients: &[(String, [u8; 32])],
    ) -> Result<Self> {
        let env = seal_multi(body.as_bytes(), recipients, false)?;
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
            enc: None,
            enc_multi: Some(env),
            sig: String::new(),
        };
        ev.id = ev.core_id();
        Ok(ev)
    }

    /// Build a **sealed-sender** event (Wave 6, FR-S4): the outer `from` is
    /// [`ANON_SENDER`] so a relay/node learns nothing about the author; the real
    /// `from` + a signature by `custodian`'s `signer_kid` are placed *inside* the
    /// CEK-protected payload, recoverable and verifiable only by a listed recipient.
    /// The outer event carries **no** signature (it is anonymous) — authenticity comes
    /// from the inner [`SenderSealed`] signature, checked after [`Self::open_sender_sealed`].
    #[allow(clippy::too_many_arguments)]
    pub fn new_sealed_sender(
        real_from: &str,
        to: &[String],
        created_at: &str,
        kind: &str,
        body: &str,
        recipients: &[(String, [u8; 32])],
        custodian: &Custodian,
        signer_kid: &str,
    ) -> Result<Self> {
        let mut inner = SenderSealed {
            from: real_from.to_string(),
            kind: kind.to_string(),
            created_at: created_at.to_string(),
            to: to.to_vec(),
            body: body.to_string(),
            // C3: commit to the outer routing metadata this event will carry (`refs`
            // is empty here, matching the outer event built below) so a relay cannot
            // rewrite the unsigned outer envelope undetected.
            outer_commitment: sealed_sender_outer_commitment(to, created_at, kind, &[]),
            sig: String::new(),
        };
        inner.sign(custodian, signer_kid)?;
        let plaintext = serde_json::to_vec(&inner.to_value())?;
        let env = seal_multi(&plaintext, recipients, true)?;
        let mut ev = Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            id: String::new(),
            from: ANON_SENDER.to_string(),
            to: to.to_vec(),
            created_at: created_at.to_string(),
            kind: kind.to_string(),
            refs: Vec::new(),
            body: None,
            enc: None,
            enc_multi: Some(env),
            sig: String::new(),
        };
        ev.id = ev.core_id();
        Ok(ev)
    }

    /// True iff this is a sealed-sender event (the outer `from` is anonymized).
    pub fn is_sealed_sender(&self) -> bool {
        self.from == ANON_SENDER
            || self
                .enc_multi
                .as_ref()
                .map(|e| e.sealed_sender)
                .unwrap_or(false)
    }

    pub fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the event: the content id is intact AND the signature verifies
    /// against a key the **sender's** sigchain authorizes for signing. This is the
    /// single check behind "forged from fails" and "download ≠ impersonation".
    ///
    /// A **sealed-sender** event (anonymized outer `from`) carries no outer signature
    /// — call [`Self::open_sender_sealed`] and verify the recovered inner author
    /// instead. Calling `verify` on one is a usage error and bails loudly.
    pub fn verify(&self, sender_auth: &AuthorizedKeys) -> Result<()> {
        if self.is_sealed_sender() {
            bail!(
                "this is a sealed-sender event (anonymized outer from) — authenticate it \
                 with open_sender_sealed() + SenderSealed::verify(), not verify()"
            );
        }
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

    /// Open a sealed event with the recipient's custody-held encryption key. Handles
    /// both the per-recipient envelope (Wave 6; only a member of the `to`/ACL set can
    /// unwrap the CEK) and the single-recipient `enc` blob (Wave 3/4). A holder of the
    /// ciphertext **without** a listed recipient enc key cannot open either.
    pub fn open(&self, custodian: &Custodian) -> Result<String> {
        if let Some(env) = &self.enc_multi {
            let plaintext = open_multi(env, custodian)?;
            if env.sealed_sender {
                // The inner payload is a SenderSealed JSON, not raw body bytes.
                let inner: SenderSealed = serde_json::from_slice(&plaintext)
                    .context("parsing sealed-sender inner payload")?;
                // C3: refuse a relay-tampered outer envelope before returning the body.
                self.enforce_outer_binding(&inner)?;
                return Ok(inner.body);
            }
            return String::from_utf8(plaintext).context("sealed payload is not valid UTF-8");
        }
        let blob = self
            .enc
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("event is not sealed"))?;
        let plaintext = open_seal(blob, custodian)?;
        String::from_utf8(plaintext).context("sealed payload is not valid UTF-8")
    }

    /// Open a **sealed-sender** event, returning the recovered inner [`SenderSealed`]
    /// payload (the real author + signature). The caller must then resolve the inner
    /// `from`'s sigchain and call [`SenderSealed::verify`] to authenticate it — the
    /// relay could not forge it, but the payload is *unverified* until that check.
    pub fn open_sender_sealed(&self, custodian: &Custodian) -> Result<SenderSealed> {
        let env = self
            .enc_multi
            .as_ref()
            .filter(|e| e.sealed_sender)
            .ok_or_else(|| anyhow::anyhow!("event is not a sealed-sender envelope"))?;
        let plaintext = open_multi(env, custodian)?;
        let inner: SenderSealed =
            serde_json::from_slice(&plaintext).context("parsing sealed-sender inner payload")?;
        // C3: bind the recovered inner to the outer routing metadata BEFORE the caller
        // trusts either. A relay that rewrote the unsigned outer `to`/`created_at`/
        // `kind`/`refs` is caught here — the commitment is inside the signed inner, so
        // it cannot be re-forged without the sender's key.
        self.enforce_outer_binding(&inner)?;
        Ok(inner)
    }

    /// Enforce that a recovered sealed-sender `inner` commits to **this** event's outer
    /// routing metadata (audit C3). Cheap, pure, and called on every sealed-sender open.
    fn enforce_outer_binding(&self, inner: &SenderSealed) -> Result<()> {
        let expected =
            sealed_sender_outer_commitment(&self.to, &self.created_at, &self.kind, &self.refs);
        if inner.outer_commitment != expected {
            bail!(
                "sealed-sender outer routing metadata (to/created_at/kind/refs) does not \
                 match the signed inner commitment — relay tampering detected, refused \
                 (audit C3). Expected {expected}, inner committed {:?}",
                inner.outer_commitment
            );
        }
        Ok(())
    }
}

// ── Shared verification + sealing helpers ──────────────────────────────────────

/// Verify `sig_hex` over `value` (minus `sig`) against any key authorized to sign
/// for this identity (root or an active signer). The public entry point reused by
/// the transport node's write-auth (`super::node`) and [`super::transport::Head`]:
/// "this body was signed by a key the sigchain authorizes for this wgid".
pub fn verify_signed_value(
    value: &Value,
    sig_hex: &str,
    auth: &AuthorizedKeys,
    what: &str,
) -> Result<()> {
    verify_sig_against_authorized(value, sig_hex, auth, what)
}

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

/// HKDF-SHA256 a shared X25519 secret into a 32-byte XChaCha20 key under `info`
/// (domain separation: the single-recipient seal and the multi-recipient CEK-wrap
/// use distinct `info` labels so a key derived for one can never serve the other).
fn derive_key(shared: &[u8; 32], info: &[u8]) -> Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(None, shared);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    Ok(okm)
}

/// HKDF the single-recipient seal key (Wave 3/4 `enc` blob).
fn derive_seal_key(shared: &[u8; 32]) -> Result<[u8; 32]> {
    derive_key(shared, b"wg-fed-seal-v1")
}

/// HKDF the per-recipient CEK-wrapping key (Wave 6 `enc_multi`).
fn derive_wrap_key(shared: &[u8; 32]) -> Result<[u8; 32]> {
    derive_key(shared, b"wg-fed-cek-wrap-v1")
}

/// Fill `buf` with CSPRNG bytes or bail loudly.
fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::getrandom(buf).map_err(|e| anyhow::anyhow!("CSPRNG unavailable: {e}"))
}

/// XChaCha20-Poly1305 encrypt `plaintext` under `key`+`nonce`.
fn aead_encrypt(key: &[u8; 32], nonce: &[u8; 24], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    cipher
        .encrypt(XNonce::from_slice(nonce), plaintext)
        .map_err(|e| anyhow::anyhow!("AEAD encrypt failed: {e}"))
}

/// XChaCha20-Poly1305 decrypt `ciphertext` under `key`+`nonce`.
fn aead_decrypt(key: &[u8; 32], nonce: &[u8; 24], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {e}"))?;
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow::anyhow!("AEAD decrypt failed — wrong key or tampered ciphertext"))
}

/// Seal `plaintext` to a **set** of recipients (the ACL). The body is encrypted once
/// under a fresh CEK; the CEK is X25519-wrapped to each recipient. Only a holder of a
/// listed recipient encryption key can unwrap the CEK and decrypt (HQ4). `recipients`
/// is `(enc_kid, enc_pub)` pairs; an empty set is refused (a message no one can read).
fn seal_multi(
    plaintext: &[u8],
    recipients: &[(String, [u8; 32])],
    sealed_sender: bool,
) -> Result<SealedEnvelope> {
    if recipients.is_empty() {
        bail!("refusing to seal to an empty recipient set (the ACL would admit no one)");
    }
    let mut cek = [0u8; 32];
    fill_random(&mut cek)?;
    let mut body_nonce = [0u8; 24];
    fill_random(&mut body_nonce)?;
    let body_ct = aead_encrypt(&cek, &body_nonce, plaintext)?;

    let mut wraps = Vec::with_capacity(recipients.len());
    for (kid, enc_pub) in recipients {
        let mut eph_secret = [0u8; 32];
        fill_random(&mut eph_secret)?;
        let eph = StaticSecret::from(eph_secret);
        let eph_pub = XPublicKey::from(&eph).to_bytes();
        let shared = eph.diffie_hellman(&XPublicKey::from(*enc_pub));
        let key = derive_wrap_key(&shared.to_bytes())?;
        let mut nonce = [0u8; 24];
        fill_random(&mut nonce)?;
        let wrapped = aead_encrypt(&key, &nonce, &cek)?;
        wraps.push(RecipientWrap {
            recipient_kid: kid.clone(),
            ephemeral_pub: hex::encode(eph_pub),
            nonce: hex::encode(nonce),
            wrapped_cek: hex::encode(wrapped),
        });
    }
    Ok(SealedEnvelope {
        scheme: SEAL_CEK_SCHEME.to_string(),
        recipients: wraps,
        body_nonce: hex::encode(body_nonce),
        body_ct: hex::encode(body_ct),
        sealed_sender,
    })
}

/// Open a per-recipient envelope with the custody-held encryption key of *any* wrap
/// addressed to us. A holder of the ciphertext but no listed recipient key matches no
/// wrap and is locked out — the ACL boundary.
fn open_multi(env: &SealedEnvelope, custodian: &Custodian) -> Result<Vec<u8>> {
    if env.scheme != SEAL_CEK_SCHEME {
        bail!("unknown sealed-envelope scheme {:?}", env.scheme);
    }
    let body_nonce = decode_nonce(&env.body_nonce, "body nonce")?;
    let body_ct = hex::decode(&env.body_ct).context("bad body ciphertext hex")?;
    for wrap in &env.recipients {
        // Only attempt wraps whose recipient key we actually hold (the ACL check).
        if !custodian.has_key(&wrap.recipient_kid)? {
            continue;
        }
        let eph_pub = decode_pub(&wrap.ephemeral_pub).context("bad wrap ephemeral pub")?;
        // ECDH inside custody — the static enc secret never leaves.
        let shared = custodian.agree(&wrap.recipient_kid, &eph_pub)?;
        let key = derive_wrap_key(&shared)?;
        let nonce = decode_nonce(&wrap.nonce, "wrap nonce")?;
        let wrapped = hex::decode(&wrap.wrapped_cek).context("bad wrapped CEK hex")?;
        let cek_bytes = aead_decrypt(&key, &nonce, &wrapped)?;
        if cek_bytes.len() != 32 {
            bail!("unwrapped CEK is {} bytes, expected 32", cek_bytes.len());
        }
        let mut cek = [0u8; 32];
        cek.copy_from_slice(&cek_bytes);
        return aead_decrypt(&cek, &body_nonce, &body_ct);
    }
    bail!(
        "not in the recipient ACL set — this custody holds no encryption key for any \
         of the {} sealed recipients (a third party cannot decrypt, HQ4)",
        env.recipients.len()
    )
}

/// Decode a 24-byte hex nonce.
fn decode_nonce(nonce_hex: &str, what: &str) -> Result<[u8; 24]> {
    let b = hex::decode(nonce_hex).with_context(|| format!("bad {what} hex"))?;
    if b.len() != 24 {
        bail!("{what} is {} bytes, expected 24", b.len());
    }
    let mut out = [0u8; 24];
    out.copy_from_slice(&b);
    Ok(out)
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

    #[test]
    fn multi_recipient_only_acl_set_opens() {
        // Wave 6: the `to` set IS the ACL. Sender seals to {alice, bob}; both open,
        // a third party (carol) holding only her own key cannot.
        let (cust_sender, sender) = mint("sender_acl");
        let (cust_alice, alice) = mint("alice_acl");
        let (cust_bob, bob) = mint("bob_acl");
        let (cust_carol, _carol) = mint("carol_acl");

        let recipients = vec![
            (alice.enc_kid.clone(), alice.enc_pub),
            (bob.enc_kid.clone(), bob.enc_pub),
        ];
        let mut ev = SignedEvent::new_sealed_multi(
            &sender.wgid,
            &[alice.wgid.clone(), bob.wgid.clone()],
            "2026-06-25T00:00:00Z",
            "msg",
            "secret for the ACL",
            &recipients,
        )
        .unwrap();
        ev.sign(&cust_sender, &sender.signer_kid).unwrap();
        // The outer signature still authenticates the (non-anonymous) sender.
        assert!(ev.verify(&sender.auth).is_ok());

        // Every member of the ACL decrypts the SAME body.
        assert_eq!(ev.open(&cust_alice).unwrap(), "secret for the ACL");
        assert_eq!(ev.open(&cust_bob).unwrap(), "secret for the ACL");
        // A third party with the full ciphertext but no listed key is locked out.
        assert!(ev.open(&cust_carol).is_err());
        // The sender (also not in the ACL) cannot read it back either.
        assert!(ev.open(&cust_sender).is_err());
    }

    #[test]
    fn sealed_sender_hides_from_yet_recipient_authenticates() {
        let (cust_alice, alice) = mint("alice_ss");
        let (cust_bob, bob) = mint("bob_ss");
        let (_cust_mallory, mallory) = mint("mallory_ss");

        let recipients = vec![(bob.enc_kid.clone(), bob.enc_pub)];
        let ev = SignedEvent::new_sealed_sender(
            &alice.wgid,
            &[bob.wgid.clone()],
            "2026-06-25T00:00:00Z",
            "msg",
            "anonymous-to-the-relay secret",
            &recipients,
            &cust_alice,
            &alice.signer_kid,
        )
        .unwrap();

        // The relay/node sees only an anonymized outer `from`.
        assert_eq!(ev.from, ANON_SENDER);
        assert!(ev.is_sealed_sender());
        // verify() refuses a sealed-sender event (no outer signature to check).
        assert!(ev.verify(&alice.auth).is_err());

        // Bob unseals → learns the real author + a signature he can verify.
        let inner = ev.open_sender_sealed(&cust_bob).unwrap();
        assert_eq!(inner.from, alice.wgid);
        assert_eq!(inner.body, "anonymous-to-the-relay secret");
        assert!(inner.verify(&alice.auth).is_ok());
        // ...and verifying the inner author against the WRONG chain fails.
        assert!(inner.verify(&mallory.auth).is_err());
        // open() also yields the body for a sealed-sender event.
        assert_eq!(ev.open(&cust_bob).unwrap(), "anonymous-to-the-relay secret");

        // A forged inner author (mallory rewrites from→alice on her own payload) fails
        // the inner signature check.
        let mut forged = ev.open_sender_sealed(&cust_bob).unwrap();
        forged.from = alice.wgid.clone();
        forged.body = "i am alice".into();
        // No re-sign by alice's key is possible (mallory lacks it) → verify fails.
        assert!(forged.verify(&alice.auth).is_err());
    }

    #[test]
    fn sealed_sender_outer_metadata_tamper_is_detected() {
        // C3: the sealed-sender outer envelope carries no signature, so a relay could
        // rewrite the routing metadata (`to`/`created_at`/`kind`/`refs`). The commitment
        // folded into the signed inner + enforced on open catches every such tamper.
        let (cust_alice, alice) = mint("alice_c3");
        let (cust_bob, bob) = mint("bob_c3");

        let recipients = vec![(bob.enc_kid.clone(), bob.enc_pub)];
        let make = || {
            SignedEvent::new_sealed_sender(
                &alice.wgid,
                &[bob.wgid.clone()],
                "2026-06-25T00:00:00Z",
                "msg",
                "routing-bound secret",
                &recipients,
                &cust_alice,
                &alice.signer_kid,
            )
            .unwrap()
        };

        // Genuine event: opens cleanly and binds.
        let ev = make();
        assert_eq!(ev.open(&cust_bob).unwrap(), "routing-bound secret");
        assert!(ev.open_sender_sealed(&cust_bob).is_ok());

        // Relay tampers the outer `to` (re-routes to a different victim) — caught.
        let mut t_to = make();
        t_to.to = vec!["wgid:zMallory".into()];
        let err = t_to.open(&cust_bob).unwrap_err().to_string();
        assert!(err.contains("C3") || err.contains("tampering"), "{err}");
        assert!(t_to.open_sender_sealed(&cust_bob).is_err());

        // Relay tampers the outer `kind` — caught.
        let mut t_kind = make();
        t_kind.kind = "task-ref".into();
        assert!(t_kind.open_sender_sealed(&cust_bob).is_err());

        // Relay tampers the outer `created_at` — caught.
        let mut t_time = make();
        t_time.created_at = "2030-01-01T00:00:00Z".into();
        assert!(t_time.open(&cust_bob).is_err());

        // Relay injects outer `refs` not present in the signed commitment — caught.
        let mut t_refs = make();
        t_refs.refs = vec![serde_json::json!({"evil": true})];
        assert!(t_refs.open_sender_sealed(&cust_bob).is_err());
    }
}
