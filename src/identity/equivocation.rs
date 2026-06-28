//! Equivocation / fork detection (audit M12, B9) — **head gossip via persistent memory**.
//!
//! `sigchain::verify` validates *one* linear chain handed to it. It cannot, on its own,
//! catch a signer who produces **two divergent, each-validly-signed** histories from the
//! same genesis (different links at the same `seq`) and shows different ones to different
//! peers — the *equivocation* / forked-history attack (B9). A signature check passes on
//! both branches; what is wrong is that *both exist*.
//!
//! WG-Fed has no central transparency log (verification is never central, ADR-fed-001
//! §D2), so the substitute is **head gossip**: a verifier remembers, per identity, the
//! per-`seq` content ids of the chain it has already accepted. On every later observation
//! it requires the new chain to be a **consistent linear extension** of that memory — any
//! shared `seq` whose cid differs is a fork, and the two branches are equivocation
//! evidence. The simplest gossip partner is the verifier's own past self (this module);
//! the same [`detect_fork`] also compares two chains fetched from two relays directly.
//!
//! This is content-addressed and offline: a fork is proven by exhibiting two validly
//! signed links at the same `seq` with different cids — no trusted third party.

use anyhow::{Context, Result};
use std::path::Path;

use super::sigchain::{self, SigchainLink};

/// Proof that an identity equivocated: two distinct, each-validly-signed links at the
/// **same** `seq` of the same `wgid` (different content ids). Exhibiting both is the
/// non-repudiable evidence of a forked history (B9).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForkEvidence {
    pub wgid: String,
    /// The sequence number at which the two histories diverge.
    pub seq: u64,
    /// The two conflicting link content ids at that `seq`.
    pub cid_a: String,
    pub cid_b: String,
}

impl std::fmt::Display for ForkEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EQUIVOCATION for {} at seq {}: two validly-signed links {} != {} (forked history, audit B9)",
            self.wgid, self.seq, self.cid_a, self.cid_b
        )
    }
}

/// The per-`seq` content-id view of a chain (index = seq).
fn cids_by_seq(chain: &[SigchainLink]) -> Vec<String> {
    chain.iter().map(|l| l.cid()).collect()
}

/// Detect equivocation between two chains both claiming `expected_wgid`. **Both must
/// verify** (each is a validly-signed history for this identity); then the lowest shared
/// `seq` whose cids differ is a fork. `Ok(None)` ⇒ one chain is a consistent linear
/// extension of the other (no equivocation). A non-verifying chain is an error, not a
/// fork (it is simply invalid).
pub fn detect_fork(
    expected_wgid: &str,
    a: &[SigchainLink],
    b: &[SigchainLink],
) -> Result<Option<ForkEvidence>> {
    sigchain::verify(a, expected_wgid).context("verifying chain A for fork detection")?;
    sigchain::verify(b, expected_wgid).context("verifying chain B for fork detection")?;
    Ok(diff_first_seq(
        expected_wgid,
        &cids_by_seq(a),
        &cids_by_seq(b),
    ))
}

/// The first overlapping seq whose cids differ (a fork), or `None` if every overlapping
/// seq agrees (a consistent prefix/extension).
fn diff_first_seq(wgid: &str, a: &[String], b: &[String]) -> Option<ForkEvidence> {
    for seq in 0..a.len().min(b.len()) {
        if a[seq] != b[seq] {
            return Some(ForkEvidence {
                wgid: wgid.to_string(),
                seq: seq as u64,
                cid_a: a[seq].clone(),
                cid_b: b[seq].clone(),
            });
        }
    }
    None
}

/// The outcome of observing a chain against the verifier's head-gossip memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// Consistent with memory. `extended` is true when this observation grew the head.
    Consistent { head: String, extended: bool },
    /// A fork vs. what was previously accepted — **the memory is NOT updated**.
    Fork(ForkEvidence),
}

impl Observation {
    pub fn is_fork(&self) -> bool {
        matches!(self, Observation::Fork(_))
    }
}

fn gossip_path(dir: &Path, wgid: &str) -> std::path::PathBuf {
    dir.join(super::transport::sanitize(wgid))
}

/// Load the remembered per-seq cids for `wgid`, or `None` if never observed.
pub fn load_seen(dir: &Path, wgid: &str) -> Option<Vec<String>> {
    let bytes = std::fs::read(gossip_path(dir, wgid)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn store_seen(dir: &Path, wgid: &str, cids: &[String]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = gossip_path(dir, wgid);
    std::fs::write(&path, serde_json::to_vec(cids)?)
        .with_context(|| format!("writing head-gossip memory {}", path.display()))?;
    Ok(())
}

/// Observe a (caller-already-verified) `chain` for `wgid` against the head-gossip memory
/// rooted at `dir`, the head-gossip step (M12). On first sight it is recorded. On a later
/// sight it must be a consistent linear extension — any divergent shared seq is a
/// [`Observation::Fork`] (and the memory is left untouched so the fork cannot overwrite
/// the honest head). A consistent, longer chain advances the remembered head.
///
/// The caller is responsible for having verified `chain` (so `observe` records only
/// validly-signed histories); pass the chain straight from `sigchain::verify`.
pub fn observe(dir: &Path, wgid: &str, chain: &[SigchainLink]) -> Result<Observation> {
    let fresh = cids_by_seq(chain);
    let head = fresh.last().cloned().unwrap_or_default();
    match load_seen(dir, wgid) {
        None => {
            store_seen(dir, wgid, &fresh)?;
            Ok(Observation::Consistent {
                head,
                extended: false,
            })
        }
        Some(seen) => {
            if let Some(ev) = diff_first_seq(wgid, &seen, &fresh) {
                // A fork — do NOT overwrite the honest memory.
                return Ok(Observation::Fork(ev));
            }
            // Consistent. Advance the remembered head only if this view is longer.
            let extended = fresh.len() > seen.len();
            if extended {
                store_seen(dir, wgid, &fresh)?;
            }
            Ok(Observation::Consistent { head, extended })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keys::{Custodian, gen_ed25519, kid_for, wgid_from_pubkey};
    use crate::identity::sigchain::{KeyEntry, KeyRole, KeyStatus, add_key, genesis};

    fn scratch() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wg-equiv-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    /// Mint a genesis chain, returning the custodian, root keypair, kid, wgid, genesis.
    struct M {
        cust: Custodian,
        root: crate::identity::keys::Ed25519Keypair,
        root_kid: String,
        wgid: String,
        genesis: SigchainLink,
    }

    fn mint(name: &str) -> M {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let cust = Custodian::with_keystore_dir(name, dir);
        let root = gen_ed25519().unwrap();
        let root_kid = kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = wgid_from_pubkey(&root.public);
        let genesis = genesis(&cust, &root.public, &root_kid, None).unwrap();
        M {
            cust,
            root,
            root_kid,
            wgid,
            genesis,
        }
    }

    /// Append a fresh add_key (a distinct signer) onto `prev`, signed by the root.
    fn add_signer(m: &M, prev: &SigchainLink, tag: &str) -> SigchainLink {
        let signer = gen_ed25519().unwrap();
        let kid = kid_for(&signer.public);
        m.cust.store_signing_key(&kid, &signer.seed).unwrap();
        add_key(
            &m.cust,
            prev,
            &m.root.public,
            &m.root_kid,
            KeyEntry {
                kid,
                public: hex::encode(signer.public),
                role: KeyRole::Signer,
                scope: vec![tag.into()],
                status: KeyStatus::Active,
            },
        )
        .unwrap()
    }

    #[test]
    fn two_divergent_chains_are_an_equivocation() {
        let m = mint("equivA");
        // Two DIFFERENT seq=1 links from the same genesis = the signer equivocated.
        let branch_a = vec![m.genesis.clone(), add_signer(&m, &m.genesis, "branch-a")];
        let branch_b = vec![m.genesis.clone(), add_signer(&m, &m.genesis, "branch-b")];
        // Both validly verify on their own…
        assert!(sigchain::verify(&branch_a, &m.wgid).is_ok());
        assert!(sigchain::verify(&branch_b, &m.wgid).is_ok());
        // …but together they are a provable fork at seq 1.
        let ev = detect_fork(&m.wgid, &branch_a, &branch_b)
            .unwrap()
            .expect("a fork must be detected");
        assert_eq!(ev.seq, 1);
        assert_ne!(ev.cid_a, ev.cid_b);
    }

    #[test]
    fn linear_extension_is_not_a_fork() {
        let m = mint("equivB");
        let l1 = add_signer(&m, &m.genesis, "s1");
        let short = vec![m.genesis.clone(), l1.clone()];
        let l2 = add_signer(&m, &l1, "s2");
        let long = vec![m.genesis.clone(), l1, l2];
        // The longer chain extends the shorter — no fork.
        assert!(detect_fork(&m.wgid, &short, &long).unwrap().is_none());
        assert!(detect_fork(&m.wgid, &long, &short).unwrap().is_none());
    }

    #[test]
    fn head_gossip_flags_a_fork_and_protects_memory() {
        let dir = scratch();
        let m = mint("equivC");
        let honest = vec![m.genesis.clone(), add_signer(&m, &m.genesis, "honest")];
        let evil = vec![m.genesis.clone(), add_signer(&m, &m.genesis, "evil")];

        // First observation records the honest head.
        let o1 = observe(&dir, &m.wgid, &honest).unwrap();
        assert!(matches!(
            o1,
            Observation::Consistent {
                extended: false,
                ..
            }
        ));

        // A re-observation of the SAME chain is consistent (not extended).
        assert!(matches!(
            observe(&dir, &m.wgid, &honest).unwrap(),
            Observation::Consistent {
                extended: false,
                ..
            }
        ));

        // The evil divergent chain is flagged as a FORK and does not overwrite memory.
        let o2 = observe(&dir, &m.wgid, &evil).unwrap();
        assert!(o2.is_fork(), "divergent history must be flagged: {o2:?}");
        // Memory still reflects the honest chain (the fork did not poison it).
        let still_honest = observe(&dir, &m.wgid, &honest).unwrap();
        assert!(!still_honest.is_fork());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_gossip_accepts_a_consistent_extension() {
        let dir = scratch();
        let m = mint("equivD");
        let l1 = add_signer(&m, &m.genesis, "s1");
        let short = vec![m.genesis.clone(), l1.clone()];
        let long = vec![m.genesis.clone(), l1.clone(), add_signer(&m, &l1, "s2")];

        observe(&dir, &m.wgid, &short).unwrap();
        // A genuine linear extension advances the head, no fork.
        let o = observe(&dir, &m.wgid, &long).unwrap();
        assert!(matches!(o, Observation::Consistent { extended: true, .. }));
        // The remembered head is now the longer chain's head.
        assert_eq!(load_seen(&dir, &m.wgid).unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
