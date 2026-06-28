//! Key generation, the `wgid:` address encoding, and the **custody boundary**.
//!
//! The custody boundary is the load-bearing security property of the whole spark
//! (ADR-fed-003 §D1, the headline assertion of the milestone). It is an
//! ssh-agent-style "use a key without holding it" service over `wg secret`:
//!
//! - Private keys (root ed25519, signer ed25519, encryption X25519) are stored in
//!   the `wg secret` **keystore** (`~/.wg/keystore/<name>`, 0600). Because the
//!   keystore is `$HOME`-relative, two WG instances on one host are isolated by
//!   `HOME` alone — exactly what the two-graph spark needs.
//! - A caller may only [`Custodian::sign_digest`] (sign a 32-byte digest) or
//!   [`Custodian::agree`] (X25519 ECDH for sealing). **No method ever returns a
//!   private key.** The root in particular signs sigchain links and is otherwise
//!   never touched.
//!
//! This is why "download ≠ impersonation": a thief who copies the published
//! `IdentityRecord` gets only public material; authoring as that identity requires
//! a `sign_digest` against a keystore entry the thief's custodian does not hold.

use anyhow::{Context, Result, anyhow, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::path::PathBuf;
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

use crate::secret;

/// Varint-encoded multicodec prefix for `ed25519-pub` (`0xed`), i.e. the two
/// bytes `did:key` prepends before base58btc encoding (ADR-fed-001 §OQ1).
pub const MULTICODEC_ED25519_PUB: [u8; 2] = [0xed, 0x01];

/// A freshly generated ed25519 keypair. `seed` is the 32-byte secret scalar seed;
/// `public` is the 32-byte verifying key. The seed is handed straight to the
/// custodian and never kept in the clear beyond minting.
pub struct Ed25519Keypair {
    pub seed: [u8; 32],
    pub public: [u8; 32],
}

/// A freshly generated X25519 (static) keypair for per-recipient sealing.
pub struct X25519Keypair {
    pub secret: [u8; 32],
    pub public: [u8; 32],
}

/// Fill `buf` with cryptographically secure random bytes.
fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::getrandom(buf).map_err(|e| anyhow!("CSPRNG unavailable: {e}"))
}

/// Warn once (per process) that custody seeds are stored plaintext-at-rest because no
/// KEK is available (audit M1). **Opt-in** via `WG_FED_WARN_PLAINTEXT_KEYS` so it never
/// pollutes a JSON/stdout consumer by default; the state is always shown by
/// `wg secret backend show`. Production should set `WG_FED_KEYSTORE_PASSPHRASE` or run
/// on a host with a reachable OS keyring.
fn warn_no_at_rest_kek_once() {
    if std::env::var("WG_FED_WARN_PLAINTEXT_KEYS").is_err() {
        return;
    }
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        eprintln!(
            "Warning: WG-Fed custody keys are stored PLAINTEXT at rest — no key-encryption \
             key available (no WG_FED_KEYSTORE_PASSPHRASE and no reachable OS keyring). \
             A reader of ~/.wg/keystore obtains the root key. Set \
             WG_FED_KEYSTORE_PASSPHRASE to encrypt seeds at rest (audit M1/A3)."
        );
    });
}

/// Generate a fresh ed25519 keypair (root or signer tier).
pub fn gen_ed25519() -> Result<Ed25519Keypair> {
    let mut seed = [0u8; 32];
    fill_random(&mut seed)?;
    let sk = SigningKey::from_bytes(&seed);
    let public = sk.verifying_key().to_bytes();
    Ok(Ed25519Keypair { seed, public })
}

/// Generate a fresh X25519 static keypair (encryption tier).
pub fn gen_x25519() -> Result<X25519Keypair> {
    let mut secret = [0u8; 32];
    fill_random(&mut secret)?;
    let ss = StaticSecret::from(secret);
    let public = XPublicKey::from(&ss).to_bytes();
    Ok(X25519Keypair { secret, public })
}

/// Verify an ed25519 signature over a digest against a raw public key.
///
/// Uses `verify_strict` (rejects small-order points / malleable encodings). This
/// is the *only* operation needed to authenticate any signed WG-Fed artifact, and
/// it is a pure local check — no network, no central authority (ADR-fed-001 §D5).
pub fn verify_sig(pubkey: &[u8; 32], digest: &[u8; 32], sig: &[u8; 64]) -> bool {
    let vk = match VerifyingKey::from_bytes(pubkey) {
        Ok(vk) => vk,
        Err(_) => return false,
    };
    let signature = Signature::from_bytes(sig);
    vk.verify_strict(digest, &signature).is_ok()
}

/// Derive a short, stable key id from a public key: first 16 hex chars of
/// `blake3(pub)`. Distinct tiers (root/signer/enc) get distinct kids.
pub fn kid_for(public: &[u8; 32]) -> String {
    let h = blake3::hash(public);
    hex::encode(&h.as_bytes()[..8])
}

// ── wgid / did:key encoding ────────────────────────────────────────────────────

/// Encode a raw ed25519 public key as the multibase body shared by `wgid:` and
/// `did:key:` — `z` (base58btc) over `0xed 0x01 ++ pubkey` (ADR-fed-001 §OQ1).
fn multibase_body(public: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&MULTICODEC_ED25519_PUB);
    bytes.extend_from_slice(public);
    format!("z{}", bs58::encode(bytes).into_string())
}

/// Render a public key as the canonical `wgid:<multibase>` address.
pub fn wgid_from_pubkey(public: &[u8; 32]) -> String {
    format!("wgid:{}", multibase_body(public))
}

/// Render a public key as `did:key:<multibase>` — byte-identical body to `wgid:`,
/// offered for interop with external DID verifiers (ADR-fed-001 §OQ2). `did:key`
/// is the *anchor* only; it carries no sigchain.
pub fn didkey_from_pubkey(public: &[u8; 32]) -> String {
    format!("did:key:{}", multibase_body(public))
}

/// Decode a multibase body (`z…` base58btc canonical, or `b…` base32-lower per the
/// liberal-acceptance rule) into its raw bytes.
fn multibase_decode(body: &str) -> Result<Vec<u8>> {
    let mut chars = body.chars();
    let base = chars
        .next()
        .ok_or_else(|| anyhow!("empty multibase body"))?;
    let rest: String = chars.collect();
    match base {
        'z' => bs58::decode(rest.as_bytes())
            .into_vec()
            .context("invalid base58btc (z) multibase body"),
        'b' => base32_lower_decode(&rest).context("invalid base32 (b) multibase body"),
        other => bail!(
            "unsupported multibase prefix {other:?}; WG-Fed accepts 'z' (base58btc, \
             canonical) and 'b' (base32-lower). An npub (bech32) is never a wgid."
        ),
    }
}

/// Parse a `wgid:` (or, liberally, a `did:key:`) string into the raw 32-byte
/// ed25519 public key it names. The pubkey IS the address (self-certifying), so
/// this is all a verifier needs to root a signature check.
pub fn pubkey_from_wgid(s: &str) -> Result<[u8; 32]> {
    let body = if let Some(rest) = s.strip_prefix("wgid:") {
        rest
    } else if let Some(rest) = s.strip_prefix("did:key:") {
        rest
    } else {
        bail!("not a wgid:/did:key: address: {s:?}");
    };
    let bytes = multibase_decode(body)?;
    if bytes.len() != 2 + 32 {
        bail!(
            "decoded {} bytes, expected {} (multicodec prefix + 32-byte ed25519 key)",
            bytes.len(),
            2 + 32
        );
    }
    if bytes[0..2] != MULTICODEC_ED25519_PUB {
        bail!(
            "multicodec prefix {:02x?} is not ed25519-pub ({:02x?}); a non-ed25519 \
             key cannot be a spark wgid",
            &bytes[0..2],
            MULTICODEC_ED25519_PUB
        );
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&bytes[2..]);
    Ok(pubkey)
}

/// Minimal RFC4648 base32 (lowercase, no padding) decoder for the liberal `b`
/// multibase acceptance (ADR-fed-001 §OQ1). The canonical emit form is base58btc.
fn base32_lower_decode(s: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase() as u8;
        let val = ALPHABET
            .iter()
            .position(|&a| a == c)
            .ok_or_else(|| anyhow!("invalid base32 character {ch:?}"))? as u32;
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

// ── The custody boundary over `wg secret` ──────────────────────────────────────

/// Tag stored secrets by key kind so a signer key can never be mistaken for an
/// encryption key (and vice versa) at use time.
const SIGN_TAG: &str = "ed25519:";
const SEAL_TAG: &str = "x25519:";

/// Keystore entry name for `(identity, kid)`. The `wgfed.` namespace keeps these
/// distinct from API-key secrets; `.` is a legal secret-name character.
fn entry_name(identity: &str, kid: &str) -> String {
    format!("wgfed.{identity}.{kid}")
}

/// The ssh-agent-style signing/agreement service for one identity.
///
/// Holds only the identity *name* (and, in tests, an injected keystore dir);
/// every private key stays in the `wg secret` keystore and is loaded transiently
/// inside a single operation, never returned.
pub struct Custodian {
    identity: String,
    /// Override keystore directory. `None` = the real `wg secret` keystore
    /// (`~/.wg/keystore`). Tests inject a unique dir so they need not mutate the
    /// process-global `$HOME` (not parallel-test-safe).
    keystore_dir: Option<PathBuf>,
    /// At-rest key-encryption key (audit M1/A3). When `Some`, every stored seed is
    /// wrapped in an XChaCha20-Poly1305 envelope before it touches disk and decrypted
    /// transiently on use, so the keystore file holds only ciphertext. `None` = legacy
    /// plaintext-at-rest (no KEK available; a loud warning is emitted once).
    kek: Option<[u8; 32]>,
}

impl Custodian {
    /// Bind a custodian to an identity name (the keystore namespace). Uses the real
    /// `wg secret` keystore and **encrypts seeds at rest** under a KEK resolved from a
    /// separate trust domain (operator passphrase or OS keyring; audit M1). When no KEK
    /// is available it warns once and falls back to the legacy plaintext store.
    pub fn new(identity: &str) -> Self {
        let kek = secret::at_rest::resolve_kek();
        if kek.is_none() {
            // Plaintext-at-rest fallback (no KEK). Off by default so it never pollutes
            // JSON/stdout-captured callers; opt in with `WG_FED_WARN_PLAINTEXT_KEYS=1`.
            // The at-rest protection state is always discoverable via
            // `wg secret backend show`. (audit M1/A3)
            warn_no_at_rest_kek_once();
        }
        Self {
            identity: identity.to_string(),
            keystore_dir: None,
            kek,
        }
    }

    /// Bind a custodian to an explicit keystore directory (test/isolation use).
    /// Plaintext-at-rest (no KEK) — for at-rest tests use [`Custodian::with_keystore_dir_kek`].
    pub fn with_keystore_dir(identity: &str, dir: PathBuf) -> Self {
        Self {
            identity: identity.to_string(),
            keystore_dir: Some(dir),
            kek: None,
        }
    }

    /// Bind a custodian to an explicit keystore directory **with at-rest encryption**
    /// under `kek` (test/isolation use — proves the M1 at-rest property hermetically).
    pub fn with_keystore_dir_kek(identity: &str, dir: PathBuf, kek: [u8; 32]) -> Self {
        Self {
            identity: identity.to_string(),
            keystore_dir: Some(dir),
            kek: Some(kek),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        // Encrypt the seed at rest when a KEK is configured (audit M1); otherwise store
        // the legacy plaintext value.
        let to_store = match &self.kek {
            Some(kek) => secret::at_rest::seal_at_rest(kek, value.as_bytes())?,
            None => value.to_string(),
        };
        match &self.keystore_dir {
            Some(dir) => secret::keystore_set_in(dir, name, &to_store),
            None => secret::keystore_set(name, &to_store),
        }
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        let raw = match &self.keystore_dir {
            Some(dir) => secret::keystore_get_in(dir, name)?,
            None => secret::keystore_get(name)?,
        };
        let Some(stored) = raw else {
            return Ok(None);
        };
        // Transparently decrypt an at-rest envelope (audit M1); a legacy plaintext entry
        // (no `aead1:` prefix) is returned verbatim for back-compat.
        if secret::at_rest::is_at_rest(&stored) {
            let kek = self
                .kek
                .ok_or_else(|| anyhow!("keystore entry {name} is encrypted at rest but no KEK is available — set WG_FED_KEYSTORE_PASSPHRASE"))?;
            let pt = secret::at_rest::open_at_rest(&kek, &stored)
                .with_context(|| format!("decrypting at-rest custody entry {name}"))?;
            let s = String::from_utf8(pt)
                .with_context(|| format!("at-rest custody entry {name} is not UTF-8"))?;
            return Ok(Some(s));
        }
        Ok(Some(stored))
    }

    /// Store an ed25519 signing-key seed (root or signer tier) under its kid.
    ///
    /// Used only at mint time. After this returns, the seed should be dropped by
    /// the caller; the only way to use it again is [`Custodian::sign_digest`].
    pub fn store_signing_key(&self, kid: &str, seed: &[u8; 32]) -> Result<()> {
        let value = format!("{SIGN_TAG}{}", hex::encode(seed));
        self.set(&entry_name(&self.identity, kid), &value)
    }

    /// Store an X25519 static secret (encryption tier) under its kid.
    pub fn store_sealing_key(&self, kid: &str, secret_key: &[u8; 32]) -> Result<()> {
        let value = format!("{SEAL_TAG}{}", hex::encode(secret_key));
        self.set(&entry_name(&self.identity, kid), &value)
    }

    /// Whether this custodian holds a key for `kid`. The impersonation defense:
    /// `wg identity send --from <name>` checks this before attempting to author.
    pub fn has_key(&self, kid: &str) -> Result<bool> {
        Ok(self.get(&entry_name(&self.identity, kid))?.is_some())
    }

    /// Load a raw 32-byte secret of the expected `tag`, erroring if absent or of
    /// the wrong kind. Private to the module — callers never see the bytes.
    fn load_secret(&self, kid: &str, tag: &str) -> Result<[u8; 32]> {
        let name = entry_name(&self.identity, kid);
        let stored = self.get(&name)?.ok_or_else(|| {
            anyhow!(
                "no key {kid} in custody for identity {:?} — the custodian holds no \
                 private key to authorize this (download ≠ impersonation, \
                 ADR-fed-003 §D1)",
                self.identity
            )
        })?;
        let hexed = stored
            .strip_prefix(tag)
            .ok_or_else(|| anyhow!("custody entry {name} is not a {tag} key"))?;
        let bytes = hex::decode(hexed).with_context(|| format!("corrupt custody entry {name}"))?;
        if bytes.len() != 32 {
            bail!(
                "custody entry {name} has {} bytes, expected 32",
                bytes.len()
            );
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// Sign a 32-byte digest with the ed25519 key `kid` (the "sign this digest"
    /// boundary). Returns only the 64-byte signature; the private key never leaves.
    pub fn sign_digest(&self, kid: &str, digest: &[u8; 32]) -> Result<[u8; 64]> {
        let seed = self.load_secret(kid, SIGN_TAG)?;
        let sk = SigningKey::from_bytes(&seed);
        let sig: Signature = sk.sign(digest);
        Ok(sig.to_bytes())
    }

    /// X25519 ECDH between the custody-held key `kid` and a peer public key,
    /// returning the raw shared secret for HKDF (sealing). The static secret never
    /// leaves; only the shared point is returned.
    pub fn agree(&self, kid: &str, their_public: &[u8; 32]) -> Result<[u8; 32]> {
        let secret_key = self.load_secret(kid, SEAL_TAG)?;
        let ss = StaticSecret::from(secret_key);
        let shared = ss.diffie_hellman(&XPublicKey::from(*their_public));
        Ok(shared.to_bytes())
    }

    /// Delete every key this custodian holds (test/cleanup affordance).
    pub fn forget(&self, kids: &[String]) -> Result<()> {
        for kid in kids {
            let name = entry_name(&self.identity, kid);
            match &self.keystore_dir {
                Some(dir) => secret::keystore_delete_in(dir, &name)?,
                None => secret::keystore_delete(&name)?,
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        let kp = gen_ed25519().unwrap();
        let digest = *blake3::hash(b"hello wg-fed").as_bytes();
        let sk = SigningKey::from_bytes(&kp.seed);
        let sig = sk.sign(&digest).to_bytes();
        assert!(verify_sig(&kp.public, &digest, &sig));
        // A different digest must fail.
        let other = *blake3::hash(b"tampered").as_bytes();
        assert!(!verify_sig(&kp.public, &other, &sig));
    }

    #[test]
    fn wgid_roundtrip_and_didkey_share_body() {
        let kp = gen_ed25519().unwrap();
        let wgid = wgid_from_pubkey(&kp.public);
        assert!(wgid.starts_with("wgid:z6Mk"), "wgid was {wgid}");
        let didkey = didkey_from_pubkey(&kp.public);
        assert_eq!(
            wgid.strip_prefix("wgid:").unwrap(),
            didkey.strip_prefix("did:key:").unwrap(),
            "wgid and did:key must be a pure prefix swap (OQ1)"
        );
        // Round-trips back to the same key, and a did:key parses identically.
        assert_eq!(pubkey_from_wgid(&wgid).unwrap(), kp.public);
        assert_eq!(pubkey_from_wgid(&didkey).unwrap(), kp.public);
    }

    #[test]
    fn wgid_base32_b_form_accepted() {
        // The 'b' (base32-lower) rendering must decode to the identical key.
        let kp = gen_ed25519().unwrap();
        let mut payload = Vec::new();
        payload.extend_from_slice(&MULTICODEC_ED25519_PUB);
        payload.extend_from_slice(&kp.public);
        let b32 = base32_lower_encode_for_test(&payload);
        let wgid_b = format!("wgid:b{b32}");
        assert_eq!(pubkey_from_wgid(&wgid_b).unwrap(), kp.public);
    }

    #[test]
    fn signing_and_sealing_keys_encrypted_at_rest() {
        // Audit M1/A3: a custodian with a KEK must NOT write plaintext seeds to disk;
        // sign/agree still work (transparent decrypt); a wrong KEK cannot read them.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let kek = [7u8; 32];
        let cust = Custodian::with_keystore_dir_kek("atrest", dir.clone(), kek);

        // Store a signing seed and an encryption secret.
        let kp = gen_ed25519().unwrap();
        let sign_kid = kid_for(&kp.public);
        cust.store_signing_key(&sign_kid, &kp.seed).unwrap();
        let xk = gen_x25519().unwrap();
        let enc_kid = kid_for(&xk.public);
        cust.store_sealing_key(&enc_kid, &xk.secret).unwrap();

        // The raw keystore files must be at-rest AEAD envelopes with NO plaintext seed.
        let sign_raw = std::fs::read_to_string(dir.join(entry_name("atrest", &sign_kid))).unwrap();
        assert!(
            sign_raw.starts_with("aead1:"),
            "expected an at-rest envelope, got {sign_raw}"
        );
        assert!(
            !sign_raw.contains(&hex::encode(kp.seed)),
            "signing seed must NOT be plaintext-at-rest"
        );
        let enc_raw = std::fs::read_to_string(dir.join(entry_name("atrest", &enc_kid))).unwrap();
        assert!(
            !enc_raw.contains(&hex::encode(xk.secret)),
            "encryption secret must NOT be plaintext-at-rest"
        );

        // sign_digest works through the transparent decrypt.
        let digest = *blake3::hash(b"at-rest works").as_bytes();
        let sig = cust.sign_digest(&sign_kid, &digest).unwrap();
        assert!(verify_sig(&kp.public, &digest, &sig));
        // agree() (ECDH for sealing) also works through the decrypt.
        assert!(cust.agree(&enc_kid, &xk.public).is_ok());

        // A custodian holding the WRONG KEK cannot use the keys (AEAD tag fails).
        let wrong = Custodian::with_keystore_dir_kek("atrest", dir.clone(), [9u8; 32]);
        assert!(
            wrong.sign_digest(&sign_kid, &digest).is_err(),
            "wrong KEK must not decrypt the seed"
        );

        // A legacy plaintext entry (no KEK custodian) is still readable by a KEK custodian
        // (back-compat): write plaintext, then read through the at-rest custodian.
        let legacy = Custodian::with_keystore_dir("legacy", dir.clone());
        let lkp = gen_ed25519().unwrap();
        let lkid = kid_for(&lkp.public);
        legacy.store_signing_key(&lkid, &lkp.seed).unwrap();
        let read_back = Custodian::with_keystore_dir_kek("legacy", dir.clone(), kek);
        assert!(
            read_back.sign_digest(&lkid, &digest).is_ok(),
            "a KEK custodian must still read a legacy plaintext entry"
        );
    }

    #[test]
    fn flipped_byte_breaks_wgid() {
        let kp = gen_ed25519().unwrap();
        let wgid = wgid_from_pubkey(&kp.public);
        let parsed = pubkey_from_wgid(&wgid).unwrap();
        let mut tampered = parsed;
        tampered[0] ^= 0x01;
        assert_ne!(tampered, kp.public);
        assert_ne!(wgid_from_pubkey(&tampered), wgid);
    }

    // Tiny base32 lower encoder, test-only, to exercise the decoder.
    fn base32_lower_encode_for_test(data: &[u8]) -> String {
        const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
        let mut out = String::new();
        let mut buffer: u32 = 0;
        let mut bits: u32 = 0;
        for &b in data {
            buffer = (buffer << 8) | b as u32;
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
        }
        out
    }
}
