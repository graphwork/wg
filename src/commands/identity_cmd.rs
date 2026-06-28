//! `wg identity` — the WG-Fed identity surface (Wave 3 spark + Wave 4 transport).
//!
//! Implements minting a self-certifying identity whose root never leaves custody;
//! publish/fetch of a self-verifying `IdentityRecord` + `StateSnapshot`; and
//! send/poll of a signed (optionally sealed) cross-graph `SignedEvent` over a
//! store-and-forward inbox.
//!
//! Wave 4 (ADR-fed-002) generalizes the `--store <L>` argument: `L` is now any
//! [`FedStore`] rung — a dumb directory **or** an `http://` WG node inbox — opened by
//! [`open_store`]. The same signed/sealed bytes traverse either, and the recipient's
//! offline self-verification is unchanged (verification is never central, ADR-fed-001
//! §D5). Wave 4 also adds **freshness attestations** (S-3): `publish`/`attest` emit a
//! signed `valid-as-of T, expires T+Δ` over the current head, and `check-fresh`
//! re-fetches it and **fails closed on stale** for high-value actions.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use worksgood::identity::custody::{self, LeashPolicy, Revocation, Scope};
use worksgood::identity::envelope::{
    AgentFields, Endpoint, IdentityRecord, SignedEvent, StateSnapshot, payload_cid,
};
use worksgood::identity::freshness::{
    ActionClass, FreshVerdict, FreshnessAttestation, ROUTINE_DELTA_SECS, check_fresh,
    load_seen_seq, record_seen_seq,
};
use worksgood::identity::keys::{self, Custodian};
use worksgood::identity::sigchain::{
    self, AuthorizedKeys, KeyEntry, KeyRole, KeyStatus, SigchainLink,
};
use worksgood::identity::state_safety::{
    KindClass, LoadDecision, ScanResult, classify_kind, evaluate, parse_trust, scan_transparent,
};
use worksgood::identity::transport::{FedStore, Head, open_store};
use worksgood::identity::{ALG_ED25519, ENVELOPE_V, WG_FED_COMPAT_VERSION};
use worksgood::review::{
    ContentClass, Provenance, Sensitivity, VerdictStore, review_inbound, review_inbound_ctx,
};
use worksgood::trust::{resolve_author_trust, strictest_trust};

/// Reserved store "inbox" the C-tier revocation-list convenience publishes signed
/// [`Revocation`]s into (Wave 6). It is a discovery *hint* only — `verify-cap` always
/// re-checks each revocation's signature and authorization, never trusting the store.
const REVOCATION_INBOX: &str = "wgfed:revocations";

// ── Local identity state (public; private keys live only in custody) ───────────

/// Everything WG needs locally to *use* an identity it minted, or to *reference*
/// one it fetched. Private keys are **never** here — they live in `wg secret`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LocalIdentity {
    /// Local handle (the `wg identity new <name>` name).
    name: String,
    wgid: String,
    /// Key ids for custody lookups (derivable from pubkeys; cached for clarity).
    root_kid: String,
    signer_kid: String,
    enc_kid: String,
    /// True if this WG minted it and holds its private keys in custody; false for
    /// a downloaded (key-less) bundle. The latter cannot author — the spark's
    /// "download ≠ impersonation" boundary (ADR-fed-003 §D1).
    holds_private: bool,
    /// The **active** root kid after any `rotate_root`/recovery (Wave 5). Empty for
    /// pre-Wave-5 records ⇒ falls back to `root_kid` (the genesis root). Use
    /// [`LocalIdentity::cur_root_kid`].
    #[serde(default)]
    active_root_kid: String,
    /// The offline recovery key's kid in custody, if one was minted at genesis
    /// (`wg identity new --recovery`). `None` for agents anchored to a custodian.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_kid: Option<String>,
    record: IdentityRecord,
    sigchain: Vec<SigchainLink>,
    /// Highest freshness-attestation `seq` this identity has *issued* (monotonic).
    /// Bumped on every `publish`/`attest` so a verifier can detect rollback.
    #[serde(default)]
    freshness_seq: u64,
}

impl LocalIdentity {
    /// The currently-active root kid (Wave 5): `active_root_kid` if set, else the
    /// genesis `root_kid` (back-compat for records minted before rotation existed).
    fn cur_root_kid(&self) -> String {
        if self.active_root_kid.is_empty() {
            self.root_kid.clone()
        } else {
            self.active_root_kid.clone()
        }
    }

    // ── Accessors for the WG-Exec provider plane (`src/providers/`, exec_fed_cmd) ──
    // The execution plane reuses these saved identities verbatim — no second identity
    // system (NFR-4). Read-only views; private keys stay in `wg secret` custody.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn wgid(&self) -> &str {
        &self.wgid
    }
    pub(crate) fn signer_kid(&self) -> &str {
        &self.signer_kid
    }
    pub(crate) fn enc_kid(&self) -> &str {
        &self.enc_kid
    }
    pub(crate) fn sigchain(&self) -> &[SigchainLink] {
        &self.sigchain
    }
    /// Replay this identity's sigchain to its authorized key set (offline self-verify).
    pub(crate) fn auth(&self) -> Result<AuthorizedKeys> {
        sigchain::verify(&self.sigchain, &self.wgid)
    }
}

fn identity_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("identity")
}

fn local_path(workgraph_dir: &Path, name: &str) -> PathBuf {
    identity_dir(workgraph_dir).join(format!("{name}.json"))
}

/// Dir tracking the highest freshness `seq` a verifier has *seen* per identity (the
/// rollback backstop, distinct from the per-identity *issued* seq above).
fn freshness_seen_dir(workgraph_dir: &Path) -> PathBuf {
    identity_dir(workgraph_dir).join("freshness_seen")
}

fn save_local(workgraph_dir: &Path, id: &LocalIdentity) -> Result<()> {
    let dir = identity_dir(workgraph_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = local_path(workgraph_dir, &id.name);
    let json = serde_json::to_string_pretty(id)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_local(workgraph_dir: &Path, name: &str) -> Result<LocalIdentity> {
    let path = local_path(workgraph_dir, name);
    let json = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no local identity {name:?} ({}). Mint one with `wg identity new {name}` \
             or fetch one with `wg identity fetch <wgid> --save {name}`.",
            path.display()
        )
    })?;
    serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
}

/// Scan the identity dir for a saved (own or fetched) bundle whose wgid matches —
/// the local "cached signed endpoint record" used for offline sender authentication.
fn load_local_by_wgid(workgraph_dir: &Path, wgid: &str) -> Option<LocalIdentity> {
    let dir = identity_dir(workgraph_dir);
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(id) = serde_json::from_str::<LocalIdentity>(&text) {
                if id.wgid == wgid {
                    return Some(id);
                }
            }
        }
    }
    None
}

// ── Minting (step 1) ───────────────────────────────────────────────────────────

/// Genesis-time recovery configuration (ADR-fed-003 §D5). Default is empty
/// (agent / node-present, recovery anchors to the custodian). `--recovery` adds an
/// offline recovery key; `--node-less` additionally mandates an M-of-N guardian
/// quorum (the ceremony that defuses Fatal A-4 — [`RecoverySlot::validate_node_less`]).
#[derive(Default, Clone)]
pub struct RecoveryConfig {
    pub with_recovery_key: bool,
    pub guardians: Vec<String>,
    pub threshold: u8,
    pub node_less: bool,
}

impl RecoveryConfig {
    fn is_empty(&self) -> bool {
        !self.with_recovery_key
            && self.guardians.is_empty()
            && self.threshold == 0
            && !self.node_less
    }
}

/// Mint root/signer/enc keys into custody, build the genesis+add_key sigchain,
/// sign the `IdentityRecord`, and persist local public state. The root private
/// key is written to `wg secret` and **never returned or displayed**. When `parent`
/// is `Some`, the genesis cites it as a **fork** parent (ADR-fed-003 §D4) — a new
/// `wgid:`, a verifiable child, never the parent.
fn mint(
    name: &str,
    rec_cfg: &RecoveryConfig,
    parent: Option<sigchain::ParentRef>,
) -> Result<LocalIdentity> {
    let cust = Custodian::new(name);

    // Root (ed25519) — signs the sigchain only; lives in custody forever.
    let root = keys::gen_ed25519()?;
    let root_kid = keys::kid_for(&root.public);
    cust.store_signing_key(&root_kid, &root.seed)?;
    let wgid = keys::wgid_from_pubkey(&root.public);

    // Signer (ed25519) — the day-to-day key for events/records/snapshots.
    let signer = keys::gen_ed25519()?;
    let signer_kid = keys::kid_for(&signer.public);
    cust.store_signing_key(&signer_kid, &signer.seed)?;

    // Encryption (X25519) — per-recipient sealing.
    let enc = keys::gen_x25519()?;
    let enc_kid = keys::kid_for(&enc.public);
    cust.store_sealing_key(&enc_kid, &enc.secret)?;

    // Optional offline recovery key (the §D5 owner backstop). Minted into custody;
    // only its PUBLIC key goes into the genesis recovery slot.
    let mut recovery_kid: Option<String> = None;
    let recovery_slot = if rec_cfg.is_empty() {
        None
    } else {
        let recovery_key = if rec_cfg.with_recovery_key || rec_cfg.node_less {
            let rk = keys::gen_ed25519()?;
            let rk_kid = keys::kid_for(&rk.public);
            cust.store_signing_key(&rk_kid, &rk.seed)?;
            recovery_kid = Some(rk_kid);
            Some(hex::encode(rk.public))
        } else {
            None
        };
        let slot = sigchain::RecoverySlot {
            guardians: rec_cfg.guardians.clone(),
            threshold: rec_cfg.threshold,
            recovery_key,
        };
        if rec_cfg.node_less {
            // The mandatory node-less ceremony: refuse to mint an unrecoverable id.
            slot.validate_node_less()?;
        }
        Some(slot)
    };

    // Build the sigchain: genesis (root self-signed) → add_key signer → add_key enc.
    // add_key links are signed by the root (the hydra lock, S-4). A fork's genesis
    // cites its parent (§D4).
    let g = match parent {
        Some(p) => sigchain::genesis_fork(&cust, &root.public, &root_kid, recovery_slot, p)?,
        None => sigchain::genesis(&cust, &root.public, &root_kid, recovery_slot)?,
    };
    let signer_entry = KeyEntry {
        kid: signer_kid.clone(),
        public: hex::encode(signer.public),
        role: KeyRole::Signer,
        scope: vec!["event".into(), "state".into()],
        status: KeyStatus::Active,
    };
    let l1 = sigchain::add_key(&cust, &g, &root.public, &root_kid, signer_entry.clone())?;
    let enc_entry = KeyEntry {
        kid: enc_kid.clone(),
        public: hex::encode(enc.public),
        role: KeyRole::Enc,
        scope: vec![],
        status: KeyStatus::Active,
    };
    let l2 = sigchain::add_key(&cust, &l1, &root.public, &root_kid, enc_entry.clone())?;
    let chain = vec![g, l1, l2];
    let head_cid = chain.last().unwrap().cid();

    // Build + sign the IdentityRecord (signed by the signer key).
    let mut record = IdentityRecord {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        id: wgid.clone(),
        sigchain_head: head_cid,
        keys: vec![signer_entry, enc_entry],
        endpoints: vec![Endpoint {
            kind: "inbox".into(),
            uri: format!("wgfed-inbox:{wgid}"),
        }],
        alias_proofs: vec![],
        agent_fields: Some(AgentFields {
            role_id: name.to_string(),
            trust_level: "untrusted".into(), // first-contact default (TOFU, HQ8)
            executor: String::new(),
            capabilities: vec![],
        }),
        sig: String::new(),
    };
    record.sign(&cust, &signer_kid)?;

    Ok(LocalIdentity {
        name: name.to_string(),
        wgid,
        root_kid: root_kid.clone(),
        signer_kid,
        enc_kid,
        holds_private: true,
        active_root_kid: root_kid,
        recovery_kid,
        record,
        sigchain: chain,
        freshness_seq: 0,
    })
}

// ── resolve + verify a bundle from a store (offline, self-certifying) ──────────

/// A fetched, fully-verified identity bundle.
struct ResolvedBundle {
    record: IdentityRecord,
    chain: Vec<SigchainLink>,
    auth: AuthorizedKeys,
    snapshots: Vec<String>,
    attestation: Option<String>,
}

/// Fetch a `wgid`'s record + sigchain from a [`FedStore`] and **verify offline** —
/// the signature checks against the genesis pubkey embedded in the address; no call
/// to the origin and no central authority (ADR-fed-001 §D5). Walks the content-
/// addressed sigchain from `record.sigchain_head` back to genesis.
fn resolve_bundle(store: &dyn FedStore, wgid: &str) -> Result<ResolvedBundle> {
    let head = store.get_head(wgid)?;
    let record_bytes = store.get_object(&head.record)?;
    let record: IdentityRecord =
        serde_json::from_slice(&record_bytes).context("parsing fetched IdentityRecord")?;
    if record.id != wgid {
        bail!("fetched record id {:?} != requested {wgid:?}", record.id);
    }

    // Walk the sigchain by content id, head → genesis, then reverse.
    let mut chain_rev: Vec<SigchainLink> = Vec::new();
    let mut cursor = Some(record.sigchain_head.clone());
    let mut guard = 0;
    while let Some(cid) = cursor {
        guard += 1;
        if guard > 10_000 {
            bail!("sigchain too long / cyclic while resolving {wgid}");
        }
        let link_bytes = store.get_object(&cid)?;
        let link: SigchainLink =
            serde_json::from_slice(&link_bytes).context("parsing fetched sigchain link")?;
        cursor = link.prev.clone();
        chain_rev.push(link);
    }
    chain_rev.reverse();
    let chain = chain_rev;

    // Verify the chain against the address, then the record against the chain.
    let auth =
        sigchain::verify(&chain, wgid).with_context(|| format!("verifying sigchain for {wgid}"))?;
    record
        .verify(&auth)
        .with_context(|| format!("verifying IdentityRecord for {wgid}"))?;

    Ok(ResolvedBundle {
        record,
        chain,
        auth,
        snapshots: head.snapshots,
        attestation: head.attestation,
    })
}

/// Resolve a sender's authorized-key set, preferring a **locally cached, already-
/// verified** bundle (so the sender's node may be offline at poll time) and falling
/// back to the store. This is the ADR-fed-001 §D5 cascade applied to authentication:
/// the cached signed record can only help, never override a self-verification.
pub(crate) fn resolve_auth_cached(
    workgraph_dir: &Path,
    store: &dyn FedStore,
    wgid: &str,
) -> Result<AuthorizedKeys> {
    if let Some(local) = load_local_by_wgid(workgraph_dir, wgid) {
        // Re-verify the cached chain offline (do not trust the cache blindly).
        if let Ok(auth) = sigchain::verify(&local.sigchain, wgid) {
            return Ok(auth);
        }
    }
    Ok(resolve_bundle(store, wgid)?.auth)
}

fn enc_pub_of(auth: &AuthorizedKeys) -> Result<(String, [u8; 32])> {
    let enc = auth
        .active_enc()
        .ok_or_else(|| anyhow::anyhow!("identity has no active encryption key to seal to"))?;
    let bytes = hex::decode(&enc.public).context("enc pubkey not hex")?;
    if bytes.len() != 32 {
        bail!("enc pubkey is {} bytes, expected 32", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok((enc.kid.clone(), out))
}

/// Build, sign, and publish a freshness attestation over `id`'s current head; bump
/// and persist the issued `seq`. Shared by `publish` and `attest`.
fn emit_attestation(
    workgraph_dir: &Path,
    store: &dyn FedStore,
    id: &mut LocalIdentity,
    ttl_secs: i64,
) -> Result<FreshnessAttestation> {
    let cust = Custodian::new(&id.name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    let sk = signing_kid(id, &cust, &auth)?;
    id.freshness_seq += 1;
    let mut att = FreshnessAttestation::build(
        &id.wgid,
        &id.record.sigchain_head,
        chrono::Utc::now(),
        ttl_secs,
        id.freshness_seq,
    );
    att.sign(&cust, &sk)?;
    let att_cid = att.cid();
    store.put_object(&att_cid, &serde_json::to_vec(&att)?)?;
    store.put_attestation(&id.wgid, &serde_json::to_vec(&att)?)?;
    save_local(workgraph_dir, id)?;
    Ok(att)
}

// ── Wave 5 shared helpers: re-publish after a chain mutation ────────────────────

/// The custody kid to sign new records / snapshots / attestations with: the active
/// signer if we still hold it, else the active root (e.g. after the signer was
/// revoked). A downloaded bundle holds neither and cannot author.
pub(crate) fn signing_kid(
    id: &LocalIdentity,
    cust: &Custodian,
    auth: &AuthorizedKeys,
) -> Result<String> {
    if let Some(s) = auth.active_signer() {
        if cust.has_key(&s.kid)? {
            return Ok(s.kid.clone());
        }
    }
    let root_kid = id.cur_root_kid();
    if cust.has_key(&root_kid)? {
        return Ok(root_kid);
    }
    bail!(
        "no active signing key for {:?} in custody — cannot author (download ≠ \
         impersonation, ADR-fed-003 §D1)",
        id.name
    )
}

/// Re-sign the `IdentityRecord` over the current head + active key set and publish
/// all sigchain links + record + head + a fresh attestation. Used after every
/// sigchain mutation (rotate / revoke / recover / enroll-signer).
fn republish(workgraph_dir: &Path, id: &mut LocalIdentity, store: &dyn FedStore) -> Result<()> {
    let cust = Custodian::new(&id.name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    let head_cid = id.sigchain.last().unwrap().cid();
    id.record.sigchain_head = head_cid;
    id.record.keys = auth.keys.clone();
    let sk = signing_kid(id, &cust, &auth)?;
    id.record.sig = String::new();
    id.record.sign(&cust, &sk)?;

    for link in &id.sigchain {
        store.put_object(&link.cid(), &serde_json::to_vec(link)?)?;
    }
    let record_cid = id.record.cid();
    store.put_object(&record_cid, &serde_json::to_vec(&id.record)?)?;

    // Preserve any previously-published state snapshots in the head pointer.
    let snapshots = store
        .get_head(&id.wgid)
        .map(|h| h.snapshots)
        .unwrap_or_default();
    let att = emit_attestation(workgraph_dir, store, id, ROUTINE_DELTA_SECS)?;
    let mut head = Head {
        record: record_cid,
        snapshots,
        attestation: Some(att.cid()),
        sig: String::new(),
    };
    head.sign(&cust, &sk)?; // owner-sign for the node's write-auth (B1)
    store.put_head(&id.wgid, &head)?;
    save_local(workgraph_dir, id)?;
    Ok(())
}

// ── Command handlers ───────────────────────────────────────────────────────────

/// `wg identity new <name>` — mint a self-certifying identity (spark step 1).
/// `--recovery`/`--guardian`/`--threshold`/`--node-less` populate the genesis
/// recovery slot (Wave 5, ADR-fed-003 §D5).
pub fn run_new(
    workgraph_dir: &Path,
    name: &str,
    rec_cfg: &RecoveryConfig,
    json: bool,
) -> Result<()> {
    if local_path(workgraph_dir, name).exists() {
        bail!("identity {name:?} already exists locally; pick another name");
    }
    let id = mint(name, rec_cfg, None)?;
    save_local(workgraph_dir, &id)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "name": id.name,
                "wgid": id.wgid,
                "signer_kid": id.signer_kid,
                "enc_kid": id.enc_kid,
                "sigchain_head": id.record.sigchain_head,
                "compat": WG_FED_COMPAT_VERSION,
                "root_custody": "wg-secret-keystore",
                "has_recovery_key": id.recovery_kid.is_some(),
                "node_less": rec_cfg.node_less,
            })
        );
    } else {
        println!("Minted WG-Fed identity '{}'", id.name);
        println!("  wgid: {}", id.wgid);
        println!("  signer kid: {}", id.signer_kid);
        println!("  enc kid:    {}", id.enc_kid);
        if id.recovery_kid.is_some() {
            println!(
                "  recovery:   offline recovery key in custody{}",
                if rec_cfg.node_less {
                    format!(
                        " + {}-of-{} guardian quorum (node-less ceremony)",
                        rec_cfg.threshold,
                        rec_cfg.guardians.len()
                    )
                } else {
                    String::new()
                }
            );
        }
        println!(
            "  root key:   held in custody (wg secret keystore) behind a sign-this-digest \
             boundary — never displayed, exported, or written to any record/file/env."
        );
    }
    Ok(())
}

/// `wg identity show <name>` — print a local identity (no private material).
pub fn run_show(workgraph_dir: &Path, name: &str, json: bool) -> Result<()> {
    let id = load_local(workgraph_dir, name)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "name": id.name,
                "wgid": id.wgid,
                "holds_private": id.holds_private,
                "signer_kid": id.signer_kid,
                "enc_kid": id.enc_kid,
                "sigchain_head": id.record.sigchain_head,
                "sigchain_len": id.sigchain.len(),
                "freshness_seq": id.freshness_seq,
            })
        );
    } else {
        println!("identity '{}'", id.name);
        println!("  wgid: {}", id.wgid);
        println!(
            "  holds private keys: {}",
            if id.holds_private {
                "yes"
            } else {
                "no (downloaded bundle)"
            }
        );
        println!(
            "  sigchain head: {} ({} links)",
            id.record.sigchain_head,
            id.sigchain.len()
        );
    }
    Ok(())
}

/// `wg identity list` — list local identities.
pub fn run_list(workgraph_dir: &Path, json: bool) -> Result<()> {
    let dir = identity_dir(workgraph_dir);
    let mut names: Vec<String> = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    if json {
        println!("{}", serde_json::json!({ "identities": names }));
    } else if names.is_empty() {
        println!("no local identities (mint one with `wg identity new <name>`)");
    } else {
        for n in names {
            let id = load_local(workgraph_dir, &n)?;
            println!(
                "{n}\t{}\t{}",
                id.wgid,
                if id.holds_private { "own" } else { "fetched" }
            );
        }
    }
    Ok(())
}

/// `wg identity publish <name> --store <L>` — publish the self-verifying
/// `IdentityRecord` + sigchain + one `StateSnapshot` **and a freshness attestation**
/// to `L` (a directory or an `http://` node). No private key is ever written.
pub fn run_publish(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    fresh_ttl: Option<i64>,
    state_text: Option<&str>,
    json: bool,
) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; you can only publish your own");
    }
    let store = open_store(store_loc)?;

    // Publish each sigchain link as a content-addressed object.
    for link in &id.sigchain {
        let bytes = serde_json::to_vec(link)?;
        store.put_object(&link.cid(), &bytes)?;
    }
    // Publish the IdentityRecord.
    let record_cid = id.record.cid();
    store.put_object(&record_cid, &serde_json::to_vec(&id.record)?)?;

    // Build + sign + publish one StateSnapshot (payload_kind conv-cache-v1). The
    // `--state-text` override lets a publisher seed a custom conversation turn — used
    // to demonstrate the S-5 scan blocking a poisoned (e.g. injection-bearing) cache.
    let turn_text = state_text.unwrap_or("wg-fed spark conversation cache");
    let cust = Custodian::new(name);
    let payload = serde_json::to_vec(&serde_json::json!({
        "kind": "conv-cache-v1",
        "owner": id.wgid,
        "turns": [{"role": "system", "text": turn_text}],
    }))?;
    let pcid = payload_cid(&payload);
    store.put_object(&pcid, &payload)?;
    let mut snap = StateSnapshot {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        identity: id.wgid.clone(),
        payload_kind: "conv-cache-v1".into(),
        model_binding: Some(serde_json::json!({
            "model": "claude-opus-4-8",
            "min_reader": "conv-cache-v1",
        })),
        content_cid: pcid,
        prev: None,
        sig: String::new(),
    };
    snap.sign(&cust, &id.signer_kid)?;
    let snap_cid = snap.cid();
    store.put_object(&snap_cid, &serde_json::to_vec(&snap)?)?;

    // Freshness attestation over the current head (S-3). `--fresh-ttl 0` publishes an
    // already-expired attestation, useful for exercising fail-closed-on-stale.
    let ttl = fresh_ttl.unwrap_or(ROUTINE_DELTA_SECS);
    let att = emit_attestation(workgraph_dir, store.as_ref(), &mut id, ttl)?;
    let att_cid = att.cid();

    // Sign the head so the (untrusted) network node can authenticate the owner before
    // accepting the write (audit B1) — the local directory rung ignores the signature.
    let mut head = Head {
        record: record_cid.clone(),
        snapshots: vec![snap_cid.clone()],
        attestation: Some(att_cid.clone()),
        sig: String::new(),
    };
    head.sign(&cust, &id.signer_kid)?;
    store.put_head(&id.wgid, &head)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid,
                "record_cid": record_cid,
                "snapshot_cid": snap_cid,
                "attestation_cid": att_cid,
                "freshness_seq": id.freshness_seq,
                "store": store_loc,
            })
        );
    } else {
        println!("Published '{}' to {store_loc}", id.name);
        println!("  wgid:            {}", id.wgid);
        println!("  record cid:      {record_cid}");
        println!("  snapshot cid:    {snap_cid}");
        println!("  attestation cid: {att_cid} (seq {})", id.freshness_seq);
        println!("  (bundle carries NO private key — verify with `wg identity fetch`)");
    }
    Ok(())
}

/// `wg identity attest <name> --store <L>` — (re)emit a fresh signed attestation over
/// the current head, bumping `seq`. The custodian/node runs this periodically so a
/// verifier can always re-fetch a recent `valid-as-of` (ADR-fed-001 §OQ4).
pub fn run_attest(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    fresh_ttl: Option<i64>,
    json: bool,
) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; cannot issue attestations for it");
    }
    let store = open_store(store_loc)?;
    let ttl = fresh_ttl.unwrap_or(ROUTINE_DELTA_SECS);
    let att = emit_attestation(workgraph_dir, store.as_ref(), &mut id, ttl)?;
    // Update the head's attestation pointer if a head already exists, then re-sign so
    // the mutated head re-authenticates against the node's write-auth (audit B1).
    if let Ok(mut head) = store.get_head(&id.wgid) {
        head.attestation = Some(att.cid());
        let cust = Custodian::new(name);
        head.sign(&cust, id.signer_kid())?;
        store.put_head(&id.wgid, &head)?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid,
                "attestation_cid": att.cid(),
                "seq": id.freshness_seq,
                "as_of": att.as_of,
                "expires": att.expires,
            })
        );
    } else {
        println!("Attested {} (seq {})", id.wgid, id.freshness_seq);
        println!("  as_of:   {}", att.as_of);
        println!("  expires: {}", att.expires);
    }
    Ok(())
}

/// `wg identity check-fresh <wgid> --store <L> [--class routine|high-value]` — the
/// **verifier side** of S-3: re-fetch the attestation, verify its signature, and
/// apply the **fail-closed** freshness rule. Exits non-zero on stale/rollback so a
/// high-value caller can gate on it.
pub fn run_check_fresh(
    workgraph_dir: &Path,
    wgid: &str,
    store_loc: &str,
    class: &str,
    json: bool,
) -> Result<()> {
    let pubkey = keys::pubkey_from_wgid(wgid)?;
    let wgid = keys::wgid_from_pubkey(&pubkey);
    let class = ActionClass::parse(class)?;
    let store = open_store(store_loc)?;

    let bytes = store.get_attestation(&wgid)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no freshness attestation published for {wgid} — \
            failing closed (cannot confirm the key is current)"
        )
    })?;
    let att: FreshnessAttestation =
        serde_json::from_slice(&bytes).context("parsing fetched freshness attestation")?;

    // Verify the attestation signature against the identity's authorized keys.
    let auth = resolve_auth_cached(workgraph_dir, store.as_ref(), &wgid)?;
    att.verify_signature(&auth)
        .context("freshness attestation failed signature verification")?;

    let seen_dir = freshness_seen_dir(workgraph_dir);
    let last_seen = load_seen_seq(&seen_dir, &wgid);
    let verdict = check_fresh(&att, chrono::Utc::now(), class, last_seen);

    match &verdict {
        FreshVerdict::Fresh { seq, head } => {
            // Advance the rollback high-water mark only on a fresh accept.
            record_seen_seq(&seen_dir, &wgid, *seq)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "wgid": wgid, "fresh": true, "class": format!("{class:?}"),
                        "seq": seq, "head": head, "as_of": att.as_of, "expires": att.expires,
                    })
                );
            } else {
                println!("FRESH {wgid} ({class:?}, seq {seq}) — head {head} confirmed current");
            }
            Ok(())
        }
        FreshVerdict::Stale { reason } | FreshVerdict::Rollback { reason } => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "wgid": wgid, "fresh": false, "class": format!("{class:?}"),
                        "reason": reason,
                    })
                );
            } else {
                println!("STALE {wgid} ({class:?}) — FAIL CLOSED: {reason}");
            }
            bail!("freshness check failed closed: {reason}")
        }
    }
}

/// `wg identity fetch <wgid> --store <L> [--save <name>]` — fetch + verify a
/// record offline against `L` alone (spark steps 3 & 7). Optionally cache the
/// key-less bundle locally under `<name>`.
pub fn run_fetch(
    workgraph_dir: &Path,
    wgid: &str,
    store_loc: &str,
    save: Option<&str>,
    json: bool,
) -> Result<()> {
    // Normalize a did:key spelling to the wgid the store is keyed by.
    let pubkey = keys::pubkey_from_wgid(wgid)?;
    let wgid = keys::wgid_from_pubkey(&pubkey);
    let store = open_store(store_loc)?;

    let bundle = resolve_bundle(store.as_ref(), &wgid)?;

    // Verify the published snapshot too, if any (step 2/7 completeness).
    let mut verified_snapshots = 0usize;
    for scid in &bundle.snapshots {
        let bytes = store.get_object(scid)?;
        let snap: StateSnapshot = serde_json::from_slice(&bytes).context("parsing snapshot")?;
        snap.verify(&bundle.auth)
            .with_context(|| format!("verifying snapshot {scid}"))?;
        verified_snapshots += 1;
    }

    if let Some(name) = save {
        let id = LocalIdentity {
            name: name.to_string(),
            wgid: wgid.clone(),
            root_kid: keys::kid_for(&bundle.auth.root_pub),
            signer_kid: bundle
                .auth
                .active_signer()
                .map(|k| k.kid.clone())
                .unwrap_or_default(),
            enc_kid: bundle
                .auth
                .active_enc()
                .map(|k| k.kid.clone())
                .unwrap_or_default(),
            holds_private: false,
            active_root_kid: String::new(),
            recovery_kid: None,
            record: bundle.record.clone(),
            sigchain: bundle.chain.clone(),
            freshness_seq: 0,
        };
        save_local(workgraph_dir, &id)?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": wgid,
                "verified": true,
                "offline": true,
                "sigchain_len": bundle.chain.len(),
                "verified_snapshots": verified_snapshots,
                "has_attestation": bundle.attestation.is_some(),
                "saved_as": save,
            })
        );
    } else {
        println!("VERIFIED {wgid}");
        println!("  offline self-certifying check against L alone (no origin contacted)");
        println!("  sigchain links: {}", bundle.chain.len());
        println!("  snapshots verified: {verified_snapshots}");
        if let Some(name) = save {
            println!("  saved local (key-less) bundle as '{name}'");
        }
    }
    Ok(())
}

/// `wg identity send --from <name> --to <wgid> --store <L>` — author + sign (and
/// optionally seal) a cross-graph `SignedEvent` into `L`'s store-and-forward inbox
/// (spark step 4 / Wave 4 node inbox). Authoring requires the **signer private key in
/// custody**: a downloaded bundle cannot author (the impersonation defense, step 6).
#[allow(clippy::too_many_arguments)]
pub fn run_send(
    workgraph_dir: &Path,
    from: &str,
    to: &[String],
    store_loc: &str,
    body: &str,
    kind: &str,
    seal: bool,
    sealed_sender: bool,
    json: bool,
) -> Result<()> {
    let id = load_local(workgraph_dir, from)?;
    let cust = Custodian::new(from);

    // The custody gate: download confers no signing ability (ADR-fed-003 §D1).
    if !cust.has_key(&id.signer_kid)? {
        bail!(
            "cannot author as {from:?} ({}): no signer private key in this custody. \
             Possessing a downloaded identity bundle does NOT grant the ability to \
             sign as that identity — download ≠ impersonation (ADR-fed-003 §D1).",
            id.wgid
        );
    }

    // Normalize every recipient address (the `to` set — which, when sealing, IS the ACL).
    let to_wgids: Vec<String> = to
        .iter()
        .map(|t| keys::pubkey_from_wgid(t).map(|p| keys::wgid_from_pubkey(&p)))
        .collect::<Result<_>>()?;
    if to_wgids.is_empty() {
        bail!("at least one --to recipient is required");
    }

    let created_at = chrono::Utc::now().to_rfc3339();
    let store = open_store(store_loc)?;
    // Sealed-sender implies sealing (the inner author is recovered from the seal).
    let seal = seal || sealed_sender;

    let mut ev = if seal {
        // Resolve each recipient's encryption key (fetch + verify their bundle from L).
        // The set of resolved enc keys is the access-control list (HQ4).
        let mut recipients: Vec<(String, [u8; 32])> = Vec::with_capacity(to_wgids.len());
        for w in &to_wgids {
            let recipient = resolve_bundle(store.as_ref(), w)
                .with_context(|| format!("resolving recipient {w} to seal to"))?;
            recipients.push(enc_pub_of(&recipient.auth)?);
        }
        if sealed_sender {
            SignedEvent::new_sealed_sender(
                &id.wgid,
                &to_wgids,
                &created_at,
                kind,
                body,
                &recipients,
                &cust,
                &id.signer_kid,
            )?
        } else {
            SignedEvent::new_sealed_multi(
                &id.wgid,
                &to_wgids,
                &created_at,
                kind,
                body,
                &recipients,
            )?
        }
    } else {
        SignedEvent::new_plain(&id.wgid, &to_wgids, &created_at, kind, body)
    };
    // A sealed-sender event carries NO outer signature (it is anonymous — authenticity
    // is the inner author signature). Every other event is signed by the sender.
    if !sealed_sender {
        ev.sign(&cust, &id.signer_kid)?;
    }

    // Deliver: write to EACH recipient's inbox at L (store-and-forward; a recipient
    // may be offline and receives it on its next poll).
    let event_bytes = serde_json::to_vec(&ev)?;
    for w in &to_wgids {
        store.put_event(w, &ev.id, &event_bytes)?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event_id": ev.id,
                "from": ev.from,
                "to": to_wgids,
                "kind": kind,
                "sealed": seal,
                "sealed_sender": sealed_sender,
                "recipients": to_wgids.len(),
                "accepted": true,
                "store": store_loc,
            })
        );
    } else {
        println!("Accepted for delivery (store-and-forward; recipient may be offline)");
        println!("  event id: {}", ev.id);
        println!("  from: {}", ev.from);
        println!("  to:   {}", to_wgids.join(", "));
        println!("  via:  {store_loc}");
        println!("  sealed: {seal}  sealed-sender: {sealed_sender}");
    }
    Ok(())
}

/// `wg identity poll <name> --store <L>` — fetch events from `L`'s inbox and
/// authenticate each by key (spark step 5 / Wave 4 node inbox). A forged "from" or a
/// tampered event is REJECTED. Sealed events addressed to us are opened with our
/// custody-held encryption key. When `require_fresh` is set, accepting an event is
/// **gated on a fresh attestation** for the sender and **fails closed on stale**.
///
/// When `review` is set, this is the **live IC4 ingest auto-gate** (the IC4 seam, ON BY
/// DEFAULT for `wg msg poll`; opt-in for the raw `wg identity poll` primitive): every
/// *authenticated* inbound event is additionally screened through the [`review_inbound`]
/// pipeline with author-trust **derived** from the canonical [`resolve_author_trust`]
/// dial (federation peer registry + WG-Exec pool) — no hand-passed `--trust` flag. The
/// gate is **ENFORCING**, not advisory: a non-`accept` verdict **withholds the body** —
/// the bytes are never printed or returned (`body` is null + `body_withheld:true`,
/// `consumable:false`), so a consuming agent can never read un-screened content
/// (received ≠ consumed, ADR-CS1 D1). Each verdict is recorded to the verdict sigchain
/// (the audit leg). Authentication counts (`accepted`/`rejected`) are unchanged; a
/// forged/tampered event never reaches the gate (it is rejected at auth).
pub fn run_poll(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    require_fresh: Option<&str>,
    review: bool,
    json: bool,
) -> Result<()> {
    let id = load_local(workgraph_dir, name)?;
    let cust = Custodian::new(name);
    let store = open_store(store_loc)?;
    let fresh_class = match require_fresh {
        Some(c) => Some(ActionClass::parse(c)?),
        None => None,
    };

    let mut verdicts: Vec<serde_json::Value> = Vec::new();
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    // Review (IC4 auto-gate) tallies — only meaningful when `review` is set.
    let mut screened = 0usize;
    let mut consumable = 0usize;
    let mut quarantined = 0usize;

    for ev in store.list_events(&id.wgid)? {
        let verdict = authenticate_event(
            &ev.bytes,
            store.as_ref(),
            &id,
            &cust,
            workgraph_dir,
            fresh_class,
        );
        match verdict {
            Ok((from, body)) => {
                accepted += 1;
                if review {
                    // ── IC4 ingest auto-gate (ENFORCING): screen the authenticated event
                    // BEFORE its body is exposed (received ≠ consumed). A non-accept
                    // verdict WITHHOLDS the body — it is never printed or returned, so a
                    // consuming agent can never read un-screened bytes.
                    let r = review_inbound_event(workgraph_dir, &from, &body);
                    screened += 1;
                    let consume = r.permits_consumption;
                    if consume {
                        consumable += 1;
                    } else {
                        quarantined += 1;
                    }
                    let mut ev_json = serde_json::json!({
                        "verdict": "VERIFIED",
                        "from": from,
                        "consumable": consume,
                        "review": {
                            "verdict": r.verdict,
                            "reason": r.reason,
                            "effective_trust": r.effective_trust,
                            "permits_consumption": r.permits_consumption,
                            "content_cid": r.content_cid,
                            "trust_derived": true,
                        },
                    });
                    if consume {
                        ev_json["body"] = serde_json::json!(body);
                    } else {
                        // Withheld: the bytes are not handed downstream.
                        ev_json["body"] = serde_json::Value::Null;
                        ev_json["body_withheld"] = serde_json::json!(true);
                    }
                    if !json {
                        if consume {
                            println!("VERIFIED from {from}: {body}");
                        } else {
                            println!("VERIFIED from {from}: <body withheld — non-accept verdict>");
                        }
                        println!(
                            "  review: {} ({}, trust={}) → {}",
                            r.verdict,
                            r.reason,
                            r.effective_trust,
                            if consume {
                                "CONSUMABLE"
                            } else {
                                "BLOCKED (body withheld; non-accept verdict)"
                            }
                        );
                    }
                    verdicts.push(ev_json);
                } else {
                    // Auto-gate disabled (`--no-review` / the raw `identity poll`
                    // primitive): expose the authenticated body unscreened.
                    let ev_json = serde_json::json!({
                        "verdict": "VERIFIED", "from": from, "body": body,
                    });
                    if !json {
                        println!("VERIFIED from {from}: {body}");
                    }
                    verdicts.push(ev_json);
                }
            }
            Err(e) => {
                rejected += 1;
                verdicts.push(serde_json::json!({
                    "verdict": "REJECTED", "reason": e.to_string(),
                }));
                if !json {
                    println!("REJECTED: {e}");
                }
            }
        }
    }

    if json {
        let mut out = serde_json::json!({
            "wgid": id.wgid,
            "accepted": accepted,
            "rejected": rejected,
            "events": verdicts,
        });
        if review {
            out["review"] = serde_json::json!({
                "screened": screened,
                "consumable": consumable,
                "quarantined": quarantined,
            });
        }
        println!("{out}");
    } else {
        println!(
            "polled {} ({accepted} verified, {rejected} rejected)",
            id.wgid
        );
        if review {
            println!(
                "  ingest gate: {screened} screened → {consumable} consumable, {quarantined} blocked"
            );
        }
    }
    Ok(())
}

/// The outcome of auto-gating one authenticated inbound event.
struct IngestReview {
    verdict: &'static str,
    reason: &'static str,
    effective_trust: &'static str,
    permits_consumption: bool,
    content_cid: String,
}

/// Screen one authenticated inbound message (IC4) through the review pipeline with
/// **derived** author-trust, fold any revoke override (strictest-wins), record the
/// verdict to the audit sigchain, and report whether consumption is permitted.
///
/// Sensitivity is left **`Unlabeled`** (fail-closed → High): an inbound message does
/// not declare its blast radius, so it never gets the light path on sensitivity alone —
/// a *Verified* author still accepts on clean content, an *Unknown* one quarantines.
fn review_inbound_event(workgraph_dir: &Path, from: &str, body: &str) -> IngestReview {
    let baseline = resolve_author_trust(workgraph_dir, from);
    let store = VerdictStore::open(workgraph_dir);
    // Fold a revoke-lowered override (D4): a previously-poisoned author's next item
    // takes the deeper path. Strictest wins, identical to `wg review check`.
    let effective_trust = match store.trust_override(from).ok().flatten() {
        Some(overridden) => strictest_trust(baseline, overridden),
        None => baseline,
    };
    let provenance = Provenance {
        author: Some(from.to_string()),
        trust: effective_trust.clone(),
    };
    // Real model-driven review when a model is available (production); deterministic
    // pipeline otherwise (credential-free CI / smoke). `review_inbound_ctx` is
    // byte-identical to `review_inbound` when no model is available.
    let outcome = match worksgood::config::Config::load_merged(workgraph_dir) {
        Ok(cfg) => review_inbound_ctx(
            &cfg,
            ContentClass::Ic4Message,
            body,
            &provenance,
            Sensitivity::Unlabeled,
        ),
        Err(_) => review_inbound(
            ContentClass::Ic4Message,
            body,
            &provenance,
            Sensitivity::Unlabeled,
        ),
    };
    // Audit leg: record on the hash-linked verdict sigchain (best-effort; a recording
    // failure must not crash the poll — the gate decision still stands).
    let _ = store.record(&outcome, Some(from), None);
    IngestReview {
        verdict: outcome.verdict.tag(),
        reason: outcome.reason.tag(),
        effective_trust: trust_tag(&effective_trust),
        permits_consumption: outcome.verdict.permits_consumption(),
        content_cid: outcome.content_cid,
    }
}

/// Render a [`worksgood::graph::TrustLevel`] in its kebab-case wire spelling.
fn trust_tag(t: &worksgood::graph::TrustLevel) -> &'static str {
    use worksgood::graph::TrustLevel;
    match t {
        TrustLevel::Verified => "verified",
        TrustLevel::Provisional => "provisional",
        TrustLevel::Unknown => "unknown",
    }
}

/// Verify one inbox event: resolve the sender's bundle (cache-first for offline
/// tolerance), verify the signature against the sender's authorized signer set, then
/// (if sealed and addressed to us) open it. When `fresh_class` is set, additionally
/// require a fresh attestation for the sender (fail closed). Returns `(from, body)`.
fn authenticate_event(
    bytes: &[u8],
    store: &dyn FedStore,
    me: &LocalIdentity,
    cust: &Custodian,
    workgraph_dir: &Path,
    fresh_class: Option<ActionClass>,
) -> Result<(String, String)> {
    let ev: SignedEvent = serde_json::from_slice(bytes).context("parsing inbox event")?;

    // Sealed-sender (Wave 6): the relay saw an anonymized `from`. Recover the real
    // author from inside the seal (only a recipient can), then authenticate it.
    if ev.is_sealed_sender() {
        let inner = ev.open_sender_sealed(cust).context(
            "sealed-sender event could not be opened with our key (we are not a recipient)",
        )?;
        let sender_auth = resolve_auth_cached(workgraph_dir, store, &inner.from)
            .with_context(|| format!("resolving sealed-sender author {}", inner.from))?;
        inner
            .verify(&sender_auth)
            .context("sealed-sender inner author signature failed verification")?;
        if let Some(class) = fresh_class {
            gate_freshness(workgraph_dir, store, &inner.from, &sender_auth, class)?;
        }
        if !inner.to.iter().any(|t| t == &me.wgid) {
            bail!("sealed-sender event not addressed to {}", me.wgid);
        }
        return Ok((inner.from, inner.body));
    }

    // Resolve and verify the *claimed* sender's identity (cache-first → store).
    let sender_auth = resolve_auth_cached(workgraph_dir, store, &ev.from)
        .with_context(|| format!("resolving claimed sender {}", ev.from))?;
    // Authenticate: signature must verify against a key the sender's chain
    // authorizes. A forged "from" (wrong signature) fails here.
    ev.verify(&sender_auth)?;

    // Freshness gate (S-3): for a freshness-required action, the sender's key must be
    // confirmed current by a freshly re-fetched attestation, or we fail closed.
    if let Some(class) = fresh_class {
        gate_freshness(workgraph_dir, store, &ev.from, &sender_auth, class)?;
    }

    let body = if ev.enc.is_some() || ev.enc_multi.is_some() {
        // Sealed (single-recipient `enc` or the Wave 6 per-recipient `enc_multi`):
        // only a member of the ACL can open it — a third party fails here.
        ev.open(cust)
            .context("event is sealed but could not be opened with our key (not in the ACL?)")?
    } else {
        ev.body.clone().unwrap_or_default()
    };
    // Ensure it was addressed to us.
    if !ev.to.iter().any(|t| t == &me.wgid) {
        bail!("event not addressed to {}", me.wgid);
    }
    Ok((ev.from, body))
}

/// Fail-closed freshness gate for a high-value accept (ADR-fed-001 §OQ4). Re-fetches
/// the **sender's** signed `valid-as-of` attestation — preferring the sender's own
/// advertised endpoint (the S-3 "re-fetch from the source" so a frozen inbox can't
/// keep a revoked key alive), falling back to the inbox we polled from.
fn gate_freshness(
    workgraph_dir: &Path,
    store: &dyn FedStore,
    wgid: &str,
    auth: &AuthorizedKeys,
    class: ActionClass,
) -> Result<()> {
    let bytes = fetch_sender_attestation(workgraph_dir, store, wgid).ok_or_else(|| {
        anyhow::anyhow!(
            "no freshness attestation for {wgid} — failing closed on a {class:?} action \
             (cannot confirm the sender's key is current; possible withheld revoke)"
        )
    })?;
    let att: FreshnessAttestation =
        serde_json::from_slice(&bytes).context("parsing sender freshness attestation")?;
    att.verify_signature(auth)
        .context("sender freshness attestation failed signature verification")?;
    let seen_dir = freshness_seen_dir(workgraph_dir);
    let last_seen = load_seen_seq(&seen_dir, wgid);
    match check_fresh(&att, chrono::Utc::now(), class, last_seen) {
        FreshVerdict::Fresh { seq, .. } => {
            record_seen_seq(&seen_dir, wgid, seq)?;
            Ok(())
        }
        FreshVerdict::Stale { reason } | FreshVerdict::Rollback { reason } => {
            bail!("freshness gate failed closed for {wgid}: {reason}")
        }
    }
}

/// Re-fetch a sender's freshness attestation, preferring the sender's advertised
/// endpoint(s) from the resolution cascade and falling back to the poll store.
fn fetch_sender_attestation(
    workgraph_dir: &Path,
    poll_store: &dyn FedStore,
    wgid: &str,
) -> Option<Vec<u8>> {
    if let Ok(resolved) = worksgood::federation::resolve_peer_endpoint(wgid, workgraph_dir) {
        for ep in &resolved.endpoints {
            if let Ok(store) = open_store(ep) {
                if let Ok(Some(bytes)) = store.get_attestation(wgid) {
                    return Some(bytes);
                }
            }
        }
    }
    poll_store.get_attestation(wgid).ok().flatten()
}

/// `wg identity verify <file> [--store <L>]` — verify a record or event file
/// offline. For an event, the sender's chain is resolved from `--store`.
pub fn run_verify(
    workgraph_dir: &Path,
    file: &str,
    store_loc: Option<&str>,
    json: bool,
) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {file}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).context("parsing JSON")?;

    let (kind, ok, detail) = if value.get("from").is_some() && value.get("kind").is_some() {
        // A SignedEvent — needs the sender's chain.
        let store_loc = store_loc.ok_or_else(|| {
            anyhow::anyhow!("verifying a SignedEvent needs --store to resolve the sender")
        })?;
        let store = open_store(store_loc)?;
        let ev: SignedEvent = serde_json::from_value(value).context("parsing SignedEvent")?;
        let from = ev.from.clone();
        match resolve_auth_cached(workgraph_dir, store.as_ref(), &from).and_then(|a| ev.verify(&a))
        {
            Ok(()) => ("SignedEvent", true, format!("authored by {from}")),
            Err(e) => ("SignedEvent", false, e.to_string()),
        }
    } else if value.get("sigchain_head").is_some() {
        // An IdentityRecord — self-contained needs its chain from the store.
        let rec: IdentityRecord =
            serde_json::from_value(value).context("parsing IdentityRecord")?;
        let id = rec.id.clone();
        match store_loc {
            Some(store_loc) => {
                let store = open_store(store_loc)?;
                match resolve_bundle(store.as_ref(), &id) {
                    Ok(_) => ("IdentityRecord", true, format!("verified {id}")),
                    Err(e) => ("IdentityRecord", false, e.to_string()),
                }
            }
            None => bail!("verifying an IdentityRecord needs --store to resolve its sigchain"),
        }
    } else {
        bail!("unrecognized artifact in {file} (not a SignedEvent or IdentityRecord)");
    };

    if json {
        println!(
            "{}",
            serde_json::json!({ "kind": kind, "verified": ok, "detail": detail })
        );
    } else if ok {
        println!("VERIFIED {kind}: {detail}");
    } else {
        println!("REJECTED {kind}: {detail}");
    }
    if ok {
        Ok(())
    } else {
        bail!("verification failed: {detail}")
    }
}

// ── Wave 5 command handlers: rotate / revoke / recover / fork / enroll / load ────

/// `wg identity rotate <name> --store <L>` — normal-succession `rotate_root`: mint a
/// new active root and have the *current* active root sign it in (ADR-fed-003 §D5).
/// The `wgid:` address is unchanged; the active signing root rotates underneath.
pub fn run_rotate(workgraph_dir: &Path, name: &str, store_loc: &str, json: bool) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; it holds no root and cannot rotate");
    }
    let cust = Custodian::new(name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    let active_root_kid = id.cur_root_kid();
    if !cust.has_key(&active_root_kid)? {
        bail!("the active root key is not in this custody — cannot sign a succession rotate");
    }

    // Mint the new root into custody, then append a succession rotate_root link.
    let new_root = keys::gen_ed25519()?;
    let new_kid = keys::kid_for(&new_root.public);
    cust.store_signing_key(&new_kid, &new_root.seed)?;
    let rot = sigchain::rotate_root(
        &cust,
        id.sigchain.last().unwrap(),
        &auth.active_root,
        &active_root_kid,
        &new_root.public,
    )?;
    id.sigchain.push(rot);
    id.active_root_kid = new_kid.clone();

    let store = open_store(store_loc)?;
    republish(workgraph_dir, &mut id, store.as_ref())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid, "rotated": true, "new_active_root_kid": new_kid,
                "sigchain_head": id.record.sigchain_head, "store": store_loc,
            })
        );
    } else {
        println!("Rotated active root of {} (address unchanged)", id.wgid);
        println!("  new active root kid: {new_kid}");
        println!("  sigchain head:       {}", id.record.sigchain_head);
    }
    Ok(())
}

/// `wg identity revoke <name> --kid <kid> --store <L>` — append a root-signed
/// `revoke_key` retiring an authorized key (ADR-fed-003 §D6). The revoked key no
/// longer authorizes signing; the durable, content-addressed revocation publishes
/// in the same self-verifying cascade (composed with freshness, S-3).
pub fn run_revoke(
    workgraph_dir: &Path,
    name: &str,
    kid: &str,
    store_loc: &str,
    json: bool,
) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; it cannot author a revocation");
    }
    let cust = Custodian::new(name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    if !auth.keys.iter().any(|k| k.kid == kid) {
        bail!(
            "{kid:?} is not an authorized key of {} — nothing to revoke",
            id.wgid
        );
    }
    let active_root_kid = id.cur_root_kid();
    if !cust.has_key(&active_root_kid)? {
        bail!("the active root key is not in this custody — cannot sign a revoke");
    }
    let rk = sigchain::revoke_key(
        &cust,
        id.sigchain.last().unwrap(),
        &auth.active_root,
        &active_root_kid,
        kid,
    )?;
    id.sigchain.push(rk);

    let store = open_store(store_loc)?;
    republish(workgraph_dir, &mut id, store.as_ref())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid, "revoked_kid": kid, "sigchain_head": id.record.sigchain_head,
                "store": store_loc,
            })
        );
    } else {
        println!("Revoked key {kid} of {}", id.wgid);
        println!("  sigchain head: {}", id.record.sigchain_head);
    }
    Ok(())
}

/// `wg identity recover <name> --store <L>` — recover the identity using the offline
/// **recovery key** registered at genesis (`wg identity new --recovery`): mint a new
/// root and rotate it in under the higher-priority recovery key, even against a lost
/// or hostile active root (ADR-fed-003 §D5, the node-default owner backstop, V6).
///
/// The node-less **M-of-N guardian** recovery path is the same `rotate_root` link
/// authorized by a guardian quorum (`sigchain::rotate_root_via_guardians`), exercised
/// end-to-end by the `recover_via_guardian_quorum` library test.
pub fn run_recover(workgraph_dir: &Path, name: &str, store_loc: &str, json: bool) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; recovery is performed by the owner");
    }
    let cust = Custodian::new(name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    let recovery = auth.recovery.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "no recovery configuration in {}'s genesis — mint with `--recovery` (offline \
             recovery key) or `--node-less` (paper key + M-of-N guardians) to enable recovery \
             (ADR-fed-003 §D5)",
            id.wgid
        )
    })?;
    let rk_kid = id.recovery_kid.clone().ok_or_else(|| {
        anyhow::anyhow!("no offline recovery key in this custody for {}", id.wgid)
    })?;
    let rk_hex = recovery
        .recovery_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("genesis registered no offline recovery key"))?;
    let rk_pub = decode_pub32(&rk_hex)?;

    // Mint the fresh post-recovery root, then rotate it in under the recovery key.
    let new_root = keys::gen_ed25519()?;
    let new_kid = keys::kid_for(&new_root.public);
    cust.store_signing_key(&new_kid, &new_root.seed)?;
    let rot = sigchain::rotate_root_via_recovery_key(
        &cust,
        id.sigchain.last().unwrap(),
        &rk_pub,
        &rk_kid,
        &new_root.public,
    )?;
    id.sigchain.push(rot);
    id.active_root_kid = new_kid.clone();

    let store = open_store(store_loc)?;
    republish(workgraph_dir, &mut id, store.as_ref())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid, "recovered": true, "via": "recovery-key",
                "new_active_root_kid": new_kid, "sigchain_head": id.record.sigchain_head,
            })
        );
    } else {
        println!(
            "Recovered {} via the offline recovery key (address unchanged)",
            id.wgid
        );
        println!("  new active root kid: {new_kid}");
        println!("  sigchain head:       {}", id.record.sigchain_head);
    }
    Ok(())
}

/// `wg identity fork --from <name> --as <child>` — the **default** "download onto a
/// new host" semantics (ADR-fed-003 §D4): mint a brand-new identity (new root → new
/// `wgid:`) whose genesis cites `<name>` as its fork parent. A verifiable child,
/// cryptographically **not** the parent — it cannot sign as the parent.
pub fn run_fork(workgraph_dir: &Path, from: &str, as_name: &str, json: bool) -> Result<()> {
    if local_path(workgraph_dir, as_name).exists() {
        bail!("identity {as_name:?} already exists locally; pick another name");
    }
    let parent = load_local(workgraph_dir, from)?;
    // Verify the parent we are forking from (its chain must be sound).
    let parent_auth = sigchain::verify(&parent.sigchain, &parent.wgid)
        .with_context(|| format!("verifying parent {from} before fork"))?;
    let pref = sigchain::ParentRef {
        wgid: parent.wgid.clone(),
        sigchain_head: parent_auth.head.clone(),
    };
    let child = mint(as_name, &RecoveryConfig::default(), Some(pref))?;
    save_local(workgraph_dir, &child)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "forked_from": parent.wgid,
                "child_wgid": child.wgid,
                "same_identity": false,
                "saved_as": as_name,
            })
        );
    } else {
        println!("Forked a NEW identity from {}", parent.wgid);
        println!(
            "  child wgid: {}  (a verifiable child — NOT the parent)",
            child.wgid
        );
        println!("  saved local as '{as_name}' (holds its own fresh root)");
        println!(
            "  note: a download is a FORK by default. Continuing as the SAME identity on a \
             new host requires a root-signed add_key (`wg identity enroll-signer`)."
        );
    }
    Ok(())
}

/// `wg identity enroll-signer <name> --store <L>` — the **same-self** continuation
/// boundary (ADR-fed-003 §D4): enroll a fresh signer key onto the *existing* `wgid:`
/// via a root-signed `add_key`. This requires a control a mere downloader lacks (the
/// root in custody, and `add_key` is root-locked, S-4) — so "download and it just
/// works as the same identity" is cryptographically unskippable, by design.
pub fn run_enroll_signer(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    json: bool,
) -> Result<()> {
    let mut id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!(
            "cannot enroll a same-self signer onto {}: this is a downloaded, key-less bundle. \
             Continuing as the SAME identity on a new host requires a root-signed add_key \
             (ADR-fed-003 §D4) — a control a downloader does NOT have. A download is a FORK by \
             default; use `wg identity fork --from {name} --as <child>` instead.",
            id.wgid
        );
    }
    let cust = Custodian::new(name);
    let auth = sigchain::verify(&id.sigchain, &id.wgid)?;
    let active_root_kid = id.cur_root_kid();
    if !cust.has_key(&active_root_kid)? {
        bail!("the active root key is not in this custody — cannot sign add_key (S-4 lock)");
    }
    // Mint the new host's signer key; authorize it with a root-signed add_key.
    let signer = keys::gen_ed25519()?;
    let signer_kid = keys::kid_for(&signer.public);
    cust.store_signing_key(&signer_kid, &signer.seed)?;
    let entry = KeyEntry {
        kid: signer_kid.clone(),
        public: hex::encode(signer.public),
        role: KeyRole::Signer,
        scope: vec!["event".into(), "state".into()],
        status: KeyStatus::Active,
    };
    let link = sigchain::add_key(
        &cust,
        id.sigchain.last().unwrap(),
        &auth.active_root,
        &active_root_kid,
        entry,
    )?;
    id.sigchain.push(link);

    let store = open_store(store_loc)?;
    republish(workgraph_dir, &mut id, store.as_ref())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid, "same_self": true, "enrolled_signer_kid": signer_kid,
                "sigchain_head": id.record.sigchain_head,
            })
        );
    } else {
        println!(
            "Enrolled a same-self signer onto {} (root-authorized add_key)",
            id.wgid
        );
        println!("  new signer kid: {signer_kid}");
        println!("  this host can now author as the SAME identity (not a fork)");
    }
    Ok(())
}

/// `wg identity load-state <name> --store <L> [--from <wgid>] [--author-trust <lvl>]`
/// — the S-5 load pipeline (ADR-fed-004 §D6). Loaded `StateSnapshot`s are **untrusted
/// input**: a valid signature proves *who wrote* it, never that it is *safe to load*.
/// The pipeline is fail-closed — CAS integrity → signature/provenance → model_binding
/// → kind dispatch → AI-input-safety scan → provenance-gate by `trust_level`. Low-trust
/// or flagged state is **never silently consumed**.
pub fn run_load_state(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    from: Option<&str>,
    author_trust: &str,
    json: bool,
) -> Result<()> {
    let me = load_local(workgraph_dir, name)?;
    let store = open_store(store_loc)?;
    let trust = parse_trust(author_trust)?;

    // Whose state are we loading? Default: our own (same-self resume happy path).
    let author_wgid = match from {
        Some(w) => keys::wgid_from_pubkey(&keys::pubkey_from_wgid(w)?),
        None => me.wgid.clone(),
    };
    let same_self = author_wgid == me.wgid;

    // Steps 1–2 (provenance): resolve + verify the author's bundle (chain + record +
    // snapshots all signature/CAS-checked). A forged author fails here.
    let bundle = resolve_bundle(store.as_ref(), &author_wgid)
        .with_context(|| format!("resolving author {author_wgid} for state load"))?;
    let scid = bundle
        .snapshots
        .last()
        .ok_or_else(|| anyhow::anyhow!("author {author_wgid} has no published state snapshot"))?;
    let snap_bytes = store.get_object(scid)?;
    let snap: StateSnapshot =
        serde_json::from_slice(&snap_bytes).context("parsing author state snapshot")?;

    // Step 1 (CAS): the payload bytes must hash to content_cid (tamper-evident).
    let payload = store.get_object(&snap.content_cid)?;
    if payload_cid(&payload) != snap.content_cid {
        bail!("CAS integrity failure: payload does not match content_cid (tampered) — fail closed");
    }
    // Step 2 (signature/provenance) on the specific snapshot.
    snap.verify(&bundle.auth)
        .context("snapshot signature/provenance check failed")?;

    // Step 4 (model_binding): must be present + well-formed; an opaque kind with no
    // binding is contained, never loaded.
    let kind_class = classify_kind(&snap.payload_kind);
    if snap.model_binding.is_none() && kind_class == KindClass::Opaque {
        bail!("opaque snapshot has no model_binding — fail closed (ADR-fed-004 §OQ1)");
    }

    // Step 5 (kind dispatch): an unknown kind degrades gracefully and STOPS.
    if kind_class == KindClass::Unknown {
        return finish_load(
            json,
            &author_wgid,
            same_self,
            trust,
            kind_class,
            &ScanResult::default(),
            &LoadDecision::Refuse {
                reason: format!(
                    "unknown payload_kind {:?} — state present but unreadable by this client; \
                     verified provenance, did not load (ADR-fed-004 §D4)",
                    snap.payload_kind
                ),
            },
            "degrade",
        );
    }

    // Step 6 (AI-input-safety scan): transparent kinds are content-scanned; opaque
    // kinds are contained (not inspected) and forced through the trust gate.
    let scan = if kind_class == KindClass::Transparent {
        let payload_json: serde_json::Value =
            serde_json::from_slice(&payload).unwrap_or(serde_json::Value::Null);
        scan_transparent(&snap.payload_kind, &payload_json)
    } else {
        ScanResult::default()
    };

    // Step 7 (provenance gate): decide auto-load / human-in-loop / refuse.
    let decision = evaluate(trust, same_self, kind_class, &scan);
    finish_load(
        json,
        &author_wgid,
        same_self,
        trust,
        kind_class,
        &scan,
        &decision,
        decision.label(),
    )
}

/// Print the load decision and map it to a process outcome: `AutoLoad` ⇒ loaded /
/// exit 0; `HumanInLoop` ⇒ held / exit 0 (not loaded); `Refuse` ⇒ fail closed (error).
#[allow(clippy::too_many_arguments)]
fn finish_load(
    json: bool,
    author: &str,
    same_self: bool,
    trust: worksgood::graph::TrustLevel,
    kind: KindClass,
    scan: &ScanResult,
    decision: &LoadDecision,
    label: &str,
) -> Result<()> {
    let loaded = decision.loads();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "author": author,
                "same_self": same_self,
                "author_trust": format!("{trust:?}"),
                "kind_class": format!("{kind:?}"),
                "decision": label,
                "loaded": loaded,
                "reason": decision.reason(),
                "hard_hits": scan.hard_hits,
                "soft_hits": scan.soft_hits,
            })
        );
    } else {
        let verb = match decision {
            LoadDecision::AutoLoad => "LOADED",
            LoadDecision::HumanInLoop { .. } => "HELD (human-in-loop)",
            LoadDecision::Refuse { .. } => "REFUSED",
        };
        println!("{verb} state from {author} [{label}]");
        println!("  same-self: {same_self}; author trust: {trust:?}; kind: {kind:?}");
        if let Some(r) = decision.reason() {
            println!("  {r}");
        }
        for h in &scan.hard_hits {
            println!("  scan(block): {h}");
        }
        for h in &scan.soft_hits {
            println!("  scan(escalate): {h}");
        }
    }
    match decision {
        // Fail closed on refuse so a caller can gate on the exit code.
        LoadDecision::Refuse { reason } => bail!("state load refused (S-5, fail-closed): {reason}"),
        _ => Ok(()),
    }
}

/// Decode a 32-byte hex public key.
fn decode_pub32(hexed: &str) -> Result<[u8; 32]> {
    let b = hex::decode(hexed).context("public key not hex")?;
    if b.len() != 32 {
        bail!("public key is {} bytes, expected 32", b.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b);
    Ok(out)
}

// ── Wave 6: UCAN-style capability delegation (issue / verify / revoke) ───────────

/// The custody gate shared by the capability + revocation issuers: a downloaded
/// (key-less) bundle holds no signer and cannot author (download ≠ impersonation).
fn require_signer(id: &LocalIdentity, cust: &Custodian, what: &str) -> Result<()> {
    if !cust.has_key(&id.signer_kid)? {
        bail!(
            "cannot {what} as {:?} ({}): no signer private key in this custody — \
             possessing a downloaded bundle does NOT grant authority (ADR-fed-003 §D1)",
            id.name,
            id.wgid
        );
    }
    Ok(())
}

/// `wg identity delegate` — issue a UCAN-style capability (a root grant or an
/// attenuating-only sub-delegation). Honors the leash dial (broad/long by default,
/// `WG_FED_LEASH_*`-tightenable; §D2/§D3).
#[allow(clippy::too_many_arguments)]
pub fn run_delegate(
    workgraph_dir: &Path,
    from: &str,
    to: &str,
    grants: &[String],
    ttl: Option<i64>,
    parent: Option<&str>,
    human: bool,
    out: Option<&str>,
    store_loc: Option<&str>,
    json: bool,
) -> Result<()> {
    let id = load_local(workgraph_dir, from)?;
    let cust = Custodian::new(from);
    require_signer(&id, &cust, "issue a capability")?;

    let to_pub = keys::pubkey_from_wgid(to)?;
    let to_wgid = keys::wgid_from_pubkey(&to_pub);
    let now = chrono::Utc::now();
    // The dial reads from the environment — unset ⇒ the broad/long birth default.
    let policy = LeashPolicy::from_env();

    // The requested scope: explicit `--grant can@with` pairs, or the broad default.
    let requested_scope = if grants.is_empty() {
        Scope::broad_default(&id.wgid)
    } else {
        Scope::new(
            grants
                .iter()
                .map(|g| custody::parse_ability(g))
                .collect::<Result<Vec<_>>>()?,
        )
    };

    let cap = match parent {
        None => custody::issue_root(
            &cust,
            &id.signer_kid,
            &id.wgid,
            &to_wgid,
            requested_scope,
            ttl,
            now,
            &policy,
            human,
        )?,
        Some(pfile) => {
            let pbytes = std::fs::read(pfile).with_context(|| format!("reading parent {pfile}"))?;
            // Depth-capped parse (audit M13): refuse an over-deep delegation chain.
            let parent_cap =
                custody::capability_from_slice(&pbytes).context("parsing parent capability")?;
            // The delegator must be the parent's audience (it sub-delegates its grant).
            if parent_cap.aud != id.wgid {
                bail!(
                    "cannot sub-delegate: {from} ({}) is not the parent capability's \
                     audience ({})",
                    id.wgid,
                    parent_cap.aud
                );
            }
            // No explicit grants ⇒ pass through the parent's scope (still attenuating).
            let scope = if grants.is_empty() {
                parent_cap.scope.clone()
            } else {
                requested_scope
            };
            custody::delegate(
                &cust,
                &id.signer_kid,
                &parent_cap,
                &to_wgid,
                scope,
                ttl,
                now,
                &policy,
            )?
        }
    };

    if let Some(o) = out {
        std::fs::write(o, serde_json::to_vec_pretty(&cap)?)
            .with_context(|| format!("writing capability to {o}"))?;
    }
    if let Some(s) = store_loc {
        let store = open_store(s)?;
        store.put_object(&cap.cid(), &serde_json::to_vec(&cap)?)?;
    }

    let granted: Vec<String> = cap
        .scope
        .abilities
        .iter()
        .map(|a| format!("{}@{}", a.can, a.with))
        .collect();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "cid": cap.cid(),
                "iss": cap.iss,
                "aud": cap.aud,
                "expires": cap.expires,
                "granted": granted,
                "chain_len": cap.chain_len(),
                "leash_slack": policy.is_slack(),
                "human": human,
                "capability": cap,
            })
        );
    } else {
        println!("Issued capability {}", cap.cid());
        println!("  iss: {}", cap.iss);
        println!("  aud: {}", cap.aud);
        println!("  expires: {}", cap.expires);
        println!("  granted: {}", granted.join(", "));
        println!(
            "  leash: {} (chain depth {})",
            if policy.is_slack() {
                "slack (broad/long birth default)"
            } else {
                "tightened by environment policy"
            },
            cap.chain_len()
        );
    }
    Ok(())
}

/// `wg identity verify-cap` — verify a capability chain offline (signatures,
/// attenuation, expiry, revocation). Exits non-zero on invalid / expired / revoked.
pub fn run_verify_cap(
    workgraph_dir: &Path,
    cap_file: &str,
    store_loc: &str,
    json: bool,
) -> Result<()> {
    // Depth-capped parse (audit M13): refuse an over-deep delegation chain before the
    // recursive verify can be driven into a stack overflow.
    let cap = custody::capability_from_slice(
        &std::fs::read(cap_file).with_context(|| format!("reading {cap_file}"))?,
    )?;
    let store = open_store(store_loc)?;
    let now = chrono::Utc::now();

    // Map every cid → iss along the presented chain so we only honor a revocation
    // that names a cap in THIS chain and is signed by that cap's own issuer.
    let mut chain_iss: Vec<(String, String)> = Vec::new();
    let mut cur = Some(&cap);
    while let Some(c) = cur {
        chain_iss.push((c.cid(), c.iss.clone()));
        cur = c.proof.as_deref();
    }

    // Discover + authenticate revocations from the store's reserved list (a hint —
    // each is re-verified, never trusted blindly).
    let mut revoked: Vec<String> = Vec::new();
    if let Ok(events) = store.list_events(REVOCATION_INBOX) {
        for ev in events {
            let Ok(rev) = serde_json::from_slice::<Revocation>(&ev.bytes) else {
                continue;
            };
            let Some((_, expected_iss)) = chain_iss.iter().find(|(cid, _)| cid == &rev.cap_cid)
            else {
                continue; // not about a cap in this chain
            };
            if &rev.revoked_by != expected_iss {
                continue; // only the issuing identity may revoke its capability
            }
            if let Ok(auth) = resolve_auth_cached(workgraph_dir, store.as_ref(), &rev.revoked_by) {
                if rev.verify_signature(&auth).is_ok() {
                    revoked.push(rev.cap_cid.clone());
                }
            }
        }
    }

    let resolve = |w: &str| resolve_auth_cached(workgraph_dir, store.as_ref(), w);
    let verdict = custody::verify(&cap, now, &revoked, &resolve);

    match &verdict {
        Ok(v) => {
            let granted: Vec<String> = v
                .granted
                .abilities
                .iter()
                .map(|a| format!("{}@{}", a.can, a.with))
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "valid": true,
                        "principal": v.principal,
                        "aud": v.aud,
                        "granted": granted,
                        "chain_len": v.chain_len,
                    })
                );
            } else {
                println!("VALID capability for {}", v.aud);
                println!("  principal: {}", v.principal);
                println!("  granted: {}", granted.join(", "));
                println!("  chain depth: {}", v.chain_len);
            }
            Ok(())
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "valid": false, "reason": e.to_string() })
                );
            } else {
                println!("INVALID capability: {e}");
            }
            bail!("capability verification failed: {e}")
        }
    }
}

/// `wg identity revoke-cap` — revoke a capability and its whole delegated subtree
/// (issuer-subtree revocation, §D3). Publishes a signed revocation to the store.
pub fn run_revoke_cap(
    workgraph_dir: &Path,
    from: &str,
    cap_file: &str,
    store_loc: &str,
    json: bool,
) -> Result<()> {
    let id = load_local(workgraph_dir, from)?;
    let cust = Custodian::new(from);
    require_signer(&id, &cust, "revoke a capability")?;

    // Depth-capped parse (audit M13).
    let cap = custody::capability_from_slice(
        &std::fs::read(cap_file).with_context(|| format!("reading {cap_file}"))?,
    )?;
    if cap.iss != id.wgid {
        bail!(
            "only the capability's issuer ({}) may revoke it; {from} is {}",
            cap.iss,
            id.wgid
        );
    }

    let now = chrono::Utc::now();
    let rev = Revocation::issue(&cust, &id.signer_kid, &cap, now)?;
    let rev_bytes = serde_json::to_vec(&rev)?;
    let store = open_store(store_loc)?;
    store.put_object(&rev.cid(), &rev_bytes)?;
    store.put_event(REVOCATION_INBOX, &rev.cid(), &rev_bytes)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "revoked": true,
                "cap_cid": rev.cap_cid,
                "revoked_by": rev.revoked_by,
                "revocation_cid": rev.cid(),
            })
        );
    } else {
        println!(
            "Revoked capability {} (and its delegated subtree)",
            rev.cap_cid
        );
        println!("  by: {}", rev.revoked_by);
    }
    Ok(())
}
