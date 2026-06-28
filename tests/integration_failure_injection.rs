//! Failure-injection tests for the federation wire (audit M22).
//!
//! Before this, the load-bearing fences were tested **sequentially** only: the lease epoch
//! CAS, the node inbox, and the verdict chain. This file exercises them under the
//! adversarial conditions the audit names — real concurrency, crash-mid-write + restart,
//! malformed / truncated / oversize wire input, and a `serde`-parser fuzz sweep — so the
//! "atomic CAS" / "fail-closed" claims are *proven under stress*, not asserted.
//!
//! Everything is in-process (no `python3` / `curl` / `tmux`), so these run in CI on a
//! minimal runner alongside the M29 wire tests.

mod common;
use common::{addr_of, http_get, http_put, spawn_node};

use std::sync::atomic::{AtomicUsize, Ordering};

use worksgood::identity::envelope::payload_cid;
use worksgood::identity::transport::{FedStore, FileStore, HttpStore};
use worksgood::providers::lease::{FenceError, LeaseLedger};

// ─────────────────────────────────────────────────────────────────────────────────
// 1. Concurrency on the lease epoch fence (audit "must test" #3)
// ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn concurrent_commits_of_the_same_epoch_yield_exactly_one_winner() {
    // The double-commit defense under REAL threads: many rounds, each with two threads
    // racing to commit the SAME (task, epoch=1) through the disk-locked guard. The
    // exclusive lock + the atomic compare-and-set must admit exactly one winner; the loser
    // is fenced with `AlreadyCommitted`. (The prior test only placed *distinct* tasks, so
    // it never contended the CAS itself.)
    for round in 0..25 {
        let dir = tempfile::tempdir().expect("tempdir");
        let wg = dir.path().to_path_buf();
        {
            // Place T at epoch 1.
            let mut g = LeaseLedger::open_locked(&wg).expect("open");
            g.ledger.place("T", "wgid:zP");
            g.save().expect("save");
        }

        let wins = AtomicUsize::new(0);
        let already = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..2 {
                let wg = wg.clone();
                let wins = &wins;
                let already = &already;
                s.spawn(move || {
                    let mut g = LeaseLedger::open_locked(&wg).expect("open under contention");
                    match g.ledger.try_commit("T", 1) {
                        Ok(()) => {
                            g.save().expect("save winner");
                            wins.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(FenceError::AlreadyCommitted { .. }) => {
                            already.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => panic!("unexpected fence error in round {round}: {e}"),
                    }
                });
            }
        });

        assert_eq!(
            wins.load(Ordering::SeqCst),
            1,
            "round {round}: exactly one committer wins the epoch CAS"
        );
        assert_eq!(
            already.load(Ordering::SeqCst),
            1,
            "round {round}: the loser is fenced AlreadyCommitted (no double-commit)"
        );

        // The committed state is durable: a fresh load still fences a replay.
        let mut g = LeaseLedger::open_locked(&wg).expect("reopen");
        assert_eq!(
            g.ledger.try_commit("T", 1),
            Err(FenceError::AlreadyCommitted { epoch: 1 }),
            "round {round}: a post-commit replay is fenced after reload"
        );
    }
}

#[test]
fn reclaim_fences_an_in_flight_stale_epoch_commit() {
    // A reclaim (epoch 1→2) racing a resurrected worker's epoch-1 write: after the reclaim
    // the old write is StaleEpoch, never committed (X-4 / ADR-E3 D6). The lock serializes
    // the two, so whichever order they run, the invariant holds: an epoch-1 commit that
    // lands *after* the reclaim is rejected.
    let dir = tempfile::tempdir().expect("tempdir");
    let wg = dir.path().to_path_buf();
    {
        let mut g = LeaseLedger::open_locked(&wg).expect("open");
        g.ledger.place("T", "wgid:zP");
        g.save().expect("save");
    }
    // Reclaim to a new provider — epoch becomes 2, committed cleared.
    {
        let mut g = LeaseLedger::open_locked(&wg).expect("open");
        let new_epoch = g.ledger.reclaim("T", "wgid:zQ").expect("reclaim");
        assert_eq!(new_epoch, 2);
        g.save().expect("save");
    }
    // The resurrected worker presents its stale epoch-1 result — fenced.
    let mut g = LeaseLedger::open_locked(&wg).expect("open");
    assert_eq!(
        g.ledger.try_commit("T", 1),
        Err(FenceError::StaleEpoch {
            presented: 1,
            current: 2
        }),
        "a stale-epoch write after reclaim is fenced"
    );
    // The current epoch can still commit exactly once.
    assert!(g.ledger.try_commit("T", 2).is_ok());
    assert_eq!(
        g.ledger.try_commit("T", 2),
        Err(FenceError::AlreadyCommitted { epoch: 2 })
    );
}

// ─────────────────────────────────────────────────────────────────────────────────
// 2. Concurrency on the node inbox (audit "must test" #4)
// ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn concurrent_inbox_deliveries_lose_no_events() {
    // Many senders deliver distinct events to one offline recipient over the wire,
    // concurrently. The thread-per-connection node must not lose a delivery (each event is
    // a distinct content file). Every delivered id is present afterward.
    let (base, _dir) = spawn_node();
    let recipient = "wgid:zBusy";
    let n_senders = 8;
    let per_sender = 6;

    std::thread::scope(|s| {
        for t in 0..n_senders {
            let base = base.clone();
            s.spawn(move || {
                let client = HttpStore::new(&base);
                for i in 0..per_sender {
                    let id = format!("evt-{t}-{i}");
                    client
                        .put_event(recipient, &id, format!("payload-{t}-{i}").as_bytes())
                        .expect("PUT event under concurrency");
                }
            });
        }
    });

    let client = HttpStore::new(&base);
    let events = client.list_events(recipient).expect("list");
    assert_eq!(
        events.len(),
        n_senders * per_sender,
        "no concurrent delivery is lost"
    );
}

#[test]
fn concurrent_pollers_ack_idempotently_without_error() {
    // Two pollers race to consume + ack the same set of events. Delivery is at-least-once
    // and the ack (DELETE) is idempotent, so neither poller errors and the inbox is empty
    // afterward — no lost delete, no double-delete failure.
    let (base, _dir) = spawn_node();
    let recipient = "wgid:zDrain";
    let client = HttpStore::new(&base);
    for i in 0..20 {
        client
            .put_event(recipient, &format!("e{i}"), b"x")
            .expect("seed");
    }

    std::thread::scope(|s| {
        for _ in 0..2 {
            let base = base.clone();
            s.spawn(move || {
                let client = HttpStore::new(&base);
                let events = client.list_events(recipient).unwrap_or_default();
                for ev in events {
                    // Idempotent ack: a concurrent poller may have deleted it already.
                    client
                        .delete_event(recipient, &ev.id)
                        .expect("idempotent ack");
                }
            });
        }
    });

    let left = client.list_events(recipient).expect("list after drain");
    assert!(left.is_empty(), "all events drained, left: {}", left.len());
}

// ─────────────────────────────────────────────────────────────────────────────────
// 2b. Concurrency on the verdict chain (audit M22 — "verdict-chain ... tested only
//     sequentially"). The append is now lock-guarded (M23); this proves it.
// ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn concurrent_verdict_appends_lose_no_records_and_keep_the_hash_link() {
    use worksgood::graph::TrustLevel;
    use worksgood::review::verdict::VerdictStore;
    use worksgood::review::{ContentClass, Provenance, Sensitivity, review_inbound};

    let dir = tempfile::tempdir().expect("tempdir");
    let wg = dir.path().to_path_buf();
    let n_threads = 6;
    let per_thread = 8;

    let outcome = review_inbound(
        ContentClass::Ic1Task,
        "a benign task to record",
        &Provenance {
            author: Some("wgid:zAuthor".into()),
            trust: TrustLevel::Verified,
        },
        Sensitivity::Low,
    );

    std::thread::scope(|s| {
        for t in 0..n_threads {
            let wg = wg.clone();
            let outcome = outcome.clone();
            s.spawn(move || {
                let store = VerdictStore::open(&wg);
                for i in 0..per_thread {
                    store
                        .record(
                            &outcome,
                            Some(&format!("wgid:z{t}")),
                            Some(&format!("task-{t}-{i}")),
                        )
                        .expect("concurrent record");
                }
            });
        }
    });

    // Every record survives the concurrent appends (no lost update under the lock).
    let chain = VerdictStore::open(&wg).load_chain().expect("load");
    assert_eq!(
        chain.len(),
        n_threads * per_thread,
        "no verdict record is lost under concurrent append"
    );
    // The chain is internally consistent: seqs are exactly 0..N in order and every link's
    // `prev` is the prior record's `cid` (the hash link is unbroken).
    for (i, rec) in chain.iter().enumerate() {
        assert_eq!(rec.seq, i as u64, "seq is dense + ordered");
        let expected_prev = if i == 0 {
            String::new()
        } else {
            chain[i - 1].cid.clone()
        };
        assert_eq!(rec.prev, expected_prev, "hash link intact at record {i}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────────
// 3. Crash-mid-PUT + restart recovery (audit "must test" #4)
// ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn store_state_survives_a_node_restart() {
    // Store-and-forward must survive a node restart: objects + inbox events written by one
    // FileStore instance ("before the crash") are readable by a fresh instance over the
    // same dir ("after the restart").
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("store");
    let path_s = path.to_string_lossy().to_string();

    let bytes = b"durable object";
    let cid = payload_cid(bytes);
    {
        let store = FileStore::new(&path_s);
        store.put_object(&cid, bytes).expect("put object");
        store
            .put_event("wgid:zR", "evt-1", b"durable event")
            .expect("put event");
    } // drop = "crash"

    let restarted = FileStore::new(&path_s);
    assert_eq!(restarted.get_object(&cid).expect("object survives"), bytes);
    let events = restarted.list_events("wgid:zR").expect("events survive");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].bytes, b"durable event");
}

#[test]
fn a_partial_event_file_does_not_break_listing_valid_events() {
    // A crash mid-PUT can leave a half-written event file. Listing must still surface the
    // valid events (the partial file is just opaque bytes the consumer's verify rejects),
    // never panic or fail the whole inbox.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("store");
    let path_s = path.to_string_lossy().to_string();
    let store = FileStore::new(&path_s);
    store
        .put_event("wgid:zR", "good", b"{\"ok\":true}")
        .expect("good");
    // Simulate a torn write: a partial JSON file landed under a second id.
    store
        .put_event("wgid:zR", "torn", b"{\"ok\":")
        .expect("torn");

    let events = store
        .list_events("wgid:zR")
        .expect("list tolerates a torn file");
    assert_eq!(events.len(), 2, "both files are listed as opaque bytes");
    // The torn one is just bytes; a consumer that parses it would get an Err, never a panic.
    let torn = events.iter().find(|e| e.id == "torn").unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&torn.bytes).is_err());
}

#[test]
fn a_corrupt_lease_ledger_refuses_rather_than_resetting_after_restart() {
    // B3 restated as a restart story: a torn ledger on disk (crash mid-write) is REFUSED on
    // the next load — never silently reset to empty (which would drop the epoch fence and
    // re-open replay/double-commit).
    let dir = tempfile::tempdir().expect("tempdir");
    let wg = dir.path().to_path_buf();
    {
        let mut g = LeaseLedger::open_locked(&wg).expect("open");
        g.ledger.place("T", "wgid:zP");
        let _ = g.ledger.reclaim("T", "wgid:zQ"); // epoch 2
        g.save().expect("save");
    }
    // Crash mid-write: a partial JSON at the canonical path.
    let path = LeaseLedger::path(&wg);
    std::fs::write(&path, b"{ \"tasks\": { \"T\": { \"epoch\":").expect("tear");
    let err = LeaseLedger::load(&wg).expect_err("corrupt ledger must refuse, not reset");
    assert!(format!("{err:#}").contains("REFUSING"));
    // The bytes are untouched (no reset-to-empty clobber).
    assert!(!std::fs::read(&path).unwrap().is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────────
// 4. Malformed / truncated / oversize wire input (audit "must test" #5)
// ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn node_rejects_malformed_wire_input_without_panicking() {
    let (base, _dir) = spawn_node();
    let addr = addr_of(&base);

    // A malformed head body (not JSON) → 4xx (bad request / forbidden), never a 5xx/panic.
    let (status, _b) = http_put(addr, "/wgfed/v1/heads/wgid:zX", b"\xff\xfe not json")
        .expect("PUT malformed head");
    assert!(
        status.contains("400") || status.contains("403"),
        "malformed head is a client error, got {status}"
    );

    // An object whose bytes do not hash to the requested CID → 409 (cid mismatch), refused.
    let (status, _b) = http_put(
        addr,
        "/wgfed/v1/objects/b3_not_the_hash",
        b"arbitrary bytes",
    )
    .expect("PUT cid-mismatched object");
    assert!(
        status.contains("409"),
        "cid mismatch is refused, got {status}"
    );

    // An unknown route → 404, not a crash.
    let (status, _b) = http_get(addr, "/wgfed/v1/no/such/route").expect("GET unknown");
    assert!(status.contains("404"), "unknown route 404s, got {status}");

    // The node is still alive after the abuse.
    let (status, _b) = http_get(addr, "/wgfed/v1/health").expect("health after abuse");
    assert!(
        status.contains("200"),
        "node survives malformed input, got {status}"
    );
}

#[test]
fn node_refuses_an_oversize_declared_body_without_allocating() {
    // B2: a request declaring a body far beyond the cap is refused (413) WITHOUT reading or
    // pre-allocating the body — driven raw so we control Content-Length without sending the
    // bytes.
    let (base, _dir) = spawn_node();
    let addr = addr_of(&base);
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    use std::io::{Read, Write};
    // Declare 4 GiB; send no body. The node must answer 413 from the header alone.
    write!(
        stream,
        "PUT /wgfed/v1/objects/x HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 4294967296\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    stream.flush().ok();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    assert!(
        raw.contains("413"),
        "an oversize declared length is refused without OOM, got: {}",
        raw.lines().next().unwrap_or_default()
    );
}

// ─────────────────────────────────────────────────────────────────────────────────
// 5. Fuzz the serde parsers (audit "must test" #5)
// ─────────────────────────────────────────────────────────────────────────────────

/// A tiny deterministic xorshift PRNG — seeded by a constant so the fuzz sweep is
/// reproducible in CI (no wall clock / OS RNG).
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Mutate `seed` in place: bit flips, byte inserts, truncation, and run injection. The
/// goal is structurally-near-valid garbage that exercises the parser's edge handling.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = seed.to_vec();
    let ops = 1 + rng.below(6);
    for _ in 0..ops {
        if v.is_empty() {
            v.push(rng.byte());
            continue;
        }
        match rng.below(5) {
            0 => {
                let i = rng.below(v.len());
                v[i] ^= 1 << (rng.below(8));
            }
            1 => {
                let i = rng.below(v.len() + 1);
                v.insert(i, rng.byte());
            }
            2 => {
                let i = rng.below(v.len());
                v.truncate(i);
            }
            3 => {
                let i = rng.below(v.len());
                v[i] = b'{'; // unbalance structure
            }
            _ => {
                for _ in 0..rng.below(8) {
                    v.push(rng.byte());
                }
            }
        }
    }
    v
}

/// Parse `bytes` as `T`; the ONLY contract is that it never panics (Ok or Err is fine).
fn try_parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) {
    let _ = serde_json::from_slice::<T>(bytes);
}

#[test]
fn serde_parsers_never_panic_on_fuzzed_input() {
    use worksgood::identity::custody::Capability;
    use worksgood::identity::envelope::{IdentityRecord, SignedEvent, StateSnapshot};
    use worksgood::identity::freshness::FreshnessAttestation;
    use worksgood::identity::sigchain::SigchainLink;
    use worksgood::identity::transport::Head;
    use worksgood::providers::lease::Lease;
    use worksgood::providers::{Claim, PlacementOffer, ResultEnvelope, RunGrant};

    // A spread of seeds: empty, plain garbage, near-JSON, deep nesting, huge numbers, and
    // unicode — plus thousands of mutations of a generic JSON object.
    let seeds: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"null".to_vec(),
        b"{}".to_vec(),
        b"[]".to_vec(),
        b"\xff\xfe\x00\x01".to_vec(),
        b"{\"v\":1,\"sig\":\"\",\"id\":\"wgid:z\"".to_vec(), // truncated object
        b"{\"v\":\"not-an-int\"}".to_vec(),                  // type confusion
        b"123456789012345678901234567890".to_vec(),          // overflowing number
        "{\"id\":\"wgïd:𝕫\",\"keys\":[]}".as_bytes().to_vec(), // unicode/homoglyph
        format!("{}{}{}", "[".repeat(200), "1", "]".repeat(200)).into_bytes(), // deep nest
        b"{\"refs\":[".to_vec(),
    ];

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let iters = 400;
    for seed in &seeds {
        // Each seed, and `iters` mutations of it, fed to every parser.
        let mut inputs = vec![seed.clone()];
        for _ in 0..iters {
            inputs.push(mutate(seed, &mut rng));
        }
        // Plus pure-random buffers.
        for _ in 0..iters {
            let len = rng.below(64);
            let buf: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
            inputs.push(buf);
        }
        for input in &inputs {
            try_parse::<IdentityRecord>(input);
            try_parse::<SignedEvent>(input);
            try_parse::<StateSnapshot>(input);
            try_parse::<SigchainLink>(input);
            try_parse::<Capability>(input);
            try_parse::<FreshnessAttestation>(input);
            try_parse::<Head>(input);
            try_parse::<Lease>(input);
            try_parse::<PlacementOffer>(input);
            try_parse::<Claim>(input);
            try_parse::<RunGrant>(input);
            try_parse::<ResultEnvelope>(input);
        }
    }
    // Reaching here without a panic IS the assertion.
}
