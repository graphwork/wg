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
use worksgood::identity::transport::{FedStore, Head, open_store};
use worksgood::identity::{ALG_ED25519, ENVELOPE_V, WG_FED_COMPAT_VERSION};

// ── Local identity state (public; private keys live only in custody) ───────────

/// Everything WG needs locally to *use* an identity it minted, or to *reference*
/// one it fetched. Private keys are **never** here — they live in `wg secret`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LocalIdentity {
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
    record: IdentityRecord,
    sigchain: Vec<SigchainLink>,
    /// Highest freshness-attestation `seq` this identity has *issued* (monotonic).
    /// Bumped on every `publish`/`attest` so a verifier can detect rollback.
    #[serde(default)]
    freshness_seq: u64,
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

fn load_local(workgraph_dir: &Path, name: &str) -> Result<LocalIdentity> {
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

/// Mint root/signer/enc keys into custody, build the genesis+add_key sigchain,
/// sign the `IdentityRecord`, and persist local public state. The root private
/// key is written to `wg secret` and **never returned or displayed**.
fn mint(name: &str) -> Result<LocalIdentity> {
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

    // Build the sigchain: genesis (root self-signed) → add_key signer → add_key enc.
    // add_key links are signed by the root (the hydra lock, S-4).
    let g = sigchain::genesis(&cust, &root.public, &root_kid, None)?;
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
        root_kid,
        signer_kid,
        enc_kid,
        holds_private: true,
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
fn resolve_auth_cached(
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
    id.freshness_seq += 1;
    let mut att = FreshnessAttestation::build(
        &id.wgid,
        &id.record.sigchain_head,
        chrono::Utc::now(),
        ttl_secs,
        id.freshness_seq,
    );
    att.sign(&cust, &id.signer_kid)?;
    let att_cid = att.cid();
    store.put_object(&att_cid, &serde_json::to_vec(&att)?)?;
    store.put_attestation(&id.wgid, &serde_json::to_vec(&att)?)?;
    save_local(workgraph_dir, id)?;
    Ok(att)
}

// ── Command handlers ───────────────────────────────────────────────────────────

/// `wg identity new <name>` — mint a self-certifying identity (spark step 1).
pub fn run_new(workgraph_dir: &Path, name: &str, json: bool) -> Result<()> {
    if local_path(workgraph_dir, name).exists() {
        bail!("identity {name:?} already exists locally; pick another name");
    }
    let id = mint(name)?;
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
            })
        );
    } else {
        println!("Minted WG-Fed identity '{}'", id.name);
        println!("  wgid: {}", id.wgid);
        println!("  signer kid: {}", id.signer_kid);
        println!("  enc kid:    {}", id.enc_kid);
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

    // Build + sign + publish one StateSnapshot (payload_kind conv-cache-v1).
    let cust = Custodian::new(name);
    let payload = serde_json::to_vec(&serde_json::json!({
        "kind": "conv-cache-v1",
        "owner": id.wgid,
        "turns": [{"role": "system", "text": "wg-fed spark conversation cache"}],
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

    store.put_head(
        &id.wgid,
        &Head {
            record: record_cid.clone(),
            snapshots: vec![snap_cid.clone()],
            attestation: Some(att_cid.clone()),
        },
    )?;

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
    // Update the head's attestation pointer if a head already exists.
    if let Ok(mut head) = store.get_head(&id.wgid) {
        head.attestation = Some(att.cid());
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
    to: &str,
    store_loc: &str,
    body: &str,
    kind: &str,
    seal: bool,
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

    let to_pub = keys::pubkey_from_wgid(to)?;
    let to_wgid = keys::wgid_from_pubkey(&to_pub);
    let created_at = chrono::Utc::now().to_rfc3339();
    let store = open_store(store_loc)?;

    let mut ev = if seal {
        // Need the recipient's enc key — fetch + verify their bundle from L.
        let recipient = resolve_bundle(store.as_ref(), &to_wgid)
            .with_context(|| format!("resolving recipient {to_wgid} to seal to"))?;
        let (enc_kid, enc_pub) = enc_pub_of(&recipient.auth)?;
        SignedEvent::new_sealed(
            &id.wgid,
            std::slice::from_ref(&to_wgid),
            &created_at,
            kind,
            body,
            &enc_kid,
            &enc_pub,
        )?
    } else {
        SignedEvent::new_plain(
            &id.wgid,
            std::slice::from_ref(&to_wgid),
            &created_at,
            kind,
            body,
        )
    };
    ev.sign(&cust, &id.signer_kid)?;

    // Deliver: write to the recipient's inbox at L (store-and-forward; the
    // recipient may be offline and will receive it on its next poll).
    let event_bytes = serde_json::to_vec(&ev)?;
    store.put_event(&to_wgid, &ev.id, &event_bytes)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event_id": ev.id,
                "from": ev.from,
                "to": to_wgid,
                "kind": kind,
                "sealed": seal,
                "accepted": true,
                "store": store_loc,
            })
        );
    } else {
        println!("Accepted for delivery (store-and-forward; recipient may be offline)");
        println!("  event id: {}", ev.id);
        println!("  from: {}", ev.from);
        println!("  to:   {to_wgid}");
        println!("  via:  {store_loc}");
        println!("  sealed: {seal}");
    }
    Ok(())
}

/// `wg identity poll <name> --store <L>` — fetch events from `L`'s inbox and
/// authenticate each by key (spark step 5 / Wave 4 node inbox). A forged "from" or a
/// tampered event is REJECTED. Sealed events addressed to us are opened with our
/// custody-held encryption key. When `require_fresh` is set, accepting an event is
/// **gated on a fresh attestation** for the sender and **fails closed on stale**.
pub fn run_poll(
    workgraph_dir: &Path,
    name: &str,
    store_loc: &str,
    require_fresh: Option<&str>,
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
                verdicts.push(serde_json::json!({
                    "verdict": "VERIFIED", "from": from, "body": body,
                }));
                if !json {
                    println!("VERIFIED from {from}: {body}");
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
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid,
                "accepted": accepted,
                "rejected": rejected,
                "events": verdicts,
            })
        );
    } else {
        println!(
            "polled {} ({accepted} verified, {rejected} rejected)",
            id.wgid
        );
    }
    Ok(())
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

    let body = if ev.enc.is_some() {
        ev.open(cust)
            .context("event is sealed but could not be opened with our key")?
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
