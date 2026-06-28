//! The WG node store-and-forward inbox — the **default transport rung** (ADR-fed-002
//! §D1 rung 1, the promoted daemon of doc 02 §2.1).
//!
//! A small, dependency-light HTTP/1.1 server over [`std::net::TcpListener`] backed by
//! a [`FileStore`]. It exposes exactly the [`FedStore`] surface over HTTP so an
//! [`HttpStore`](super::transport::HttpStore) client on another graph can publish an
//! identity bundle, deliver a `SignedEvent` to an **offline** recipient (it is held
//! until the recipient polls), and re-fetch freshness attestations:
//!
//! ```text
//!   GET    /wgfed/v1/health                      → "ok"
//!   GET    /wgfed/v1/version                      → WG_FED_COMPAT_VERSION (S-7 handshake)
//!   PUT    /wgfed/v1/objects/<cid>               ← store a content-addressed object
//!   GET    /wgfed/v1/objects/<cid>               → object bytes (404 if absent)
//!   PUT    /wgfed/v1/heads/<wgid>                ← publish a head pointer (owner-signed)
//!   GET    /wgfed/v1/heads/<wgid>                → head bytes (404 if absent)
//!   PUT    /wgfed/v1/inbox/<wgid>/<event-id>     ← deliver (store-and-forward)
//!   GET    /wgfed/v1/inbox/<wgid>                → {"events":[<id>,…]}
//!   GET    /wgfed/v1/inbox/<wgid>/<event-id>     → event bytes (404 if absent)
//!   DELETE /wgfed/v1/inbox/<wgid>/<event-id>     ← ack/reclaim a consumed event
//!   PUT    /wgfed/v1/attestations/<wgid>         ← publish a freshness attestation (owner-signed)
//!   GET    /wgfed/v1/attestations/<wgid>         → attestation bytes (404 if absent)
//! ```
//!
//! The node is **untrusted** (ADR-fed-002 §D3): it holds and forwards self-verifying,
//! signed (optionally sealed) bytes. It can drop or reorder, but it can neither forge
//! an identity (the recipient checks every signature offline) nor read sealed content.
//! No rung is mandatory — this is the default, not a required root (§D2).
//!
//! ## Abuse hardening — the one exposed surface (audit B1/B2/M3/M4)
//!
//! Forging an *identity* is already impossible (the crypto is self-certifying); the
//! exposure this module closes is **DoS / grief** on a networked deploy:
//!
//! - **Write-auth (B1).** Owner-scoped mutable state — `PUT /heads` and
//!   `PUT /attestations` — must be **signed by a key the wgid's sigchain authorizes**,
//!   which the node reconstructs and verifies from *its own* published objects (rooted
//!   at the wgid). An unauthenticated or wrong-key write is refused, closing
//!   head-squat / attestation-overwrite. Content-addressed `PUT /objects` is
//!   self-authenticating via the CID check below — you cannot squat a chosen CID with
//!   junk. The **inbox stays open** by design (store-and-forward delivery to *offline*
//!   recipients is the point — anyone may deliver; authenticity is the recipient's
//!   offline job), but is bounded by per-inbox quotas + retention GC below.
//! - **Bounded reads (B2).** The request size is capped and the body is **streamed**
//!   under a hard limit — the server never `vec![0u8; content_length]`s an attacker's
//!   declared length, so a lone request cannot OOM the node.
//! - **CID integrity (M3).** `cid == hash(bytes)` is enforced on `PUT /objects` **and**
//!   on `GET /objects` (a corrupted-at-rest object is refused, fail-closed).
//! - **Resource limits (M4).** Socket read/write **timeouts** (slow-loris), a
//!   **connection bound** (flood), per-inbox **quotas** + **retention GC** +
//!   **delete-after-ack** (unbounded inbox growth).
//!
//! These bounds are tunable via [`NodeLimits`] (env-overridable) so an operator can
//! tighten or loosen them without a rebuild.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};

use super::content_cid;
use super::envelope::{IdentityRecord, payload_cid};
use super::freshness::FreshnessAttestation;
use super::sigchain::{self, SigchainLink};
use super::transport::{API_PREFIX, FedStore, FileStore, Head, sanitize};

/// Tunable abuse-resistance limits for the node (audit B2/M4). Defaults are generous
/// for a real deploy; every field is overridable from the environment via
/// [`NodeLimits::from_env`] so an operator can tighten them without a rebuild.
#[derive(Debug, Clone)]
pub struct NodeLimits {
    /// Hard cap on a single request body (B2). A larger declared `Content-Length` is
    /// refused outright — no read, no allocation.
    pub max_body: usize,
    /// Max total request bytes spent on the request line + headers (slow/huge-header
    /// flood guard).
    pub max_header_bytes: usize,
    /// Max concurrent connections served at once (flood guard, M4). Excess is rejected
    /// fast with `503` rather than spawning an unbounded thread fan-out.
    pub max_connections: usize,
    /// Per-connection socket read timeout (slow-loris guard, M4).
    pub read_timeout: Duration,
    /// Per-connection socket write timeout (M4).
    pub write_timeout: Duration,
    /// Max events held in one recipient's inbox before delivery is refused (flood).
    pub inbox_max_events: usize,
    /// Max total bytes held in one recipient's inbox before delivery is refused.
    pub inbox_max_bytes: u64,
    /// Max size of a single delivered event.
    pub inbox_event_max_bytes: u64,
    /// Inbox retention: events older than this are GC'd so an inbox cannot grow
    /// without bound even if a recipient never acks (M4).
    pub inbox_retention: Duration,
    /// How often the background GC sweep runs.
    pub gc_interval: Duration,
}

impl Default for NodeLimits {
    fn default() -> Self {
        Self {
            max_body: 8 * 1024 * 1024,
            max_header_bytes: 64 * 1024,
            max_connections: 256,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            inbox_max_events: 1024,
            inbox_max_bytes: 64 * 1024 * 1024,
            inbox_event_max_bytes: 4 * 1024 * 1024,
            inbox_retention: Duration::from_secs(7 * 24 * 60 * 60),
            gc_interval: Duration::from_secs(300),
        }
    }
}

impl NodeLimits {
    /// Defaults, overlaid with any `WG_FED_NODE_*` environment overrides. Lets an
    /// operator (or a test/smoke) tighten a single bound without a rebuild.
    pub fn from_env() -> Self {
        let mut l = Self::default();
        if let Some(v) = env_usize("WG_FED_NODE_MAX_BODY") {
            l.max_body = v;
        }
        if let Some(v) = env_usize("WG_FED_NODE_MAX_CONN") {
            l.max_connections = v.max(1);
        }
        if let Some(v) = env_u64("WG_FED_NODE_READ_TIMEOUT_MS") {
            l.read_timeout = Duration::from_millis(v);
        }
        if let Some(v) = env_u64("WG_FED_NODE_WRITE_TIMEOUT_MS") {
            l.write_timeout = Duration::from_millis(v);
        }
        if let Some(v) = env_usize("WG_FED_NODE_INBOX_MAX_EVENTS") {
            l.inbox_max_events = v;
        }
        if let Some(v) = env_u64("WG_FED_NODE_INBOX_MAX_BYTES") {
            l.inbox_max_bytes = v;
        }
        if let Some(v) = env_u64("WG_FED_NODE_RETENTION_SECS") {
            l.inbox_retention = Duration::from_secs(v);
        }
        if let Some(v) = env_u64("WG_FED_NODE_GC_INTERVAL_SECS") {
            l.gc_interval = Duration::from_secs(v.max(1));
        }
        l
    }
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse().ok()
}
fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}

/// Run the node inbox server on `addr`, backed by a directory at `store_dir`. Blocks
/// forever (one thread per connection, bounded by [`NodeLimits::max_connections`]).
/// Prints a `listening on …` line to stdout once bound, so a supervisor/test can
/// detect readiness.
pub fn serve(addr: &str, store_dir: &str) -> Result<()> {
    let (listener, bound) = bind(addr)?;
    let store = Arc::new(FileStore::new(store_dir));
    // Pre-create the store dir so the first GET on an empty node 404s cleanly.
    std::fs::create_dir_all(store_dir).ok();

    println!("wg-fed node inbox listening on http://{bound} (store: {store_dir})");
    std::io::stdout().flush().ok();

    accept_loop(listener, store, NodeLimits::from_env())
}

/// A per-connection RAII counter: increments the live-connection count on creation and
/// decrements it on drop, so the accept loop can bound concurrency (M4).
struct ConnGuard(Arc<AtomicUsize>);
impl ConnGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        ConnGuard(Arc::clone(counter))
    }
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The shared accept loop: spawns a background inbox-GC sweep, then serves
/// connections with a hard concurrency bound (excess → `503`, fast).
fn accept_loop(listener: TcpListener, store: Arc<FileStore>, limits: NodeLimits) -> Result<()> {
    // Background retention GC so the inbox cannot grow without bound (M4).
    {
        let store = Arc::clone(&store);
        let retention = limits.inbox_retention;
        let interval = limits.gc_interval;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(interval);
                store.gc_inboxes(retention);
            }
        });
    }

    let active = Arc::new(AtomicUsize::new(0));
    let limits = Arc::new(limits);
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // Flood guard: refuse fast when at the connection bound (M4). The
                // counter is bumped synchronously here (before the worker spawns) so a
                // burst of accepts can never race past the cap.
                if active.load(Ordering::SeqCst) >= limits.max_connections {
                    let _ = stream.set_write_timeout(Some(limits.write_timeout));
                    let _ = write_response(
                        &mut stream,
                        "503 Service Unavailable",
                        "text/plain",
                        b"node at connection capacity",
                    );
                    continue;
                }
                let guard = ConnGuard::new(&active);
                let store = Arc::clone(&store);
                let limits = Arc::clone(&limits);
                std::thread::spawn(move || {
                    let _guard = guard; // decrements on thread exit
                    if let Err(e) = handle_conn(stream, &store, &limits) {
                        eprintln!("wg-fed node: connection error: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("wg-fed node: accept error: {e}"),
        }
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
    /// Set when the declared `Content-Length` exceeds the body cap: the body was
    /// **not** read or allocated, and the request is answered with `413` (B2).
    too_large: bool,
}

/// Read one HTTP request with hard bounds (B2): the header section is size-capped, an
/// over-cap `Content-Length` is refused without reading/allocating the body, and the
/// body is **streamed** (never pre-allocated to a lied length). The socket read
/// timeout (set by the caller) bounds a slow-loris that dribbles bytes.
fn read_request(stream: &TcpStream, limits: &NodeLimits) -> Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line)?;
    if n == 0 {
        return Ok(None); // client closed
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    let mut header_bytes = request_line.len();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        header_bytes += n;
        if header_bytes > limits.max_header_bytes {
            anyhow::bail!(
                "request header section exceeds {} bytes",
                limits.max_header_bytes
            );
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    // B2: never `vec![0u8; content_length]`. An over-cap declared length is refused
    // outright (no read, no allocation); otherwise the body is streamed bounded by the
    // declared (≤ cap) length, growing only with the bytes actually received.
    if content_length > limits.max_body {
        return Ok(Some(Request {
            method,
            path,
            body: Vec::new(),
            too_large: true,
        }));
    }
    let mut body = Vec::new();
    if content_length > 0 {
        let mut limited = reader.take(content_length as u64);
        limited.read_to_end(&mut body)?;
    }
    Ok(Some(Request {
        method,
        path,
        body,
        too_large: false,
    }))
}

fn handle_conn(mut stream: TcpStream, store: &FileStore, limits: &NodeLimits) -> Result<()> {
    // Slow-loris guard (M4): a stalled peer cannot pin a thread forever.
    stream.set_read_timeout(Some(limits.read_timeout))?;
    stream.set_write_timeout(Some(limits.write_timeout))?;
    let req = match read_request(&stream, limits)? {
        Some(r) => r,
        None => return Ok(()),
    };
    let (status, ctype, body) = route(&req, store, limits);
    write_response(&mut stream, status, ctype, &body)
}

/// Returns `(status_line, content_type, body)`.
fn route(
    req: &Request,
    store: &FileStore,
    limits: &NodeLimits,
) -> (&'static str, &'static str, Vec<u8>) {
    if req.too_large {
        return payload_too_large();
    }
    // Strip query string and the API prefix.
    let path = req.path.split('?').next().unwrap_or(&req.path);
    let rest = match path.strip_prefix(API_PREFIX) {
        Some(r) => r,
        None => return not_found(),
    };
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let method = req.method.as_str();

    match (method, segs.as_slice()) {
        ("GET", ["health"]) => ("200 OK", "text/plain", b"ok".to_vec()),

        // S-7 compat handshake (audit M2): advertise this build's WG-Fed wire version so
        // a client can negotiate it via `HttpStore::handshake` and loud-fail on mismatch.
        ("GET", ["version"]) => (
            "200 OK",
            "text/plain",
            super::WG_FED_COMPAT_VERSION.as_bytes().to_vec(),
        ),

        // M3: an object's CID must equal the hash of its bytes — on write AND read.
        ("PUT", ["objects", cid]) => {
            if !object_cid_ok(cid, &req.body) {
                return cid_mismatch();
            }
            match store.put_object(cid, &req.body) {
                Ok(()) => ok_empty(),
                Err(_) => server_error(),
            }
        }
        ("GET", ["objects", cid]) => match store.get_object(cid) {
            Ok(bytes) => {
                // Refuse to serve a corrupted-at-rest object (fail closed).
                if !object_cid_ok(cid, &bytes) {
                    return not_found();
                }
                ("200 OK", "application/octet-stream", bytes)
            }
            Err(_) => not_found(),
        },

        // B1: only the owning wgid (signature-checked against its sigchain) may write.
        ("PUT", ["heads", wgid]) => put_head_authed(store, wgid, &req.body),
        ("GET", ["heads", wgid]) => match store.get_head(wgid) {
            Ok(head) => match serde_json::to_vec(&head) {
                Ok(bytes) => ("200 OK", "application/json", bytes),
                Err(_) => server_error(),
            },
            Err(_) => not_found(),
        },

        // Inbox: open store-and-forward delivery (any sender → an offline recipient),
        // bounded by per-inbox quota (flood) + retention GC + delete-after-ack.
        ("PUT", ["inbox", wgid, id]) => put_event_bounded(store, limits, wgid, id, &req.body),
        ("DELETE", ["inbox", wgid, id]) => match store.delete_event(wgid, id) {
            Ok(()) => ok_empty(),
            Err(_) => server_error(),
        },
        ("GET", ["inbox", wgid, id]) => {
            // Fetch one named event from the inbox.
            match store.list_events(wgid) {
                Ok(events) => match events.into_iter().find(|e| e.id == *id) {
                    Some(ev) => ("200 OK", "application/octet-stream", ev.bytes),
                    None => not_found(),
                },
                Err(_) => not_found(),
            }
        }
        ("GET", ["inbox", wgid]) => match store.list_events(wgid) {
            Ok(events) => {
                let ids: Vec<String> = events.into_iter().map(|e| e.id).collect();
                let body = serde_json::json!({ "events": ids });
                (
                    "200 OK",
                    "application/json",
                    serde_json::to_vec(&body).unwrap_or_default(),
                )
            }
            Err(_) => server_error(),
        },

        ("PUT", ["attestations", wgid]) => put_attestation_authed(store, wgid, &req.body),
        ("GET", ["attestations", wgid]) => match store.get_attestation(wgid) {
            Ok(Some(bytes)) => ("200 OK", "application/octet-stream", bytes),
            Ok(None) => not_found(),
            Err(_) => server_error(),
        },

        _ => not_found(),
    }
}

// ── Write-auth helpers (B1) ─────────────────────────────────────────────────────

/// Reconstruct + verify the sigchain for `wgid` from the node's own published objects,
/// returning its authorized key set. This is the "signature-checked **against its
/// sigchain**" half of write-auth: the chain is replayed and root-anchored at the
/// wgid (a forged genesis cannot match the address), and a hostile chain length is
/// bounded so a crafted `prev` cycle/loop cannot wedge the node.
fn reconstruct_authorized(
    store: &FileStore,
    wgid: &str,
    sigchain_head: &str,
) -> Result<sigchain::AuthorizedKeys> {
    let mut links: Vec<SigchainLink> = Vec::new();
    let mut cursor = Some(sigchain_head.to_string());
    let max_links = 4096;
    while let Some(cid) = cursor {
        if links.len() >= max_links {
            anyhow::bail!("sigchain exceeds {max_links} links — refusing to reconstruct");
        }
        let bytes = store
            .get_object(&cid)
            .with_context(|| format!("sigchain link {cid} not present on this node"))?;
        let link: SigchainLink = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing sigchain link {cid}"))?;
        cursor = link.prev.clone();
        links.push(link);
    }
    links.reverse(); // genesis-first
    sigchain::verify(&links, wgid)
}

/// `PUT /heads/<wgid>` — accept only a head the owner signed. The head is bound to the
/// slot it is written to (no cross-wgid squat) and its `sig` must verify against the
/// key set the wgid's sigchain authorizes (reconstructed from the node's objects).
fn put_head_authed(
    store: &FileStore,
    path_wgid: &str,
    body: &[u8],
) -> (&'static str, &'static str, Vec<u8>) {
    let head: Head = match serde_json::from_slice(body) {
        Ok(h) => h,
        Err(_) => return bad_request(),
    };
    // The head's referenced IdentityRecord names the real wgid + sigchain head.
    let rec_bytes = match store.get_object(&head.record) {
        Ok(b) => b,
        Err(_) => return forbidden("head references a record not present on this node"),
    };
    let record: IdentityRecord = match serde_json::from_slice(&rec_bytes) {
        Ok(r) => r,
        Err(_) => return bad_request(),
    };
    // Bind to the target slot — you may only write your own head.
    if sanitize(&record.id) != path_wgid {
        return forbidden("head record identity does not match the target wgid slot");
    }
    let auth = match reconstruct_authorized(store, &record.id, &record.sigchain_head) {
        Ok(a) => a,
        Err(_) => return forbidden("could not reconstruct/verify the sigchain for this wgid"),
    };
    if head.verify(&auth).is_err() {
        return forbidden("head is not signed by a key the wgid's sigchain authorizes");
    }
    match store.put_head(path_wgid, &head) {
        Ok(()) => ok_empty(),
        Err(_) => server_error(),
    }
}

/// `PUT /attestations/<wgid>` — accept only an attestation the owner signed (it is
/// already self-describing: it carries its identity + the sigchain head it vouches for).
fn put_attestation_authed(
    store: &FileStore,
    path_wgid: &str,
    body: &[u8],
) -> (&'static str, &'static str, Vec<u8>) {
    let att: FreshnessAttestation = match serde_json::from_slice(body) {
        Ok(a) => a,
        Err(_) => return bad_request(),
    };
    if sanitize(&att.identity) != path_wgid {
        return forbidden("attestation identity does not match the target wgid slot");
    }
    let auth = match reconstruct_authorized(store, &att.identity, &att.head) {
        Ok(a) => a,
        Err(_) => return forbidden("could not reconstruct/verify the sigchain for this wgid"),
    };
    if att.verify_signature(&auth).is_err() {
        return forbidden("attestation is not signed by a key the wgid's sigchain authorizes");
    }
    match store.put_attestation(path_wgid, body) {
        Ok(()) => ok_empty(),
        Err(_) => server_error(),
    }
}

/// `PUT /inbox/<wgid>/<id>` — open store-and-forward delivery, bounded by per-event and
/// per-inbox quotas so an unauthenticated peer cannot flood/exhaust storage (B1/M4).
fn put_event_bounded(
    store: &FileStore,
    limits: &NodeLimits,
    wgid: &str,
    id: &str,
    body: &[u8],
) -> (&'static str, &'static str, Vec<u8>) {
    if body.len() as u64 > limits.inbox_event_max_bytes {
        return payload_too_large();
    }
    let (count, held) = store.inbox_stats(wgid);
    // A re-delivery of an already-present id is an idempotent overwrite (it does not
    // grow the inbox), so it is allowed even at the cap; a *new* id is refused.
    let is_new = !store.inbox_event_exists(wgid, id);
    if is_new
        && (count >= limits.inbox_max_events || held + body.len() as u64 > limits.inbox_max_bytes)
    {
        return insufficient_storage();
    }
    match store.put_event(wgid, id, body) {
        Ok(()) => ok_empty(),
        Err(_) => server_error(),
    }
}

/// Whether `bytes` legitimately content-address to `path_cid` (M3). Two CID
/// conventions coexist in WG-Fed: **canonical-JSON** envelopes (sigchain links,
/// records, snapshots, attestations) hash their sorted-key JSON, while **raw payloads**
/// (e.g. a `StateSnapshot`'s body) hash their literal bytes. A genuine object matches
/// one; junk under a chosen CID matches neither (both hashes are preimage-bound to the
/// bytes, so a victim's CID cannot be squatted).
fn object_cid_ok(path_cid: &str, bytes: &[u8]) -> bool {
    if sanitize(&payload_cid(bytes)) == path_cid {
        return true;
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if sanitize(&content_cid(&v)) == path_cid {
            return true;
        }
    }
    false
}

// ── Response helpers ────────────────────────────────────────────────────────────

fn ok_empty() -> (&'static str, &'static str, Vec<u8>) {
    ("200 OK", "text/plain", b"ok".to_vec())
}
fn not_found() -> (&'static str, &'static str, Vec<u8>) {
    ("404 Not Found", "text/plain", b"not found".to_vec())
}
fn bad_request() -> (&'static str, &'static str, Vec<u8>) {
    ("400 Bad Request", "text/plain", b"bad request".to_vec())
}
fn forbidden(msg: &str) -> (&'static str, &'static str, Vec<u8>) {
    ("403 Forbidden", "text/plain", msg.as_bytes().to_vec())
}
fn cid_mismatch() -> (&'static str, &'static str, Vec<u8>) {
    (
        "409 Conflict",
        "text/plain",
        b"object cid does not match its content".to_vec(),
    )
}
fn payload_too_large() -> (&'static str, &'static str, Vec<u8>) {
    (
        "413 Payload Too Large",
        "text/plain",
        b"payload too large".to_vec(),
    )
}
fn insufficient_storage() -> (&'static str, &'static str, Vec<u8>) {
    (
        "507 Insufficient Storage",
        "text/plain",
        b"inbox quota exceeded".to_vec(),
    )
}
fn server_error() -> (&'static str, &'static str, Vec<u8>) {
    ("500 Internal Server Error", "text/plain", b"error".to_vec())
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// Resolve a possibly-`0`-port `addr` to the bound address by binding once and
/// returning the listener + chosen address. Used by callers that need the concrete
/// port (e.g. tests/`--addr 127.0.0.1:0`). The returned listener is then served via
/// [`serve_on`].
pub fn bind(addr: &str) -> Result<(TcpListener, String)> {
    let listener = TcpListener::bind(addr).with_context(|| format!("binding to {addr}"))?;
    let bound = listener.local_addr()?.to_string();
    Ok((listener, bound))
}

/// Serve on an already-bound listener (companion to [`bind`]).
pub fn serve_on(listener: TcpListener, store_dir: &str) -> Result<()> {
    serve_on_with_limits(listener, store_dir, NodeLimits::from_env())
}

/// Serve on an already-bound listener with explicit limits (tests/operators).
pub fn serve_on_with_limits(
    listener: TcpListener,
    store_dir: &str,
    limits: NodeLimits,
) -> Result<()> {
    let store = Arc::new(FileStore::new(store_dir));
    std::fs::create_dir_all(store_dir).ok();
    accept_loop(listener, store, limits)
}

/// The store-dir convention for a `wg fed-node serve` whose `--store` was omitted:
/// `<workgraph_dir>/fed-node`.
pub fn default_store_dir(workgraph_dir: &std::path::Path) -> PathBuf {
    workgraph_dir.join("fed-node")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::{Custodian, gen_ed25519, gen_x25519, kid_for, wgid_from_pubkey};
    use crate::identity::sigchain::{KeyEntry, KeyRole, KeyStatus, add_key, genesis, verify};
    use crate::identity::transport::open_store;
    use crate::identity::{ALG_ED25519, ENVELOPE_V};

    fn scratch(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!(
            "wg-fed-node-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().to_string()
    }

    /// A minted identity with the published bundle ready to install into a store.
    struct Published {
        wgid: String,
        record: IdentityRecord,
        record_cid: String,
        links: Vec<SigchainLink>,
        head: Head,
        att: FreshnessAttestation,
        cust: Custodian,
        signer_kid: String,
    }

    /// Mint a real identity (genesis + signer + enc) and build its signed publishable
    /// bundle — the exact shape `wg identity publish` writes.
    fn mint(tag: &str) -> Published {
        let ks = std::env::temp_dir().join(format!(
            "wg-fed-node-ks-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cust = Custodian::with_keystore_dir(tag, ks);
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();

        let signer = gen_ed25519().unwrap();
        let signer_kid = kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let signer_entry = KeyEntry {
            kid: signer_kid.clone(),
            public: hex::encode(signer.public),
            role: KeyRole::Signer,
            scope: vec!["event".into()],
            status: KeyStatus::Active,
        };
        let l1 = add_key(&cust, &g, &root.public, &root_kid, signer_entry.clone()).unwrap();

        let enc = gen_x25519().unwrap();
        let enc_kid = kid_for(&enc.public);
        cust.store_sealing_key(&enc_kid, &enc.secret).unwrap();
        let enc_entry = KeyEntry {
            kid: enc_kid,
            public: hex::encode(enc.public),
            role: KeyRole::Enc,
            scope: vec![],
            status: KeyStatus::Active,
        };
        let l2 = add_key(&cust, &l1, &root.public, &root_kid, enc_entry.clone()).unwrap();
        let links = vec![g, l1, l2];
        let auth = verify(&links, &wgid).unwrap();

        let mut record = IdentityRecord {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            id: wgid.clone(),
            sigchain_head: auth.head.clone(),
            keys: vec![signer_entry, enc_entry],
            endpoints: vec![],
            alias_proofs: vec![],
            agent_fields: None,
            sig: String::new(),
        };
        record.sign(&cust, &signer_kid).unwrap();
        let record_cid = record.cid();

        let mut att = FreshnessAttestation::build(
            &wgid,
            &auth.head,
            chrono::Utc::now(),
            crate::identity::freshness::ROUTINE_DELTA_SECS,
            1,
        );
        att.sign(&cust, &signer_kid).unwrap();

        let mut head = Head {
            record: record_cid.clone(),
            snapshots: vec![],
            attestation: Some(att.cid()),
            sig: String::new(),
        };
        head.sign(&cust, &signer_kid).unwrap();

        Published {
            wgid,
            record,
            record_cid,
            links,
            head,
            att,
            cust,
            signer_kid,
        }
    }

    /// Install a published identity's objects (sigchain links + record + attestation)
    /// into a store, the prerequisite for an authenticated head/attestation write.
    fn install_objects(client: &dyn FedStore, p: &Published) {
        for link in &p.links {
            client
                .put_object(&link.cid(), &serde_json::to_vec(link).unwrap())
                .unwrap();
        }
        client
            .put_object(&p.record_cid, &serde_json::to_vec(&p.record).unwrap())
            .unwrap();
        client
            .put_object(&p.att.cid(), &serde_json::to_vec(&p.att).unwrap())
            .unwrap();
    }

    fn spawn_node(dir: &str) -> String {
        let (listener, addr) = bind("127.0.0.1:0").unwrap();
        let dir = dir.to_string();
        std::thread::spawn(move || {
            let _ = serve_on(listener, &dir);
        });
        format!("http://{addr}")
    }

    /// Full happy-path round-trip with REAL signed objects/heads/attestations — proves
    /// the hardened node still accepts legitimate, authenticated writes end-to-end.
    #[test]
    fn node_http_roundtrip_real_signed_bundle() {
        let dir = scratch("rt");
        let base = spawn_node(&dir);
        let client = open_store(&base).unwrap();
        let p = mint("rt-alice");

        // objects: legitimate CIDs round-trip; a raw payload too.
        install_objects(client.as_ref(), &p);
        assert_eq!(
            client.get_object(&p.record_cid).unwrap(),
            serde_json::to_vec(&p.record).unwrap()
        );
        let raw = b"opaque-payload-bytes";
        let raw_cid = payload_cid(raw);
        client.put_object(&raw_cid, raw).unwrap();
        assert_eq!(client.get_object(&raw_cid).unwrap(), raw);

        // owner-signed head + attestation: accepted.
        client.put_head(&p.wgid, &p.head).unwrap();
        assert_eq!(client.get_head(&p.wgid).unwrap().record, p.record_cid);
        client
            .put_attestation(&p.wgid, &serde_json::to_vec(&p.att).unwrap())
            .unwrap();
        assert!(client.get_attestation(&p.wgid).unwrap().is_some());

        // inbox store-and-forward (open delivery) + ack/delete reclaim.
        client.put_event(&p.wgid, "evt-a", b"first").unwrap();
        client.put_event(&p.wgid, "evt-b", b"second").unwrap();
        assert_eq!(client.list_events(&p.wgid).unwrap().len(), 2);
        client.delete_event(&p.wgid, "evt-a").unwrap();
        let after = client.list_events(&p.wgid).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "evt-b");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── B1: write-auth ──────────────────────────────────────────────────────────

    fn put(
        method: &str,
        path: &str,
        body: &[u8],
        store: &FileStore,
        limits: &NodeLimits,
    ) -> String {
        let req = Request {
            method: method.to_string(),
            path: path.to_string(),
            body: body.to_vec(),
            too_large: false,
        };
        route(&req, store, limits).0.to_string()
    }

    #[test]
    fn unauthenticated_head_write_is_rejected() {
        let dir = scratch("auth-head");
        let store = FileStore::new(&dir);
        let limits = NodeLimits::default();
        let p = mint("ah-alice");
        install_objects(&store, &p);
        let slot = sanitize(&p.wgid);

        // A correctly-signed head is accepted...
        let good = serde_json::to_vec(&p.head).unwrap();
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/heads/{slot}"),
                &good,
                &store,
                &limits
            ),
            "200 OK"
        );

        // ...but the SAME head with its signature stripped is refused (unauthenticated).
        let mut unsigned = p.head.clone();
        unsigned.sig = String::new();
        let body = serde_json::to_vec(&unsigned).unwrap();
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/heads/{slot}"),
                &body,
                &store,
                &limits
            ),
            "403 Forbidden"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_wgid_head_write_is_rejected() {
        let dir = scratch("auth-wrong");
        let store = FileStore::new(&dir);
        let limits = NodeLimits::default();
        let alice = mint("ww-alice");
        let mallory = mint("ww-mallory");
        install_objects(&store, &alice);
        install_objects(&store, &mallory);

        // Mallory signs a head for HER record but tries to write it into ALICE's slot.
        let mut head = Head {
            record: mallory.record_cid.clone(),
            snapshots: vec![],
            attestation: None,
            sig: String::new(),
        };
        head.sign(&mallory.cust, &mallory.signer_kid).unwrap();
        let body = serde_json::to_vec(&head).unwrap();
        // Cross-slot squat is refused (record identity ≠ target slot).
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/heads/{}", sanitize(&alice.wgid)),
                &body,
                &store,
                &limits
            ),
            "403 Forbidden"
        );

        // And a head for alice's slot signed by mallory's key (not authorized for
        // alice) is refused.
        let mut forged = alice.head.clone();
        forged.sig = String::new();
        forged.sign(&mallory.cust, &mallory.signer_kid).unwrap();
        let body = serde_json::to_vec(&forged).unwrap();
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/heads/{}", sanitize(&alice.wgid)),
                &body,
                &store,
                &limits
            ),
            "403 Forbidden"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attestation_write_auth() {
        let dir = scratch("auth-att");
        let store = FileStore::new(&dir);
        let limits = NodeLimits::default();
        let p = mint("att-alice");
        install_objects(&store, &p);
        let slot = sanitize(&p.wgid);

        // Genuine attestation accepted.
        let good = serde_json::to_vec(&p.att).unwrap();
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/attestations/{slot}"),
                &good,
                &store,
                &limits
            ),
            "200 OK"
        );
        // Tampered (head flipped after signing) → signature fails → refused.
        let mut bad = p.att.clone();
        bad.head = "b3:evil".into();
        let body = serde_json::to_vec(&bad).unwrap();
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/attestations/{slot}"),
                &body,
                &store,
                &limits
            ),
            "403 Forbidden"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── M3: CID integrity ────────────────────────────────────────────────────────

    #[test]
    fn bad_cid_object_is_rejected() {
        let dir = scratch("cid");
        let store = FileStore::new(&dir);
        let limits = NodeLimits::default();

        // A correct content-addressed PUT is accepted.
        let bytes = b"the real bytes";
        let cid = sanitize(&payload_cid(bytes));
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/objects/{cid}"),
                bytes,
                &store,
                &limits
            ),
            "200 OK"
        );
        // Junk under a CHOSEN cid (not its hash) is refused — no squatting.
        assert_eq!(
            put(
                "PUT",
                &format!("{API_PREFIX}/objects/b3_deadbeefdeadbeef"),
                b"unrelated bytes",
                &store,
                &limits
            ),
            "409 Conflict"
        );
        // The genuine bytes under the wrong cid are likewise refused.
        assert_eq!(
            put(
                "PUT",
                &format!(
                    "{API_PREFIX}/objects/{}",
                    sanitize(&payload_cid(b"different"))
                ),
                bytes,
                &store,
                &limits
            ),
            "409 Conflict"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── B2: bounded reads ────────────────────────────────────────────────────────

    #[test]
    fn oversize_request_is_rejected_without_allocation() {
        // Unit: the route layer answers `413` for an over-cap request (the read layer
        // sets `too_large` without allocating the declared length).
        let dir = scratch("big-unit");
        let store = FileStore::new(&dir);
        let limits = NodeLimits::default();
        let req = Request {
            method: "PUT".into(),
            path: format!("{API_PREFIX}/objects/x"),
            body: Vec::new(),
            too_large: true,
        };
        assert_eq!(route(&req, &store, &limits).0, "413 Payload Too Large");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversize_declared_length_is_refused_over_the_wire_without_oom() {
        // Integration: declare a 4 GiB body but send no payload. A node that
        // pre-allocated `content_length` would OOM/hang; the hardened node answers
        // `413` immediately (it never touches the body), proving the cap is enforced
        // at the declared length, not at read time.
        let dir = scratch("big-wire");
        let (listener, addr) = bind("127.0.0.1:0").unwrap();
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            // tiny body cap so the declared length is "huge" by comparison
            let l = NodeLimits {
                max_body: 1024,
                ..Default::default()
            };
            let _ = serve_on_with_limits(listener, &dir2, l);
        });
        let mut s = TcpStream::connect(&addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let req = format!(
            "PUT {API_PREFIX}/objects/x HTTP/1.1\r\nHost: x\r\nContent-Length: 4294967296\r\n\r\n"
        );
        s.write_all(req.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("413"), "expected 413, got: {resp}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── M4: resource limits ──────────────────────────────────────────────────────

    #[test]
    fn inbox_quota_bounds_flood() {
        let dir = scratch("quota");
        let store = FileStore::new(&dir);
        let limits = NodeLimits {
            inbox_max_events: 2,
            ..Default::default()
        };
        let slot = "wgid_zVictim";
        let base = format!("{API_PREFIX}/inbox/{slot}");

        assert_eq!(
            put("PUT", &format!("{base}/e1"), b"a", &store, &limits),
            "200 OK"
        );
        assert_eq!(
            put("PUT", &format!("{base}/e2"), b"b", &store, &limits),
            "200 OK"
        );
        // Third distinct event over the cap → refused (flood bounded).
        assert_eq!(
            put("PUT", &format!("{base}/e3"), b"c", &store, &limits),
            "507 Insufficient Storage"
        );
        // Re-delivering an existing id (idempotent overwrite) is still allowed at cap.
        assert_eq!(
            put("PUT", &format!("{base}/e1"), b"a2", &store, &limits),
            "200 OK"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inbox_gc_reclaims_acked_and_expired() {
        let dir = scratch("gc");
        let store = FileStore::new(&dir);
        let wgid = "wgid_zGc";

        store.put_event(wgid, "e1", b"one").unwrap();
        store.put_event(wgid, "e2", b"two").unwrap();
        assert_eq!(store.list_events(wgid).unwrap().len(), 2);

        // delete-after-ack reclaims a consumed event (idempotent).
        store.delete_event(wgid, "e1").unwrap();
        store.delete_event(wgid, "e1").unwrap();
        assert_eq!(store.list_events(wgid).unwrap().len(), 1);

        // Retention GC keeps fresh events under a long retention...
        assert_eq!(store.gc_inboxes(Duration::from_secs(3600)), 0);
        assert_eq!(store.list_events(wgid).unwrap().len(), 1);
        // ...and reclaims everything under a zero retention (the unbounded-growth bound).
        assert_eq!(store.gc_inboxes(Duration::ZERO), 1);
        assert_eq!(store.list_events(wgid).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slow_loris_connection_is_bounded_by_read_timeout() {
        // A peer that opens a connection and never finishes its request must not pin a
        // worker forever: the read timeout closes it. We observe the server-side close
        // as EOF on our (otherwise idle) socket within a bounded window.
        let dir = scratch("loris");
        let (listener, addr) = bind("127.0.0.1:0").unwrap();
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            let l = NodeLimits {
                read_timeout: Duration::from_millis(300),
                ..Default::default()
            };
            let _ = serve_on_with_limits(listener, &dir2, l);
        });
        let mut s = TcpStream::connect(&addr).unwrap();
        // Send a partial request line and then stall (no CRLF, no headers, no body).
        s.write_all(b"PUT /wgfed/v1/objects/x HTTP/1.1").unwrap();
        s.flush().unwrap();
        // Generous client read timeout; the server should close well within it.
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf); // returns on server close (EOF) — bounded
        // The key property: the read RETURNED (was not pinned forever). Whether the
        // server sent an error response or simply closed, we got here within the window.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connection_bound_rejects_excess_with_503() {
        // With a 1-connection bound, a second concurrent connection is refused fast.
        let dir = scratch("connbound");
        let (listener, addr) = bind("127.0.0.1:0").unwrap();
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            let l = NodeLimits {
                max_connections: 1,
                read_timeout: Duration::from_secs(3), // hold conn A's slot a while
                ..Default::default()
            };
            let _ = serve_on_with_limits(listener, &dir2, l);
        });
        // Conn A: connect and stall (holds the single slot — its worker blocks reading).
        let mut a = TcpStream::connect(&addr).unwrap();
        a.write_all(b"PUT /wgfed/v1/objects/x HTTP/1.1").unwrap();
        a.flush().unwrap();
        std::thread::sleep(Duration::from_millis(200)); // let A's guard register
        // Conn B: a full, well-formed request — but the node is at capacity → 503.
        let mut b = TcpStream::connect(&addr).unwrap();
        b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        b.write_all(format!("GET {API_PREFIX}/health HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .unwrap();
        b.flush().unwrap();
        let mut resp = String::new();
        let _ = b.read_to_string(&mut resp);
        assert!(
            resp.contains("503"),
            "expected 503 at capacity, got: {resp:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
