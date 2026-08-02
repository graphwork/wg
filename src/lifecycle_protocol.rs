//! Versioned, pure reference reducer for WG lifecycle/finish conformance.
//!
//! This module intentionally models only correctness-critical control state. It
//! does not model processes, filesystems, providers, or UI implementation. The
//! production observers/finalizer translate durable facts into [`Event`]s and
//! persist the returned [`State`] and [`Decision`] atomically.

use serde::{Deserialize, Serialize};

pub const LIFECYCLE_WIRE_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapperChildCapability {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub fence: u64,
    pub wrapper_epoch: u64,
    pub child_epoch: u64,
    pub wrapper_identity_digest: String,
    pub child_identity_digest: String,
    pub owned_child: bool,
}

/// Short model-facing name for the runtime wrapper/native-child capability.
pub type Capability = WrapperChildCapability;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub base_cas: String,
    /// Abstract candidate projection proof: protected `.wg` identity/resources
    /// were excluded. The filesystem mechanism is outside this model.
    pub protected_free: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuccessfulDisposition {
    Land,
    Deliver,
    Report,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishTransaction {
    pub candidate: Candidate,
    pub disposition: SuccessfulDisposition,
    pub promotion_receipt: bool,
    pub cleanup_committed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingAction {
    ResumeSame,
    BeginFinish,
    Promote,
    Cleanup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub wire_version: u16,
    pub phase: TaskPhase,
    pub owner: Option<Capability>,
    pub worktree_lease: Option<Capability>,
    pub session_lease: Option<Capability>,
    /// Exclusive authority to replay an in-flight finish transaction. This is
    /// acquired with the immutable transaction and released only by cleanup.
    pub finish_lease: Option<Capability>,
    pub settled: bool,
    pub owner_proven_dead: bool,
    pub pending_action: Option<PendingAction>,
    pub action_deadline: Option<u64>,
    pub candidate: Option<Candidate>,
    pub accepted_candidate: Option<Candidate>,
    pub finish_tx: Option<FinishTransaction>,
    pub promotion_count: u8,
    pub breaker_charges: u32,
    pub inert_messages: u32,
}

impl State {
    pub fn initial(capability: Capability) -> Self {
        Self {
            wire_version: LIFECYCLE_WIRE_VERSION,
            phase: TaskPhase::Running,
            owner: Some(capability.clone()),
            worktree_lease: Some(capability.clone()),
            session_lease: Some(capability),
            finish_lease: None,
            settled: false,
            owner_proven_dead: false,
            pending_action: None,
            action_deadline: None,
            candidate: None,
            accepted_candidate: None,
            finish_tx: None,
            promotion_count: 0,
            breaker_charges: 0,
            inert_messages: 0,
        }
    }

    /// Recovery rank. Every enabled finish recovery action strictly decreases
    /// this value: receipt/no-tx (3), tx (2), promoted (1), cleaned (0).
    pub fn recovery_rank(&self) -> u8 {
        match &self.finish_tx {
            Some(tx) if tx.cleanup_committed => 0,
            Some(tx) if tx.promotion_receipt => 1,
            Some(_) => 2,
            None if self.settled && self.accepted_candidate.is_some() => 3,
            None => 0,
        }
    }

    pub fn dependency_satisfied(&self) -> bool {
        self.phase == TaskPhase::Done
            && self
                .finish_tx
                .as_ref()
                .is_some_and(|tx| tx.promotion_receipt && tx.cleanup_committed)
    }

    pub fn normalized(&self) -> NormalizedState {
        NormalizedState {
            wire_version: self.wire_version,
            phase: self.phase.clone(),
            owner: self.owner.clone(),
            worktree_lease: self.worktree_lease.clone(),
            session_lease: self.session_lease.clone(),
            finish_lease: self.finish_lease.clone(),
            pending_action: self.pending_action,
            action_deadline: self.action_deadline,
            candidate: self.candidate.clone(),
            accepted_candidate: self.accepted_candidate.clone(),
            finish_tx: self.finish_tx.clone(),
            promotion_count: self.promotion_count,
            breaker_charges: self.breaker_charges,
            dependency_satisfied: self.dependency_satisfied(),
            recovery_rank: self.recovery_rank(),
            inert_messages: self.inert_messages,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedState {
    pub wire_version: u16,
    pub phase: TaskPhase,
    pub owner: Option<Capability>,
    pub worktree_lease: Option<Capability>,
    pub session_lease: Option<Capability>,
    pub finish_lease: Option<Capability>,
    pub pending_action: Option<PendingAction>,
    pub action_deadline: Option<u64>,
    pub candidate: Option<Candidate>,
    pub accepted_candidate: Option<Candidate>,
    pub finish_tx: Option<FinishTransaction>,
    pub promotion_count: u8,
    pub breaker_charges: u32,
    pub dependency_satisfied: bool,
    pub recovery_rank: u8,
    pub inert_messages: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    CandidateValidated {
        caller: Capability,
        candidate: Candidate,
    },
    ChildSettled {
        caller: Capability,
        candidate: Candidate,
        deadline: u64,
    },
    OwnerProvenDead {
        caller: Capability,
        truthful: bool,
        deadline: u64,
    },
    /// The current wrapper may finish for the native child it owns. It is not
    /// required (nor expected) to be a descendant of that child.
    WrapperHandoff {
        caller: Capability,
        disposition: SuccessfulDisposition,
        deadline: u64,
    },
    ResumeSame {
        caller: Capability,
        new_wrapper_epoch: u64,
        new_child_epoch: u64,
    },
    BeginFinish {
        caller: Capability,
        disposition: SuccessfulDisposition,
    },
    Promote {
        caller: Capability,
        candidate_id: String,
        base_cas: String,
        current_base_cas: String,
    },
    CommitCleanup {
        caller: Capability,
    },
    Fail {
        caller: Capability,
    },
    OwnershipContention {
        caller: Capability,
    },
    Message {
        body: String,
    },
    Crash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum Decision {
    Applied,
    Noop,
    Rejected(RejectReason),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    WireVersion,
    StaleCapability,
    InvalidTopology,
    UntruthfulDeathObservation,
    CandidateNotProtected,
    CandidateMismatch,
    CandidateNotAccepted,
    MissingFinishTransaction,
    CasMoved,
    InvalidPhase,
    InvalidRecoveryAction,
}

fn exact_owner(state: &State, caller: &Capability) -> bool {
    state.owner.as_ref() == Some(caller)
        && state.worktree_lease.as_ref() == Some(caller)
        && state.session_lease.as_ref() == Some(caller)
}

fn exact_finish_owner(state: &State, caller: &Capability) -> bool {
    exact_owner(state, caller) && state.finish_lease.as_ref() == Some(caller)
}

fn reject(state: &State, reason: RejectReason) -> (State, Decision) {
    (state.clone(), Decision::Rejected(reason))
}

/// Execute one deterministic lifecycle event.
///
/// Rejected and replayed events are inert. Terminal states accept only inert
/// messages/crash observations as no-ops, so late writes cannot resurrect a
/// task. Expected ownership contention never increments `breaker_charges`.
#[must_use]
pub fn reduce(state: &State, event: &Event) -> (State, Decision) {
    if state.wire_version != LIFECYCLE_WIRE_VERSION {
        return reject(state, RejectReason::WireVersion);
    }

    if state.phase != TaskPhase::Running {
        return (state.clone(), Decision::Noop);
    }

    match event {
        Event::Message { .. } => {
            let mut next = state.clone();
            next.inert_messages = next.inert_messages.saturating_add(1);
            (next, Decision::Applied)
        }
        Event::Crash => (state.clone(), Decision::Noop),
        Event::OwnershipContention { .. } => (state.clone(), Decision::Noop),
        Event::CandidateValidated { caller, candidate } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if !candidate.protected_free {
                return reject(state, RejectReason::CandidateNotProtected);
            }
            if let Some(existing) = &state.accepted_candidate {
                return if existing == candidate {
                    (state.clone(), Decision::Noop)
                } else {
                    reject(state, RejectReason::CandidateMismatch)
                };
            }
            let mut next = state.clone();
            next.candidate = Some(candidate.clone());
            next.accepted_candidate = Some(candidate.clone());
            (next, Decision::Applied)
        }
        Event::ChildSettled {
            caller,
            candidate,
            deadline,
        } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if !candidate.protected_free {
                return reject(state, RejectReason::CandidateNotProtected);
            }
            if state.accepted_candidate.as_ref() != Some(candidate) {
                return reject(state, RejectReason::CandidateNotAccepted);
            }
            let mut next = state.clone();
            next.settled = true;
            next.candidate = Some(candidate.clone());
            next.pending_action = Some(PendingAction::BeginFinish);
            next.action_deadline = Some(*deadline);
            (next, Decision::Applied)
        }
        Event::OwnerProvenDead {
            caller,
            truthful,
            deadline,
        } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if !truthful {
                return reject(state, RejectReason::UntruthfulDeathObservation);
            }
            let mut next = state.clone();
            next.owner_proven_dead = true;
            next.pending_action = Some(match &state.finish_tx {
                Some(tx) if tx.promotion_receipt => PendingAction::Cleanup,
                Some(_) => PendingAction::Promote,
                None => PendingAction::ResumeSame,
            });
            next.action_deadline = Some(*deadline);
            (next, Decision::Applied)
        }
        Event::WrapperHandoff {
            caller,
            disposition,
            deadline,
        } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            // The exact wrapper/child epochs in the capability prove topology.
            // No parent/descendant inversion is consulted.
            if state.owner_proven_dead
                || caller.wrapper_epoch == 0
                || caller.child_epoch == 0
                || caller.wrapper_identity_digest.is_empty()
                || caller.child_identity_digest.is_empty()
                || !caller.owned_child
            {
                return reject(state, RejectReason::InvalidTopology);
            }
            let Some(candidate) = state.accepted_candidate.clone() else {
                return reject(state, RejectReason::CandidateNotAccepted);
            };
            if let Some(tx) = &state.finish_tx {
                return if tx.candidate == candidate && tx.disposition == *disposition {
                    (state.clone(), Decision::Noop)
                } else {
                    reject(state, RejectReason::CandidateMismatch)
                };
            }
            let mut next = state.clone();
            next.settled = true;
            next.finish_tx = Some(FinishTransaction {
                candidate,
                disposition: *disposition,
                promotion_receipt: false,
                cleanup_committed: false,
            });
            next.finish_lease = Some(caller.clone());
            next.pending_action = Some(PendingAction::Promote);
            next.action_deadline = Some(*deadline);
            (next, Decision::Applied)
        }
        Event::ResumeSame {
            caller,
            new_wrapper_epoch,
            new_child_epoch,
        } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if state.pending_action != Some(PendingAction::ResumeSame)
                || state.finish_tx.is_some()
                || state.finish_lease.is_some()
                || *new_wrapper_epoch <= caller.wrapper_epoch
                || *new_child_epoch <= caller.child_epoch
            {
                return reject(state, RejectReason::InvalidRecoveryAction);
            }
            let mut cap = caller.clone();
            cap.wrapper_epoch = *new_wrapper_epoch;
            cap.child_epoch = *new_child_epoch;
            let mut next = state.clone();
            next.owner = Some(cap.clone());
            next.worktree_lease = Some(cap.clone());
            next.session_lease = Some(cap);
            next.finish_lease = None;
            next.settled = false;
            next.owner_proven_dead = false;
            next.pending_action = None;
            next.action_deadline = None;
            (next, Decision::Applied)
        }
        Event::BeginFinish {
            caller,
            disposition,
        } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if state.pending_action != Some(PendingAction::BeginFinish) {
                return reject(state, RejectReason::InvalidRecoveryAction);
            }
            let Some(candidate) = state.accepted_candidate.clone() else {
                return reject(state, RejectReason::CandidateNotAccepted);
            };
            if state.finish_tx.is_some() {
                return (state.clone(), Decision::Noop);
            }
            let mut next = state.clone();
            next.finish_tx = Some(FinishTransaction {
                candidate,
                disposition: *disposition,
                promotion_receipt: false,
                cleanup_committed: false,
            });
            next.finish_lease = Some(caller.clone());
            next.pending_action = Some(PendingAction::Promote);
            (next, Decision::Applied)
        }
        Event::Promote {
            caller,
            candidate_id,
            base_cas,
            current_base_cas,
        } => {
            if !exact_finish_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            let Some(tx) = &state.finish_tx else {
                return reject(state, RejectReason::MissingFinishTransaction);
            };
            if tx.promotion_receipt {
                return (state.clone(), Decision::Noop);
            }
            if tx.candidate.id != *candidate_id || tx.candidate.base_cas != *base_cas {
                return reject(state, RejectReason::CandidateMismatch);
            }
            if *current_base_cas != *base_cas {
                return reject(state, RejectReason::CasMoved);
            }
            let mut next = state.clone();
            let next_tx = next.finish_tx.as_mut().expect("checked above");
            next_tx.promotion_receipt = true;
            next.promotion_count = 1;
            next.pending_action = Some(PendingAction::Cleanup);
            (next, Decision::Applied)
        }
        Event::CommitCleanup { caller } => {
            if !exact_finish_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            let Some(tx) = &state.finish_tx else {
                return reject(state, RejectReason::MissingFinishTransaction);
            };
            if !tx.promotion_receipt {
                return reject(state, RejectReason::InvalidRecoveryAction);
            }
            let mut next = state.clone();
            let next_tx = next.finish_tx.as_mut().expect("checked above");
            next_tx.cleanup_committed = true;
            next.owner = None;
            next.worktree_lease = None;
            next.session_lease = None;
            next.finish_lease = None;
            next.pending_action = None;
            next.action_deadline = None;
            next.phase = TaskPhase::Done;
            (next, Decision::Applied)
        }
        Event::Fail { caller } => {
            if !exact_owner(state, caller) {
                return reject(state, RejectReason::StaleCapability);
            }
            if state.accepted_candidate.is_some() || state.finish_tx.is_some() {
                return reject(state, RejectReason::InvalidPhase);
            }
            let mut next = state.clone();
            next.phase = TaskPhase::Failed;
            next.owner = None;
            next.worktree_lease = None;
            next.session_lease = None;
            next.pending_action = None;
            next.action_deadline = None;
            (next, Decision::Applied)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceFixture {
    pub wire_version: u16,
    pub name: String,
    pub initial: State,
    pub events: Vec<Event>,
    pub expected_decisions: Vec<Decision>,
    pub expected_final: NormalizedState,
}

pub fn replay(initial: &State, events: &[Event]) -> (State, Vec<Decision>) {
    events.iter().fold(
        (initial.clone(), Vec::with_capacity(events.len())),
        |(state, mut decisions), event| {
            let (next, decision) = reduce(&state, event);
            decisions.push(decision);
            (next, decisions)
        },
    )
}
