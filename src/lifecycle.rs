//! Authoritative task lifecycle transition kernel and durable audit ledger.
//!
//! Lifecycle requesters submit typed [`TransitionRequest`] values.  The pure
//! [`LifecycleKernel::transition`] function is the only production code that
//! decides a task status/attempt edge.  [`crate::parser::modify_graph`] appends
//! every accepted event to `.wg/lifecycle/events.jsonl` under `graph.lock`
//! before replacing the compatibility `graph.jsonl` projection.
//!
//! This is intentionally the first, compatibility-preserving migration phase:
//! legacy status spellings remain readable, while converted command families
//! acquire generation/attempt fences and durable actor/reason diagnostics.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::current_user;
use crate::graph::{FailureClass, LogEntry, Status, Task, WorkGraph};

pub const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActorKind {
    Operator,
    Dispatcher,
    Worker,
    ProcessObserver,
    WaitMatcher,
    AcceptanceController,
    EvaluationRunner,
    Finalizer,
    Reconciler,
    Importer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleActor {
    pub kind: ActorKind,
    pub id: String,
}

impl LifecycleActor {
    pub fn operator(id: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Operator,
            id: id.into(),
        }
    }

    pub fn worker(id: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Worker,
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptDisposition {
    Succeeded,
    Failed,
    Parked,
    Cancelled,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRef {
    pub id: String,
    pub generation: u64,
    pub fence: u64,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<AttemptDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PiAuthorizationState {
    Active,
    HeldOperatorRequired,
    Consumed,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiContinuationAuthorization {
    pub authorization_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
    pub session_proof_digest: String,
    pub route_snapshot_digest: String,
    pub state: PiAuthorizationState,
    pub max_replacement_epochs: u32,
    pub max_reserved_elapsed_secs: u64,
    pub epochs_used: u32,
    pub elapsed_reserved_secs: u64,
    pub issued_by_policy: String,
}

/// Durable intent to create a new source generation only after the exact prior
/// process/worktree owner is quiescent. Keeping this in the lifecycle
/// projection makes every crash boundary restart-convergent and gives
/// readiness/TUI one authoritative hold to inspect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReopenIntent {
    pub id: String,
    pub operation: String,
    pub source_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_attempt_id: Option<String>,
    pub source_fence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    pub process_epoch: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub process_identity_digest: String,
    #[serde(default)]
    pub discard_worktree: bool,
    #[serde(default)]
    pub preserve_session: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub begin_source_attempt_reason: String,
    pub requested_at: String,
}

impl ReopenIntent {
    pub fn for_task(
        task: &Task,
        operation: impl Into<String>,
        discard_worktree: bool,
        preserve_session: bool,
        begin_source_attempt_reason: impl Into<String>,
    ) -> Self {
        let operation = operation.into();
        let source_attempt_id = task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone());
        let owner_id = task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.actor_id.clone())
            .or_else(|| task.assigned.clone());
        Self {
            id: format!(
                "reopen:{}:{}:{}:{}",
                task.id,
                task.lifecycle.generation,
                source_attempt_id.as_deref().unwrap_or("none"),
                operation
            ),
            operation,
            source_generation: task.lifecycle.generation,
            source_attempt_id,
            source_fence: task.lifecycle.fence,
            owner_id,
            process_epoch: task.lifecycle.pi_process_epoch,
            process_identity_digest: task.lifecycle.pi_process_identity_digest.clone(),
            discard_worktree,
            preserve_session,
            begin_source_attempt_reason: begin_source_attempt_reason.into(),
            requested_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LifecycleProjection {
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub fence: u64,
    #[serde(default)]
    pub attempt_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_attempt: Option<AttemptRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_head: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audit: Vec<LifecycleEvent>,
    /// Current Pi child-process fence beneath the immutable source attempt.
    #[serde(default)]
    pub pi_process_epoch: u32,
    /// Digest of the exact PID/start/boot/nonce identity bound to that fence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pi_process_identity_digest: String,
    /// Prompt/continuation counter. This advances independently while the
    /// exact in-process Pi writer and `pi_process_epoch` remain unchanged.
    #[serde(default)]
    pub pi_continuation_epoch: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_continuation: Option<PiContinuationAuthorization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_terminal_reservation: Option<crate::pi_watchdog::TerminalIntentReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_intent: Option<ReopenIntent>,
}

pub fn lifecycle_projection_is_default(value: &LifecycleProjection) -> bool {
    value == &LifecycleProjection::default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FenceExpectation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

impl FenceExpectation {
    pub fn current(task: &Task) -> Self {
        Self {
            generation: Some(task.lifecycle.generation),
            attempt_id: task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.clone()),
            fence: Some(task.lifecycle.fence),
            revision: Some(task.lifecycle.revision),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TransitionKind {
    AttemptReserved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_id: Option<String>,
    },
    /// The claim crossed its serialized launch gate. This is evidence that a
    /// real source process was admitted, distinct from reservation or
    /// admission deferral; it does not change compatibility status.
    AttemptRunning {
        launch_receipt: String,
    },
    ReservationCancelled,
    AttemptSucceeded {
        /// `Some` means all currently-required acceptance evidence is present.
        /// `None` leaves the compatibility projection awaiting acceptance.
        acceptance_ref: Option<String>,
        /// Preserve the explicit legacy human-review rendering during the
        /// staged migration; other hard gates use `PendingEval` as the
        /// compatibility spelling of canonical `AwaitingAcceptance`.
        #[serde(default)]
        manual_review: bool,
    },
    AttemptFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<FailureClass>,
    },
    /// Project an exact, already-durable task-owned finish transaction after
    /// a process observer won the graph race with a late failure.  The command
    /// adapter must validate the transaction's task/generation/attempt/fence,
    /// accepted output/promotion receipt, and cleanup receipt before requesting
    /// this narrow compensating projection.
    DurableSuccessProjected {
        acceptance_ref: String,
    },
    /// The v2 completion authority. The self-contained bundle is verified by
    /// the pure kernel before `Done` is projected; adapters may materialize it
    /// from separately stored immutable objects.
    GraphSaveCommitted {
        bundle: Box<crate::completion_evidence::GraphSaveBundle>,
    },
    AttemptLost,
    AttemptParked,
    WaitSatisfied {
        wait_id: String,
        receipt_id: String,
    },
    AcceptanceSatisfied {
        acceptance_ref: String,
    },
    AcceptanceRejected {
        evidence_ref: String,
    },
    GenerationCreated,
    /// Persist a reopen request and fence a still-live old attempt without
    /// making the task runnable.
    ReopenRequested {
        intent: ReopenIntent,
    },
    /// Exact quiescence/reap proof won; release the held ownership and create
    /// the new generation in this same lifecycle transition.
    ReopenOwnerReleased {
        intent_id: String,
        exact_owner_reaped: bool,
    },
    Abandoned,
    AdmissionDeferred {
        gate: String,
    },
    EvaluationEvidence {
        evidence_ref: String,
    },
    /// Immutable candidate + deterministic validation were durably published
    /// for the exact still-running attempt. Evaluation policy may mint hidden
    /// evidence records only from this authoritative event.
    CandidateCheckpointed {
        candidate_id: String,
        manifest_cid: String,
        validation_result_id: String,
        finalization_round: u64,
    },
    ReconciliationIssue {
        issue_id: String,
    },
    /// One-time migration of an unauthenticated legacy Done row into a
    /// non-satisfying quarantine. Ordinary runtime callers cannot use this.
    LegacyCompletionQuarantined {
        record_ref: String,
    },
    MessageObserved {
        message_id: String,
    },
    LegacyCheckpointImported,
    /// Narrow policy authorization that allows a proven Pi child exit to
    /// remain pre-terminal. This is lifecycle authority, not observer state.
    PiContinuationAuthorized {
        authorization: PiContinuationAuthorization,
        initial_process_epoch: u32,
        #[serde(default)]
        initial_process_identity_digest: String,
    },
    PiContinuationHeld {
        reason: String,
    },
    PiContinuationEpochReserved {
        expected_process_epoch: u32,
        #[serde(default)]
        process_identity_digest: String,
        expected_continuation_epoch: u32,
        next_continuation_epoch: u32,
        elapsed_charge_secs: u64,
    },
    /// CAS a genuinely new exact process identity into the process fence.
    /// Continuation prompts must never use this transition.
    PiProcessEpochReplaced {
        expected_process_epoch: u32,
        expected_process_identity_digest: String,
        next_process_epoch: u32,
        next_process_identity_digest: String,
    },
    PiTerminalIntent {
        receipt: crate::pi_watchdog::TerminalIntentReceipt,
    },
    PiProcessEpochExited {
        process_epoch: u32,
        #[serde(default)]
        process_identity_digest: String,
        exact_reap_proof: bool,
        effect_safe: bool,
    },
}

impl TransitionKind {
    pub fn event_kind(&self) -> &'static str {
        match self {
            Self::AttemptReserved { .. } => "attempt-reserved",
            Self::AttemptRunning { .. } => "attempt-running",
            Self::ReservationCancelled => "reservation-cancelled",
            Self::AttemptSucceeded { .. } => "attempt-succeeded",
            Self::AttemptFailed { .. } => "attempt-failed",
            Self::DurableSuccessProjected { .. } => "durable-success-projected",
            Self::GraphSaveCommitted { .. } => "graph-save-committed",
            Self::AttemptLost => "attempt-lost",
            Self::AttemptParked => "attempt-parked",
            Self::WaitSatisfied { .. } => "wait-satisfied",
            Self::AcceptanceSatisfied { .. } => "acceptance-satisfied",
            Self::AcceptanceRejected { .. } => "acceptance-rejected",
            Self::GenerationCreated => "generation-created",
            Self::ReopenRequested { .. } => "reopen-requested",
            Self::ReopenOwnerReleased { .. } => "reopen-owner-released",
            Self::Abandoned => "abandoned",
            Self::AdmissionDeferred { .. } => "admission-deferred",
            Self::EvaluationEvidence { .. } => "evaluation-evidence",
            Self::CandidateCheckpointed { .. } => "candidate-checkpointed",
            Self::ReconciliationIssue { .. } => "reconciliation-issue",
            Self::LegacyCompletionQuarantined { .. } => "legacy-completion-quarantined",
            Self::MessageObserved { .. } => "message-observed",
            Self::LegacyCheckpointImported => "legacy-checkpoint-imported",
            Self::PiContinuationAuthorized { .. } => "pi-continuation-authorized",
            Self::PiContinuationHeld { .. } => "pi-continuation-held",
            Self::PiContinuationEpochReserved { .. } => "pi-continuation-epoch-reserved",
            Self::PiProcessEpochReplaced { .. } => "pi-process-epoch-replaced",
            Self::PiTerminalIntent { .. } => "pi-terminal-intent",
            Self::PiProcessEpochExited { .. } => "pi-process-epoch-exited",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRequest {
    pub event_id: String,
    pub idempotency_key: String,
    pub actor: LifecycleActor,
    pub reason_code: String,
    pub kind: TransitionKind,
    #[serde(default)]
    pub expected: FenceExpectation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    pub occurred_at: String,
}

impl TransitionRequest {
    pub fn new(
        kind: TransitionKind,
        actor: LifecycleActor,
        reason_code: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            event_id: format!("ev_{}", Uuid::now_v7()),
            idempotency_key: idempotency_key.into(),
            actor,
            reason_code: reason_code.into(),
            kind,
            expected: FenceExpectation::default(),
            evidence_refs: Vec::new(),
            occurred_at: Utc::now().to_rfc3339(),
        }
    }

    pub fn expecting(mut self, expectation: FenceExpectation) -> Self {
        self.expected = expectation;
        self
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence_refs.push(evidence.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub idempotency_key: String,
    pub task_id: String,
    pub task_revision: u64,
    pub generation: u64,
    pub event_kind: String,
    pub old_state: Status,
    pub new_state: Status,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub fence: u64,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    pub occurred_at: String,
    pub committed_at: String,
    pub projection: LifecycleEventProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEventProjection {
    pub status: Status,
    pub generation: u64,
    pub revision: u64,
    pub fence: u64,
    pub attempt_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_attempt: Option<AttemptRef>,
    #[serde(default)]
    pub pi_process_epoch: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pi_process_identity_digest: String,
    #[serde(default)]
    pub pi_continuation_epoch: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_continuation: Option<PiContinuationAuthorization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_terminal_reservation: Option<crate::pi_watchdog::TerminalIntentReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_intent: Option<ReopenIntent>,
}

impl LifecycleEvent {
    fn apply_projection(&self, task: &mut Task) {
        task.status = self.projection.status;
        task.lifecycle.generation = self.projection.generation;
        task.lifecycle.revision = self.projection.revision;
        task.lifecycle.fence = self.projection.fence;
        task.lifecycle.attempt_sequence = self.projection.attempt_sequence;
        task.lifecycle.current_attempt = self.projection.current_attempt.clone();
        task.lifecycle.pi_process_epoch = self.projection.pi_process_epoch;
        task.lifecycle.pi_process_identity_digest =
            self.projection.pi_process_identity_digest.clone();
        task.lifecycle.pi_continuation_epoch = self.projection.pi_continuation_epoch;
        task.lifecycle.pi_continuation = self.projection.pi_continuation.clone();
        task.lifecycle.pi_terminal_reservation = self.projection.pi_terminal_reservation.clone();
        task.lifecycle.reopen_intent = self.projection.reopen_intent.clone();
        if self.event_kind == "graph-save-committed" {
            task.completion_disposition = match task.completion_contract {
                crate::graph::CompletionContract::Land => {
                    Some(crate::graph::CompletionDisposition::Landed)
                }
                crate::graph::CompletionContract::Deliver => {
                    Some(crate::graph::CompletionDisposition::Delivered)
                }
                crate::graph::CompletionContract::Report => {
                    Some(crate::graph::CompletionDisposition::Reported)
                }
                crate::graph::CompletionContract::Explore => {
                    Some(crate::graph::CompletionDisposition::Explored)
                }
            };
            task.completion_receipt = self
                .evidence_refs
                .iter()
                .find(|value| value.starts_with("wgcid:v2:blake3:"))
                .cloned();
        }
        task.lifecycle.ledger_head = Some(self.event_id.clone());
        if !task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.event_id == self.event_id)
        {
            task.lifecycle.audit.push(self.clone());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPlan {
    pub event: LifecycleEvent,
    duplicate: bool,
}

impl CommitPlan {
    pub fn is_duplicate(&self) -> bool {
        self.duplicate
    }

    pub fn apply(&self, task: &mut Task) -> Result<(), TransitionRejection> {
        if self.duplicate {
            return Ok(());
        }
        if task.lifecycle.revision + 1 != self.event.task_revision {
            return Err(TransitionRejection::new(
                "stale_revision",
                "projection changed after transition planning",
            ));
        }
        self.event.apply_projection(task);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRejection {
    pub code: String,
    pub message: String,
}

impl TransitionRejection {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TransitionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TransitionRejection {}

pub struct LifecycleKernel;

impl LifecycleKernel {
    /// Pure lifecycle transition decision. It does not touch clocks, files, or
    /// the graph; all nondeterministic metadata is supplied in `request`.
    pub fn transition(
        task: &Task,
        request: TransitionRequest,
    ) -> Result<CommitPlan, TransitionRejection> {
        if let Some(existing) = task
            .lifecycle
            .audit
            .iter()
            .find(|event| event.idempotency_key == request.idempotency_key)
        {
            return Ok(CommitPlan {
                event: existing.clone(),
                duplicate: true,
            });
        }

        Self::check_expectation(task, &request.expected)?;

        let old_state = task.status;
        let mut new_state = old_state;
        let mut projection = task.lifecycle.clone();
        let mut completion_receipt = None;
        let kind = &request.kind;

        match kind {
            TransitionKind::AttemptReserved { owner_id } => {
                Self::require_actor(&request, &[ActorKind::Dispatcher, ActorKind::Operator])?;
                if old_state != Status::Open {
                    return Err(Self::state_rejection(old_state));
                }
                if projection
                    .current_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.disposition.is_none())
                {
                    return Err(TransitionRejection::new(
                        "attempt_active",
                        "a nonterminal attempt already owns the task",
                    ));
                }
                projection.attempt_sequence += 1;
                projection.fence += 1;
                let attempt_id = format!(
                    "attempt-{}-{}",
                    projection.generation, projection.attempt_sequence
                );
                projection.current_attempt = Some(AttemptRef {
                    id: attempt_id,
                    generation: projection.generation,
                    fence: projection.fence,
                    actor_id: owner_id.clone().unwrap_or_else(|| request.actor.id.clone()),
                    disposition: None,
                });
                new_state = Status::InProgress;
            }
            TransitionKind::AttemptRunning { launch_receipt } => {
                Self::require_actor(&request, &[ActorKind::Dispatcher, ActorKind::Reconciler])?;
                Self::require_running_attempt(task, &request)?;
                if launch_receipt.trim().is_empty() {
                    return Err(TransitionRejection::new(
                        "launch_receipt_missing",
                        "attempt-running requires an authenticated launch receipt",
                    ));
                }
                // Evidence only: AttemptReserved already owns InProgress.
            }
            TransitionKind::ReservationCancelled => {
                Self::require_actor(&request, &[ActorKind::Dispatcher, ActorKind::Reconciler])?;
                Self::require_running_attempt(task, &request)?;
                Self::terminalize_attempt(&mut projection, AttemptDisposition::Cancelled)?;
                new_state = Status::Open;
            }
            TransitionKind::AttemptSucceeded {
                acceptance_ref,
                manual_review,
            } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::Worker,
                        ActorKind::ProcessObserver,
                        ActorKind::Operator,
                        ActorKind::Finalizer,
                    ],
                )?;
                Self::require_running_attempt(task, &request)?;
                Self::terminalize_attempt(&mut projection, AttemptDisposition::Succeeded)?;
                new_state = if acceptance_ref.is_some() {
                    Status::Done
                } else if *manual_review {
                    Status::PendingValidation
                } else {
                    // Compatibility spelling for canonical AwaitingAcceptance.
                    Status::PendingEval
                };
            }
            TransitionKind::AttemptFailed { .. } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::Worker,
                        ActorKind::ProcessObserver,
                        ActorKind::Operator,
                        ActorKind::Dispatcher,
                    ],
                )?;
                Self::require_running_attempt(task, &request)?;
                Self::terminalize_attempt(&mut projection, AttemptDisposition::Failed)?;
                new_state = Status::Failed;
            }
            TransitionKind::DurableSuccessProjected { acceptance_ref } => {
                Self::require_actor(&request, &[ActorKind::Finalizer, ActorKind::Reconciler])?;
                if acceptance_ref.trim().is_empty() {
                    return Err(TransitionRejection::new(
                        "acceptance_evidence_missing",
                        "durable success projection requires an exact cleanup receipt",
                    ));
                }
                if !matches!(old_state, Status::InProgress | Status::Failed) {
                    return Err(Self::state_rejection(old_state));
                }
                let attempt = projection.current_attempt.as_mut().ok_or_else(|| {
                    TransitionRejection::new(
                        "attempt_missing",
                        "durable success projection requires the exact current attempt",
                    )
                })?;
                if attempt.generation != projection.generation
                    || attempt.fence != projection.fence
                    || request.expected.attempt_id.as_deref() != Some(attempt.id.as_str())
                    || request.expected.generation != Some(projection.generation)
                    || request.expected.fence != Some(projection.fence)
                {
                    return Err(TransitionRejection::new(
                        "stale_attempt",
                        "durable success projection is not bound to the exact current attempt",
                    ));
                }
                if !matches!(
                    attempt.disposition,
                    None | Some(AttemptDisposition::Failed) | Some(AttemptDisposition::Lost)
                ) {
                    return Err(TransitionRejection::new(
                        "attempt_already_terminal",
                        "a non-failure terminal disposition already won",
                    ));
                }
                attempt.disposition = Some(AttemptDisposition::Succeeded);
                new_state = Status::Done;
            }
            TransitionKind::GraphSaveCommitted { bundle } => {
                Self::require_actor(&request, &[ActorKind::Finalizer, ActorKind::Reconciler])?;
                let verified = crate::completion_evidence::verify_graph_save_bundle(bundle)
                    .map_err(|error| TransitionRejection::new(error.code, error.message))?;
                let attempt = projection.current_attempt.as_mut().ok_or_else(|| {
                    TransitionRejection::new(
                        "attempt_missing",
                        "GraphSave requires the exact current attempt",
                    )
                })?;
                if verified.binding.source.task_id != task.id
                    || verified.binding.source.generation != projection.generation
                    || verified.binding.source.attempt_id != attempt.id
                    || verified.binding.source.attempt_fence != projection.fence
                    || bundle.receipt.graph_revision_before_commit != projection.revision
                    || bundle.receipt.lifecycle_event_id != request.event_id
                    || verified.contract != task.completion_contract
                {
                    return Err(TransitionRejection::new(
                        "graph_save_binding_mismatch",
                        "GraphSave does not bind the exact task generation, attempt, fence, revision, event, and contract",
                    ));
                }
                if !matches!(
                    old_state,
                    Status::InProgress
                        | Status::PendingEval
                        | Status::PendingValidation
                        | Status::Failed
                ) {
                    return Err(Self::state_rejection(old_state));
                }
                if !matches!(
                    attempt.disposition,
                    None | Some(AttemptDisposition::Failed) | Some(AttemptDisposition::Lost)
                ) {
                    return Err(TransitionRejection::new(
                        "attempt_already_terminal",
                        "a different terminal disposition already won",
                    ));
                }
                attempt.disposition = Some(AttemptDisposition::Succeeded);
                completion_receipt = Some(verified.graph_save_cid);
                new_state = Status::Done;
            }
            TransitionKind::AttemptLost => {
                Self::require_actor(
                    &request,
                    &[ActorKind::ProcessObserver, ActorKind::Reconciler],
                )?;
                Self::require_running_attempt(task, &request)?;
                Self::terminalize_attempt(&mut projection, AttemptDisposition::Lost)?;
                new_state = Status::Failed;
            }
            TransitionKind::AttemptParked => {
                Self::require_actor(&request, &[ActorKind::Worker, ActorKind::Operator])?;
                Self::require_running_attempt(task, &request)?;
                Self::terminalize_attempt(&mut projection, AttemptDisposition::Parked)?;
                new_state = Status::Waiting;
            }
            TransitionKind::WaitSatisfied { .. } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::WaitMatcher,
                        ActorKind::Operator,
                        ActorKind::Reconciler,
                    ],
                )?;
                if old_state != Status::Waiting {
                    return Err(Self::state_rejection(old_state));
                }
                new_state = Status::Open;
            }
            TransitionKind::AcceptanceSatisfied { acceptance_ref } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::AcceptanceController,
                        ActorKind::Operator,
                        ActorKind::Importer,
                    ],
                )?;
                if acceptance_ref.trim().is_empty() {
                    return Err(TransitionRejection::new(
                        "acceptance_evidence_missing",
                        "Done requires a non-empty acceptance reference",
                    ));
                }
                if !matches!(old_state, Status::PendingEval | Status::PendingValidation) {
                    return Err(Self::state_rejection(old_state));
                }
                new_state = Status::Done;
            }
            TransitionKind::AcceptanceRejected { evidence_ref } => {
                Self::require_actor(
                    &request,
                    &[ActorKind::AcceptanceController, ActorKind::Operator],
                )?;
                if evidence_ref.trim().is_empty() {
                    return Err(TransitionRejection::new(
                        "acceptance_evidence_missing",
                        "rejection requires exact evidence",
                    ));
                }
                if !matches!(old_state, Status::PendingEval | Status::PendingValidation) {
                    return Err(Self::state_rejection(old_state));
                }
                // A semantic rejection rejects this immutable candidate, not
                // the already-successful source execution. Keep the source in
                // canonical AwaitingAcceptance so repair/waiver can operate on
                // retained candidate and report evidence without an implicit
                // worker retry.
                new_state = Status::PendingEval;
            }
            TransitionKind::GenerationCreated => {
                Self::require_actor(&request, &[ActorKind::Operator, ActorKind::Reconciler])?;
                if projection.reopen_intent.is_some() {
                    return Err(TransitionRejection::new(
                        "waiting_for_owner_release",
                        "a reopen intent is fenced until its exact prior owner is reaped",
                    ));
                }
                if old_state == Status::InProgress {
                    projection.fence += 1;
                    if let Some(attempt) = projection.current_attempt.as_mut()
                        && attempt.disposition.is_none()
                    {
                        attempt.disposition = Some(AttemptDisposition::Cancelled);
                    }
                }
                projection.generation += 1;
                projection.current_attempt = None;
                projection.pi_process_epoch = 0;
                projection.pi_process_identity_digest.clear();
                projection.pi_continuation_epoch = 0;
                projection.pi_continuation = None;
                projection.pi_terminal_reservation = None;
                new_state = Status::Open;
            }
            TransitionKind::ReopenRequested { intent } => {
                Self::require_actor(&request, &[ActorKind::Operator, ActorKind::Reconciler])?;
                if projection.reopen_intent.is_some() {
                    return Err(TransitionRejection::new(
                        "reopen_already_pending",
                        "another reopen intent already waits for prior-owner release",
                    ));
                }
                let current_attempt_id = projection
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str());
                let current_owner_id = projection
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.actor_id.as_str())
                    .or(task.assigned.as_deref());
                if intent.source_generation != projection.generation
                    || intent.source_attempt_id.as_deref() != current_attempt_id
                    || intent.owner_id.as_deref() != current_owner_id
                    || intent.source_fence != projection.fence
                    || intent.process_epoch != projection.pi_process_epoch
                    || intent.process_identity_digest != projection.pi_process_identity_digest
                {
                    return Err(TransitionRejection::new(
                        "stale_reopen_source",
                        "reopen intent is not bound to the exact current source owner",
                    ));
                }
                if let Some(attempt) = projection.current_attempt.as_mut()
                    && attempt.disposition.is_none()
                {
                    attempt.disposition = Some(AttemptDisposition::Cancelled);
                    projection.fence = projection.fence.saturating_add(1);
                }
                if let Some(authorization) = projection.pi_continuation.as_mut() {
                    authorization.state = PiAuthorizationState::Revoked;
                }
                projection.reopen_intent = Some(intent.clone());
            }
            TransitionKind::ReopenOwnerReleased {
                intent_id,
                exact_owner_reaped,
            } => {
                Self::require_actor(&request, &[ActorKind::Reconciler])?;
                if !*exact_owner_reaped {
                    return Err(TransitionRejection::new(
                        "owner_still_live",
                        "new generation requires exact old-owner exit/reap proof",
                    ));
                }
                let intent = projection.reopen_intent.as_ref().ok_or_else(|| {
                    TransitionRejection::new("reopen_intent_missing", "no reopen is pending")
                })?;
                if intent.id != *intent_id
                    || intent.source_generation != projection.generation
                    || intent.source_attempt_id.as_deref()
                        != projection
                            .current_attempt
                            .as_ref()
                            .map(|attempt| attempt.id.as_str())
                {
                    return Err(TransitionRejection::new(
                        "stale_reopen_source",
                        "owner release belongs to a superseded reopen intent",
                    ));
                }
                projection.generation = projection.generation.saturating_add(1);
                projection.current_attempt = None;
                projection.pi_process_epoch = 0;
                projection.pi_process_identity_digest.clear();
                projection.pi_continuation_epoch = 0;
                projection.pi_continuation = None;
                projection.pi_terminal_reservation = None;
                projection.reopen_intent = None;
                new_state = Status::Open;
            }
            TransitionKind::Abandoned => {
                Self::require_actor(&request, &[ActorKind::Operator])?;
                if old_state.is_terminal() {
                    return Err(Self::state_rejection(old_state));
                }
                if let Some(attempt) = projection.current_attempt.as_mut()
                    && attempt.disposition.is_none()
                {
                    attempt.disposition = Some(AttemptDisposition::Cancelled);
                    // Keep the source tuple/lease fence stable for exact reap.
                    // Terminal status + disposition already reject late writes.
                }
                new_state = Status::Abandoned;
            }
            TransitionKind::AdmissionDeferred { .. } => {
                Self::require_actor(&request, &[ActorKind::Dispatcher, ActorKind::Reconciler])?;
                // Evidence only: no attempt, fence, budget, or state mutation.
            }
            TransitionKind::EvaluationEvidence { .. } => {
                Self::require_actor(
                    &request,
                    &[ActorKind::EvaluationRunner, ActorKind::AcceptanceController],
                )?;
                // Evidence handoff is append-only. Only a separate acceptance
                // request may change the source generation.
            }
            TransitionKind::CandidateCheckpointed {
                candidate_id,
                manifest_cid,
                validation_result_id,
                finalization_round,
            } => {
                Self::require_actor(&request, &[ActorKind::Finalizer])?;
                Self::require_running_attempt(task, &request)?;
                if candidate_id.trim().is_empty()
                    || manifest_cid.trim().is_empty()
                    || validation_result_id.trim().is_empty()
                    || *finalization_round == 0
                {
                    return Err(TransitionRejection::new(
                        "candidate_binding_incomplete",
                        "candidate checkpoint requires candidate, manifest, validation, and round",
                    ));
                }
                // Evidence only. AttemptSucceeded remains the sole completion
                // status writer and follows in the same graph commit.
            }
            TransitionKind::ReconciliationIssue { .. } => {
                Self::require_actor(&request, &[ActorKind::Reconciler])?;
                // Breaker-neutral evidence only.
            }
            TransitionKind::LegacyCompletionQuarantined { record_ref } => {
                Self::require_actor(&request, &[ActorKind::Reconciler])?;
                if old_state != Status::Done || record_ref.trim().is_empty() {
                    return Err(Self::state_rejection(old_state));
                }
                new_state = Status::Incomplete;
            }
            TransitionKind::MessageObserved { .. } => {
                // Ordinary messages are immutable data, never lifecycle
                // authority, regardless of sender or task state.
            }
            TransitionKind::LegacyCheckpointImported => {
                Self::require_actor(&request, &[ActorKind::Importer])?;
            }
            TransitionKind::PiContinuationAuthorized {
                authorization,
                initial_process_epoch,
                initial_process_identity_digest,
            } => {
                Self::require_actor(&request, &[ActorKind::Dispatcher, ActorKind::Reconciler])?;
                Self::require_running_attempt(task, &request)?;
                let attempt = projection.current_attempt.as_ref().ok_or_else(|| {
                    TransitionRejection::new(
                        "attempt_missing",
                        "Pi authorization requires a current attempt",
                    )
                })?;
                if authorization.task_id != task.id
                    || authorization.generation != projection.generation
                    || authorization.attempt_id != attempt.id
                    || authorization.attempt_fence != projection.fence
                    || authorization.max_replacement_epochs == 0
                    || authorization.max_reserved_elapsed_secs == 0
                {
                    return Err(TransitionRejection::new(
                        "pi_authorization_mismatch",
                        "Pi continuation authorization is not bound to the current finite source tuple",
                    ));
                }
                projection.pi_process_epoch = *initial_process_epoch;
                projection.pi_process_identity_digest = initial_process_identity_digest.clone();
                projection.pi_continuation_epoch = 0;
                projection.pi_continuation = Some(authorization.clone());
                projection.pi_terminal_reservation = None;
            }
            TransitionKind::PiContinuationHeld { reason } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::ProcessObserver,
                        ActorKind::Reconciler,
                        ActorKind::Operator,
                    ],
                )?;
                Self::require_running_attempt(task, &request)?;
                if reason.trim().is_empty() {
                    return Err(TransitionRejection::new(
                        "reason_required",
                        "Pi operator hold requires a stable reason",
                    ));
                }
                let authorization = projection.pi_continuation.as_mut().ok_or_else(|| {
                    TransitionRejection::new(
                        "pi_authorization_missing",
                        "no Pi continuation authorization exists",
                    )
                })?;
                authorization.state = PiAuthorizationState::HeldOperatorRequired;
            }
            TransitionKind::PiContinuationEpochReserved {
                expected_process_epoch,
                process_identity_digest,
                expected_continuation_epoch,
                next_continuation_epoch,
                elapsed_charge_secs,
            } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::ProcessObserver,
                        ActorKind::Reconciler,
                        ActorKind::Operator,
                    ],
                )?;
                Self::require_running_attempt(task, &request)?;
                if projection.pi_terminal_reservation.is_some() {
                    return Err(TransitionRejection::new(
                        "attempt_already_terminal",
                        "terminal reservation won before continuation epoch CAS",
                    ));
                }
                if projection.pi_process_epoch != *expected_process_epoch
                    || (!projection.pi_process_identity_digest.is_empty()
                        && projection.pi_process_identity_digest != *process_identity_digest)
                {
                    return Err(TransitionRejection::new(
                        "stale_process_epoch",
                        "Pi continuation came from a non-current process authority",
                    ));
                }
                if projection.pi_continuation_epoch != *expected_continuation_epoch
                    || *next_continuation_epoch != expected_continuation_epoch.saturating_add(1)
                {
                    return Err(TransitionRejection::new(
                        "stale_continuation_epoch",
                        "Pi continuation prompt epoch CAS no longer matches",
                    ));
                }
                let authorization = projection.pi_continuation.as_mut().ok_or_else(|| {
                    TransitionRejection::new(
                        "pi_authorization_missing",
                        "no Pi continuation authorization exists",
                    )
                })?;
                if authorization.state != PiAuthorizationState::Active {
                    return Err(TransitionRejection::new(
                        "pi_authorization_held",
                        "Pi continuation is not actively authorized",
                    ));
                }
                if authorization.epochs_used >= authorization.max_replacement_epochs
                    || authorization
                        .elapsed_reserved_secs
                        .saturating_add(*elapsed_charge_secs)
                        > authorization.max_reserved_elapsed_secs
                {
                    authorization.state = PiAuthorizationState::HeldOperatorRequired;
                    return Err(TransitionRejection::new(
                        "continuation_budget_exhausted",
                        "finite Pi continuation budget is exhausted",
                    ));
                }
                authorization.epochs_used += 1;
                authorization.elapsed_reserved_secs = authorization
                    .elapsed_reserved_secs
                    .saturating_add(*elapsed_charge_secs);
                projection.pi_continuation_epoch = *next_continuation_epoch;
            }
            TransitionKind::PiProcessEpochReplaced {
                expected_process_epoch,
                expected_process_identity_digest,
                next_process_epoch,
                next_process_identity_digest,
            } => {
                Self::require_actor(
                    &request,
                    &[
                        ActorKind::Dispatcher,
                        ActorKind::ProcessObserver,
                        ActorKind::Reconciler,
                    ],
                )?;
                Self::require_running_attempt(task, &request)?;
                if projection.pi_terminal_reservation.is_some() {
                    return Err(TransitionRejection::new(
                        "attempt_already_terminal",
                        "terminal reservation won before process replacement",
                    ));
                }
                if projection.pi_process_epoch != *expected_process_epoch
                    || projection.pi_process_identity_digest != *expected_process_identity_digest
                    || *next_process_epoch != expected_process_epoch.saturating_add(1)
                    || next_process_identity_digest.is_empty()
                    || next_process_identity_digest == expected_process_identity_digest
                {
                    return Err(TransitionRejection::new(
                        "stale_process_epoch",
                        "Pi replacement process CAS no longer matches exact authority",
                    ));
                }
                projection.pi_process_epoch = *next_process_epoch;
                projection.pi_process_identity_digest = next_process_identity_digest.clone();
            }
            TransitionKind::PiTerminalIntent { receipt } => {
                Self::require_actor(&request, &[ActorKind::Worker, ActorKind::Operator])?;
                Self::require_running_attempt(task, &request)?;
                if projection.pi_process_epoch != receipt.process_epoch
                    || (!projection.pi_process_identity_digest.is_empty()
                        && projection.pi_process_identity_digest != receipt.process_identity_digest)
                {
                    return Err(TransitionRejection::new(
                        "stale_process_epoch",
                        "terminal receipt came from an old Pi process authority",
                    ));
                }
                if receipt.task_id != task.id
                    || receipt.generation != projection.generation
                    || receipt.attempt_fence != projection.fence
                    || projection.current_attempt.as_ref().map(|a| a.id.as_str())
                        != Some(receipt.attempt_id.as_str())
                {
                    return Err(TransitionRejection::new(
                        "stale_attempt",
                        "terminal receipt source tuple no longer matches",
                    ));
                }
                if projection.pi_terminal_reservation.is_some() {
                    return Err(TransitionRejection::new(
                        "attempt_already_terminal",
                        "first Pi terminal receipt already won",
                    ));
                }
                projection.pi_terminal_reservation = Some(receipt.clone());
                if let Some(authorization) = projection.pi_continuation.as_mut() {
                    authorization.state = PiAuthorizationState::Consumed;
                }
                // Every terminal tool is an intent while the exact writer may
                // still live. The finalizer consumes the disposition only
                // after quiescence and durable rescue/candidate publication.
                // No canonical task/attempt edge is legal here.
            }
            TransitionKind::PiProcessEpochExited {
                process_epoch,
                process_identity_digest,
                exact_reap_proof,
                effect_safe,
            } => {
                Self::require_actor(
                    &request,
                    &[ActorKind::ProcessObserver, ActorKind::Reconciler],
                )?;
                Self::require_running_attempt(task, &request)?;
                if projection.pi_process_epoch != *process_epoch
                    || (!projection.pi_process_identity_digest.is_empty()
                        && projection.pi_process_identity_digest != *process_identity_digest)
                {
                    return Err(TransitionRejection::new(
                        "stale_process_epoch",
                        "exit belongs to an old Pi process authority",
                    ));
                }
                if projection.pi_terminal_reservation.is_some() {
                    // A terminal tool won the first-terminal CAS. Exit supplies
                    // exact reap evidence only; the candidate finalizer owns
                    // rescue-before-disposition and must not be raced by a
                    // contradictory generic RuntimeExit classification.
                    if !*exact_reap_proof || !*effect_safe {
                        if let Some(a) = projection.pi_continuation.as_mut() {
                            a.state = PiAuthorizationState::HeldOperatorRequired;
                        }
                    }
                } else {
                    let continuation_valid = projection.pi_continuation.as_ref().is_some_and(|a| {
                        matches!(
                            a.state,
                            PiAuthorizationState::Active
                                | PiAuthorizationState::HeldOperatorRequired
                        )
                    });
                    if continuation_valid {
                        if !*exact_reap_proof || !*effect_safe {
                            if let Some(a) = projection.pi_continuation.as_mut() {
                                a.state = PiAuthorizationState::HeldOperatorRequired;
                            }
                        }
                        // No-terminal Pi exits remain watchdog-owned completion probes.
                    } else {
                        // Generic mapping applies only when no Pi terminal or
                        // continuation authority exists.
                        Self::terminalize_attempt(&mut projection, AttemptDisposition::Failed)?;
                        new_state = Status::Failed;
                    }
                }
            }
        }

        // K6/K11: no request except explicit generation creation may leave a
        // terminal generation in a different state. Evidence-only events are
        // allowed and retain the exact state.
        if old_state.is_terminal()
            && new_state != old_state
            && !matches!(
                kind,
                TransitionKind::GenerationCreated
                    | TransitionKind::ReopenOwnerReleased { .. }
                    | TransitionKind::DurableSuccessProjected { .. }
                    | TransitionKind::GraphSaveCommitted { .. }
                    | TransitionKind::LegacyCompletionQuarantined { .. }
            )
        {
            return Err(TransitionRejection::new(
                "generation_terminal",
                format!("terminal generation in {old_state} cannot transition to {new_state}"),
            ));
        }

        projection.revision += 1;
        let attempt_id = projection
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone());
        let event = LifecycleEvent {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            event_id: request.event_id,
            idempotency_key: request.idempotency_key,
            task_id: task.id.clone(),
            task_revision: projection.revision,
            generation: projection.generation,
            event_kind: kind.event_kind().to_string(),
            old_state,
            new_state,
            actor_kind: request.actor.kind,
            actor_id: request.actor.id,
            attempt_id,
            fence: projection.fence,
            reason_code: request.reason_code,
            evidence_refs: {
                let mut refs = request.evidence_refs;
                if let Some(receipt) = completion_receipt {
                    refs.push(receipt);
                }
                refs
            },
            occurred_at: request.occurred_at,
            committed_at: Utc::now().to_rfc3339(),
            projection: LifecycleEventProjection {
                status: new_state,
                generation: projection.generation,
                revision: projection.revision,
                fence: projection.fence,
                attempt_sequence: projection.attempt_sequence,
                current_attempt: projection.current_attempt,
                pi_process_epoch: projection.pi_process_epoch,
                pi_process_identity_digest: projection.pi_process_identity_digest,
                pi_continuation_epoch: projection.pi_continuation_epoch,
                pi_continuation: projection.pi_continuation,
                pi_terminal_reservation: projection.pi_terminal_reservation,
                reopen_intent: projection.reopen_intent,
            },
        };

        Ok(CommitPlan {
            event,
            duplicate: false,
        })
    }

    fn require_actor(
        request: &TransitionRequest,
        allowed: &[ActorKind],
    ) -> Result<(), TransitionRejection> {
        if allowed.contains(&request.actor.kind) {
            Ok(())
        } else {
            Err(TransitionRejection::new(
                "actor_unauthorized",
                format!(
                    "actor {:?} may not request {}",
                    request.actor.kind,
                    request.kind.event_kind()
                ),
            ))
        }
    }

    fn check_expectation(
        task: &Task,
        expected: &FenceExpectation,
    ) -> Result<(), TransitionRejection> {
        if expected
            .revision
            .is_some_and(|revision| revision != task.lifecycle.revision)
        {
            return Err(TransitionRejection::new(
                "stale_revision",
                "task lifecycle revision no longer matches",
            ));
        }
        if expected
            .generation
            .is_some_and(|generation| generation != task.lifecycle.generation)
        {
            return Err(TransitionRejection::new(
                "stale_generation",
                "task generation no longer matches",
            ));
        }
        if expected
            .fence
            .is_some_and(|fence| fence != task.lifecycle.fence)
        {
            return Err(TransitionRejection::new(
                "stale_fence",
                "attempt fence no longer matches",
            ));
        }
        if let Some(expected_attempt) = expected.attempt_id.as_deref()
            && task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != Some(expected_attempt)
        {
            return Err(TransitionRejection::new(
                "stale_attempt",
                "attempt identity no longer matches",
            ));
        }
        Ok(())
    }

    fn require_running_attempt(
        task: &Task,
        request: &TransitionRequest,
    ) -> Result<(), TransitionRejection> {
        if task.status != Status::InProgress {
            if request.actor.kind == ActorKind::Operator && !task.status.is_terminal() {
                return Ok(());
            }
            return Err(Self::state_rejection(task.status));
        }
        if request.actor.kind == ActorKind::Worker {
            if task
                .assigned
                .as_deref()
                .is_some_and(|assigned| assigned != request.actor.id)
            {
                return Err(TransitionRejection::new(
                    "stale_attempt",
                    "worker no longer owns the task assignment",
                ));
            }
        }
        if let Some(attempt) = task.lifecycle.current_attempt.as_ref() {
            if request.actor.kind == ActorKind::Worker && attempt.actor_id != request.actor.id {
                return Err(TransitionRejection::new(
                    "stale_attempt",
                    "worker no longer owns the current attempt",
                ));
            }
            if attempt.disposition.is_some() {
                return Err(TransitionRejection::new(
                    "attempt_already_terminal",
                    "the current attempt already has a terminal disposition",
                ));
            }
            // Converted worker paths must carry the current tuple. Legacy
            // pre-cutover rows have no AttemptRef and are accepted once so
            // existing graph fixtures retain behavior.
            if request.actor.kind != ActorKind::Operator
                && (request.expected.attempt_id.is_none() || request.expected.fence.is_none())
            {
                return Err(TransitionRejection::new(
                    "fence_required",
                    "worker/process terminal request must carry attempt and fence",
                ));
            }
        }
        Ok(())
    }

    fn terminalize_attempt(
        projection: &mut LifecycleProjection,
        disposition: AttemptDisposition,
    ) -> Result<(), TransitionRejection> {
        if let Some(attempt) = projection.current_attempt.as_mut() {
            if attempt.disposition.is_some() {
                return Err(TransitionRejection::new(
                    "attempt_already_terminal",
                    "first terminal disposition already won",
                ));
            }
            attempt.disposition = Some(disposition);
        }
        Ok(())
    }

    fn state_rejection(state: Status) -> TransitionRejection {
        if state.is_terminal() {
            TransitionRejection::new(
                "generation_terminal",
                format!("generation is terminal ({state})"),
            )
        } else {
            TransitionRejection::new(
                "illegal_transition",
                format!("request is not legal from {state}"),
            )
        }
    }
}

/// Apply one request to an in-memory task projection. Converted command
/// families call this inside their existing `modify_graph` transaction.
pub fn apply_transition(
    task: &mut Task,
    request: TransitionRequest,
) -> Result<LifecycleEvent, TransitionRejection> {
    let plan = LifecycleKernel::transition(task, request)?;
    let event = plan.event.clone();
    plan.apply(task)?;
    Ok(event)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerFrame {
    event: LifecycleEvent,
    checksum: String,
}

fn ledger_path(graph_path: &Path) -> PathBuf {
    graph_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lifecycle")
        .join("events.jsonl")
}

fn event_checksum(event: &LifecycleEvent) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(event)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn read_valid_frames(graph_path: &Path) -> Result<Vec<LedgerFrame>, std::io::Error> {
    let path = ledger_path(graph_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&path)?;
    let mut frames = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        // Newline is the commit marker. Even checksum-valid JSON without it
        // may be a torn append and must not become authoritative on replay.
        if !line.ends_with(b"\n") {
            break;
        }
        let payload = &line[..line.len() - 1];
        if payload.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(frame) = serde_json::from_slice::<LedgerFrame>(payload) else {
            // Since frames are append-only, nothing after the first invalid
            // frame is authoritative.
            break;
        };
        let Ok(checksum) = event_checksum(&frame.event) else {
            break;
        };
        if checksum != frame.checksum {
            break;
        }
        frames.push(frame);
    }
    Ok(frames)
}

/// Truncate an invalid/torn ledger suffix to the last newline-terminated,
/// checksum-valid frame. Called only by a writer while `graph.lock` is held.
fn repair_invalid_ledger_tail(graph_path: &Path) -> Result<(), std::io::Error> {
    let path = ledger_path(graph_path);
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(&path)?;
    let mut valid_len = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        let payload = &line[..line.len() - 1];
        if payload.iter().all(u8::is_ascii_whitespace) {
            valid_len += line.len();
            continue;
        }
        let Ok(frame) = serde_json::from_slice::<LedgerFrame>(payload) else {
            break;
        };
        let Ok(checksum) = event_checksum(&frame.event) else {
            break;
        };
        if checksum != frame.checksum {
            break;
        }
        valid_len += line.len();
    }
    if valid_len < bytes.len() {
        OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_len(valid_len as u64)?;
    }
    Ok(())
}

/// Append newly projected lifecycle events before graph replacement. Called by
/// `parser::modify_graph` while its exclusive `graph.lock` is held.
pub(crate) fn append_new_events(
    graph_path: &Path,
    before: &WorkGraph,
    after: &WorkGraph,
) -> Result<(), std::io::Error> {
    let path = ledger_path(graph_path);
    repair_invalid_ledger_tail(graph_path)?;
    let existing: HashSet<String> = read_valid_frames(graph_path)?
        .into_iter()
        .map(|frame| frame.event.event_id)
        .collect();
    let mut pending = Vec::new();
    for task in after.tasks() {
        let previous_ids: HashSet<&str> = before
            .get_task(&task.id)
            .map(|previous| {
                previous
                    .lifecycle
                    .audit
                    .iter()
                    .map(|event| event.event_id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        for event in &task.lifecycle.audit {
            if !previous_ids.contains(event.event_id.as_str())
                && !existing.contains(&event.event_id)
            {
                pending.push(event.clone());
            }
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    for event in pending {
        let frame = LedgerFrame {
            checksum: event_checksum(&event)
                .map_err(|error| std::io::Error::other(error.to_string()))?,
            event,
        };
        serde_json::to_writer(&mut file, &frame)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        file.write_all(b"\n")?;
    }
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// Replay committed events missing from the compatibility graph projection.
/// Returns true when the in-memory projection changed.
pub(crate) fn replay_ledger(
    graph_path: &Path,
    graph: &mut WorkGraph,
) -> Result<bool, std::io::Error> {
    let mut changed = false;
    for frame in read_valid_frames(graph_path)? {
        let event = frame.event;
        let Some(task) = graph.get_task_mut(&event.task_id) else {
            continue;
        };
        if event.task_revision > task.lifecycle.revision {
            event.apply_projection(task);
            changed = true;
        }
    }
    Ok(changed)
}

/// Migrate legacy `PendingValidation` rows through an explicit imported
/// acceptance event. Human-review rows retain their manual gate.
pub fn migrate_pending_validation_tasks(graph: &mut WorkGraph) -> Vec<String> {
    let to_migrate: Vec<String> = graph
        .tasks()
        .filter(|task| task.status == Status::PendingValidation)
        .filter(|task| !task.tags.iter().any(|tag| tag == "human-review"))
        .map(|task| task.id.clone())
        .collect();

    let mut migrated = Vec::with_capacity(to_migrate.len());
    for task_id in to_migrate {
        let Some(task) = graph.get_task_mut(&task_id) else {
            continue;
        };
        let request = TransitionRequest::new(
            TransitionKind::AcceptanceSatisfied {
                acceptance_ref: "legacy-validation-migrated".to_string(),
            },
            LifecycleActor {
                kind: ActorKind::Importer,
                id: "pending-validation-migrator".to_string(),
            },
            "legacy_validation_migrated",
            format!("legacy-validation-migrated:{task_id}"),
        )
        .with_evidence("legacy:validation-policy-unknown");
        if apply_transition(task, request).is_err() {
            continue;
        }
        if task.completed_at.is_none() {
            task.completed_at = Some(Utc::now().to_rfc3339());
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: Some(current_user()),
            message: "Migrated PendingValidation → Done (deprecate-pending-validation): agency `.evaluate-*` is now the dependency-unblock gate. To force re-spawn instead, run `wg reject <task>`.".to_string(),
        });
        migrated.push(task_id);
    }
    migrated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, Task};
    use crate::parser::{load_graph, modify_graph, save_graph};
    use tempfile::tempdir;

    fn task(id: &str, status: Status) -> Task {
        Task {
            id: id.to_string(),
            title: id.to_string(),
            status,
            ..Task::default()
        }
    }

    fn request(kind: TransitionKind, actor: ActorKind, key: &str) -> TransitionRequest {
        TransitionRequest {
            event_id: format!("ev-{key}"),
            idempotency_key: key.to_string(),
            actor: LifecycleActor {
                kind: actor,
                id: format!("{actor:?}"),
            },
            reason_code: key.to_string(),
            kind,
            expected: FenceExpectation::default(),
            evidence_refs: Vec::new(),
            occurred_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn apply(task: &mut Task, request: TransitionRequest) -> Result<LifecycleEvent, String> {
        apply_transition(task, request).map_err(|error| error.code)
    }

    fn reserve(task: &mut Task) {
        let generation = task.lifecycle.generation;
        let mut reserve = request(
            TransitionKind::AttemptReserved {
                owner_id: Some("Worker".to_string()),
            },
            ActorKind::Dispatcher,
            &format!("reserve-{generation}"),
        );
        reserve.idempotency_key = format!("reserve:{generation}");
        apply(task, reserve).unwrap();
    }

    #[test]
    fn lifecycle_irrelevant_message_delivery_is_state_neutral() {
        for status in [
            Status::Open,
            Status::Done,
            Status::Failed,
            Status::InProgress,
        ] {
            let mut task = task("message-target", status);
            let before = (
                task.status,
                task.lifecycle.fence,
                task.lifecycle.current_attempt.clone(),
            );
            apply(
                &mut task,
                request(
                    TransitionKind::MessageObserved {
                        message_id: "msg-1".to_string(),
                    },
                    ActorKind::Operator,
                    "message-1",
                ),
            )
            .unwrap();
            assert_eq!(
                (
                    task.status,
                    task.lifecycle.fence,
                    task.lifecycle.current_attempt.clone()
                ),
                before,
                "ordinary message changed lifecycle for {status}"
            );
        }
    }

    #[test]
    fn lifecycle_stale_exit_cannot_terminalize_new_attempt() {
        let mut task = task("stale", Status::Open);
        reserve(&mut task);
        let stale = FenceExpectation::current(&task);
        apply(
            &mut task,
            request(
                TransitionKind::GenerationCreated,
                ActorKind::Operator,
                "reset",
            ),
        )
        .unwrap();
        reserve(&mut task);
        let mut late = request(
            TransitionKind::AttemptLost,
            ActorKind::ProcessObserver,
            "late-exit",
        );
        late.expected = stale;
        assert!(matches!(
            apply(&mut task, late),
            Err(ref code) if code == "stale_revision" || code == "stale_generation"
        ));
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.lifecycle.generation, 1);
    }

    #[test]
    fn reopen_waits_for_exact_owner_release_and_late_events_are_stale() {
        let mut task = task("reopen-race", Status::Open);
        reserve(&mut task);
        let old = FenceExpectation::current(&task);
        let intent = ReopenIntent::for_task(&task, "retry", false, true, "resume-in-place retry");
        let mut hold = request(
            TransitionKind::ReopenRequested {
                intent: intent.clone(),
            },
            ActorKind::Operator,
            "reopen-hold",
        );
        hold.expected = old.clone();
        apply(&mut task, hold).unwrap();

        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.lifecycle.generation, 0);
        assert_eq!(task.lifecycle.reopen_intent.as_ref(), Some(&intent));
        assert_eq!(
            task.lifecycle
                .current_attempt
                .as_ref()
                .and_then(|attempt| attempt.disposition),
            Some(AttemptDisposition::Cancelled)
        );
        assert!(!crate::query::is_time_ready(&task));

        let held_revision = task.lifecycle.revision;
        let mut late_while_held = request(
            TransitionKind::AttemptFailed { class: None },
            ActorKind::Worker,
            "late-old-terminal-while-held",
        );
        late_while_held.expected = old.clone();
        assert!(matches!(
            apply(&mut task, late_while_held),
            Err(ref code) if code.starts_with("stale_") || code == "illegal_transition"
        ));
        assert_eq!(task.lifecycle.revision, held_revision);

        let refused = request(
            TransitionKind::ReopenOwnerReleased {
                intent_id: intent.id.clone(),
                exact_owner_reaped: false,
            },
            ActorKind::Reconciler,
            "release-without-proof",
        );
        assert!(matches!(
            apply(&mut task, refused),
            Err(ref code) if code == "owner_still_live"
        ));
        assert_eq!(task.lifecycle.revision, held_revision);

        // Simulate a daemon crash/restart at the intent/reap boundary.
        let encoded = serde_json::to_vec(&task).unwrap();
        let mut restarted: Task = serde_json::from_slice(&encoded).unwrap();
        let mut release = request(
            TransitionKind::ReopenOwnerReleased {
                intent_id: intent.id.clone(),
                exact_owner_reaped: true,
            },
            ActorKind::Reconciler,
            "release-exact-owner",
        );
        release.expected = FenceExpectation::current(&restarted);
        apply(&mut restarted, release).unwrap();
        assert_eq!(restarted.status, Status::Open);
        assert_eq!(restarted.lifecycle.generation, 1);
        assert!(restarted.lifecycle.current_attempt.is_none());
        assert!(restarted.lifecycle.reopen_intent.is_none());

        let mut late = request(
            TransitionKind::AttemptFailed { class: None },
            ActorKind::Worker,
            "late-old-terminal",
        );
        late.expected = old;
        assert!(matches!(
            apply(&mut restarted, late),
            Err(ref code) if code.starts_with("stale_") || code == "illegal_transition"
        ));
        assert_eq!(restarted.status, Status::Open);
        assert_eq!(restarted.lifecycle.generation, 1);
    }

    #[test]
    fn lifecycle_duplicate_completion_is_idempotent_and_first_terminal_wins() {
        let mut task = task("duplicate", Status::Open);
        reserve(&mut task);
        let expected = FenceExpectation::current(&task);
        let mut completion = request(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some("acceptance-1".to_string()),
                manual_review: false,
            },
            ActorKind::Worker,
            "complete",
        );
        completion.expected = expected.clone();
        let first = apply(&mut task, completion.clone()).unwrap();
        let revision = task.lifecycle.revision;
        let duplicate = LifecycleKernel::transition(&task, completion).unwrap();
        assert!(duplicate.is_duplicate());
        duplicate.apply(&mut task).unwrap();
        assert_eq!(task.lifecycle.revision, revision);
        assert_eq!(task.status, Status::Done);

        let mut contradictory = request(
            TransitionKind::AttemptFailed { class: None },
            ActorKind::Worker,
            "late-fail",
        );
        contradictory.expected = expected;
        assert!(matches!(
            apply(&mut task, contradictory),
            Err(ref code) if code == "stale_revision" || code == "generation_terminal" || code == "attempt_already_terminal"
        ));
        assert_eq!(
            task.lifecycle.ledger_head.as_deref(),
            Some(first.event_id.as_str())
        );
    }

    #[test]
    fn completion_v3_finalizer_can_commit_exact_receipt() {
        let mut task = task("completion-v3", Status::Open);
        reserve(&mut task);
        let mut completion = request(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some("b3:reviewed-publication".to_string()),
                manual_review: false,
            },
            ActorKind::Finalizer,
            "completion-v3-receipt",
        );
        completion.expected = FenceExpectation::current(&task);
        let event = apply(&mut task, completion).unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(event.actor_kind, ActorKind::Finalizer);
        assert_eq!(
            task.lifecycle
                .current_attempt
                .as_ref()
                .and_then(|attempt| attempt.disposition),
            Some(AttemptDisposition::Succeeded)
        );
    }

    #[test]
    fn lifecycle_admission_deferral_creates_no_attempt_or_failure() {
        let mut task = task("deferred", Status::Open);
        let before = task.clone();
        apply(
            &mut task,
            request(
                TransitionKind::AdmissionDeferred {
                    gate: "capacity".to_string(),
                },
                ActorKind::Dispatcher,
                "capacity-snapshot-7",
            ),
        )
        .unwrap();
        assert_eq!(task.status, before.status);
        assert_eq!(task.lifecycle.current_attempt, None);
        assert_eq!(task.retry_count, before.retry_count);
        assert_eq!(task.spawn_failures, before.spawn_failures);
    }

    #[test]
    fn lifecycle_evaluation_handoff_is_evidence_then_acceptance() {
        let mut task = task("eval", Status::Open);
        reserve(&mut task);
        let mut success = request(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: None,
                manual_review: false,
            },
            ActorKind::Worker,
            "source-success",
        );
        success.expected = FenceExpectation::current(&task);
        apply(&mut task, success).unwrap();
        assert_eq!(task.status, Status::PendingEval);

        apply(
            &mut task,
            request(
                TransitionKind::EvaluationEvidence {
                    evidence_ref: "verdict-1".to_string(),
                },
                ActorKind::EvaluationRunner,
                "verdict-write",
            ),
        )
        .unwrap();
        assert_eq!(task.status, Status::PendingEval);

        apply(
            &mut task,
            request(
                TransitionKind::AcceptanceSatisfied {
                    acceptance_ref: "verdict-1".to_string(),
                },
                ActorKind::AcceptanceController,
                "accept-verdict",
            ),
        )
        .unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(
            apply(
                &mut task,
                request(
                    TransitionKind::EvaluationEvidence {
                        evidence_ref: "late-low".to_string(),
                    },
                    ActorKind::EvaluationRunner,
                    "late-low",
                ),
            )
            .unwrap()
            .new_state,
            Status::Done
        );
    }

    #[test]
    fn lifecycle_composite_nightmare_trace_cannot_loop_or_reopen() {
        let mut task = task("nightmare", Status::Open);
        reserve(&mut task);
        let attempt_a = FenceExpectation::current(&task);

        let mut failure = request(
            TransitionKind::AttemptFailed { class: None },
            ActorKind::Worker,
            "attempt-a-failed",
        );
        failure.expected = attempt_a.clone();
        apply(&mut task, failure).unwrap();
        assert_eq!(task.status, Status::Failed);

        let mut late_done = request(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some("late-acceptance".to_string()),
                manual_review: false,
            },
            ActorKind::Worker,
            "late-done-a",
        );
        late_done.expected = attempt_a;
        assert!(apply(&mut task, late_done).is_err());

        apply(
            &mut task,
            request(
                TransitionKind::EvaluationEvidence {
                    evidence_ref: "low-verdict".to_string(),
                },
                ActorKind::EvaluationRunner,
                "low-evaluation",
            ),
        )
        .unwrap();
        apply(
            &mut task,
            request(
                TransitionKind::MessageObserved {
                    message_id: "pending-message".to_string(),
                },
                ActorKind::Operator,
                "pending-message",
            ),
        )
        .unwrap();
        assert_eq!(task.status, Status::Failed);

        // Five repeated spawn/reconcile observations collapse to one neutral
        // issue event and never charge the breaker.
        let issue = request(
            TransitionKind::ReconciliationIssue {
                issue_id: "stale-worktree-owner".to_string(),
            },
            ActorKind::Reconciler,
            "worktree-owner-conflict",
        );
        let before_revision = task.lifecycle.revision;
        apply(&mut task, issue.clone()).unwrap();
        for _ in 0..4 {
            let duplicate = LifecycleKernel::transition(&task, issue.clone()).unwrap();
            assert!(duplicate.is_duplicate());
            duplicate.apply(&mut task).unwrap();
        }
        assert_eq!(task.lifecycle.revision, before_revision + 1);
        assert_eq!(task.spawn_failures, 0);

        apply(
            &mut task,
            request(
                TransitionKind::GenerationCreated,
                ActorKind::Operator,
                "manual-retry",
            ),
        )
        .unwrap();
        reserve(&mut task);
        let mut done_b = request(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some("manual-acceptance-b".to_string()),
                manual_review: false,
            },
            ActorKind::Worker,
            "done-b",
        );
        done_b.expected = FenceExpectation::current(&task);
        apply(&mut task, done_b).unwrap();
        assert_eq!(task.status, Status::Done);

        apply(
            &mut task,
            request(
                TransitionKind::MessageObserved {
                    message_id: "pending-message".to_string(),
                },
                ActorKind::Operator,
                "same-message-after-done",
            ),
        )
        .unwrap();
        assert_eq!(task.status, Status::Done);

        apply(
            &mut task,
            request(
                TransitionKind::GenerationCreated,
                ActorKind::Operator,
                "explicit-reset",
            ),
        )
        .unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.lifecycle.generation, 2);
        assert!(task.lifecycle.current_attempt.is_none());
        assert_eq!(task.spawn_failures, 0);
    }

    #[test]
    fn lifecycle_ledger_replays_after_projection_crash() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("replay", Status::Open)));
        save_graph(&graph, &graph_path).unwrap();

        modify_graph(&graph_path, |graph| {
            let task = graph.get_task_mut("replay").unwrap();
            apply(
                task,
                request(
                    TransitionKind::AttemptReserved {
                        owner_id: Some("Worker".to_string()),
                    },
                    ActorKind::Dispatcher,
                    "replay-reserve",
                ),
            )
            .unwrap();
            true
        })
        .unwrap();

        // Simulate a crash after ledger append but before projection save by
        // replacing graph.jsonl with the pre-transition projection.
        save_graph(&graph, &graph_path).unwrap();
        let replayed = load_graph(&graph_path).unwrap();
        let task = replayed.get_task("replay").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.lifecycle.revision, 1);
    }

    #[test]
    fn lifecycle_checksum_valid_frame_without_commit_newline_is_ignored() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("uncommitted", Status::Open)));
        save_graph(&graph, &graph_path).unwrap();

        modify_graph(&graph_path, |graph| {
            reserve(graph.get_task_mut("uncommitted").unwrap());
            true
        })
        .unwrap();

        let ledger = ledger_path(&graph_path);
        let mut bytes = fs::read(&ledger).unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        fs::write(&ledger, bytes).unwrap();
        // Simulate projection loss too: the otherwise valid JSON frame has no
        // durable commit marker and therefore cannot be replayed.
        save_graph(&graph, &graph_path).unwrap();

        let loaded = load_graph(&graph_path).unwrap();
        let task = loaded.get_task("uncommitted").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.lifecycle.revision, 0);
        assert!(read_valid_frames(&graph_path).unwrap().is_empty());
    }

    #[test]
    fn lifecycle_torn_final_ledger_frame_is_truncated_before_next_commit() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("torn", Status::Open)));
        save_graph(&graph, &graph_path).unwrap();
        modify_graph(&graph_path, |graph| {
            reserve(graph.get_task_mut("torn").unwrap());
            true
        })
        .unwrap();
        let ledger = ledger_path(&graph_path);
        OpenOptions::new()
            .append(true)
            .open(&ledger)
            .unwrap()
            .write_all(b"{\"event\":")
            .unwrap();

        modify_graph(&graph_path, |graph| {
            let task = graph.get_task_mut("torn").unwrap();
            apply(
                task,
                request(
                    TransitionKind::ReconciliationIssue {
                        issue_id: "post-torn".to_string(),
                    },
                    ActorKind::Reconciler,
                    "post-torn",
                ),
            )
            .unwrap();
            true
        })
        .unwrap();
        let contents = fs::read_to_string(&ledger).unwrap();
        assert!(contents.ends_with('\n'));
        assert_eq!(contents.lines().count(), 2);
        assert_eq!(read_valid_frames(&graph_path).unwrap().len(), 2);
    }

    #[test]
    fn lifecycle_legacy_fixture_status_is_unchanged_on_load() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        fs::write(
            &graph_path,
            "{\"kind\":\"task\",\"id\":\"legacy\",\"title\":\"legacy\",\"status\":\"failed\"}\n",
        )
        .unwrap();
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("legacy").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert_eq!(task.lifecycle, LifecycleProjection::default());
    }

    #[test]
    fn migrates_pending_validation_to_done_with_audit() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task("stuck", Status::PendingValidation)));
        graph.add_node(Node::Task(task("other", Status::Open)));
        let migrated = migrate_pending_validation_tasks(&mut graph);
        assert_eq!(migrated, vec!["stuck".to_string()]);
        let stuck = graph.get_task("stuck").unwrap();
        assert_eq!(stuck.status, Status::Done);
        assert!(stuck.completed_at.is_some());
        assert_eq!(stuck.lifecycle.audit[0].actor_kind, ActorKind::Importer);
        assert_eq!(graph.get_task("other").unwrap().status, Status::Open);
    }

    #[test]
    fn migration_is_idempotent_and_skips_human_review() {
        let mut graph = WorkGraph::new();
        let mut human = task("human", Status::PendingValidation);
        human.tags.push("human-review".to_string());
        graph.add_node(Node::Task(task("stuck", Status::PendingValidation)));
        graph.add_node(Node::Task(human));
        assert_eq!(migrate_pending_validation_tasks(&mut graph).len(), 1);
        assert!(migrate_pending_validation_tasks(&mut graph).is_empty());
        assert_eq!(
            graph.get_task("human").unwrap().status,
            Status::PendingValidation
        );
    }
}
