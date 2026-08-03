//! Pure reducer for the atomic completion write-ahead protocol.
//!
//! The reducer decides legal monotone phase edges and exact idempotent replay.
//! Persistence ordering, fsync, Git, process, and filesystem effects are adapter
//! obligations and are intentionally outside this module.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::completion_evidence::{
    AttemptSaveKey, EvidenceBinding, GraphSaveBundle, content_cid, verify_graph_save_bundle,
};

pub const SAVE_TRANSACTION_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SavePhase {
    Absent,
    Prepared,
    Quiescing,
    WorkSaved,
    CandidateSealed,
    Validated,
    AwaitingAcceptance,
    Accepted,
    DispositionRecorded,
    EffectPrepared,
    EffectCommitted,
    CleanupPrepared,
    CleanupCommitted,
    GraphSaved,
    NeedsRepair,
    AbortedPreserved,
    UpgradeBlocked,
    NeedsReconciliation,
}

impl SavePhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::GraphSaved | Self::AbortedPreserved)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTransactionState {
    pub schema_version: u32,
    pub transaction_id: String,
    pub source: AttemptSaveKey,
    pub revision: u64,
    pub phase: SavePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<EvidenceBinding>,
    #[serde(default)]
    pub evidence_cids: BTreeMap<SavePhase, String>,
    #[serde(default)]
    pub requests: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_save_cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_reason: Option<String>,
}

impl SaveTransactionState {
    pub fn new(source: AttemptSaveKey) -> Result<Self, SaveRejection> {
        source
            .transaction_id()
            .map(|transaction_id| Self {
                schema_version: SAVE_TRANSACTION_SCHEMA_VERSION,
                transaction_id,
                source,
                revision: 0,
                phase: SavePhase::Absent,
                binding: None,
                evidence_cids: BTreeMap::new(),
                requests: BTreeMap::new(),
                graph_save_cid: None,
                hold_reason: None,
            })
            .map_err(SaveRejection::evidence)
    }

    pub fn recovery_rank(&self) -> usize {
        const ORDER: &[SavePhase] = &[
            SavePhase::Absent,
            SavePhase::Prepared,
            SavePhase::Quiescing,
            SavePhase::WorkSaved,
            SavePhase::CandidateSealed,
            SavePhase::Validated,
            SavePhase::AwaitingAcceptance,
            SavePhase::Accepted,
            SavePhase::DispositionRecorded,
            SavePhase::EffectPrepared,
            SavePhase::EffectCommitted,
            SavePhase::CleanupPrepared,
            SavePhase::CleanupCommitted,
            SavePhase::GraphSaved,
        ];
        ORDER
            .iter()
            .position(|phase| *phase == self.phase)
            .map(|position| ORDER.len() - 1 - position)
            .unwrap_or(ORDER.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SaveFact {
    Evidence {
        cid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<EvidenceBinding>,
    },
    GraphSave {
        bundle: Box<GraphSaveBundle>,
    },
    Hold {
        reason: String,
    },
}

impl SaveFact {
    fn digest(&self) -> Result<String, SaveRejection> {
        content_cid(self).map_err(SaveRejection::evidence)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTransitionRequest {
    pub source: AttemptSaveKey,
    pub expected_revision: u64,
    pub expected_phase: SavePhase,
    pub next_phase: SavePhase,
    pub idempotency_key: String,
    pub action_key: String,
    pub fact: SaveFact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveCommitPlan {
    pub state: SaveTransactionState,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRejection {
    pub code: String,
    pub message: String,
}

impl SaveRejection {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    fn evidence(error: crate::completion_evidence::EvidenceError) -> Self {
        Self::new(error.code, error.message)
    }
}
impl std::fmt::Display for SaveRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for SaveRejection {}

pub struct SaveTransactionKernel;

impl SaveTransactionKernel {
    pub fn transition(
        state: &SaveTransactionState,
        request: SaveTransitionRequest,
    ) -> Result<SaveCommitPlan, SaveRejection> {
        if state.schema_version != SAVE_TRANSACTION_SCHEMA_VERSION {
            return Err(SaveRejection::new(
                "unsupported-protocol",
                "transaction schema is not supported",
            ));
        }
        if request.source != state.source {
            return Err(SaveRejection::new(
                "stale-source",
                "request is not bound to the exact transaction source tuple",
            ));
        }
        if request.idempotency_key.trim().is_empty() || request.action_key.trim().is_empty() {
            return Err(SaveRejection::new(
                "missing-idempotency",
                "idempotency and action keys are required",
            ));
        }
        let digest = request.fact.digest()?;
        if let Some(previous) = state.requests.get(&request.idempotency_key) {
            if previous == &digest {
                return Ok(SaveCommitPlan {
                    state: state.clone(),
                    duplicate: true,
                });
            }
            return Err(SaveRejection::new(
                "idempotency-conflict",
                "the idempotency key was previously committed with different bytes",
            ));
        }
        if request.expected_revision != state.revision || request.expected_phase != state.phase {
            return Err(SaveRejection::new(
                "stale-transaction",
                "transaction revision or phase changed",
            ));
        }
        if state.phase.is_terminal() {
            return Err(SaveRejection::new(
                "transaction-terminal",
                "terminal transaction is inert",
            ));
        }
        if !legal_edge(state.phase, request.next_phase) {
            return Err(SaveRejection::new(
                "illegal-phase-edge",
                format!(
                    "cannot advance {:?} to {:?}",
                    state.phase, request.next_phase
                ),
            ));
        }

        let mut next = state.clone();
        match (&request.fact, request.next_phase) {
            (
                SaveFact::Hold { reason },
                SavePhase::NeedsRepair | SavePhase::UpgradeBlocked | SavePhase::NeedsReconciliation,
            ) => {
                if reason.trim().is_empty() {
                    return Err(SaveRejection::new(
                        "missing-hold-reason",
                        "a hold must retain a named safe reason/action",
                    ));
                }
                next.hold_reason = Some(reason.clone());
            }
            (SaveFact::GraphSave { bundle }, SavePhase::GraphSaved) => {
                let verified = verify_graph_save_bundle(bundle).map_err(SaveRejection::evidence)?;
                if verified.binding.source != state.source {
                    return Err(SaveRejection::new(
                        "stale-source",
                        "GraphSave belongs to a different source tuple",
                    ));
                }
                if state
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding != &verified.binding)
                {
                    return Err(SaveRejection::new(
                        "binding-mismatch",
                        "GraphSave disagrees with the transaction candidate binding",
                    ));
                }
                next.binding = Some(verified.binding);
                next.graph_save_cid = Some(verified.graph_save_cid.clone());
                next.evidence_cids
                    .insert(SavePhase::GraphSaved, verified.graph_save_cid);
                next.hold_reason = None;
            }
            (SaveFact::Evidence { cid, binding }, phase) => {
                if cid.trim().is_empty() {
                    return Err(SaveRejection::new(
                        "missing-evidence",
                        "phase advance requires an immutable evidence CID",
                    ));
                }
                if phase >= SavePhase::WorkSaved && phase <= SavePhase::CleanupCommitted {
                    let binding = binding.as_ref().ok_or_else(|| {
                        SaveRejection::new(
                            "missing-binding",
                            "post-WorkSave evidence must repeat the full binding",
                        )
                    })?;
                    if binding.source != state.source {
                        return Err(SaveRejection::new(
                            "stale-source",
                            "evidence belongs to a different source tuple",
                        ));
                    }
                    if let Some(existing) = &state.binding {
                        if existing != binding {
                            return Err(SaveRejection::new(
                                "binding-mismatch",
                                "phase evidence changes the candidate/base binding",
                            ));
                        }
                    } else {
                        next.binding = Some(binding.clone());
                    }
                }
                next.evidence_cids.insert(phase, cid.clone());
                next.hold_reason = None;
            }
            _ => {
                return Err(SaveRejection::new(
                    "fact-phase-mismatch",
                    "fact type cannot establish the requested phase",
                ));
            }
        }
        next.phase = request.next_phase;
        next.revision += 1;
        next.requests.insert(request.idempotency_key, digest);
        Ok(SaveCommitPlan {
            state: next,
            duplicate: false,
        })
    }
}

fn legal_edge(from: SavePhase, to: SavePhase) -> bool {
    use SavePhase::*;
    matches!(
        (from, to),
        (Absent, Prepared)
            | (Prepared, Quiescing)
            | (Quiescing, WorkSaved)
            | (WorkSaved, CandidateSealed)
            | (CandidateSealed, Validated)
            | (CandidateSealed, NeedsRepair)
            | (Validated, AwaitingAcceptance)
            | (Validated, Accepted)
            | (Validated, NeedsRepair)
            | (AwaitingAcceptance, Accepted)
            | (AwaitingAcceptance, NeedsRepair)
            | (Accepted, DispositionRecorded)
            | (DispositionRecorded, EffectPrepared)
            | (EffectPrepared, EffectCommitted)
            | (EffectPrepared, NeedsRepair)
            | (EffectCommitted, CleanupPrepared)
            | (CleanupPrepared, CleanupCommitted)
            | (CleanupPrepared, NeedsRepair)
            | (CleanupCommitted, GraphSaved)
            | (Prepared, AbortedPreserved)
            | (Quiescing, AbortedPreserved)
            | (WorkSaved, AbortedPreserved)
            | (_, UpgradeBlocked)
            | (_, NeedsReconciliation)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> AttemptSaveKey {
        AttemptSaveKey {
            graph_id: "g".into(),
            task_id: "t".into(),
            generation: 2,
            attempt_id: "attempt-2-1".into(),
            attempt_fence: 7,
            worktree_lease_epoch: 3,
            process_epoch: 1,
            wrapper_epoch: 1,
            route_snapshot_cid: "route".into(),
            session_proof_digest: "session".into(),
            worktree_identity_digest: "root".into(),
        }
    }
    fn request(
        state: &SaveTransactionState,
        next_phase: SavePhase,
        key: &str,
        fact: SaveFact,
    ) -> SaveTransitionRequest {
        SaveTransitionRequest {
            source: state.source.clone(),
            expected_revision: state.revision,
            expected_phase: state.phase,
            next_phase,
            idempotency_key: key.into(),
            action_key: format!("action:{key}"),
            fact,
        }
    }
    fn evidence(cid: &str, binding: Option<EvidenceBinding>) -> SaveFact {
        SaveFact::Evidence {
            cid: cid.into(),
            binding,
        }
    }

    #[test]
    fn save_transaction_rejects_skipped_phase() {
        let state = SaveTransactionState::new(source()).unwrap();
        let error = SaveTransactionKernel::transition(
            &state,
            request(&state, SavePhase::WorkSaved, "skip", evidence("cid", None)),
        )
        .unwrap_err();
        assert_eq!(error.code, "illegal-phase-edge");
    }

    #[test]
    fn save_transaction_exact_replay_is_inert_and_conflict_fails() {
        let state = SaveTransactionState::new(source()).unwrap();
        let request = request(
            &state,
            SavePhase::Prepared,
            "intent",
            evidence("intent-cid", None),
        );
        let committed = SaveTransactionKernel::transition(&state, request.clone())
            .unwrap()
            .state;
        let replay = SaveTransactionKernel::transition(&committed, request.clone()).unwrap();
        assert!(replay.duplicate);
        let mut conflict = request;
        conflict.fact = evidence("other-cid", None);
        assert_eq!(
            SaveTransactionKernel::transition(&committed, conflict)
                .unwrap_err()
                .code,
            "idempotency-conflict"
        );
    }

    #[test]
    fn save_transaction_binding_cannot_change() {
        let mut state = SaveTransactionState::new(source()).unwrap();
        for (phase, key) in [(SavePhase::Prepared, "p"), (SavePhase::Quiescing, "q")] {
            state = SaveTransactionKernel::transition(
                &state,
                request(&state, phase, key, evidence(key, None)),
            )
            .unwrap()
            .state;
        }
        let binding = EvidenceBinding {
            source: source(),
            candidate_id: "candidate-a".into(),
            base_commit_oid: "base".into(),
        };
        state = SaveTransactionKernel::transition(
            &state,
            request(
                &state,
                SavePhase::WorkSaved,
                "w",
                evidence("w", Some(binding.clone())),
            ),
        )
        .unwrap()
        .state;
        let changed = EvidenceBinding {
            candidate_id: "candidate-b".into(),
            ..binding
        };
        let error = SaveTransactionKernel::transition(
            &state,
            request(
                &state,
                SavePhase::CandidateSealed,
                "c",
                evidence("c", Some(changed)),
            ),
        )
        .unwrap_err();
        assert_eq!(error.code, "binding-mismatch");
    }

    #[test]
    fn save_transaction_hold_requires_reason() {
        let state = SaveTransactionState::new(source()).unwrap();
        let error = SaveTransactionKernel::transition(
            &state,
            request(
                &state,
                SavePhase::NeedsReconciliation,
                "hold",
                SaveFact::Hold { reason: "".into() },
            ),
        )
        .unwrap_err();
        assert_eq!(error.code, "missing-hold-reason");
    }
}
