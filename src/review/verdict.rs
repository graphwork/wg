//! The verdict sigchain, digest-pinned consumption, and the loud revoke leg
//! (ADR-CS3).
//!
//! **Safety = containment + audit + revoke, not detection** (ADR-CS3 D5, the line
//! the whole study turns on). This module owns the right-hand column the spark
//! invests in first:
//!
//! - **D2 — the verdict sigchain.** Every verdict, for every item, at every pass, is
//!   recorded on a **hash-linked, content-addressed** append-only chain (the WG-Fed
//!   sigchain substrate, reused via [`crate::identity::content_cid`] — **no** new
//!   ledger, **no** `WG_REVIEW_COMPAT_VERSION`). **No SKIP / uncertain verdict is
//!   ever silently dropped** — the record is what makes every accept/reject a
//!   *reversible event*.
//! - **D3 / MUST-2 — digest-pinned consumption.** An accept-verdict binds to the
//!   BLAKE3 CID of the reviewed bytes; consumption is of **that exact digest, never
//!   a mutable name** ([`VerdictStore::digest_pin_consume`]). A post-review mutated
//!   byte changes the CID and is **rejected** (the RA-8 TOCTOU close).
//! - **D4 — the automatic, loud revoke leg.** When a miss is later discovered, the
//!   sigchain trace finds the **author** and the **content digest**; the author's
//!   [`TrustLevel`] is **lowered** (so the *next* item takes the deep path); and the
//!   downstream `--after` consumers that read the poisoned artifact are **found and
//!   re-run**. This reuses the landed trust machinery — no new reputation system.
//!
//! For the spark, the trust-lowering is persisted to a small `trust_overrides.json`
//! ledger the gate reads on the *next* `review check` (proving "the second shot gets
//! the deep path"); the full cross-plane D-iii TC8 re-run is **joint with WG-Exec
//! Exec-Wave C** (§4.3) and is represented here as a single hand-wired re-run plan.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{Confidence, ContentClass, PassOutcome, PipelineOutcome, ReasonCode, Verdict};
use crate::graph::TrustLevel;
use crate::identity::content_cid;

/// The provenance leg of a verdict record (ADR-CS3 D2): author + sigchain position
/// + the content digest that is the consumption pin (D3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordProvenance {
    /// The author's `wgid:` (or local handle).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The author's position in their sigchain (the verdict-chain seq, for the spark).
    pub sigchain_pos: u64,
    /// BLAKE3 CID of the reviewed bytes — the digest-pin (MUST-2).
    pub content_cid: String,
}

/// One verdict record on the sigchain (the ADR-CS3 D2 schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerdictRecord {
    /// Position in the verdict chain (0 = genesis).
    pub seq: u64,
    /// The uniform verdict (strictest any pass reached).
    pub verdict: Verdict,
    /// The bounded category code (never free-form prose — MUST-3).
    pub reason: ReasonCode,
    /// The content class screened.
    pub content_class: ContentClass,
    /// Which pass set the strictest verdict (0..4).
    pub deciding_pass: u8,
    /// Reviewer confidence.
    pub confidence: Confidence,
    /// The applied `review.depth` label (surfaced in `wg show` / `wg review`).
    pub depth_label: String,
    /// The author + sigchain position + content digest.
    pub provenance: RecordProvenance,
    /// The downstream consuming task this verdict gates (for the TC8 re-run, D4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumer_task: Option<String>,
    /// Per-pass audit trace.
    pub trace: Vec<PassOutcome>,
    /// CID of the previous record (hash-link); empty for genesis.
    pub prev: String,
    /// CID of this record (content_cid of the record with `cid` removed).
    pub cid: String,
}

/// The dir-scoped verdict store: the append-only sigchain + the trust-override
/// ledger.
pub struct VerdictStore {
    dir: PathBuf,
}

impl VerdictStore {
    /// Open (creating on first write) the review store under `<workgraph_dir>/review`.
    pub fn open(workgraph_dir: &Path) -> Self {
        Self {
            dir: workgraph_dir.join("review"),
        }
    }

    fn chain_path(&self) -> PathBuf {
        self.dir.join("verdicts.jsonl")
    }

    /// Sidecar advisory-lock path. Locked separately from the data file so the atomic
    /// temp-file+rename append (which replaces the inode) does not invalidate the lock.
    fn lock_path(&self) -> PathBuf {
        self.dir.join("verdicts.lock")
    }

    /// Acquire the exclusive append lock (audit M23). Held for the returned guard's
    /// lifetime, serializing concurrent `record` calls (in-process threads and
    /// cross-process `wg review` invocations) at the single-writer boundary.
    fn lock(&self) -> Result<ChainLock> {
        ChainLock::acquire(&self.lock_path())
    }

    fn trust_path(&self) -> PathBuf {
        self.dir.join("trust_overrides.json")
    }

    /// Load the full verdict chain (oldest first).
    pub fn load_chain(&self) -> Result<Vec<VerdictRecord>> {
        let path = self.chain_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: VerdictRecord = serde_json::from_str(line)
                .with_context(|| format!("parsing verdict record at line {}", i + 1))?;
            out.push(rec);
        }
        Ok(out)
    }

    /// Append a verdict from a pipeline outcome, hash-linked to the chain tip
    /// (ADR-CS3 D2). Returns the recorded (cid-stamped) record.
    pub fn record(
        &self,
        outcome: &PipelineOutcome,
        author: Option<&str>,
        consumer_task: Option<&str>,
    ) -> Result<VerdictRecord> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        // Serialize the read-modify-write under an exclusive advisory lock (audit M23,
        // same fix class as the lease ledger's B3 lock). Without it, two concurrent
        // recorders both load the chain at length N, both append "N+1", and the atomic
        // rename of one clobbers the other — a lost verdict and a broken hash link. The
        // lock is held for the whole load → hash-link → append critical section.
        let _lock = self.lock()?;
        let chain = self.load_chain()?;
        let seq = chain.len() as u64;
        let prev = chain.last().map(|r| r.cid.clone()).unwrap_or_default();

        let mut rec = VerdictRecord {
            seq,
            verdict: outcome.verdict,
            reason: outcome.reason,
            content_class: outcome.content_class,
            deciding_pass: outcome.deciding_pass,
            confidence: outcome.confidence,
            depth_label: outcome.depth.label.to_string(),
            provenance: RecordProvenance {
                author: author.map(|s| s.to_string()),
                sigchain_pos: seq,
                content_cid: outcome.content_cid.clone(),
            },
            consumer_task: consumer_task.map(|s| s.to_string()),
            trace: outcome.trace.clone(),
            prev,
            cid: String::new(),
        };
        // Content-address the record (cid covers everything but `cid` itself).
        let mut value = serde_json::to_value(&rec)?;
        if let serde_json::Value::Object(map) = &mut value {
            map.remove("cid");
        }
        rec.cid = content_cid(&value);

        let line = serde_json::to_string(&rec)?;
        let mut text = match std::fs::read_to_string(self.chain_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        text.push_str(&line);
        text.push('\n');
        crate::atomic_file::write_atomic(&self.chain_path(), text.as_bytes())
            .with_context(|| format!("appending to {}", self.chain_path().display()))?;
        Ok(rec)
    }

    /// Find the most recent verdict record for a content digest (the trace leg of
    /// revoke, ADR-CS3 D4).
    pub fn find_by_cid(&self, content_cid: &str) -> Result<Option<VerdictRecord>> {
        let chain = self.load_chain()?;
        Ok(chain
            .into_iter()
            .rev()
            .find(|r| r.provenance.content_cid == content_cid))
    }

    /// **Digest-pinned consumption (MUST-2 / RA-8).** Re-hash `content` and accept
    /// **only** if its CID matches an `accept` verdict on record. A post-review
    /// mutated byte changes the CID → rejected; a mutable-name swap → no match.
    pub fn digest_pin_consume(&self, content: &str) -> Result<DigestPinResult> {
        let cid = content_cid(&serde_json::Value::String(content.to_string()));
        match self.find_by_cid(&cid)? {
            Some(rec) if rec.verdict == Verdict::Accept => Ok(DigestPinResult {
                permitted: true,
                cid,
                matched_seq: Some(rec.seq),
                detail: "digest matches an accept verdict — consumption permitted".to_string(),
            }),
            Some(rec) => Ok(DigestPinResult {
                permitted: false,
                cid,
                matched_seq: Some(rec.seq),
                detail: format!(
                    "digest matches a {} verdict — consumption refused",
                    rec.verdict.tag()
                ),
            }),
            None => Ok(DigestPinResult {
                permitted: false,
                cid,
                matched_seq: None,
                detail: "no accept verdict pins this digest — the reviewed bytes and the \
                         presented bytes differ (RA-8 TOCTOU / mutable-name swap)"
                    .to_string(),
            }),
        }
    }

    /// Read the trust-override ledger (wgid → lowered trust). The gate consults this
    /// so a revoked author's **next** item takes the deep path.
    pub fn trust_override(&self, author: &str) -> Result<Option<TrustLevel>> {
        Ok(self.load_trust_overrides()?.remove(author))
    }

    fn load_trust_overrides(&self) -> Result<HashMap<String, TrustLevel>> {
        match std::fs::read_to_string(self.trust_path()) {
            Ok(t) => Ok(serde_json::from_str(&t)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// **The loud revoke leg (ADR-CS3 D4).** Trace the verdict record for `content_cid`,
    /// **lower** the author's trust (persisted so the next item takes the deep path),
    /// and report the downstream `--after` consumer to re-run (the TC8 leg — a single
    /// hand-wired re-run for the spark; the full cross-plane D-iii is Review-Wave C).
    pub fn revoke(&self, content_cid: &str) -> Result<RevokeOutcome> {
        let rec = self
            .find_by_cid(content_cid)?
            .with_context(|| format!("no verdict record pins digest {content_cid}"))?;
        let author = rec
            .provenance
            .author
            .clone()
            .ok_or_else(|| anyhow::anyhow!("verdict record has no author to revoke"))?;

        // Lower the author's trust monotonically and persist it.
        let mut overrides = self.load_trust_overrides()?;
        let prior = overrides
            .get(&author)
            .cloned()
            .unwrap_or(TrustLevel::Verified);
        let lowered = lower_trust(&prior);
        overrides.insert(author.clone(), lowered.clone());
        std::fs::create_dir_all(&self.dir)?;
        crate::atomic_file::write_atomic(
            &self.trust_path(),
            serde_json::to_string_pretty(&overrides)?.as_bytes(),
        )
        .with_context(|| format!("writing {}", self.trust_path().display()))?;

        Ok(RevokeOutcome {
            author,
            content_cid: content_cid.to_string(),
            sigchain_pos: rec.provenance.sigchain_pos,
            lowered_trust: lowered,
            rerun_consumers: rec.consumer_task.into_iter().collect(),
        })
    }
}

/// The result of a digest-pinned consumption check (MUST-2).
#[derive(Debug, Clone, Serialize)]
pub struct DigestPinResult {
    /// Consumption permitted (digest matched an `accept` verdict)?
    pub permitted: bool,
    /// The CID of the presented bytes.
    pub cid: String,
    /// The seq of the matched verdict record, if any.
    pub matched_seq: Option<u64>,
    pub detail: String,
}

/// The result of the revoke leg (ADR-CS3 D4).
#[derive(Debug, Clone, Serialize)]
pub struct RevokeOutcome {
    pub author: String,
    pub content_cid: String,
    pub sigchain_pos: u64,
    /// The author's new (lowered) trust — the next item takes the deep path.
    pub lowered_trust: TrustLevel,
    /// The downstream `--after` consumers to re-run (the TC8 leg).
    pub rerun_consumers: Vec<String>,
}

/// Lower a trust level one step (monotonic): Verified → Provisional → Unknown →
/// Unknown. The revoke leg never *raises* trust.
pub fn lower_trust(t: &TrustLevel) -> TrustLevel {
    match t {
        TrustLevel::Verified => TrustLevel::Provisional,
        TrustLevel::Provisional => TrustLevel::Unknown,
        TrustLevel::Unknown => TrustLevel::Unknown,
    }
}

/// RAII exclusive advisory lock for the verdict-chain append (audit M23). Mirrors the
/// lease ledger's `LedgerLock` (B3): `flock(LOCK_EX)` on a sidecar lock file with the
/// project's transient-error retry policy; released on drop. A no-op on non-Unix.
struct ChainLock {
    #[cfg(unix)]
    #[allow(dead_code)] // held for its RAII lock lifetime, not read
    file: std::fs::File,
}

impl ChainLock {
    #[cfg(unix)]
    fn acquire(lock_path: &Path) -> Result<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .with_context(|| format!("opening verdict-chain lock {}", lock_path.display()))?;
        let fd = file.as_raw_fd();
        let policy = crate::lock::RetryPolicy::default();
        crate::lock::retry_acquire(&policy, crate::lock::is_transient_blocking, || {
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        })
        .with_context(|| format!("acquiring exclusive lock on {}", lock_path.display()))?;
        Ok(Self { file })
    }

    #[cfg(not(unix))]
    fn acquire(_lock_path: &Path) -> Result<Self> {
        Ok(Self {})
    }
}

#[cfg(unix)]
impl Drop for ChainLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.file.as_raw_fd();
        // Best-effort release; the fd close on drop also releases the flock.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Confidence, ContentClass, PipelineOutcome, ReasonCode, TrustLevel, Verdict, VerdictStore,
        content_cid, lower_trust,
    };
    use crate::review::{ReviewDepth, Sensitivity, review_depth};
    use std::path::PathBuf;

    fn tmpdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        let uniq = format!(
            "wg-review-test-{}-{}",
            std::process::id(),
            // a per-call counter via an atomic to avoid Math.random-style nondeterminism
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );
        p.push(uniq);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn outcome(verdict: Verdict, cid: &str) -> PipelineOutcome {
        let depth: ReviewDepth = review_depth(&TrustLevel::Unknown, Sensitivity::Low);
        PipelineOutcome {
            verdict,
            reason: ReasonCode::Clean,
            content_class: ContentClass::Ic1Task,
            deciding_pass: 1,
            confidence: Confidence::High,
            depth,
            effective_sensitivity: Sensitivity::Low,
            sensitivity_overridden: false,
            content_cid: cid.to_string(),
            trace: vec![],
        }
    }

    #[test]
    fn chain_is_hash_linked() {
        let dir = tmpdir();
        let store = VerdictStore::open(&dir);
        let r0 = store
            .record(&outcome(Verdict::Accept, "b3:aaa"), Some("alice"), None)
            .unwrap();
        let r1 = store
            .record(&outcome(Verdict::Reject, "b3:bbb"), Some("mallory"), None)
            .unwrap();
        assert_eq!(r0.seq, 0);
        assert_eq!(r0.prev, "");
        assert_eq!(r1.seq, 1);
        assert_eq!(r1.prev, r0.cid, "record 1 links to record 0");
        assert!(!r0.cid.is_empty() && r0.cid.starts_with("b3:"));
    }

    #[test]
    fn digest_pin_rejects_mutated_bytes() {
        let dir = tmpdir();
        let store = VerdictStore::open(&dir);
        let reviewed = "the exact reviewed bytes";
        let cid = content_cid(&serde_json::Value::String(reviewed.to_string()));
        store
            .record(&outcome(Verdict::Accept, &cid), Some("alice"), None)
            .unwrap();

        // Exact bytes → permitted.
        let ok = store.digest_pin_consume(reviewed).unwrap();
        assert!(ok.permitted, "{:?}", ok);

        // A mutated byte → no matching accept verdict → refused (RA-8).
        let bad = store
            .digest_pin_consume("the exact reviewed byteS")
            .unwrap();
        assert!(!bad.permitted, "mutated bytes must be refused");
    }

    #[test]
    fn revoke_lowers_trust_and_names_consumer() {
        let dir = tmpdir();
        let store = VerdictStore::open(&dir);
        let cid = "b3:poison";
        store
            .record(
                &outcome(Verdict::Accept, cid),
                Some("wgid:V"),
                Some("downstream-C"),
            )
            .unwrap();
        // Before revoke, no override.
        assert!(store.trust_override("wgid:V").unwrap().is_none());

        let out = store.revoke(cid).unwrap();
        assert_eq!(out.author, "wgid:V");
        assert_eq!(out.lowered_trust, TrustLevel::Provisional);
        assert_eq!(out.rerun_consumers, vec!["downstream-C".to_string()]);
        // After revoke, the author's NEXT item is read at the lowered trust.
        assert_eq!(
            store.trust_override("wgid:V").unwrap(),
            Some(TrustLevel::Provisional)
        );
    }

    #[test]
    fn lower_trust_is_monotonic() {
        assert_eq!(lower_trust(&TrustLevel::Verified), TrustLevel::Provisional);
        assert_eq!(lower_trust(&TrustLevel::Provisional), TrustLevel::Unknown);
        assert_eq!(lower_trust(&TrustLevel::Unknown), TrustLevel::Unknown);
    }
}
