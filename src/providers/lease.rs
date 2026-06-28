//! The cross-host lease + the **monotonic lease-epoch fence** (ADR-E3 D5/D6).
//!
//! A remote claim is a *lease*: the provider holds task T for a bounded, renewable
//! term, judged live by the **authorizer's observation of accepted signed
//! `LeaseRenewal`s** (never the provider's self-report — `is_process_alive()` is
//! meaningless across a host). The double-execution hazard (a reclaimed-then-resurrected
//! worker racing the reclaim-placed worker to write one task's result) is closed by a
//! **monotonic epoch enforced by an atomic compare-and-set at the single canonical-graph
//! write boundary**. There is exactly one canonical writer — the authorizer — so the CAS
//! is one in-process check-and-set, **not** distributed consensus and **not** a
//! TOCTOU-prone read-then-write (X-4). The reclaim stance is **prefer-liveness**, which
//! the fence makes safe: reclaiming a live-but-partitioned worker costs at most one
//! wasted re-run, never a corrupt graph.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::identity::keys::verify_sig;
use crate::identity::sigchain::AuthorizedKeys;
use crate::identity::{ALG_ED25519, ENVELOPE_V, content_cid, signing_digest};

/// The signed lease carried in a `RunGrant` (ADR-E3 D5). Signed by the authorizer; the
/// epoch is the fencing token a later write must still match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub v: u16,
    pub alg: String,
    pub task_id: String,
    pub authorizer: String,
    pub provider: String,
    /// The monotonic fencing epoch for this placement of T.
    pub epoch: u64,
    /// Lease term (seconds without an accepted renewal ⇒ reclaimable).
    pub term_secs: i64,
    /// Renew cadence (seconds) the worker must beat.
    pub renew_cadence_secs: i64,
    pub granted_at: String,
    #[serde(default)]
    pub sig: String,
}

impl Lease {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        task_id: &str,
        authorizer: &str,
        provider: &str,
        epoch: u64,
        term_secs: i64,
        renew_cadence_secs: i64,
        granted_at: &str,
    ) -> Self {
        Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            task_id: task_id.to_string(),
            authorizer: authorizer.to_string(),
            provider: provider.to_string(),
            epoch,
            term_secs,
            renew_cadence_secs,
            granted_at: granted_at.to_string(),
            sig: String::new(),
        }
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("Lease serializes")
    }

    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    pub fn sign(
        &mut self,
        custodian: &crate::identity::keys::Custodian,
        signer_kid: &str,
    ) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// Verify the lease signature against the authorizer's authorized key set.
    pub fn verify_sig(&self, authorizer_auth: &AuthorizedKeys) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        super::verify_sig_authorized(&digest, &self.sig, authorizer_auth, "Lease")
    }
}

/// Why the epoch fence rejected a write at the canonical boundary (ADR-E3 D6).
#[derive(Debug, Clone, PartialEq)]
pub enum FenceError {
    /// The presented epoch is older than the current one — a partitioned/reclaimed
    /// worker's late write, or a replayed stale renewal/result. Rejected.
    StaleEpoch { presented: u64, current: u64 },
    /// The current epoch was already committed — a replay of an already-landed result.
    AlreadyCommitted { epoch: u64 },
    /// No placement on record for this task.
    NoPlacement,
}

impl std::fmt::Display for FenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FenceError::StaleEpoch { presented, current } => write!(
                f,
                "stale lease epoch {presented} (current {current}) — rejected by the fence \
                 (a partitioned/reclaimed worker or a replay; ADR-E3 D6)"
            ),
            FenceError::AlreadyCommitted { epoch } => write!(
                f,
                "lease epoch {epoch} is already committed — replay rejected by the fence"
            ),
            FenceError::NoPlacement => write!(f, "no lease placement on record for this task"),
        }
    }
}

impl std::error::Error for FenceError {}

/// The canonical per-task lease state at the authorizer (the single write boundary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseState {
    pub epoch: u64,
    pub provider: String,
    /// `true` once a result has been committed at the current epoch (the dedup flag).
    #[serde(default)]
    pub committed: bool,
    /// The highest renewal epoch the authorizer has accepted (liveness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_renewal_epoch: Option<u64>,
}

/// The canonical lease ledger — the authorizer's single-writer record of every task's
/// current epoch + commit state. The CLI persists this as JSON; mutations are the
/// in-process atomic CAS (no TOCTOU because the compare and the set are one method call).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LeaseLedger {
    #[serde(default)]
    pub tasks: std::collections::HashMap<String, LeaseState>,
}

impl LeaseLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Place (or re-place from scratch) task T on `provider`, starting the epoch at 1.
    /// Returns the new epoch. Idempotent re-placement keeps the current epoch.
    pub fn place(&mut self, task_id: &str, provider: &str) -> u64 {
        let st = self.tasks.entry(task_id.to_string()).or_insert(LeaseState {
            epoch: 1,
            provider: provider.to_string(),
            committed: false,
            last_renewal_epoch: None,
        });
        st.provider = provider.to_string();
        st.epoch
    }

    /// **Reclaim** task T: bump the epoch (`e → e+1`), clear `committed`, re-assign the
    /// provider. Returns the new epoch. The old worker's epoch is now stale (ADR-E3 D6).
    pub fn reclaim(&mut self, task_id: &str, new_provider: &str) -> Result<u64> {
        let st = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("cannot reclaim {task_id}: no placement on record"))?;
        st.epoch += 1;
        st.committed = false;
        st.provider = new_provider.to_string();
        st.last_renewal_epoch = None;
        Ok(st.epoch)
    }

    pub fn current_epoch(&self, task_id: &str) -> Option<u64> {
        self.tasks.get(task_id).map(|s| s.epoch)
    }

    /// **The atomic compare-and-set at the canonical-write boundary (ADR-E3 D6).** Accept
    /// a result write *iff* its epoch equals the current epoch AND that epoch is not
    /// already committed; on success, mark committed (the set). A stale-epoch or
    /// already-committed write is rejected — closing the double-commit + the replay.
    ///
    /// This is one method (compare and set together): there is no read-then-write TOCTOU
    /// window, which is exactly what X-4 requires.
    pub fn try_commit(&mut self, task_id: &str, presented_epoch: u64) -> Result<(), FenceError> {
        let st = self.tasks.get_mut(task_id).ok_or(FenceError::NoPlacement)?;
        if presented_epoch < st.epoch {
            return Err(FenceError::StaleEpoch {
                presented: presented_epoch,
                current: st.epoch,
            });
        }
        if presented_epoch > st.epoch {
            // A write for a *future* epoch the authorizer never granted — reject as stale
            // (the authorizer is the only epoch-minter).
            return Err(FenceError::StaleEpoch {
                presented: presented_epoch,
                current: st.epoch,
            });
        }
        if st.committed {
            return Err(FenceError::AlreadyCommitted { epoch: st.epoch });
        }
        st.committed = true;
        Ok(())
    }

    /// Accept a signed renewal only if its epoch matches the current epoch (a stale
    /// renewal after reclaim is rejected). Records the liveness signal.
    pub fn accept_renewal(
        &mut self,
        task_id: &str,
        presented_epoch: u64,
    ) -> Result<(), FenceError> {
        let st = self.tasks.get_mut(task_id).ok_or(FenceError::NoPlacement)?;
        if presented_epoch != st.epoch {
            return Err(FenceError::StaleEpoch {
                presented: presented_epoch,
                current: st.epoch,
            });
        }
        st.last_renewal_epoch = Some(
            st.last_renewal_epoch
                .map_or(presented_epoch, |p| p.max(presented_epoch)),
        );
        Ok(())
    }
}

// ── Crash-safe on-disk persistence (audit B3) ───────────────────────────────────
//
// The ledger is the authorizer's integrity backstop for the epoch fence. A silent
// reset of it — the old `unwrap_or_default()` on a corrupt/partial parse — drops every
// task's epoch back to "no placement", re-opening exactly the double-commit / replay
// the fence exists to close. The old `fs::write` was also non-atomic + unlocked, so a
// crash mid-write or a concurrent writer could truncate/clobber it. Persistence is
// therefore hardened three ways:
//
//   1. **atomic write** (temp-file + fsync + rename) so a crash mid-write never leaves
//      a half-written ledger at the canonical path;
//   2. an **advisory exclusive lock** held across the whole read-modify-write, so two
//      concurrent `wg provider` processes serialize at the single-writer boundary
//      (the CAS stays one serialized writer, ADR-E3 D6);
//   3. **refuse — never reset — on a corrupt/partial parse**: a present-but-unparseable
//      ledger is a hard error, NOT an empty default, so the fence fails closed.

impl LeaseLedger {
    /// The canonical ledger path under `<wgdir>/exec/leases.json`.
    pub fn path(workgraph_dir: &Path) -> PathBuf {
        workgraph_dir.join("exec").join("leases.json")
    }

    /// Sidecar advisory-lock path. Locking a sidecar (not the data file) keeps the
    /// atomic temp-file+rename write — which replaces the inode — from invalidating a
    /// lock taken on the ledger itself. Mirrors `parser.rs`'s `graph.lock` convention.
    fn lock_path(workgraph_dir: &Path) -> PathBuf {
        workgraph_dir.join("exec").join("leases.lock")
    }

    /// Load the ledger, **refusing (Err) on a corrupt/partial parse** rather than
    /// silently resetting to empty (audit B3). An ABSENT file is the legitimate empty
    /// ledger (`Ok(default)`); a present-but-unparseable one is a hard error so the
    /// epoch fence fails closed instead of forgetting every placement.
    pub fn load(workgraph_dir: &Path) -> Result<Self> {
        let path = Self::path(workgraph_dir);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "lease ledger at {} is corrupt or partially written — REFUSING to \
                     reset it (a silent reset drops the epoch fence and re-opens \
                     double-commit/replay; audit B3). Inspect it and move it aside \
                     manually before retrying.",
                    path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading lease ledger at {}", path.display())),
        }
    }

    /// Persist the ledger atomically (temp-file + fsync + rename; audit B3). Never a
    /// bare `fs::write`, so a crash mid-write cannot leave a truncated ledger that the
    /// next [`load`](Self::load) would (correctly) refuse.
    pub fn save(&self, workgraph_dir: &Path) -> Result<()> {
        let path = Self::path(workgraph_dir);
        let body = serde_json::to_string_pretty(self).context("serializing lease ledger")?;
        crate::atomic_file::write_atomic(&path, body.as_bytes())
            .with_context(|| format!("atomically writing lease ledger at {}", path.display()))?;
        Ok(())
    }

    /// Open the ledger under an **exclusive advisory lock** held for the returned
    /// guard's lifetime — the entry point for any read-modify-write (place / grant /
    /// commit / reclaim). The lock serializes concurrent writers so the epoch CAS stays
    /// a single serialized writer (ADR-E3 D6); the load refuses on corruption, so a
    /// mutator never overwrites a corrupt ledger with a fresh-empty one.
    pub fn open_locked(workgraph_dir: &Path) -> Result<LedgerGuard> {
        let dir = workgraph_dir.join("exec");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating exec dir {}", dir.display()))?;
        let lock = LedgerLock::acquire(&Self::lock_path(workgraph_dir))?;
        // Load AFTER taking the lock: the read + the later write are one critical
        // section, so no other writer can interleave between our read and our save.
        let ledger = Self::load(workgraph_dir)?;
        Ok(LedgerGuard {
            dir: workgraph_dir.to_path_buf(),
            ledger,
            _lock: lock,
        })
    }
}

/// An exclusive-locked, crash-safe handle to the on-disk lease ledger (audit B3). The
/// advisory lock is held until the guard drops, so the entire load → mutate → save runs
/// as one serialized critical section against other processes. Mutate `ledger` in
/// place, then call [`save`](Self::save) (still under the lock).
pub struct LedgerGuard {
    dir: PathBuf,
    pub ledger: LeaseLedger,
    _lock: LedgerLock,
}

impl LedgerGuard {
    /// Persist the (mutated) ledger atomically while still holding the lock.
    pub fn save(&self) -> Result<()> {
        self.ledger.save(&self.dir)
    }
}

/// RAII advisory **exclusive** file lock (audit B3). Unix: `flock(LOCK_EX)` on a
/// sidecar lock file with the project's transient-error retry policy (MooseFS `EIO`
/// etc.); released on drop. A no-op on non-Unix (WG targets Unix), matching the
/// graph-lock convention in `parser.rs`.
struct LedgerLock {
    #[cfg(unix)]
    #[allow(dead_code)] // held for its RAII lock lifetime, not read
    file: std::fs::File,
}

impl LedgerLock {
    #[cfg(unix)]
    fn acquire(lock_path: &Path) -> Result<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .with_context(|| format!("opening lease lock {}", lock_path.display()))?;
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
impl Drop for LedgerLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.file.as_raw_fd();
        // Best-effort release; the fd close on drop also releases the flock.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
}

/// Verify a `LeaseRenewal`'s signature against the worker's delegated signer key set
/// (ADR-E3 D5): a relay cannot fake "P is alive". The `auth` is the key set authorized
/// by the renewal's signer (the act-as-agent UCAN's `aud` provider).
pub fn verify_renewal_sig(renewal_digest: &[u8; 32], sig_hex: &str, signer_pub: &[u8; 32]) -> bool {
    let sig = match hex::decode(sig_hex) {
        Ok(b) if b.len() == 64 => {
            let mut s = [0u8; 64];
            s.copy_from_slice(&b);
            s
        }
        _ => return false,
    };
    verify_sig(signer_pub, renewal_digest, &sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_starts_at_epoch_one_and_reclaim_bumps() {
        let mut led = LeaseLedger::new();
        assert_eq!(led.place("T", "wgid:zP"), 1);
        assert_eq!(led.current_epoch("T"), Some(1));
        assert_eq!(led.reclaim("T", "wgid:zQ").unwrap(), 2);
        assert_eq!(led.current_epoch("T"), Some(2));
    }

    #[test]
    fn fence_accepts_matching_epoch_once_then_rejects_replay() {
        let mut led = LeaseLedger::new();
        led.place("T", "wgid:zP");
        // First commit at epoch 1 succeeds.
        assert!(led.try_commit("T", 1).is_ok());
        // A replay at the same epoch is rejected (already committed).
        assert_eq!(
            led.try_commit("T", 1),
            Err(FenceError::AlreadyCommitted { epoch: 1 })
        );
    }

    #[test]
    fn fence_rejects_stale_epoch_after_reclaim() {
        let mut led = LeaseLedger::new();
        led.place("T", "wgid:zP");
        led.reclaim("T", "wgid:zQ").unwrap(); // epoch → 2
        // The original (epoch-1) worker returns late — its write is fenced out.
        assert_eq!(
            led.try_commit("T", 1),
            Err(FenceError::StaleEpoch {
                presented: 1,
                current: 2
            })
        );
        // The reclaim-placed (epoch-2) worker commits fine.
        assert!(led.try_commit("T", 2).is_ok());
    }

    #[test]
    fn renewal_after_reclaim_is_stale() {
        let mut led = LeaseLedger::new();
        led.place("T", "wgid:zP");
        assert!(led.accept_renewal("T", 1).is_ok());
        led.reclaim("T", "wgid:zQ").unwrap();
        assert_eq!(
            led.accept_renewal("T", 1),
            Err(FenceError::StaleEpoch {
                presented: 1,
                current: 2
            })
        );
    }

    #[test]
    fn commit_without_placement_is_no_placement() {
        let mut led = LeaseLedger::new();
        assert_eq!(led.try_commit("ghost", 1), Err(FenceError::NoPlacement));
    }

    // ── B3: crash-safe persistence (atomic + lock + refuse-on-corrupt) ───────────

    fn scratch_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wg-lease-ledger-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn absent_ledger_loads_as_empty_ok() {
        let dir = scratch_dir("absent");
        // No exec/leases.json yet → the legitimate empty ledger, not an error.
        let led = LeaseLedger::load(&dir).expect("absent ledger is Ok(empty)");
        assert!(led.tasks.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_roundtrips_and_is_atomic() {
        let dir = scratch_dir("roundtrip");
        let mut led = LeaseLedger::new();
        led.place("T", "wgid:zP");
        led.reclaim("T", "wgid:zQ").unwrap(); // epoch → 2
        led.save(&dir).unwrap();

        // Round-trips with the epoch preserved.
        let got = LeaseLedger::load(&dir).unwrap();
        assert_eq!(got.current_epoch("T"), Some(2));

        // Atomicity: no temp file is left behind in the exec dir.
        let exec = dir.join("exec");
        let leftover_tmp: Vec<_> = std::fs::read_dir(&exec)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftover_tmp.is_empty(),
            "atomic write left a temp file behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_ledger_refuses_and_does_not_reset_to_empty() {
        let dir = scratch_dir("corrupt");
        // First persist a real placement at epoch 3.
        let mut led = LeaseLedger::new();
        led.place("T", "wgid:zP");
        led.reclaim("T", "wgid:zQ").unwrap(); // → 2
        led.reclaim("T", "wgid:zR").unwrap(); // → 3
        led.save(&dir).unwrap();

        // Simulate a crash-mid-write / corruption: truncate the file to a partial JSON.
        let path = LeaseLedger::path(&dir);
        std::fs::write(&path, b"{ \"tasks\": { \"T\": { \"epoch\":").unwrap();

        // load REFUSES — it does not silently `unwrap_or_default()` to an empty ledger
        // (which would reset the epoch fence and re-open replay/double-commit).
        let err = LeaseLedger::load(&dir).expect_err("corrupt ledger must refuse");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("REFUSING"),
            "expected a loud refuse, got: {msg}"
        );

        // open_locked also refuses, so a mutator never overwrites the corrupt file with
        // a fresh-empty one — the fence does not reset to empty on a bad parse.
        assert!(LeaseLedger::open_locked(&dir).is_err());

        // The corrupt bytes are still on disk (not clobbered to empty by a reset).
        let still = std::fs::read(&path).unwrap();
        assert!(!still.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writers_serialize_without_losing_updates() {
        // Two threads each take the exclusive lock and place a distinct task. The lock
        // serializes the read-modify-write, so NEITHER update is lost (the classic
        // lost-update race the unlocked `fs::write` allowed). Repeat to widen the race
        // window.
        let dir = scratch_dir("concurrent");
        let rounds = 20;
        std::thread::scope(|s| {
            for t in 0..2 {
                let dir = dir.clone();
                s.spawn(move || {
                    for r in 0..rounds {
                        let mut guard = LeaseLedger::open_locked(&dir).unwrap();
                        guard.ledger.place(&format!("task-{t}-{r}"), "wgid:zP");
                        guard.save().unwrap();
                    }
                });
            }
        });
        let led = LeaseLedger::load(&dir).unwrap();
        // Every placement from both threads survives — no lost update under contention.
        assert_eq!(led.tasks.len(), 2 * rounds);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_guard_commit_survives_reload() {
        // The fence state set under the lock persists across processes (load).
        let dir = scratch_dir("guard-commit");
        {
            let mut g = LeaseLedger::open_locked(&dir).unwrap();
            g.ledger.place("T", "wgid:zP");
            g.save().unwrap();
        }
        {
            let mut g = LeaseLedger::open_locked(&dir).unwrap();
            assert!(g.ledger.try_commit("T", 1).is_ok());
            g.save().unwrap();
        }
        // A replay after reload is fenced (committed state survived the round trip).
        let mut g = LeaseLedger::open_locked(&dir).unwrap();
        assert_eq!(
            g.ledger.try_commit("T", 1),
            Err(FenceError::AlreadyCommitted { epoch: 1 })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
