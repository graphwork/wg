//! `wg identity` — the WG-Fed spark surface (Wave 3).
//!
//! Implements the thinnest end-to-end slice that passes the seven-step spark test
//! (`docs/federation-study/06-decision-memo-and-roadmap.md` §4): mint a
//! self-certifying identity whose root never leaves custody; publish/fetch a
//! self-verifying `IdentityRecord` + `StateSnapshot` to a **dumb, untrusted third
//! location** `L`; and send/poll a signed (optionally sealed) cross-graph
//! `SignedEvent` over a store-and-forward inbox.
//!
//! The "third location" and the inbox are realized here as the simplest possible
//! untrusted transport — a plain directory (the spark transport = "anything that
//! returns bytes"; the HTTP/relay rungs harden in Wave 4, ADR-fed-002). Every
//! artifact `L` serves is self-verifying, so `L` is never trusted: it cannot
//! forge an identity or an author (verification is pure local crypto rooted at the
//! `wgid:`, ADR-fed-001 §D5).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use worksgood::identity::envelope::{
    AgentFields, Endpoint, IdentityRecord, SignedEvent, StateSnapshot, payload_cid,
};
use worksgood::identity::keys::{self, Custodian};
use worksgood::identity::sigchain::{
    self, AuthorizedKeys, KeyEntry, KeyRole, KeyStatus, SigchainLink,
};
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
}

fn identity_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("identity")
}

fn local_path(workgraph_dir: &Path, name: &str) -> PathBuf {
    identity_dir(workgraph_dir).join(format!("{name}.json"))
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

// ── The dumb object store + inbox (the untrusted third location L) ─────────────

/// Map a content id / wgid to a filesystem-safe leaf name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn store_root(store: &str) -> PathBuf {
    // Accept a bare path or a `file://` URI (HTTP/relay rungs are Wave 4).
    let s = store.strip_prefix("file://").unwrap_or(store);
    PathBuf::from(s)
}

fn objects_dir(store: &str) -> PathBuf {
    store_root(store).join("objects")
}
fn heads_dir(store: &str) -> PathBuf {
    store_root(store).join("heads")
}
fn inbox_dir(store: &str, wgid: &str) -> PathBuf {
    store_root(store).join("inbox").join(sanitize(wgid))
}

/// Put a content-addressed object. The CID is computed by the caller and is the
/// integrity check — `L` cannot tamper without breaking it.
fn store_put(store: &str, cid: &str, bytes: &[u8]) -> Result<()> {
    let dir = objects_dir(store);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(sanitize(cid)), bytes)?;
    Ok(())
}

fn store_get(store: &str, cid: &str) -> Result<Vec<u8>> {
    let path = objects_dir(store).join(sanitize(cid));
    std::fs::read(&path).with_context(|| format!("object {cid} not found in store {store}"))
}

/// A head pointer published at `L` for a `wgid` (mutable, untrusted — it only
/// points at self-verifying objects, so a forged head cannot forge an identity).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Head {
    record: String,
    #[serde(default)]
    snapshots: Vec<String>,
}

fn head_put(store: &str, wgid: &str, head: &Head) -> Result<()> {
    let dir = heads_dir(store);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(sanitize(wgid)), serde_json::to_vec(head)?)?;
    Ok(())
}

fn head_get(store: &str, wgid: &str) -> Result<Head> {
    let path = heads_dir(store).join(sanitize(wgid));
    let bytes =
        std::fs::read(&path).with_context(|| format!("no published head for {wgid} at {store}"))?;
    serde_json::from_slice(&bytes).context("parsing head pointer")
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
    })
}

// ── resolve + verify a bundle from the store (offline, self-certifying) ────────

/// A fetched, fully-verified identity bundle.
struct ResolvedBundle {
    record: IdentityRecord,
    chain: Vec<SigchainLink>,
    auth: AuthorizedKeys,
    snapshots: Vec<String>,
}

/// Fetch a `wgid`'s record + sigchain from `L` and **verify offline** — the
/// signature checks against the genesis pubkey embedded in the address; no call to
/// the origin and no central authority (ADR-fed-001 §D5). Walks the content-
/// addressed sigchain from `record.sigchain_head` back to genesis.
fn resolve_bundle(store: &str, wgid: &str) -> Result<ResolvedBundle> {
    let head = head_get(store, wgid)?;
    let record_bytes = store_get(store, &head.record)?;
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
        let link_bytes = store_get(store, &cid)?;
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
    })
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
/// `IdentityRecord` + sigchain + one `StateSnapshot` to the dumb location `L`
/// (spark step 2). No private key is ever written.
pub fn run_publish(workgraph_dir: &Path, name: &str, store: &str, json: bool) -> Result<()> {
    let id = load_local(workgraph_dir, name)?;
    if !id.holds_private {
        bail!("identity {name:?} is a downloaded bundle; you can only publish your own");
    }

    // Publish each sigchain link as a content-addressed object.
    for link in &id.sigchain {
        let bytes = serde_json::to_vec(link)?;
        store_put(store, &link.cid(), &bytes)?;
    }
    // Publish the IdentityRecord.
    let record_cid = id.record.cid();
    store_put(store, &record_cid, &serde_json::to_vec(&id.record)?)?;

    // Build + sign + publish one StateSnapshot (payload_kind conv-cache-v1).
    let cust = Custodian::new(name);
    let payload = serde_json::to_vec(&serde_json::json!({
        "kind": "conv-cache-v1",
        "owner": id.wgid,
        "turns": [{"role": "system", "text": "wg-fed spark conversation cache"}],
    }))?;
    let pcid = payload_cid(&payload);
    store_put(store, &pcid, &payload)?;
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
    store_put(store, &snap_cid, &serde_json::to_vec(&snap)?)?;

    head_put(
        store,
        &id.wgid,
        &Head {
            record: record_cid.clone(),
            snapshots: vec![snap_cid.clone()],
        },
    )?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "wgid": id.wgid,
                "record_cid": record_cid,
                "snapshot_cid": snap_cid,
                "store": store,
            })
        );
    } else {
        println!("Published '{}' to {store}", id.name);
        println!("  wgid:         {}", id.wgid);
        println!("  record cid:   {record_cid}");
        println!("  snapshot cid: {snap_cid}");
        println!("  (bundle carries NO private key — verify with `wg identity fetch`)");
    }
    Ok(())
}

/// `wg identity fetch <wgid> --store <L> [--save <name>]` — fetch + verify a
/// record offline against `L` alone (spark steps 3 & 7). Optionally cache the
/// key-less bundle locally under `<name>`.
pub fn run_fetch(
    workgraph_dir: &Path,
    wgid: &str,
    store: &str,
    save: Option<&str>,
    json: bool,
) -> Result<()> {
    // Normalize a did:key spelling to the wgid the store is keyed by.
    let pubkey = keys::pubkey_from_wgid(wgid)?;
    let wgid = keys::wgid_from_pubkey(&pubkey);

    let bundle = resolve_bundle(store, &wgid)?;

    // Verify the published snapshot too, if any (step 2/7 completeness).
    let mut verified_snapshots = 0usize;
    for scid in &bundle.snapshots {
        let bytes = store_get(store, scid)?;
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
/// (spark step 4). Authoring requires the **signer private key in custody**:
/// a downloaded bundle cannot author (the impersonation defense, step 6).
#[allow(clippy::too_many_arguments)]
pub fn run_send(
    workgraph_dir: &Path,
    from: &str,
    to: &str,
    store: &str,
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

    let mut ev = if seal {
        // Need the recipient's enc key — fetch + verify their bundle from L.
        let recipient = resolve_bundle(store, &to_wgid)
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
    let dir = inbox_dir(store, &to_wgid);
    std::fs::create_dir_all(&dir)?;
    let event_bytes = serde_json::to_vec(&ev)?;
    std::fs::write(dir.join(format!("{}.json", sanitize(&ev.id))), &event_bytes)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event_id": ev.id,
                "from": ev.from,
                "to": to_wgid,
                "sealed": seal,
                "accepted": true,
            })
        );
    } else {
        println!("Accepted for delivery (store-and-forward; recipient may be offline)");
        println!("  event id: {}", ev.id);
        println!("  from: {}", ev.from);
        println!("  to:   {to_wgid}");
        println!("  sealed: {seal}");
    }
    Ok(())
}

/// `wg identity poll <name> --store <L>` — fetch events from `L`'s inbox and
/// authenticate each by key (spark step 5). Prints a per-event verdict; a forged
/// "from" or a tampered event is REJECTED. Sealed events addressed to us are
/// opened with our custody-held encryption key.
pub fn run_poll(workgraph_dir: &Path, name: &str, store: &str, json: bool) -> Result<()> {
    let id = load_local(workgraph_dir, name)?;
    let cust = Custodian::new(name);
    let dir = inbox_dir(store, &id.wgid);

    let mut verdicts: Vec<serde_json::Value> = Vec::new();
    let mut accepted = 0usize;
    let mut rejected = 0usize;

    if dir.exists() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        entries.sort();
        for path in entries {
            let bytes = std::fs::read(&path)?;
            let verdict = authenticate_event(&bytes, store, &id, &cust);
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

/// Verify one inbox event: resolve the sender's bundle from `L`, verify the
/// signature against the sender's authorized signer set, then (if sealed and
/// addressed to us) open it. Returns `(from_wgid, body)` on success.
fn authenticate_event(
    bytes: &[u8],
    store: &str,
    me: &LocalIdentity,
    cust: &Custodian,
) -> Result<(String, String)> {
    let ev: SignedEvent = serde_json::from_slice(bytes).context("parsing inbox event")?;
    // Resolve and verify the *claimed* sender's identity from L.
    let sender = resolve_bundle(store, &ev.from)
        .with_context(|| format!("resolving claimed sender {}", ev.from))?;
    // Authenticate: signature must verify against a key the sender's chain
    // authorizes. A forged "from" (wrong signature) fails here.
    ev.verify(&sender.auth)?;

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

/// `wg identity verify <file> [--store <L>]` — verify a record or event file
/// offline. For an event, the sender's chain is resolved from `--store`.
pub fn run_verify(
    _workgraph_dir: &Path,
    file: &str,
    store: Option<&str>,
    json: bool,
) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {file}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).context("parsing JSON")?;

    let (kind, ok, detail) = if value.get("from").is_some() && value.get("kind").is_some() {
        // A SignedEvent — needs the sender's chain.
        let store = store.ok_or_else(|| {
            anyhow::anyhow!("verifying a SignedEvent needs --store to resolve the sender")
        })?;
        let ev: SignedEvent = serde_json::from_value(value).context("parsing SignedEvent")?;
        let from = ev.from.clone();
        match resolve_bundle(store, &from).and_then(|b| ev.verify(&b.auth)) {
            Ok(()) => ("SignedEvent", true, format!("authored by {from}")),
            Err(e) => ("SignedEvent", false, e.to_string()),
        }
    } else if value.get("sigchain_head").is_some() {
        // An IdentityRecord — self-contained needs its chain from the store.
        let rec: IdentityRecord =
            serde_json::from_value(value).context("parsing IdentityRecord")?;
        let id = rec.id.clone();
        match store {
            Some(store) => match resolve_bundle(store, &id) {
                Ok(_) => ("IdentityRecord", true, format!("verified {id}")),
                Err(e) => ("IdentityRecord", false, e.to_string()),
            },
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
