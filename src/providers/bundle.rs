//! The context bundle — build / seal / verify the minimal `ContextScope` slice over
//! WG-Fed crypto (ADR-E2 D4, the confidentiality TCB).
//!
//! The slice-builder is the confidentiality TCB: it must ship the **smallest slice
//! that lets the task work** (task T's input + its `--after` dependency artifacts) and
//! **nothing more** — a "minimal" slice that silently pulls in a transitive secret is
//! the X-2 residual. The sealed form reuses WG-Fed's **per-recipient sealed envelope**
//! ([`crate::identity::envelope::SignedEvent::new_sealed_multi`]) verbatim: the slice is
//! encrypted once under a fresh CEK wrapped to the provider's enrollment encryption key
//! — the recipient set *is* the ACL (Wave 6). A holder of the ciphertext without the
//! provider's enc key cannot read a byte. **No new crypto (NFR-4).**

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::context_scope::ContextScope;
use crate::identity::content_cid;
use crate::identity::envelope::SignedEvent;
use crate::identity::keys::Custodian;
use crate::identity::sigchain::AuthorizedKeys;

/// One `--after` dependency artifact carried in the slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepArtifact {
    pub task_id: String,
    pub artifact: String,
}

/// The cleartext context slice — the successor to today's `.wg/` symlink, bounded to a
/// tier (`Clean < Task < Graph < Full`). The default is `Task`: T's input + its
/// `--after` artifacts, **not** the whole graph (ADR-E2 D4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSlice {
    pub task_id: String,
    /// The tier label (reuses the one `ContextScope` enum — no second tier system).
    pub scope_tier: String,
    /// The task's own input/prompt.
    pub task_input: String,
    /// The artifacts of T's `--after` dependencies (and nothing else).
    #[serde(default)]
    pub after_artifacts: Vec<DepArtifact>,
}

impl ContextSlice {
    /// Build the minimal slice for a task (the default-smallest policy, D4).
    pub fn build(
        task_id: &str,
        scope_tier: ContextScope,
        task_input: &str,
        after_artifacts: Vec<DepArtifact>,
    ) -> Self {
        Self {
            task_id: task_id.to_string(),
            scope_tier: scope_tier.to_string(),
            task_input: task_input.to_string(),
            after_artifacts,
        }
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("ContextSlice serializes")
    }

    /// Content id of the cleartext slice — pinned in the [`SealedBundle`] so the provider
    /// (and the authorizer) can confirm the decrypted slice is exactly what was sealed.
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    /// A field-scan for **out-of-slice content**: does any token of `needle` appear in
    /// the cleartext slice? The spark seeds an out-of-slice secret into the broader
    /// graph and asserts it never appears here (the minimization / X-2 assertion).
    pub fn contains(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        serde_json::to_string(self)
            .unwrap_or_default()
            .contains(needle)
    }
}

/// The sealed bundle carried in a `RunGrant` (ADR-E2 D4). The slice is sealed to the
/// provider's enrollment encryption key via WG-Fed's per-recipient envelope; the
/// `slice_cid` pins the cleartext so a recipient verifies what it decrypts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedBundle {
    pub task_id: String,
    pub scope_tier: String,
    /// Content id of the cleartext slice (integrity pin).
    pub slice_cid: String,
    /// The per-recipient sealed envelope carrying the slice JSON — the ciphertext.
    pub event: SignedEvent,
}

impl SealedBundle {
    /// Build + seal a bundle: encrypt the slice to each recipient enc key (the ACL),
    /// signed by the authorizer's signer so the provider authenticates the sealer.
    ///
    /// `recipients` is `(enc_kid, enc_pub)` pairs — for the spark, the provider's single
    /// enrollment encryption key, resolved from its published sigchain. The seal reuses
    /// `SignedEvent::new_sealed_multi` verbatim (no new crypto).
    pub fn seal(
        slice: &ContextSlice,
        authorizer_wgid: &str,
        provider_wgid: &str,
        custodian: &Custodian,
        signer_kid: &str,
        recipients: &[(String, [u8; 32])],
        created_at: &str,
    ) -> Result<Self> {
        if recipients.is_empty() {
            bail!("cannot seal a bundle to an empty recipient set (no provider enc key)");
        }
        let body = serde_json::to_string(slice).context("serializing context slice")?;
        let mut event = SignedEvent::new_sealed_multi(
            authorizer_wgid,
            &[provider_wgid.to_string()],
            created_at,
            "exec-context-bundle",
            &body,
            recipients,
        )?;
        event.sign(custodian, signer_kid)?;
        Ok(Self {
            task_id: slice.task_id.clone(),
            scope_tier: slice.scope_tier.clone(),
            slice_cid: slice.cid(),
            event,
        })
    }

    /// Verify the sealer's signature against the authorizer's authorized key set. The
    /// provider authenticates *who sealed* the bundle before opening it.
    pub fn verify_sealer(&self, authorizer_auth: &AuthorizedKeys) -> Result<()> {
        self.event.verify(authorizer_auth)
    }

    /// Open the bundle with the provider's custody-held encryption key and confirm the
    /// decrypted slice matches the pinned `slice_cid` (no swap). Only a recipient in the
    /// ACL can decrypt; a third party holding the ciphertext cannot (encryption = ACL).
    pub fn open(&self, custodian: &Custodian) -> Result<ContextSlice> {
        let body = self
            .event
            .open(custodian)
            .context("opening sealed context bundle")?;
        let slice: ContextSlice =
            serde_json::from_str(&body).context("parsing decrypted context slice")?;
        if slice.cid() != self.slice_cid {
            bail!(
                "decrypted slice cid {} != pinned slice_cid {} (bundle tampered or swapped)",
                slice.cid(),
                self.slice_cid
            );
        }
        Ok(slice)
    }

    /// Whether the *sealed bytes on the wire* leak any plaintext of `needle`. The body is
    /// ciphertext, so a secret in the slice never appears in the delivered bundle — the
    /// confidentiality-in-transit assertion (FR-D2). Returns `true` only if it leaked.
    pub fn wire_leaks(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        serde_json::to_string(self)
            .unwrap_or_default()
            .contains(needle)
    }
}

/// Resolve a provider's active encryption key `(kid, pub)` from its verified sigchain —
/// the recipient the bundle seals to. Reuses the WG-Fed key model (no new crypto).
pub fn recipient_enc_key(provider_auth: &AuthorizedKeys) -> Result<(String, [u8; 32])> {
    let enc = provider_auth
        .active_enc()
        .ok_or_else(|| anyhow::anyhow!("provider has no active encryption key to seal to"))?;
    let bytes = hex::decode(&enc.public).context("provider enc pubkey not hex")?;
    if bytes.len() != 32 {
        bail!("provider enc pubkey is {} bytes, expected 32", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok((enc.kid.clone(), out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys;
    use crate::identity::sigchain::{
        KeyEntry, KeyRole, KeyStatus, add_key, genesis, verify as verify_chain,
    };

    struct Party {
        wgid: String,
        cust: Custodian,
        signer_kid: String,
        enc_kid: String,
        enc_pub: [u8; 32],
        auth: AuthorizedKeys,
    }

    fn mint(name: &str) -> Party {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let cust = Custodian::with_keystore_dir(name, dir);
        let root = keys::gen_ed25519().unwrap();
        let root_kid = keys::kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = keys::wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();

        let signer = keys::gen_ed25519().unwrap();
        let signer_kid = keys::kid_for(&signer.public);
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

        let enc = keys::gen_x25519().unwrap();
        let enc_kid = keys::kid_for(&enc.public);
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
                scope: vec!["seal".into()],
                status: KeyStatus::Active,
            },
        )
        .unwrap();
        let auth = verify_chain(&[g, l1, l2], &wgid).unwrap();
        Party {
            wgid,
            cust,
            signer_kid,
            enc_kid,
            enc_pub: enc.public,
            auth,
        }
    }

    #[test]
    fn seal_then_open_roundtrips_and_pins_cid() {
        let alice = mint("alice_bundle");
        let provider = mint("provider_bundle");
        let slice = ContextSlice::build(
            "T",
            ContextScope::Task,
            "implement the parser",
            vec![DepArtifact {
                task_id: "dep1".into(),
                artifact: "grammar.md".into(),
            }],
        );
        let bundle = SealedBundle::seal(
            &slice,
            &alice.wgid,
            &provider.wgid,
            &alice.cust,
            &alice.signer_kid,
            &[(provider.enc_kid.clone(), provider.enc_pub)],
            "2026-06-26T00:00:00Z",
        )
        .unwrap();
        // The provider authenticates the sealer, then opens.
        bundle.verify_sealer(&alice.auth).unwrap();
        let opened = bundle.open(&provider.cust).unwrap();
        assert_eq!(opened, slice);
        assert_eq!(bundle.scope_tier, "task");
    }

    #[test]
    fn out_of_slice_secret_never_appears_in_wire_or_cleartext() {
        let alice = mint("alice_min");
        let provider = mint("provider_min");
        // The minimal slice carries ONLY T's input + its dep artifact.
        let slice = ContextSlice::build("T", ContextScope::Task, "do T", vec![]);
        let bundle = SealedBundle::seal(
            &slice,
            &alice.wgid,
            &provider.wgid,
            &alice.cust,
            &alice.signer_kid,
            &[(provider.enc_kid.clone(), provider.enc_pub)],
            "2026-06-26T00:00:00Z",
        )
        .unwrap();
        let out_of_slice_secret = "GRAPH_WIDE_SECRET_sk-XYZ";
        // It is not in the cleartext slice (minimization)…
        assert!(!slice.contains(out_of_slice_secret));
        // …and it is not in the sealed wire bytes (encryption in transit).
        assert!(!bundle.wire_leaks(out_of_slice_secret));
    }

    #[test]
    fn third_party_cannot_open_a_bundle_not_addressed_to_it() {
        let alice = mint("alice_acl");
        let provider = mint("provider_acl");
        let thief = mint("thief_acl");
        let slice = ContextSlice::build("T", ContextScope::Task, "secret task body", vec![]);
        let bundle = SealedBundle::seal(
            &slice,
            &alice.wgid,
            &provider.wgid,
            &alice.cust,
            &alice.signer_kid,
            &[(provider.enc_kid.clone(), provider.enc_pub)],
            "2026-06-26T00:00:00Z",
        )
        .unwrap();
        // The provider can open; the thief (not in the ACL) cannot.
        assert!(bundle.open(&provider.cust).is_ok());
        assert!(bundle.open(&thief.cust).is_err());
    }

    #[test]
    fn recipient_enc_key_resolves_from_sigchain() {
        let provider = mint("provider_enc");
        let (kid, pubk) = recipient_enc_key(&provider.auth).unwrap();
        assert_eq!(kid, provider.enc_kid);
        assert_eq!(pubk, provider.enc_pub);
    }
}
