//! WG-Fed transport: the untrusted store-and-forward substrate (ADR-fed-002).
//!
//! Wave 3 (the spark) carried the entire transport as a single rung: a **dumb
//! directory** that returns bytes. Wave 4 generalizes that rung into a small
//! [`FedStore`] trait — *put/get content-addressed objects, head pointers,
//! freshness attestations, and store-and-forward inbox events* — and adds the
//! **default network rung**: the WG node's HTTP store-and-forward inbox
//! ([`HttpStore`] talking to [`super::node`]). The two implementations are wire-
//! compatible: the same `SignedEvent`/`IdentityRecord`/sigchain bytes traverse a
//! `file://` directory or an `http://` node with identical semantics.
//!
//! Crucially, the transport is **never trusted** (ADR-fed-002 §D3): every object
//! is content-addressed (a flipped byte breaks its CID) and every event is signed
//! (a forged author fails the signature check at the recipient). A `FedStore` —
//! including your own node — can lose or reorder bytes, but it can neither forge an
//! identity nor read a sealed body. Resolution of *where* to deliver is the
//! ADR-fed-001 §D5 cascade in [`super::super::federation`]; this module is only the
//! "anything that returns bytes" mechanism underneath it.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Wire-path prefix shared by [`HttpStore`] and the node server ([`super::node`]).
pub const API_PREFIX: &str = "/wgfed/v1";

/// Map a content id / wgid / event id to a filesystem-and-URL-safe leaf name.
///
/// Identical to the spark's `sanitize` (the smoke `san()` helper mirrors it), so a
/// directory store written by Wave 3 is readable by Wave 4 and vice-versa.
pub fn sanitize(s: &str) -> String {
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

/// A head pointer published for a `wgid` (mutable, untrusted — it only points at
/// self-verifying objects, so a forged head cannot forge an identity).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Head {
    pub record: String,
    #[serde(default)]
    pub snapshots: Vec<String>,
    /// CID of the latest published freshness attestation, if any (S-3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<String>,
}

/// One event sitting in an inbox: its id (dedup key) and raw bytes (verbatim, so a
/// recipient verifies the exact bytes the sender signed).
pub struct InboxEvent {
    pub id: String,
    pub bytes: Vec<u8>,
}

/// The untrusted store-and-forward substrate. Two rungs implement it: a local
/// directory ([`FileStore`]) and a remote WG node ([`HttpStore`]). Both are
/// **untrusted**: integrity rides on content-addressing + signatures, never on the
/// store.
pub trait FedStore {
    /// Put a content-addressed object (sigchain link, IdentityRecord, StateSnapshot,
    /// payload). The CID is the integrity check — the store cannot tamper.
    fn put_object(&self, cid: &str, bytes: &[u8]) -> Result<()>;
    /// Get a content-addressed object by CID.
    fn get_object(&self, cid: &str) -> Result<Vec<u8>>;

    /// Publish the head pointer for a `wgid`.
    fn put_head(&self, wgid: &str, head: &Head) -> Result<()>;
    /// Fetch the head pointer for a `wgid`.
    fn get_head(&self, wgid: &str) -> Result<Head>;

    /// Deliver an event into a recipient's store-and-forward inbox (at-least-once;
    /// idempotent by event id — re-delivering the same id is a no-op overwrite).
    fn put_event(&self, recipient_wgid: &str, event_id: &str, bytes: &[u8]) -> Result<()>;
    /// List + fetch all events currently in a recipient's inbox (sorted by id).
    fn list_events(&self, recipient_wgid: &str) -> Result<Vec<InboxEvent>>;

    /// Publish a freshness attestation for a `wgid` (S-3; latest wins).
    fn put_attestation(&self, wgid: &str, bytes: &[u8]) -> Result<()>;
    /// Fetch the freshness attestation for a `wgid`, or `None` if none published.
    fn get_attestation(&self, wgid: &str) -> Result<Option<Vec<u8>>>;
}

/// Open a `--store` reference into a concrete transport rung.
///
/// - `http://` / `https://` → [`HttpStore`] (the default network rung, a WG node).
/// - anything else (a bare path or `file://`) → [`FileStore`] (the dumb directory).
///
/// This keeps every `wg identity … --store <L>` working unchanged with a directory
/// **and** newly with an `http://` node — the Wave-3 surface is forward-compatible.
pub fn open_store(reference: &str) -> Result<Box<dyn FedStore>> {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        Ok(Box::new(HttpStore::new(reference)))
    } else {
        Ok(Box::new(FileStore::new(reference)))
    }
}

// ── FileStore: the dumb directory rung (Wave 3, unchanged layout) ──────────────

/// A plain directory used as a dumb, untrusted object store + inbox. Layout:
/// `objects/<cid>`, `heads/<wgid>`, `inbox/<wgid>/<id>.json`,
/// `attestations/<wgid>` — the Wave-3 spark layout plus the additive
/// `attestations/` dir.
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn new(reference: &str) -> Self {
        // Accept a bare path or a `file://` URI.
        let s = reference.strip_prefix("file://").unwrap_or(reference);
        Self {
            root: PathBuf::from(s),
        }
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }
    fn heads_dir(&self) -> PathBuf {
        self.root.join("heads")
    }
    fn inbox_dir(&self, wgid: &str) -> PathBuf {
        self.root.join("inbox").join(sanitize(wgid))
    }
    fn attest_dir(&self) -> PathBuf {
        self.root.join("attestations")
    }
}

impl FedStore for FileStore {
    fn put_object(&self, cid: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.objects_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(sanitize(cid)), bytes)?;
        Ok(())
    }

    fn get_object(&self, cid: &str) -> Result<Vec<u8>> {
        let path = self.objects_dir().join(sanitize(cid));
        std::fs::read(&path).with_context(|| format!("object {cid} not found in store"))
    }

    fn put_head(&self, wgid: &str, head: &Head) -> Result<()> {
        let dir = self.heads_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(sanitize(wgid)), serde_json::to_vec(head)?)?;
        Ok(())
    }

    fn get_head(&self, wgid: &str) -> Result<Head> {
        let path = self.heads_dir().join(sanitize(wgid));
        let bytes =
            std::fs::read(&path).with_context(|| format!("no published head for {wgid}"))?;
        serde_json::from_slice(&bytes).context("parsing head pointer")
    }

    fn put_event(&self, recipient_wgid: &str, event_id: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.inbox_dir(recipient_wgid);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{}.json", sanitize(event_id))), bytes)?;
        Ok(())
    }

    fn list_events(&self, recipient_wgid: &str) -> Result<Vec<InboxEvent>> {
        let dir = self.inbox_dir(recipient_wgid);
        let mut out = Vec::new();
        if dir.exists() {
            let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
                .collect();
            paths.sort();
            for path in paths {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let bytes = std::fs::read(&path)?;
                out.push(InboxEvent { id, bytes });
            }
        }
        Ok(out)
    }

    fn put_attestation(&self, wgid: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.attest_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(sanitize(wgid)), bytes)?;
        Ok(())
    }

    fn get_attestation(&self, wgid: &str) -> Result<Option<Vec<u8>>> {
        let path = self.attest_dir().join(sanitize(wgid));
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading attestation for {wgid}")),
        }
    }
}

// ── HttpStore: the default network rung (a WG node, ADR-fed-002 §D1 rung 1) ─────

/// The default transport rung: an HTTP client against a WG node's store-and-forward
/// inbox ([`super::node`]). Talks the same object/head/inbox/attestation surface as
/// [`FileStore`], over `reqwest::blocking`. The node is **untrusted** — it holds and
/// forwards self-verifying bytes; it cannot forge or read sealed content.
pub struct HttpStore {
    base: String,
    client: reqwest::blocking::Client,
}

impl HttpStore {
    pub fn new(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self { base, client }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base, API_PREFIX, path)
    }
}

impl FedStore for HttpStore {
    fn put_object(&self, cid: &str, bytes: &[u8]) -> Result<()> {
        let url = self.url(&format!("/objects/{}", sanitize(cid)));
        let resp = self
            .client
            .put(&url)
            .body(bytes.to_vec())
            .send()
            .with_context(|| format!("PUT {url}"))?;
        if !resp.status().is_success() {
            bail!("node rejected object {cid}: HTTP {}", resp.status());
        }
        Ok(())
    }

    fn get_object(&self, cid: &str) -> Result<Vec<u8>> {
        let url = self.url(&format!("/objects/{}", sanitize(cid)));
        let resp = self
            .client
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("object {cid} not found at node: HTTP {}", resp.status());
        }
        Ok(resp.bytes()?.to_vec())
    }

    fn put_head(&self, wgid: &str, head: &Head) -> Result<()> {
        let url = self.url(&format!("/heads/{}", sanitize(wgid)));
        let resp = self
            .client
            .put(&url)
            .body(serde_json::to_vec(head)?)
            .send()
            .with_context(|| format!("PUT {url}"))?;
        if !resp.status().is_success() {
            bail!("node rejected head for {wgid}: HTTP {}", resp.status());
        }
        Ok(())
    }

    fn get_head(&self, wgid: &str) -> Result<Head> {
        let url = self.url(&format!("/heads/{}", sanitize(wgid)));
        let resp = self
            .client
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!(
                "no published head for {wgid} at node: HTTP {}",
                resp.status()
            );
        }
        let bytes = resp.bytes()?;
        serde_json::from_slice(&bytes).context("parsing head pointer from node")
    }

    fn put_event(&self, recipient_wgid: &str, event_id: &str, bytes: &[u8]) -> Result<()> {
        let url = self.url(&format!(
            "/inbox/{}/{}",
            sanitize(recipient_wgid),
            sanitize(event_id)
        ));
        let resp = self
            .client
            .put(&url)
            .body(bytes.to_vec())
            .send()
            .with_context(|| format!("PUT {url} (deliver to offline recipient)"))?;
        if !resp.status().is_success() {
            bail!(
                "node rejected delivery to {recipient_wgid}: HTTP {}",
                resp.status()
            );
        }
        Ok(())
    }

    fn list_events(&self, recipient_wgid: &str) -> Result<Vec<InboxEvent>> {
        let index_url = self.url(&format!("/inbox/{}", sanitize(recipient_wgid)));
        let resp = self
            .client
            .get(&index_url)
            .send()
            .with_context(|| format!("GET {index_url}"))?;
        if !resp.status().is_success() {
            bail!(
                "could not list inbox for {recipient_wgid} at node: HTTP {}",
                resp.status()
            );
        }
        #[derive(serde::Deserialize)]
        struct Index {
            #[serde(default)]
            events: Vec<String>,
        }
        let index: Index = resp.json().context("parsing inbox index from node")?;
        let mut out = Vec::with_capacity(index.events.len());
        let mut ids = index.events;
        ids.sort();
        for id in ids {
            let url = self.url(&format!(
                "/inbox/{}/{}",
                sanitize(recipient_wgid),
                sanitize(&id)
            ));
            let r = self
                .client
                .get(&url)
                .send()
                .with_context(|| format!("GET {url}"))?;
            if !r.status().is_success() {
                // Event vanished between index and fetch (GC race) — skip it.
                continue;
            }
            out.push(InboxEvent {
                id,
                bytes: r.bytes()?.to_vec(),
            });
        }
        Ok(out)
    }

    fn put_attestation(&self, wgid: &str, bytes: &[u8]) -> Result<()> {
        let url = self.url(&format!("/attestations/{}", sanitize(wgid)));
        let resp = self
            .client
            .put(&url)
            .body(bytes.to_vec())
            .send()
            .with_context(|| format!("PUT {url}"))?;
        if !resp.status().is_success() {
            bail!(
                "node rejected attestation for {wgid}: HTTP {}",
                resp.status()
            );
        }
        Ok(())
    }

    fn get_attestation(&self, wgid: &str) -> Result<Option<Vec<u8>>> {
        let url = self.url(&format!("/attestations/{}", sanitize(wgid)));
        let resp = self
            .client
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("fetching attestation for {wgid}: HTTP {}", resp.status());
        }
        Ok(Some(resp.bytes()?.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let mut p = std::env::temp_dir();
        let n: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
            ^ (std::process::id() as u64).wrapping_shl(20);
        p.push(format!("wg-fed-transport-{n}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn open_store_routes_by_scheme() {
        // A bare path / file:// → FileStore; http(s):// → HttpStore. We can only
        // observe the routing indirectly, so exercise the FileStore round-trip.
        let dir = tmp();
        let s = open_store(dir.to_str().unwrap()).unwrap();
        s.put_object("b3:abc", b"hello").unwrap();
        assert_eq!(s.get_object("b3:abc").unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filestore_object_head_inbox_roundtrip() {
        let dir = tmp();
        let s = FileStore::new(dir.to_str().unwrap());

        // objects
        s.put_object("b3:deadbeef", b"obj").unwrap();
        assert_eq!(s.get_object("b3:deadbeef").unwrap(), b"obj");
        assert!(s.get_object("b3:missing").is_err());

        // head
        let head = Head {
            record: "b3:rec".into(),
            snapshots: vec!["b3:snap".into()],
            attestation: Some("b3:att".into()),
        };
        s.put_head("wgid:zAlice", &head).unwrap();
        let got = s.get_head("wgid:zAlice").unwrap();
        assert_eq!(got.record, "b3:rec");
        assert_eq!(got.attestation.as_deref(), Some("b3:att"));

        // inbox: deliver two events, list them back sorted, idempotent re-put
        s.put_event("wgid:zBob", "evt-2", b"two").unwrap();
        s.put_event("wgid:zBob", "evt-1", b"one").unwrap();
        s.put_event("wgid:zBob", "evt-1", b"one").unwrap(); // dedup overwrite
        let events = s.list_events("wgid:zBob").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "evt-1");
        assert_eq!(events[0].bytes, b"one");
        assert_eq!(events[1].id, "evt-2");

        // empty inbox is not an error
        assert!(s.list_events("wgid:zNobody").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filestore_attestation_absent_then_present() {
        let dir = tmp();
        let s = FileStore::new(dir.to_str().unwrap());
        assert!(s.get_attestation("wgid:zA").unwrap().is_none());
        s.put_attestation("wgid:zA", b"att-bytes").unwrap();
        assert_eq!(
            s.get_attestation("wgid:zA").unwrap().as_deref(),
            Some(&b"att-bytes"[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
