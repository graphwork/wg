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

use anyhow::Result;
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
}
