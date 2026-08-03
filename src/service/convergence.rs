//! Durable deterministic wake scheduling for the service daemon.
//!
//! This module is deliberately a scheduler, not another graph actor. It keeps
//! restart-stable wake/backoff and route-probe leases while the lifecycle,
//! triage, wait, evaluation, finalization, and cleanup modules retain their
//! existing mutation authority. No graph task or LLM controller is created.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::ConvergenceConfig;
use crate::finalization::{FinalizationPhase, FinalizationStore, FinalizationTransaction};
use crate::graph::{FailureReason, LogEntry, Status, Task};
use crate::lifecycle::{
    ActorKind, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use crate::parser::{load_graph, modify_graph};
use crate::save_transaction::{SaveFact, SavePhase, SaveTransactionState, SaveTransitionRequest};
use crate::service::{AgentStatus, ProviderHealth};

pub const CONVERGENCE_SCHEMA_VERSION: u32 = 1;
/// Wire/conformance version for the pure exited-worker finish reducer.  This
/// is intentionally independent from the durable scheduler schema: changing
/// transition meaning must be loud to model/replay consumers.
pub const EXITED_WORKER_FINISH_REDUCER_VERSION: u32 = 1;
pub const SAVE_TRANSACTION_REPLAY_VERSION: u32 = 1;
pub const FAILED_PREREQUISITE_CONVERGENCE_VERSION: u32 = 1;
const MAX_AUTOMATIC_FAILED_PREREQUISITE_RETRIES: u8 = 1;
const STATE_FILE: &str = "convergence-state.json";
const EXITED_WORKER_CONVERGENCE_DELAY_SECS: i64 = 5;

/// Exact capability established while the wrapper still owns the native Pi
/// child.  Wrapper and child are peers in this tuple, not an inverted ancestry
/// claim: the wrapper is the child's authenticated parent/supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapperChildCapability {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub fence: u64,
    pub wrapper_epoch: u32,
    pub child_epoch: u32,
    pub wrapper_identity_digest: String,
    pub child_identity_digest: String,
    pub owned_child: bool,
}

/// Monotone transaction rank used by runtime replay and the Lean conformance
/// model.  `AwaitReceipt` remains semantic-neutral: a dead process is never
/// interpreted as successful merely because no receipt exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishConvergenceRank {
    AwaitReceipt,
    ReceiptNoTransaction,
    TransactionDurable,
    Promoted,
    Cleaned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishConvergenceAction {
    WaitForReceipt,
    ResumeSameSession,
    AdvanceTransaction,
    Promote,
    Cleanup,
    Complete,
    RejectStale,
}

impl FinishConvergenceAction {
    pub fn description(self) -> &'static str {
        match self {
            Self::WaitForReceipt => "wait for exact wrapper/child completion receipt",
            Self::ResumeSameSession => {
                "release exact dead owner once and resume the same session/worktree"
            }
            Self::AdvanceTransaction => "advance the exact durable finish transaction",
            Self::Promote => "promote the already-validated exact candidate once",
            Self::Cleanup => "clean the already-promoted task-owned worktree once",
            Self::Complete => "completion convergence is finished",
            Self::RejectStale => "reject stale or unrelated terminal writer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishConvergenceSnapshot {
    pub presented_capability: WrapperChildCapability,
    pub authoritative_capability: WrapperChildCapability,
    pub owner_proven_dead: bool,
    pub completion_receipted: bool,
    pub transaction_phase: Option<FinalizationPhase>,
    pub now_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishConvergenceDecision {
    pub reducer_version: u32,
    pub rank: FinishConvergenceRank,
    pub pending_action: FinishConvergenceAction,
    /// Required for every nonterminal, non-rejection decision.  The service
    /// may act sooner, but it may never leave this condition unscheduled.
    pub deadline_unix: Option<i64>,
}

/// Mechanics a daemon may replay from an existing SaveTransaction. Semantic
/// evidence remains task-owner/evaluator authority and therefore only waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveReplayAction {
    WaitForOwnerExit,
    Quiesce,
    CaptureWorkSave,
    AdvanceMechanics,
    AwaitSemanticEvidence,
    ReplayEffect,
    ReplayCleanup,
    ReplayGraphSave,
    Hold,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveReplayDecision {
    pub reducer_version: u32,
    pub transaction_id: String,
    pub phase: SavePhase,
    pub recovery_rank: usize,
    pub action: SaveReplayAction,
    pub deadline_unix: Option<i64>,
}

/// Production projection of the pure planner's failed-prerequisite effect.
/// `evidence_ids` are the exact opaque WIP/session/candidate identities supplied
/// to the reducer and then copied into the lifecycle audit event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedPrerequisiteConvergence {
    pub reducer_version: u32,
    pub descendant_task_id: String,
    pub prerequisite_task_id: String,
    pub class: crate::service::planner::FailedPrerequisiteClass,
    pub action: crate::service::planner::ActionKind,
    pub deadline_unix: Option<i64>,
    pub evidence_ids: Vec<String>,
}

/// Pure replay plan. The daemon never changes disposition or manufactures an
/// acceptance verdict: phases that require either return AwaitSemanticEvidence.
pub fn reduce_save_transaction_replay(
    state: &SaveTransactionState,
    owner_proven_dead: bool,
    now_unix: i64,
) -> SaveReplayDecision {
    let action = if !owner_proven_dead && state.phase < SavePhase::EffectCommitted {
        SaveReplayAction::WaitForOwnerExit
    } else {
        match state.phase {
            SavePhase::Absent => SaveReplayAction::Hold,
            SavePhase::Prepared => SaveReplayAction::Quiesce,
            SavePhase::Quiescing => SaveReplayAction::CaptureWorkSave,
            SavePhase::WorkSaved => SaveReplayAction::AdvanceMechanics,
            SavePhase::CandidateSealed | SavePhase::Validated | SavePhase::AwaitingAcceptance => {
                SaveReplayAction::AwaitSemanticEvidence
            }
            SavePhase::Accepted => SaveReplayAction::AdvanceMechanics,
            SavePhase::DispositionRecorded | SavePhase::EffectPrepared => {
                SaveReplayAction::ReplayEffect
            }
            SavePhase::EffectCommitted | SavePhase::CleanupPrepared => {
                SaveReplayAction::ReplayCleanup
            }
            SavePhase::CleanupCommitted => SaveReplayAction::ReplayGraphSave,
            SavePhase::GraphSaved | SavePhase::AbortedPreserved => SaveReplayAction::Complete,
            SavePhase::NeedsRepair | SavePhase::UpgradeBlocked | SavePhase::NeedsReconciliation => {
                SaveReplayAction::Hold
            }
        }
    };
    let deadline_unix = (!matches!(action, SaveReplayAction::Complete | SaveReplayAction::Hold))
        .then(|| now_unix.saturating_add(EXITED_WORKER_CONVERGENCE_DELAY_SECS));
    SaveReplayDecision {
        reducer_version: SAVE_TRANSACTION_REPLAY_VERSION,
        transaction_id: state.transaction_id.clone(),
        phase: state.phase,
        recovery_rank: state.recovery_rank(),
        action,
        deadline_unix,
    }
}

/// Pure, total reducer for the exited-worker handoff.  It authorizes no I/O;
/// callers execute its single action under the existing lifecycle/finalization
/// CAS fences.  Replaying identical evidence therefore yields an identical
/// decision and cannot charge a breaker or create a competing owner.
pub fn reduce_exited_worker_finish(
    snapshot: &FinishConvergenceSnapshot,
) -> FinishConvergenceDecision {
    let rank = match snapshot.transaction_phase {
        None if snapshot.completion_receipted => FinishConvergenceRank::ReceiptNoTransaction,
        None => FinishConvergenceRank::AwaitReceipt,
        Some(FinalizationPhase::Promoted)
        | Some(FinalizationPhase::Delivered)
        | Some(FinalizationPhase::Reported)
        | Some(FinalizationPhase::Cleaning) => FinishConvergenceRank::Promoted,
        Some(FinalizationPhase::Cleaned) => FinishConvergenceRank::Cleaned,
        Some(_) => FinishConvergenceRank::TransactionDurable,
    };
    if !snapshot.presented_capability.owned_child
        || snapshot.presented_capability != snapshot.authoritative_capability
    {
        return FinishConvergenceDecision {
            reducer_version: EXITED_WORKER_FINISH_REDUCER_VERSION,
            rank,
            pending_action: FinishConvergenceAction::RejectStale,
            deadline_unix: None,
        };
    }

    let action = if !snapshot.owner_proven_dead {
        FinishConvergenceAction::WaitForReceipt
    } else {
        match snapshot.transaction_phase {
            None => FinishConvergenceAction::ResumeSameSession,
            Some(FinalizationPhase::MergePending | FinalizationPhase::Merged) => {
                FinishConvergenceAction::Promote
            }
            Some(
                FinalizationPhase::Promoted
                | FinalizationPhase::Delivered
                | FinalizationPhase::Reported
                | FinalizationPhase::Cleaning,
            ) => FinishConvergenceAction::Cleanup,
            Some(FinalizationPhase::Cleaned) => FinishConvergenceAction::Complete,
            Some(_) => FinishConvergenceAction::AdvanceTransaction,
        }
    };
    FinishConvergenceDecision {
        reducer_version: EXITED_WORKER_FINISH_REDUCER_VERSION,
        rank,
        pending_action: action,
        deadline_unix: (!matches!(
            action,
            FinishConvergenceAction::Complete | FinishConvergenceAction::RejectStale
        ))
        .then(|| {
            snapshot
                .now_unix
                .saturating_add(EXITED_WORKER_CONVERGENCE_DELAY_SECS)
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvergencePolicy {
    pub base_seconds: u64,
    pub cap_seconds: u64,
    pub route_probe_base_seconds: u64,
    pub route_probe_cap_seconds: u64,
    pub action_lease_seconds: u64,
    pub jitter_divisor: u64,
}

impl Default for ConvergencePolicy {
    fn default() -> Self {
        Self::from(&ConvergenceConfig::default())
    }
}

impl From<&ConvergenceConfig> for ConvergencePolicy {
    fn from(value: &ConvergenceConfig) -> Self {
        Self {
            base_seconds: value.base_seconds.max(1),
            cap_seconds: value.cap_seconds.max(value.base_seconds.max(1)),
            route_probe_base_seconds: value.route_probe_base_seconds.max(1),
            route_probe_cap_seconds: value
                .route_probe_cap_seconds
                .max(value.route_probe_base_seconds.max(1)),
            action_lease_seconds: value.action_lease_seconds.max(1),
            jitter_divisor: value.jitter_divisor.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConvergenceStage {
    ObserveOwner,
    /// A graph dependency or not-before condition is not a failed dispatch
    /// attempt. It owns no retry budget and schedules no convergence wake;
    /// the graph change that makes the task ready resets it to AwaitDispatch.
    AwaitDependency,
    AwaitDispatch,
    AwaitWait,
    AwaitEvaluation,
    AwaitSourceRepair,
    AwaitSourceFinish,
    AwaitPromotion,
    AwaitCleanup,
    NeedsHuman,
}

impl ConvergenceStage {
    fn schedules_wake(self) -> bool {
        !matches!(self, Self::ObserveOwner | Self::AwaitDependency)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockerClass {
    Dependency,
    Dispatch,
    Wait,
    EvaluationInfrastructure,
    SourceRepair,
    SourceFinish,
    Promotion,
    Cleanup,
    NeedsHuman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRef {
    pub task_id: String,
    pub generation: u64,
    pub goal_digest: String,
    pub completion_contract: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressStamp {
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackoffState {
    pub class: BlockerClass,
    pub failures_without_progress: u32,
    pub base_seconds: u64,
    pub cap_seconds: u64,
    pub jitter_seed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionLease {
    pub action_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: Option<String>,
    pub fence: u64,
    pub revision: u64,
    pub stage: ConvergenceStage,
    pub progress_digest: String,
    pub lease_epoch: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub goal: GoalRef,
    pub priority: u32,
    pub stage: ConvergenceStage,
    pub blocker: BlockerClass,
    pub next_wake_at: String,
    /// Derived read-model projection, never a second lifecycle authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_convergence_rank: Option<FinishConvergenceRank>,
    /// Concrete bounded action shown by status/why-blocked and replay tooling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_convergence_action: Option<FinishConvergenceAction>,
    pub backoff: BackoffState,
    pub last_authoritative_progress: ProgressStamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_action: Option<ActionLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_human: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteBreakerState {
    Healthy,
    Unavailable,
    Probing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteProbeLease {
    pub action_id: String,
    pub task_id: String,
    pub epoch: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteBreaker {
    pub route_id: String,
    pub epoch: u64,
    pub state: RouteBreakerState,
    pub consecutive_outages: u32,
    pub next_probe_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_lease: Option<RouteProbeLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvergenceState {
    pub schema_version: u32,
    #[serde(default)]
    pub goals: BTreeMap<String, GoalRecord>,
    #[serde(default)]
    pub route_breakers: BTreeMap<String, RouteBreaker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reconciled_at: Option<String>,
}

impl Default for ConvergenceState {
    fn default() -> Self {
        Self {
            schema_version: CONVERGENCE_SCHEMA_VERSION,
            goals: BTreeMap::new(),
            route_breakers: BTreeMap::new(),
            last_reconciled_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Allowed { action_id: String },
    Deferred { until: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAdmission {
    Allowed,
    Probe { action_id: String },
    Deferred { until: String, reason: String },
}

impl ConvergenceState {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("service").join(STATE_FILE)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let value: Self = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if value.schema_version != CONVERGENCE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported convergence schema {} (expected {})",
                value.schema_version,
                CONVERGENCE_SCHEMA_VERSION
            );
        }
        Ok(value)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic_file::write_atomic(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn earliest_wake(&self) -> Option<DateTime<Utc>> {
        // A route probe is credential-bearing ordinary work, never a daemon
        // synthetic call. Its affected goal deadline wakes the service; the
        // route deadline is enforced when that exact task reaches admission.
        // Excluding bare route deadlines prevents an unavailable idle route
        // from spinning the daemon when no goal can probe it.
        self.goals
            .values()
            .filter(|record| record.stage.schedules_wake())
            .filter_map(|record| parse_time(&record.next_wake_at))
            .min()
    }

    fn reconcile_goals(
        &mut self,
        tasks: impl Iterator<Item = Task>,
        transactions: &BTreeMap<String, FinalizationTransaction>,
        save_transactions: Option<&BTreeMap<String, SaveTransactionState>>,
        dispatchable: Option<&BTreeSet<String>>,
        policy: &ConvergencePolicy,
        now: DateTime<Utc>,
    ) {
        let mut live = BTreeSet::new();
        for task in tasks {
            if matches!(task.status, Status::Done | Status::Abandoned) {
                continue;
            }
            let key = goal_key(&task);
            live.insert(key.clone());
            let transaction = transactions.get(&task.id);
            let save_transaction = save_transactions.and_then(|values| values.get(&task.id));
            let is_dispatchable = dispatchable.is_none_or(|ready| ready.contains(&task.id));
            let (stage, blocker, needs_human) =
                classify_stage_full(&task, transaction, save_transaction, is_dispatchable);
            let progress = authoritative_progress(&task, transaction, save_transaction);
            let goal = GoalRef {
                task_id: task.id.clone(),
                generation: task.lifecycle.generation,
                goal_digest: goal_digest(&task),
                completion_contract: task.completion_contract.to_string(),
            };
            match self.goals.get_mut(&key) {
                Some(record)
                    if record.last_authoritative_progress == progress
                        && record.blocker == blocker =>
                {
                    // Stage is a projection. ObserveOwner ↔ AwaitDispatch is
                    // intentionally the same dispatch blocker, so a respawn
                    // does not reset falloff merely by changing status.
                    record.goal = goal;
                    record.priority = task.priority;
                    record.stage = stage;
                    let (rank, action) =
                        finish_projection_full(&task, transaction, save_transaction);
                    record.finish_convergence_rank = rank;
                    record.pending_convergence_action = action;
                    record.needs_human = needs_human;
                }
                Some(record) => {
                    *record = new_goal_record(
                        &task,
                        transaction,
                        save_transaction,
                        goal,
                        stage,
                        blocker,
                        progress,
                        needs_human,
                        policy,
                        now,
                    );
                }
                None => {
                    self.goals.insert(
                        key,
                        new_goal_record(
                            &task,
                            transaction,
                            save_transaction,
                            goal,
                            stage,
                            blocker,
                            progress,
                            needs_human,
                            policy,
                            now,
                        ),
                    );
                }
            }
        }
        self.goals.retain(|key, _| live.contains(key));
    }

    fn sync_route_health(
        &mut self,
        health: &ProviderHealth,
        policy: &ConvergencePolicy,
        now: DateTime<Utc>,
    ) {
        for (route_id, status) in &health.providers {
            if status.is_paused {
                let marker = status
                    .last_failure_at
                    .clone()
                    .unwrap_or_else(|| "paused-without-timestamp".to_string());
                match self.route_breakers.get_mut(route_id) {
                    Some(breaker) if breaker.last_failure_marker.as_deref() == Some(&marker) => {}
                    Some(breaker) => {
                        breaker.epoch = breaker.epoch.saturating_add(1);
                        breaker.state = RouteBreakerState::Unavailable;
                        breaker.consecutive_outages = breaker.consecutive_outages.saturating_add(1);
                        breaker.next_probe_at =
                            route_deadline(route_id, breaker.consecutive_outages, policy, now);
                        breaker.probe_lease = None;
                        breaker.last_failure_marker = Some(marker);
                        breaker.recovered_at = None;
                    }
                    None => {
                        self.route_breakers.insert(
                            route_id.clone(),
                            RouteBreaker {
                                route_id: route_id.clone(),
                                epoch: 1,
                                state: RouteBreakerState::Unavailable,
                                consecutive_outages: 1,
                                next_probe_at: route_deadline(route_id, 1, policy, now),
                                probe_lease: None,
                                last_failure_marker: Some(marker),
                                recovered_at: None,
                            },
                        );
                    }
                }
            } else if let Some(breaker) = self.route_breakers.get_mut(route_id)
                && breaker.state != RouteBreakerState::Healthy
            {
                breaker.epoch = breaker.epoch.saturating_add(1);
                breaker.state = RouteBreakerState::Healthy;
                breaker.consecutive_outages = 0;
                breaker.next_probe_at = now.to_rfc3339();
                breaker.probe_lease = None;
                breaker.recovered_at = Some(now.to_rfc3339());
                breaker.last_failure_marker = None;
            }
        }
    }

    fn advance_one_due_without_progress(
        &mut self,
        policy: &ConvergencePolicy,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let selected = self
            .goals
            .iter()
            .filter(|(_, record)| record.stage.schedules_wake())
            .filter(|(_, record)| {
                parse_time(&record.next_wake_at).is_none_or(|deadline| deadline <= now)
            })
            .filter(|(_, record)| {
                record.pending_action.as_ref().is_none_or(|lease| {
                    parse_time(&lease.expires_at).is_none_or(|expiry| expiry <= now)
                })
            })
            .min_by(|(key_a, a), (key_b, b)| {
                parse_time(&a.next_wake_at)
                    .cmp(&parse_time(&b.next_wake_at))
                    .then_with(|| b.priority.cmp(&a.priority))
                    .then_with(|| key_a.cmp(key_b))
                    .then_with(|| a.stage.cmp(&b.stage))
            })
            .map(|(key, _)| key.clone())?;
        let record = self.goals.get_mut(&selected)?;
        let exponent = record.backoff.failures_without_progress;
        let delay = exponential_delay(
            record.backoff.base_seconds,
            record.backoff.cap_seconds,
            exponent,
        );
        let jitter = deterministic_jitter(
            &record.backoff.jitter_seed,
            exponent,
            delay,
            policy.jitter_divisor,
        );
        record.backoff.failures_without_progress = exponent.saturating_add(1);
        record.next_wake_at = (now + Duration::seconds((delay + jitter) as i64)).to_rfc3339();
        record.pending_action = None;
        Some(selected)
    }

    fn claim_goal_action(
        &mut self,
        task: &Task,
        policy: &ConvergencePolicy,
        now: DateTime<Utc>,
    ) -> Admission {
        let key = goal_key(task);
        let Some(record) = self.goals.get_mut(&key) else {
            return Admission::Allowed {
                action_id: digest(format!(
                    "untracked:{}:{}",
                    task.id, task.lifecycle.generation
                )),
            };
        };
        let due = parse_time(&record.next_wake_at).unwrap_or(now);
        if now < due {
            return Admission::Deferred {
                until: record.next_wake_at.clone(),
                reason: format!(
                    "durable {:?} falloff ({} failure(s) without authoritative progress)",
                    record.blocker, record.backoff.failures_without_progress
                ),
            };
        }
        if let Some(lease) = &record.pending_action
            && parse_time(&lease.expires_at).is_some_and(|expiry| expiry > now)
        {
            return Admission::Deferred {
                until: lease.expires_at.clone(),
                reason: format!("fenced action {} is already leased", lease.action_id),
            };
        }

        let exponent = record.backoff.failures_without_progress;
        let delay = exponential_delay(
            record.backoff.base_seconds,
            record.backoff.cap_seconds,
            exponent,
        );
        let jitter = deterministic_jitter(
            &record.backoff.jitter_seed,
            exponent,
            delay,
            policy.jitter_divisor,
        );
        record.backoff.failures_without_progress = exponent.saturating_add(1);
        record.next_wake_at = (now + Duration::seconds((delay + jitter) as i64)).to_rfc3339();
        let lease_epoch = record
            .pending_action
            .as_ref()
            .map_or(1, |lease| lease.lease_epoch.saturating_add(1));
        let material = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            task.id,
            task.lifecycle.generation,
            task.lifecycle.fence,
            task.lifecycle.revision,
            record.last_authoritative_progress.digest,
            lease_epoch,
            record.stage as u8
        );
        let action_id = digest(material);
        record.pending_action = Some(ActionLease {
            action_id: action_id.clone(),
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.clone()),
            fence: task.lifecycle.fence,
            revision: task.lifecycle.revision,
            stage: record.stage,
            progress_digest: record.last_authoritative_progress.digest.clone(),
            lease_epoch,
            expires_at: (now
                + Duration::seconds(policy.action_lease_seconds.min(delay.max(1)) as i64))
            .to_rfc3339(),
        });
        Admission::Allowed { action_id }
    }

    fn admit_route(
        &mut self,
        route_id: &str,
        task_id: &str,
        policy: &ConvergencePolicy,
        now: DateTime<Utc>,
    ) -> RouteAdmission {
        let Some(breaker) = self.route_breakers.get_mut(route_id) else {
            return RouteAdmission::Allowed;
        };
        if breaker.state == RouteBreakerState::Healthy {
            if let Some(recovered_at) = breaker.recovered_at.as_deref().and_then(parse_time) {
                let spread_ms =
                    stable_hash_u64(&format!("{}:{}:{}", route_id, breaker.epoch, task_id))
                        % policy
                            .route_probe_base_seconds
                            .saturating_mul(1_000)
                            .saturating_add(1);
                let release = recovered_at + Duration::milliseconds(spread_ms as i64);
                if now < release {
                    return RouteAdmission::Deferred {
                        until: release.to_rfc3339(),
                        reason: "deterministic post-recovery route stagger".to_string(),
                    };
                }
            }
            return RouteAdmission::Allowed;
        }

        if breaker.state == RouteBreakerState::Probing {
            if let Some(lease) = &breaker.probe_lease
                && parse_time(&lease.expires_at).is_some_and(|expiry| expiry > now)
            {
                return RouteAdmission::Deferred {
                    until: lease.expires_at.clone(),
                    reason: format!("route probe {} already leased", lease.action_id),
                };
            }
            breaker.state = RouteBreakerState::Unavailable;
            breaker.consecutive_outages = breaker.consecutive_outages.saturating_add(1);
            breaker.next_probe_at =
                route_deadline(route_id, breaker.consecutive_outages, policy, now);
            breaker.probe_lease = None;
        }

        let due = parse_time(&breaker.next_probe_at).unwrap_or(now);
        if now < due {
            return RouteAdmission::Deferred {
                until: breaker.next_probe_at.clone(),
                reason: "route breaker unavailable; same route retained".to_string(),
            };
        }
        breaker.epoch = breaker.epoch.saturating_add(1);
        breaker.state = RouteBreakerState::Probing;
        let action_id = digest(format!(
            "route-probe:{}:{}:{}",
            route_id, breaker.epoch, task_id
        ));
        let expires_at = (now + Duration::seconds(policy.action_lease_seconds as i64)).to_rfc3339();
        breaker.probe_lease = Some(RouteProbeLease {
            action_id: action_id.clone(),
            task_id: task_id.to_string(),
            epoch: breaker.epoch,
            expires_at: expires_at.clone(),
        });
        breaker.next_probe_at = expires_at;
        RouteAdmission::Probe { action_id }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DeadOwnerQuiescenceFact<'a> {
    schema_version: u32,
    transaction_id: &'a str,
    task_id: &'a str,
    generation: u64,
    attempt_id: &'a str,
    attempt_fence: u64,
    agent_id: &'a str,
    registry_completed_at: &'a str,
}

/// Replay the first authorized mechanical edge for terminal intents whose
/// exact owner has been marked dead by triage. Later phases are deliberately
/// returned as plans for their owning adapters; this function never invents a
/// validation/FLIP verdict, disposition, effect target, or cleanup receipt.
pub fn replay_dead_owner_save_transactions(dir: &Path) -> Result<Vec<SaveReplayDecision>> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let registry = crate::service::AgentRegistry::load(dir)?;
    let mut decisions = Vec::new();
    for state in crate::worker_control::list_save_transactions(dir)? {
        let Some(task) = graph.get_task(&state.source.task_id) else {
            continue;
        };
        if !crate::worker_control::save_transaction_matches_task(&state, task) {
            continue;
        }
        let Some(agent_id) = task.assigned.as_deref() else {
            continue;
        };
        let owner_dead = registry
            .get_agent(agent_id)
            .is_some_and(|agent| agent.status == AgentStatus::Dead);
        let decision = reduce_save_transaction_replay(&state, owner_dead, Utc::now().timestamp());
        if decision.action != SaveReplayAction::Quiesce {
            decisions.push(decision);
            continue;
        }
        let agent = registry
            .get_agent(agent_id)
            .expect("owner_dead requires a registry entry");
        let completed_at = agent.completed_at.as_deref().unwrap_or("dead-observed");
        let fact = DeadOwnerQuiescenceFact {
            schema_version: 2,
            transaction_id: &state.transaction_id,
            task_id: &state.source.task_id,
            generation: state.source.generation,
            attempt_id: &state.source.attempt_id,
            attempt_fence: state.source.attempt_fence,
            agent_id,
            registry_completed_at: completed_at,
        };
        let fact_cid = crate::worker_control::store_completion_object(dir, &fact)?;
        let next = crate::worker_control::commit_save_transition(
            dir,
            SaveTransitionRequest {
                source: state.source.clone(),
                expected_revision: state.revision,
                expected_phase: state.phase,
                next_phase: SavePhase::Quiescing,
                idempotency_key: format!("quiesce:{}:{completed_at}", state.transaction_id),
                action_key: format!("quiesce:{}", state.transaction_id),
                fact: SaveFact::Evidence {
                    cid: fact_cid,
                    binding: None,
                },
            },
        )?;
        decisions.push(reduce_save_transaction_replay(
            &next,
            true,
            Utc::now().timestamp(),
        ));
    }
    Ok(decisions)
}

fn exact_save_transactions_by_task(
    dir: &Path,
    graph: &crate::graph::WorkGraph,
) -> BTreeMap<String, SaveTransactionState> {
    let mut values = BTreeMap::new();
    if let Ok(states) = crate::worker_control::list_save_transactions(dir) {
        for state in states {
            let Some(task) = graph.get_task(&state.source.task_id) else {
                continue;
            };
            if !crate::worker_control::save_transaction_matches_task(&state, task) {
                continue;
            }
            let replace = values
                .get(&task.id)
                .is_none_or(|current: &SaveTransactionState| current.revision < state.revision);
            if replace {
                values.insert(task.id.clone(), state);
            }
        }
    }
    values
}

fn planner_id(value: impl AsRef<[u8]>) -> crate::service::planner::OpaqueId {
    let bytes = value.as_ref();
    let candidate = String::from_utf8_lossy(bytes).to_string();
    crate::service::planner::OpaqueId::new(candidate).unwrap_or_else(|_| {
        crate::service::planner::OpaqueId::new(digest(bytes)).expect("digest is a planner id")
    })
}

fn evidence_slot(value: Option<String>) -> crate::service::planner::EvidenceSlot {
    value.map_or(crate::service::planner::EvidenceSlot::Absent, |value| {
        crate::service::planner::EvidenceSlot::Present {
            evidence_id: planner_id(value),
        }
    })
}

fn semantic_rejection(task: &Task, transaction: Option<&FinalizationTransaction>) -> bool {
    task.lifecycle.audit.iter().any(|event| {
        event.generation == task.lifecycle.generation && event.event_kind == "acceptance-rejected"
    }) || transaction.is_some_and(|tx| {
        tx.validation
            .as_ref()
            .is_some_and(|validation| !validation.passed)
            || tx
                .retained_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("acceptance.rejected:"))
    })
}

fn automatic_prerequisite_retries(task: &Task) -> u8 {
    task.lifecycle
        .audit
        .iter()
        .filter(|event| {
            matches!(
                event.reason_code.as_str(),
                "nonsemantic_failed_prerequisite_retry"
                    | "nonsemantic_failed_prerequisite_finish_retry"
            )
        })
        .count()
        .min(u8::MAX as usize) as u8
}

/// Convert exact graph/finalization evidence to the pure v2 planner and apply
/// at most one idempotent source retry or reconciliation issue per descendant.
/// A semantic rejection is only recorded as an explicit repair wait. Durable
/// candidate/session/worktree identities are copied into lifecycle evidence;
/// no source bytes or arbitrary provider text enter the planner wire.
pub fn converge_failed_prerequisites(
    dir: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<FailedPrerequisiteConvergence>> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let graph_id = planner_id(crate::worker_control::load_or_create_graph_identity(dir)?);
    let transactions = FinalizationStore::open(dir)
        .and_then(|store| store.list())
        .unwrap_or_default()
        .into_iter()
        .map(|transaction| (transaction.task_id.clone(), transaction))
        .collect::<BTreeMap<_, _>>();
    let save_transactions = exact_save_transactions_by_task(dir, &graph);
    let mut observations = Vec::new();

    let mut descendants = graph
        .tasks()
        .filter(|task| {
            matches!(
                task.status,
                Status::Open | Status::Blocked | Status::Incomplete
            )
        })
        .collect::<Vec<_>>();
    descendants.sort_by(|a, b| a.id.cmp(&b.id));
    for descendant in descendants {
        let mut failed = descendant
            .after
            .iter()
            .filter_map(|id| graph.get_task(id))
            .filter(|task| task.status == Status::Failed)
            .collect::<Vec<_>>();
        failed.sort_by(|a, b| a.id.cmp(&b.id));
        let Some(source) = failed.first().copied() else {
            continue;
        };
        let transaction = transactions.get(&source.id);
        let save = save_transactions.get(&source.id);
        let semantic = semantic_rejection(source, transaction);
        let has_progress = source
            .token_usage
            .as_ref()
            .is_some_and(|usage| usage.total_tokens() > 0);
        let has_durable_candidate = transaction.and_then(|tx| tx.candidate.as_ref()).is_some();
        let provider_failure = source.failure_signal.as_ref().is_some_and(|signal| {
            matches!(
                signal.reason,
                FailureReason::ProviderUnavailable
                    | FailureReason::ProviderOverloaded
                    | FailureReason::Transient5xx
                    | FailureReason::RateLimit
            )
        });
        let class = if semantic {
            crate::service::planner::FailedPrerequisiteClass::SemanticValidationRejected
        } else if provider_failure && has_durable_candidate {
            crate::service::planner::FailedPrerequisiteClass::ProviderUnavailableAfterDurableCandidate
        } else if source.lifecycle.current_attempt.is_none() {
            crate::service::planner::FailedPrerequisiteClass::OrphanBeforeSpawn
        } else if has_progress {
            crate::service::planner::FailedPrerequisiteClass::SourceExecutionWithProgress
        } else {
            crate::service::planner::FailedPrerequisiteClass::SourceExecutionNoProgress
        };
        let source_key = crate::service::planner::TaskKey {
            graph_id: graph_id.clone(),
            task_id: planner_id(&source.id),
            generation: source.lifecycle.generation,
            attempt_id: planner_id(
                source
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str())
                    .unwrap_or("attempt-absent"),
            ),
            fence: source.lifecycle.fence,
        };
        let evidence = crate::service::planner::FailedPrerequisiteEvidence {
            work_save: evidence_slot(save.and_then(|state| {
                state
                    .evidence_cids
                    .get(&SavePhase::WorkSaved)
                    .cloned()
                    .or_else(|| state.evidence_cids.values().next().cloned())
            })),
            candidate: evidence_slot(
                transaction
                    .and_then(|tx| tx.candidate.as_ref())
                    .map(|candidate| candidate.candidate_id.clone()),
            ),
            session: evidence_slot(source.session_id.clone()),
            worktree: evidence_slot(
                transaction
                    .and_then(|tx| tx.candidate.as_ref())
                    .map(|candidate| candidate.worktree_id.clone())
                    .or_else(|| source.assigned.clone()),
            ),
        };
        let automatic_retries = automatic_prerequisite_retries(source);
        let failed_prerequisite = crate::service::planner::FailedPrerequisite {
            source: source_key,
            class,
            evidence,
            automatic_retries,
            max_automatic_retries: MAX_AUTOMATIC_FAILED_PREREQUISITE_RETRIES,
        };
        let progress = digest(
            serde_json::to_vec(&failed_prerequisite).expect("typed prerequisite serializes"),
        );
        let observation = crate::service::planner::ObservationEnvelope {
            sequence: 1,
            logical_time: now.timestamp().max(0) as u64,
            observation: crate::service::planner::Observation::Task(Box::new(
                crate::service::planner::TaskObservation {
                    key: crate::service::planner::TaskKey {
                        graph_id: graph_id.clone(),
                        task_id: planner_id(&descendant.id),
                        generation: descendant.lifecycle.generation,
                        attempt_id: planner_id("blocked-before-attempt"),
                        fence: descendant.lifecycle.fence,
                    },
                    progress_id: planner_id(progress),
                    unfinished: true,
                    owner: crate::service::planner::OwnerEvidence::None,
                    runnable: None,
                    external_wait: None,
                    scheduled: None,
                    incidents: BTreeSet::new(),
                    failed_prerequisite: Some(failed_prerequisite),
                },
            )),
        };
        let state = crate::service::planner::PlannerState::new(graph_id.clone());
        let step = crate::service::planner::plan(
            &state,
            &observation,
            crate::service::planner::PlannerRuleset::Corrected,
        );
        let task_projection = step.state.tasks.values().next().expect("task projected");
        let action = step.effects.first().map_or(
            crate::service::planner::ActionKind::RecordNeedsReconciliation,
            |effect| effect.action,
        );
        let evidence_ids = task_projection
            .failed_prerequisite
            .as_ref()
            .into_iter()
            .flat_map(|failed| {
                [
                    &failed.evidence.work_save,
                    &failed.evidence.candidate,
                    &failed.evidence.session,
                    &failed.evidence.worktree,
                ]
            })
            .filter_map(|slot| match slot {
                crate::service::planner::EvidenceSlot::Present { evidence_id } => {
                    Some(evidence_id.to_string())
                }
                crate::service::planner::EvidenceSlot::Absent => None,
            })
            .collect::<Vec<_>>();
        observations.push(FailedPrerequisiteConvergence {
            reducer_version: FAILED_PREREQUISITE_CONVERGENCE_VERSION,
            descendant_task_id: descendant.id.clone(),
            prerequisite_task_id: source.id.clone(),
            class,
            action: if class
                == crate::service::planner::FailedPrerequisiteClass::SemanticValidationRejected
            {
                // The production record names the operator action while the
                // pure task projection remains an external semantic wait.
                crate::service::planner::ActionKind::RecordNeedsReconciliation
            } else {
                action
            },
            deadline_unix: (!semantic).then_some(now.timestamp()),
            evidence_ids,
        });
    }
    drop(graph);

    for decision in &observations {
        let evidence = decision.evidence_ids.clone();
        let prerequisite = decision.prerequisite_task_id.clone();
        let descendant = decision.descendant_task_id.clone();
        match decision.action {
            crate::service::planner::ActionKind::RetryFailedPrerequisite => {
                modify_graph(&graph_path, |graph| {
                    let Some(task) = graph.get_task_mut(&prerequisite) else {
                        return false;
                    };
                    if task.status != Status::Failed
                        || automatic_prerequisite_retries(task)
                            >= MAX_AUTOMATIC_FAILED_PREREQUISITE_RETRIES
                    {
                        return false;
                    }
                    let mut request = TransitionRequest::new(
                        TransitionKind::GenerationCreated,
                        LifecycleActor {
                            kind: ActorKind::Reconciler,
                            id: "failed-prerequisite-convergence".into(),
                        },
                        "nonsemantic_failed_prerequisite_retry",
                        format!(
                            "failed-prerequisite-retry:{}:{}",
                            task.id, task.lifecycle.generation
                        ),
                    );
                    for item in &evidence {
                        request.evidence_refs.push(item.clone());
                    }
                    if apply_transition(task, request).is_err() {
                        return false;
                    }
                    task.assigned = None;
                    task.started_at = None;
                    task.failure_reason = None;
                    task.failure_class = None;
                    task.failure_signal = None;
                    task.log.push(LogEntry {
                        timestamp: now.to_rfc3339(),
                        actor: Some("failed-prerequisite-convergence".into()),
                        user: Some(crate::current_user()),
                        message: format!(
                            "Scheduled one bounded non-semantic retry because unfinished descendant '{}' depends on this failed source; exact WIP/session evidence retained",
                            descendant
                        ),
                    });
                    true
                })?;
            }
            crate::service::planner::ActionKind::RecordNeedsReconciliation => {
                modify_graph(&graph_path, |graph| {
                    let Some(task) = graph.get_task_mut(&descendant) else {
                        return false;
                    };
                    let semantic = decision.class
                        == crate::service::planner::FailedPrerequisiteClass::SemanticValidationRejected;
                    let issue_id = format!(
                        "failed-prerequisite:{}:{}:{}",
                        prerequisite,
                        task.lifecycle.generation,
                        if semantic { "semantic" } else { "reconcile" }
                    );
                    let mut request = TransitionRequest::new(
                        TransitionKind::ReconciliationIssue {
                            issue_id: issue_id.clone(),
                        },
                        LifecycleActor {
                            kind: ActorKind::Reconciler,
                            id: "failed-prerequisite-convergence".into(),
                        },
                        if semantic {
                            "semantic_failed_prerequisite_terminal"
                        } else {
                            "failed_prerequisite_needs_reconciliation"
                        },
                        issue_id,
                    );
                    request.evidence_refs.push(prerequisite.clone());
                    request.evidence_refs.extend(evidence.clone());
                    apply_transition(task, request).is_ok()
                })?;
                if decision.class
                    != crate::service::planner::FailedPrerequisiteClass::SemanticValidationRejected
                {
                    modify_graph(&graph_path, |graph| {
                        let Some(task) = graph.get_task_mut(&prerequisite) else {
                            return false;
                        };
                        let issue_id = format!(
                            "failed-prerequisite-source-hold:{}:{}",
                            task.id, task.lifecycle.generation
                        );
                        let mut request = TransitionRequest::new(
                            TransitionKind::ReconciliationIssue {
                                issue_id: issue_id.clone(),
                            },
                            LifecycleActor {
                                kind: ActorKind::Reconciler,
                                id: "failed-prerequisite-convergence".into(),
                            },
                            "failed_prerequisite_needs_reconciliation",
                            issue_id,
                        );
                        request.evidence_refs.extend(evidence.clone());
                        apply_transition(task, request).is_ok()
                    })?;
                }
            }
            // Authorize exactly one existing transaction replay and record
            // that finite budget before the coordinator invokes the finish
            // adapter. The exact WIP/session/candidate IDs stay in the audit.
            crate::service::planner::ActionKind::ReplanFinish => {
                modify_graph(&graph_path, |graph| {
                    let Some(task) = graph.get_task_mut(&prerequisite) else {
                        return false;
                    };
                    if automatic_prerequisite_retries(task)
                        >= MAX_AUTOMATIC_FAILED_PREREQUISITE_RETRIES
                    {
                        return false;
                    }
                    let issue_id = format!(
                        "failed-prerequisite-finish-retry:{}:{}",
                        task.id, task.lifecycle.generation
                    );
                    let mut request = TransitionRequest::new(
                        TransitionKind::ReconciliationIssue {
                            issue_id: issue_id.clone(),
                        },
                        LifecycleActor {
                            kind: ActorKind::Reconciler,
                            id: "failed-prerequisite-convergence".into(),
                        },
                        "nonsemantic_failed_prerequisite_finish_retry",
                        issue_id,
                    );
                    request.evidence_refs.extend(evidence.clone());
                    apply_transition(task, request).is_ok()
                })?;
            }
            _ => {}
        }
    }
    Ok(observations)
}

/// Refresh the durable read model from authoritative graph/finalization/route
/// evidence. This performs no graph mutation.
pub fn reconcile_dir(
    dir: &Path,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> Result<ConvergenceState> {
    let mut state = ConvergenceState::load(dir)?;
    let graph = load_graph(dir.join("graph.jsonl"))?;
    // Readiness is authoritative dependency state. Merely existing in Open is
    // not a dispatch attempt: blocked work must be falloff/breaker-neutral.
    let dispatchable = crate::query::ready_tasks(&graph)
        .into_iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let transactions = FinalizationStore::open(dir)
        .and_then(|store| store.list())
        .unwrap_or_default()
        .into_iter()
        .map(|transaction| (transaction.task_id.clone(), transaction))
        .collect::<BTreeMap<_, _>>();
    let save_transactions = exact_save_transactions_by_task(dir, &graph);
    state.reconcile_goals(
        graph.tasks().cloned(),
        &transactions,
        Some(&save_transactions),
        Some(&dispatchable),
        policy,
        now,
    );
    if let Ok(health) = ProviderHealth::load(dir) {
        state.sync_route_health(&health, policy, now);
    }
    state.last_reconciled_at = Some(now.to_rfc3339());
    state.save(dir)?;
    Ok(state)
}

/// Observe one completed service pass and advance at most one due unchanged
/// record. Domain owners may have produced authoritative evidence during the
/// pass; `reconcile_dir` resets those records before this selection.
pub fn reconcile_after_service_pass(
    dir: &Path,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    let mut state = reconcile_dir(dir, policy, now)?;
    let selected = state.advance_one_due_without_progress(policy, now);
    state.save(dir)?;
    Ok(selected)
}

/// Acquire one fenced action wake for an existing goal. Repeated attempts with
/// unchanged authoritative progress fall off exponentially and never turn the
/// task into generic Failed.
pub fn admit_goal_action(
    dir: &Path,
    task: &Task,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> Result<Admission> {
    let mut state = ConvergenceState::load(dir)?;
    if !state.goals.contains_key(&goal_key(task)) {
        let transactions = BTreeMap::new();
        state.reconcile_goals(
            std::iter::once(task.clone()),
            &transactions,
            None,
            None,
            policy,
            now,
        );
    }
    let admission = state.claim_goal_action(task, policy, now);
    state.save(dir)?;
    Ok(admission)
}

/// Route-key admission with exactly one credential-bearing probe lease. The
/// route is never rewritten or cross-fallen-back.
pub fn admit_route_action(
    dir: &Path,
    route_id: &str,
    task_id: &str,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> Result<RouteAdmission> {
    let mut state = ConvergenceState::load(dir)?;
    if let Ok(health) = ProviderHealth::load(dir) {
        state.sync_route_health(&health, policy, now);
    }
    let admission = state.admit_route(route_id, task_id, policy, now);
    state.save(dir)?;
    Ok(admission)
}

pub fn earliest_wake(dir: &Path) -> Result<Option<DateTime<Utc>>> {
    Ok(ConvergenceState::load(dir)?.earliest_wake())
}

fn new_goal_record(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
    save_transaction: Option<&SaveTransactionState>,
    goal: GoalRef,
    stage: ConvergenceStage,
    blocker: BlockerClass,
    progress: ProgressStamp,
    needs_human: Option<String>,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> GoalRecord {
    let (finish_convergence_rank, pending_convergence_action) =
        finish_projection_full(task, transaction, save_transaction);
    GoalRecord {
        goal,
        priority: task.priority,
        stage,
        blocker,
        next_wake_at: now.to_rfc3339(),
        finish_convergence_rank,
        pending_convergence_action,
        backoff: BackoffState {
            class: blocker,
            failures_without_progress: 0,
            base_seconds: policy.base_seconds,
            cap_seconds: policy.cap_seconds,
            jitter_seed: digest(format!("{}:{}", task.id, task.lifecycle.generation)),
        },
        last_authoritative_progress: progress,
        pending_action: None,
        needs_human,
    }
}

fn classify_stage(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
) -> (ConvergenceStage, BlockerClass, Option<String>) {
    classify_stage_with_readiness(task, transaction, true)
}

fn classify_stage_with_readiness(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
    dispatchable: bool,
) -> (ConvergenceStage, BlockerClass, Option<String>) {
    classify_stage_full(task, transaction, None, dispatchable)
}

fn classify_stage_full(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
    save_transaction: Option<&SaveTransactionState>,
    dispatchable: bool,
) -> (ConvergenceStage, BlockerClass, Option<String>) {
    if let Some(issue) = task.lifecycle.audit.iter().rev().find(|event| {
        event.generation == task.lifecycle.generation
            && matches!(
                event.reason_code.as_str(),
                "failed_prerequisite_needs_reconciliation"
                    | "semantic_failed_prerequisite_terminal"
            )
    }) {
        let prerequisite = issue
            .evidence_refs
            .first()
            .map(String::as_str)
            .unwrap_or("unknown");
        return (
            ConvergenceStage::NeedsHuman,
            BlockerClass::NeedsHuman,
            Some(
                if issue.reason_code == "semantic_failed_prerequisite_terminal" {
                    format!(
                        "semantic rejection of prerequisite '{prerequisite}' is terminal; repair/waive it or create an explicit new generation"
                    )
                } else {
                    format!(
                        "prerequisite '{prerequisite}' exhausted bounded automatic convergence; inspect retained evidence and run `wg retry {prerequisite}` or abandon/reconstruct it"
                    )
                },
            ),
        );
    }
    if let Some(save) = save_transaction {
        let (stage, blocker, needs_human) = match save.phase {
            SavePhase::Absent
            | SavePhase::Prepared
            | SavePhase::Quiescing
            | SavePhase::WorkSaved
            | SavePhase::Accepted => (
                ConvergenceStage::AwaitSourceFinish,
                BlockerClass::SourceFinish,
                None,
            ),
            SavePhase::CandidateSealed | SavePhase::Validated | SavePhase::AwaitingAcceptance => (
                ConvergenceStage::AwaitEvaluation,
                BlockerClass::EvaluationInfrastructure,
                None,
            ),
            SavePhase::DispositionRecorded | SavePhase::EffectPrepared => (
                ConvergenceStage::AwaitPromotion,
                BlockerClass::Promotion,
                None,
            ),
            SavePhase::EffectCommitted
            | SavePhase::CleanupPrepared
            | SavePhase::CleanupCommitted
            | SavePhase::GraphSaved => {
                (ConvergenceStage::AwaitCleanup, BlockerClass::Cleanup, None)
            }
            SavePhase::NeedsRepair => (
                ConvergenceStage::AwaitSourceRepair,
                BlockerClass::SourceRepair,
                save.hold_reason.clone(),
            ),
            SavePhase::AbortedPreserved
            | SavePhase::UpgradeBlocked
            | SavePhase::NeedsReconciliation => (
                ConvergenceStage::NeedsHuman,
                BlockerClass::NeedsHuman,
                Some(
                    save.hold_reason
                        .clone()
                        .unwrap_or_else(|| format!("SaveTransaction held at {:?}", save.phase)),
                ),
            ),
        };
        return (stage, blocker, needs_human);
    }
    if let Some(transaction) = transaction {
        let stage = match transaction.phase {
            FinalizationPhase::RepairNeeded | FinalizationPhase::FailedPreserved => {
                ConvergenceStage::AwaitSourceRepair
            }
            FinalizationPhase::WaitingFinishLease
            | FinalizationPhase::Integrating
            | FinalizationPhase::RescueCheckpointed
            | FinalizationPhase::NeedsFinalization => ConvergenceStage::AwaitSourceFinish,
            FinalizationPhase::CandidateCheckpointed
            | FinalizationPhase::Validating
            | FinalizationPhase::Evaluating
            | FinalizationPhase::WaitingEvaluation => ConvergenceStage::AwaitEvaluation,
            FinalizationPhase::MergePending | FinalizationPhase::Merged => {
                ConvergenceStage::AwaitPromotion
            }
            FinalizationPhase::Promoted
            | FinalizationPhase::Delivered
            | FinalizationPhase::Reported
            | FinalizationPhase::Cleaning
            | FinalizationPhase::Cleaned => ConvergenceStage::AwaitCleanup,
            FinalizationPhase::OperatorHold => ConvergenceStage::NeedsHuman,
        };
        let blocker = match stage {
            ConvergenceStage::AwaitSourceRepair => BlockerClass::SourceRepair,
            ConvergenceStage::AwaitSourceFinish => BlockerClass::SourceFinish,
            ConvergenceStage::AwaitEvaluation => BlockerClass::EvaluationInfrastructure,
            ConvergenceStage::AwaitPromotion => BlockerClass::Promotion,
            ConvergenceStage::AwaitCleanup => BlockerClass::Cleanup,
            ConvergenceStage::NeedsHuman => BlockerClass::NeedsHuman,
            _ => BlockerClass::Dispatch,
        };
        return (
            stage,
            blocker,
            (stage == ConvergenceStage::NeedsHuman).then(|| {
                transaction
                    .retained_reason
                    .clone()
                    .unwrap_or_else(|| "operator hold".into())
            }),
        );
    }
    match task.status {
        Status::Open | Status::Blocked | Status::Incomplete if !dispatchable => (
            ConvergenceStage::AwaitDependency,
            BlockerClass::Dependency,
            None,
        ),
        Status::Open | Status::Blocked | Status::Incomplete => (
            ConvergenceStage::AwaitDispatch,
            BlockerClass::Dispatch,
            None,
        ),
        Status::InProgress
            if task.lifecycle.pi_terminal_reservation.is_some()
                || task.lifecycle.audit.iter().any(|event| {
                    event.generation == task.lifecycle.generation
                        && event.attempt_id.as_deref()
                            == task
                                .lifecycle
                                .current_attempt
                                .as_ref()
                                .map(|attempt| attempt.id.as_str())
                        && event.event_kind == "pi-process-epoch-exited"
                }) =>
        {
            (
                ConvergenceStage::AwaitSourceFinish,
                BlockerClass::SourceFinish,
                None,
            )
        }
        Status::InProgress => (ConvergenceStage::ObserveOwner, BlockerClass::Dispatch, None),
        Status::Waiting => (ConvergenceStage::AwaitWait, BlockerClass::Wait, None),
        Status::PendingEval | Status::FailedPendingEval | Status::PendingValidation => (
            ConvergenceStage::AwaitEvaluation,
            BlockerClass::EvaluationInfrastructure,
            None,
        ),
        Status::Failed => (
            ConvergenceStage::NeedsHuman,
            BlockerClass::NeedsHuman,
            task.failure_reason
                .clone()
                .or_else(|| Some("failed task requires explicit policy".into())),
        ),
        Status::Done | Status::Abandoned => unreachable!("terminal tasks are filtered"),
    }
}

fn finish_projection(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
) -> (
    Option<FinishConvergenceRank>,
    Option<FinishConvergenceAction>,
) {
    finish_projection_full(task, transaction, None)
}

fn finish_projection_full(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
    save_transaction: Option<&SaveTransactionState>,
) -> (
    Option<FinishConvergenceRank>,
    Option<FinishConvergenceAction>,
) {
    if let Some(save) = save_transaction {
        let projection = match save.phase {
            SavePhase::DispositionRecorded | SavePhase::EffectPrepared => (
                FinishConvergenceRank::TransactionDurable,
                FinishConvergenceAction::Promote,
            ),
            SavePhase::EffectCommitted
            | SavePhase::CleanupPrepared
            | SavePhase::CleanupCommitted => (
                FinishConvergenceRank::Promoted,
                FinishConvergenceAction::Cleanup,
            ),
            SavePhase::GraphSaved | SavePhase::AbortedPreserved => (
                FinishConvergenceRank::Cleaned,
                FinishConvergenceAction::Complete,
            ),
            SavePhase::NeedsRepair | SavePhase::UpgradeBlocked | SavePhase::NeedsReconciliation => {
                return (Some(FinishConvergenceRank::TransactionDurable), None);
            }
            _ => (
                FinishConvergenceRank::TransactionDurable,
                FinishConvergenceAction::AdvanceTransaction,
            ),
        };
        return (Some(projection.0), Some(projection.1));
    }
    if let Some(transaction) = transaction {
        let (rank, action) = match transaction.phase {
            FinalizationPhase::MergePending | FinalizationPhase::Merged => (
                FinishConvergenceRank::TransactionDurable,
                FinishConvergenceAction::Promote,
            ),
            FinalizationPhase::Promoted
            | FinalizationPhase::Delivered
            | FinalizationPhase::Reported
            | FinalizationPhase::Cleaning => (
                FinishConvergenceRank::Promoted,
                FinishConvergenceAction::Cleanup,
            ),
            FinalizationPhase::Cleaned => (
                FinishConvergenceRank::Cleaned,
                FinishConvergenceAction::Complete,
            ),
            _ => (
                FinishConvergenceRank::TransactionDurable,
                FinishConvergenceAction::AdvanceTransaction,
            ),
        };
        return (Some(rank), Some(action));
    }
    let exact_epoch_exited = task.lifecycle.audit.iter().any(|event| {
        event.generation == task.lifecycle.generation
            && event.attempt_id.as_deref()
                == task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str())
            && event.event_kind == "pi-process-epoch-exited"
    });
    if exact_epoch_exited || task.lifecycle.pi_terminal_reservation.is_some() {
        return (
            Some(if task.lifecycle.pi_terminal_reservation.is_some() {
                FinishConvergenceRank::ReceiptNoTransaction
            } else {
                FinishConvergenceRank::AwaitReceipt
            }),
            Some(FinishConvergenceAction::ResumeSameSession),
        );
    }
    (None, None)
}

fn authoritative_progress(
    task: &Task,
    transaction: Option<&FinalizationTransaction>,
    save_transaction: Option<&SaveTransactionState>,
) -> ProgressStamp {
    let meaningful_events = task
        .lifecycle
        .audit
        .iter()
        .filter(|event| {
            matches!(
                event.event_kind.as_str(),
                "attempt-succeeded"
                    | "wait-satisfied"
                    | "acceptance-satisfied"
                    | "acceptance-rejected"
                    | "generation-created"
                    | "evaluation-evidence"
                    | "candidate-checkpointed"
                    | "reconciliation-issue"
                    | "pi-terminal-intent"
                    | "pi-process-epoch-exited"
                    | "reopen-requested"
                    | "reopen-owner-released"
            )
        })
        .map(|event| event.event_id.as_str())
        .collect::<Vec<_>>();
    let tx = transaction.map(|value| {
        serde_json::json!({
            "phase": value.phase,
            "candidate": value.candidate.as_ref().map(|v| &v.candidate_id),
            "validation": value.validation.as_ref().map(|v| &v.result_id),
            "evaluation": value.evaluation_receipt.as_ref().map(|v| &v.receipt_id),
            "merge": value.merge_receipt.as_ref().map(|v| &v.receipt_id),
            "output": value.output_receipt.as_ref().map(|v| &v.receipt_id),
            "cleanup": value.cleanup_receipt.as_ref().map(|v| &v.receipt_id),
            "conflict": value.merge_conflict.as_ref().map(|v| &v.conflict_id),
        })
    });
    let material = serde_json::json!({
        "generation": task.lifecycle.generation,
        "goal": goal_digest(task),
        "events": meaningful_events,
        "transaction": tx,
        "save_transaction": save_transaction.map(|save| serde_json::json!({
            "transaction_id": save.transaction_id,
            "revision": save.revision,
            "phase": save.phase,
            "evidence": save.evidence_cids,
            "graph_save": save.graph_save_cid,
            "hold": save.hold_reason,
        })),
        "completion_receipt": task.completion_receipt,
        "completion_disposition": task.completion_disposition,
    });
    ProgressStamp {
        digest: digest(serde_json::to_vec(&material).expect("progress JSON is serializable")),
    }
}

fn goal_digest(task: &Task) -> String {
    digest(
        serde_json::to_vec(&serde_json::json!({
            "title": task.title,
            "description": task.description,
            "contract": task.completion_contract,
            "after": task.after,
            "inputs": task.input_dependencies,
        }))
        .expect("goal JSON is serializable"),
    )
}

fn goal_key(task: &Task) -> String {
    format!("{}#{}", task.id, task.lifecycle.generation)
}

fn route_deadline(
    route_id: &str,
    outages: u32,
    policy: &ConvergencePolicy,
    now: DateTime<Utc>,
) -> String {
    let exponent = outages.saturating_sub(1);
    let delay = exponential_delay(
        policy.route_probe_base_seconds,
        policy.route_probe_cap_seconds,
        exponent,
    );
    let jitter = deterministic_jitter(route_id, exponent, delay, policy.jitter_divisor);
    (now + Duration::seconds((delay + jitter) as i64)).to_rfc3339()
}

fn exponential_delay(base: u64, cap: u64, exponent: u32) -> u64 {
    base.saturating_mul(1u64.checked_shl(exponent.min(62)).unwrap_or(u64::MAX))
        .min(cap)
}

fn deterministic_jitter(seed: &str, exponent: u32, delay: u64, divisor: u64) -> u64 {
    let width = delay / divisor.max(1);
    stable_hash_u64(&format!("{seed}:{exponent}")) % width.saturating_add(1)
}

fn stable_hash_u64(value: &str) -> u64 {
    let hash = blake3::hash(value.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn digest(value: impl AsRef<[u8]>) -> String {
    format!("b3:{}", blake3::hash(value.as_ref()).to_hex())
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CompletionContract, Node, Task, WorkGraph};
    use crate::lifecycle::{
        ActorKind as LifecycleActorKind, AttemptDisposition, AttemptRef, LifecycleEvent,
        LifecycleEventProjection,
    };
    use crate::parser::save_graph;
    use tempfile::TempDir;

    fn policy() -> ConvergencePolicy {
        ConvergencePolicy {
            base_seconds: 10,
            cap_seconds: 80,
            route_probe_base_seconds: 20,
            route_probe_cap_seconds: 160,
            action_lease_seconds: 2,
            jitter_divisor: u64::MAX,
        }
    }

    fn task(id: &str) -> Task {
        Task {
            id: id.into(),
            title: id.into(),
            completion_contract: CompletionContract::Land,
            status: Status::Open,
            ..Task::default()
        }
    }

    fn wrapper_capability() -> WrapperChildCapability {
        WrapperChildCapability {
            task_id: "incident".into(),
            generation: 0,
            attempt_id: "attempt-0-1".into(),
            fence: 1,
            wrapper_epoch: 1,
            child_epoch: 1,
            wrapper_identity_digest: "wrapper:3913000:start:10".into(),
            child_identity_digest: "child:3913691:start:11".into(),
            owned_child: true,
        }
    }

    #[test]
    fn exited_worker_finish_reducer_crash_boundaries_are_monotone_and_exact() {
        let capability = wrapper_capability();
        let decide = |receipted, phase| {
            reduce_exited_worker_finish(&FinishConvergenceSnapshot {
                presented_capability: capability.clone(),
                authoritative_capability: capability.clone(),
                owner_proven_dead: true,
                completion_receipted: receipted,
                transaction_phase: phase,
                now_unix: 100,
            })
        };
        let boundaries = [
            (
                decide(false, None),
                FinishConvergenceRank::AwaitReceipt,
                FinishConvergenceAction::ResumeSameSession,
            ),
            (
                decide(true, None),
                FinishConvergenceRank::ReceiptNoTransaction,
                FinishConvergenceAction::ResumeSameSession,
            ),
            (
                decide(true, Some(FinalizationPhase::MergePending)),
                FinishConvergenceRank::TransactionDurable,
                FinishConvergenceAction::Promote,
            ),
            (
                decide(true, Some(FinalizationPhase::Promoted)),
                FinishConvergenceRank::Promoted,
                FinishConvergenceAction::Cleanup,
            ),
            (
                decide(true, Some(FinalizationPhase::Cleaned)),
                FinishConvergenceRank::Cleaned,
                FinishConvergenceAction::Complete,
            ),
        ];
        for pair in boundaries.windows(2) {
            assert!(pair[0].0.rank <= pair[1].0.rank, "rank must be monotone");
        }
        for (decision, rank, action) in boundaries {
            assert_eq!(decision.reducer_version, 1);
            assert_eq!(decision.rank, rank);
            assert_eq!(decision.pending_action, action);
            if action == FinishConvergenceAction::Complete {
                assert_eq!(decision.deadline_unix, None);
            } else {
                assert_eq!(decision.deadline_unix, Some(105));
            }
        }

        let waiting = reduce_exited_worker_finish(&FinishConvergenceSnapshot {
            presented_capability: capability.clone(),
            authoritative_capability: capability.clone(),
            owner_proven_dead: false,
            completion_receipted: true,
            transaction_phase: None,
            now_unix: 100,
        });
        assert_eq!(
            waiting.pending_action,
            FinishConvergenceAction::WaitForReceipt
        );
        assert_eq!(waiting.deadline_unix, Some(105));

        let mut unrelated = capability.clone();
        unrelated.wrapper_identity_digest = "unrelated:pid:start".into();
        let rejected = reduce_exited_worker_finish(&FinishConvergenceSnapshot {
            presented_capability: unrelated,
            authoritative_capability: capability,
            owner_proven_dead: true,
            completion_receipted: true,
            transaction_phase: Some(FinalizationPhase::Promoted),
            now_unix: 100,
        });
        assert_eq!(
            rejected.pending_action,
            FinishConvergenceAction::RejectStale
        );
        assert_eq!(rejected.deadline_unix, None);
        assert_eq!(rejected.rank, FinishConvergenceRank::Promoted);
    }

    #[test]
    fn exited_worker_finish_decision_wire_fixture_is_versioned() {
        let capability = wrapper_capability();
        let decision = reduce_exited_worker_finish(&FinishConvergenceSnapshot {
            presented_capability: capability.clone(),
            authoritative_capability: capability,
            owner_proven_dead: true,
            completion_receipted: true,
            transaction_phase: None,
            now_unix: 100,
        });
        assert_eq!(
            serde_json::to_string(&decision).unwrap(),
            r#"{"reducer_version":1,"rank":"receipt_no_transaction","pending_action":"resume_same_session","deadline_unix":105}"#
        );
    }

    #[test]
    fn save_transaction_replay_never_invents_semantic_evidence() {
        let source = crate::completion_evidence::AttemptSaveKey {
            graph_id: "graph".into(),
            task_id: "task".into(),
            generation: 1,
            attempt_id: "attempt-1-1".into(),
            attempt_fence: 2,
            worktree_lease_epoch: 2,
            process_epoch: 1,
            wrapper_epoch: 1,
            route_snapshot_cid: "route".into(),
            session_proof_digest: "session".into(),
            worktree_identity_digest: "root".into(),
        };
        let mut state = SaveTransactionState::new(source).unwrap();
        state.phase = SavePhase::Prepared;
        assert_eq!(
            reduce_save_transaction_replay(&state, true, 100).action,
            SaveReplayAction::Quiesce
        );
        state.phase = SavePhase::Validated;
        assert_eq!(
            reduce_save_transaction_replay(&state, true, 100).action,
            SaveReplayAction::AwaitSemanticEvidence
        );
        state.phase = SavePhase::EffectPrepared;
        assert_eq!(
            reduce_save_transaction_replay(&state, true, 100).action,
            SaveReplayAction::ReplayEffect
        );
    }

    #[test]
    fn dead_owner_prepared_intent_replays_once_to_quiescing() {
        let project = TempDir::new().unwrap();
        let dir = project.path().join(".wg");
        std::fs::create_dir_all(&dir).unwrap();
        let worktree = project.path().join(".wg-worktrees/agent-1");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: test\n").unwrap();
        let row = serde_json::json!({
            "kind": "task",
            "id": "task-a",
            "title": "Task A",
            "status": "in-progress",
            "assigned": "agent-1",
            "lifecycle": {
                "generation": 1,
                "fence": 2,
                "attempt_sequence": 1,
                "current_attempt": {
                    "id": "attempt-1-1",
                    "generation": 1,
                    "fence": 2,
                    "actor_id": "agent-1"
                }
            }
        });
        std::fs::write(dir.join("graph.jsonl"), format!("{row}\n")).unwrap();
        let (token, binding) = crate::worker_control::mint_attempt_capability(
            &dir,
            "task-a",
            1,
            "attempt-1-1",
            2,
            2,
            "agent-1",
        )
        .unwrap();
        let operation = crate::worker_control::WorkerOperation::DoneHandoff {
            converged: false,
            full_smoke: false,
        };
        crate::worker_control::begin_request(&dir, "intent-1", &token, &operation).unwrap();
        let prepared =
            crate::worker_control::prepare_done_transaction(&dir, &binding, "intent-1", &operation)
                .unwrap();
        assert_eq!(prepared.phase, SavePhase::Prepared);

        let mut registry = crate::service::AgentRegistry::new();
        let agent_id = registry.register_agent(999_999, "task-a", "pi", "output.log");
        assert_eq!(agent_id, "agent-1");
        let agent = registry.get_agent_mut(&agent_id).unwrap();
        agent.status = AgentStatus::Dead;
        agent.completed_at = Some("2026-01-01T00:00:00Z".into());
        registry.save(&dir).unwrap();

        let first = replay_dead_owner_save_transactions(&dir).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].phase, SavePhase::Quiescing);
        assert_eq!(first[0].action, SaveReplayAction::CaptureWorkSave);
        let after_first =
            crate::worker_control::load_save_transaction(&dir, &prepared.transaction_id)
                .unwrap()
                .unwrap();
        assert_eq!(after_first.revision, 2);

        let second = replay_dead_owner_save_transactions(&dir).unwrap();
        assert_eq!(second[0].action, SaveReplayAction::CaptureWorkSave);
        let after_second =
            crate::worker_control::load_save_transaction(&dir, &prepared.transaction_id)
                .unwrap()
                .unwrap();
        assert_eq!(after_second.revision, after_first.revision);
        let task = load_graph(dir.join("graph.jsonl"))
            .unwrap()
            .get_task("task-a")
            .unwrap()
            .clone();
        assert_eq!(task.status, Status::InProgress);
    }

    fn failed_source(id: &str) -> Task {
        let mut source = task(id);
        source.status = Status::Failed;
        source.lifecycle.generation = 1;
        source.lifecycle.fence = 2;
        source.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-1-1".into(),
            generation: 1,
            fence: 2,
            actor_id: "agent-1".into(),
            disposition: Some(AttemptDisposition::Failed),
        });
        source.retry_count = 1;
        source.session_id = Some("session-preserved".into());
        source
    }

    fn semantic_rejection_event(task: &Task, now: DateTime<Utc>) -> LifecycleEvent {
        LifecycleEvent {
            schema_version: 1,
            event_id: "semantic-reject-event".into(),
            idempotency_key: "semantic-reject-event".into(),
            task_id: task.id.clone(),
            task_revision: 1,
            generation: task.lifecycle.generation,
            event_kind: "acceptance-rejected".into(),
            old_state: Status::PendingEval,
            new_state: Status::Failed,
            actor_kind: LifecycleActorKind::AcceptanceController,
            actor_id: "validation".into(),
            attempt_id: task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.clone()),
            fence: task.lifecycle.fence,
            reason_code: "semantic_validation_rejected".into(),
            evidence_refs: vec!["semantic-report-1".into()],
            occurred_at: now.to_rfc3339(),
            committed_at: now.to_rfc3339(),
            projection: LifecycleEventProjection {
                status: Status::Failed,
                generation: task.lifecycle.generation,
                revision: 1,
                fence: task.lifecycle.fence,
                attempt_sequence: 1,
                current_attempt: task.lifecycle.current_attempt.clone(),
                pi_process_epoch: 0,
                pi_process_identity_digest: String::new(),
                pi_continuation_epoch: 0,
                pi_continuation: None,
                pi_terminal_reservation: None,
                reopen_intent: None,
            },
        }
    }

    #[test]
    fn production_nonsemantic_failed_prerequisite_retries_once_and_preserves_session_evidence() {
        let dir = TempDir::new().unwrap();
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let source = failed_source("source");
        let mut descendant = task("descendant");
        descendant.after = vec![source.id.clone()];
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(descendant));
        save_graph(&graph, dir.path().join("graph.jsonl")).unwrap();

        let decisions = converge_failed_prerequisites(dir.path(), now).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].action,
            crate::service::planner::ActionKind::RetryFailedPrerequisite
        );
        let graph = load_graph(dir.path().join("graph.jsonl")).unwrap();
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Open);
        assert_eq!(source.lifecycle.generation, 2);
        assert_eq!(source.session_id.as_deref(), Some("session-preserved"));
        assert!(source.lifecycle.audit.iter().any(|event| {
            event.reason_code == "nonsemantic_failed_prerequisite_retry"
                && event
                    .evidence_refs
                    .iter()
                    .any(|id| id == "session-preserved")
        }));
        assert!(
            converge_failed_prerequisites(dir.path(), now)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn production_semantic_rejection_never_retries_and_descendant_is_actionable() {
        let dir = TempDir::new().unwrap();
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut source = failed_source("semantic-source");
        let semantic_event = semantic_rejection_event(&source, now);
        source.lifecycle.audit.push(semantic_event);
        let mut descendant = task("descendant");
        descendant.after = vec![source.id.clone()];
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(descendant));
        save_graph(&graph, dir.path().join("graph.jsonl")).unwrap();

        let decisions = converge_failed_prerequisites(dir.path(), now).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].class,
            crate::service::planner::FailedPrerequisiteClass::SemanticValidationRejected
        );
        let graph = load_graph(dir.path().join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task("semantic-source").unwrap().status,
            Status::Failed
        );
        assert_eq!(
            graph
                .get_task("semantic-source")
                .unwrap()
                .lifecycle
                .generation,
            1
        );
        let descendant = graph.get_task("descendant").unwrap();
        assert!(
            descendant
                .lifecycle
                .audit
                .iter()
                .any(|event| { event.reason_code == "semantic_failed_prerequisite_terminal" })
        );
        let state = reconcile_dir(dir.path(), &policy(), now).unwrap();
        let record = state.goals.get("descendant#0").unwrap();
        assert_eq!(record.stage, ConvergenceStage::NeedsHuman);
        assert!(
            record
                .needs_human
                .as_deref()
                .unwrap()
                .contains("semantic rejection")
        );
    }

    #[test]
    fn dependency_blocked_work_is_falloff_neutral_until_ready() {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let blocked = task("blocked");
        let mut state = ConvergenceState::default();
        let none_ready = BTreeSet::new();
        state.reconcile_goals(
            std::iter::once(blocked.clone()),
            &BTreeMap::new(),
            None,
            Some(&none_ready),
            &policy(),
            now,
        );
        assert!(state.earliest_wake().is_none());
        assert!(
            state
                .advance_one_due_without_progress(&policy(), now)
                .is_none()
        );
        let record = state.goals.get("blocked#0").unwrap();
        assert_eq!(record.stage, ConvergenceStage::AwaitDependency);
        assert_eq!(record.backoff.failures_without_progress, 0);

        let ready = BTreeSet::from(["blocked".to_string()]);
        state.reconcile_goals(
            std::iter::once(blocked.clone()),
            &BTreeMap::new(),
            None,
            Some(&ready),
            &policy(),
            now + Duration::seconds(1),
        );
        let ready_record = state.goals.get("blocked#0").unwrap();
        assert_eq!(ready_record.stage, ConvergenceStage::AwaitDispatch);
        assert_eq!(ready_record.backoff.failures_without_progress, 0);
        assert!(matches!(
            state.claim_goal_action(&blocked, &policy(), now + Duration::seconds(1)),
            Admission::Allowed { .. }
        ));
    }

    #[test]
    fn restart_preserves_deadline_exponent_seed_and_pending_lease_byte_for_byte() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("service")).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut state = ConvergenceState::default();
        let t = task("goal");
        state.reconcile_goals(
            std::iter::once(t.clone()),
            &BTreeMap::new(),
            None,
            None,
            &policy(),
            now,
        );
        assert!(matches!(
            state.claim_goal_action(&t, &policy(), now),
            Admission::Allowed { .. }
        ));
        state.save(dir.path()).unwrap();
        let bytes_before = std::fs::read(ConvergenceState::path(dir.path())).unwrap();
        let loaded = ConvergenceState::load(dir.path()).unwrap();
        loaded.save(dir.path()).unwrap();
        let bytes_after = std::fs::read(ConvergenceState::path(dir.path())).unwrap();
        assert_eq!(bytes_before, bytes_after);
        let record = loaded.goals.get("goal#0").unwrap();
        assert_eq!(record.backoff.failures_without_progress, 1);
        assert_eq!(record.next_wake_at, "2026-01-01T00:00:10+00:00");
        assert!(record.pending_action.is_some());
    }

    #[test]
    fn unchanged_attempts_fall_off_but_authoritative_candidate_resets() {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut state = ConvergenceState::default();
        let mut t = task("goal");
        state.reconcile_goals(
            std::iter::once(t.clone()),
            &BTreeMap::new(),
            None,
            None,
            &policy(),
            now,
        );
        state.claim_goal_action(&t, &policy(), now);
        state.claim_goal_action(&t, &policy(), now + Duration::seconds(10));
        let record = state.goals.get("goal#0").unwrap();
        assert_eq!(record.backoff.failures_without_progress, 2);
        assert_eq!(record.next_wake_at, "2026-01-01T00:00:30+00:00");

        t.lifecycle.audit.push(crate::lifecycle::LifecycleEvent {
            schema_version: 1,
            event_id: "candidate-progress".into(),
            idempotency_key: "candidate-progress".into(),
            task_id: t.id.clone(),
            task_revision: 1,
            generation: 0,
            event_kind: "candidate-checkpointed".into(),
            old_state: Status::Open,
            new_state: Status::Open,
            actor_kind: crate::lifecycle::ActorKind::Finalizer,
            actor_id: "finalization".into(),
            attempt_id: None,
            fence: 0,
            reason_code: "candidate".into(),
            evidence_refs: vec!["candidate".into()],
            occurred_at: now.to_rfc3339(),
            committed_at: now.to_rfc3339(),
            projection: crate::lifecycle::LifecycleEventProjection {
                status: Status::Open,
                generation: 0,
                revision: 1,
                fence: 0,
                attempt_sequence: 0,
                current_attempt: None,
                pi_process_epoch: 0,
                pi_process_identity_digest: String::new(),
                pi_continuation_epoch: 0,
                pi_continuation: None,
                pi_terminal_reservation: None,
                reopen_intent: None,
            },
        });
        state.reconcile_goals(
            std::iter::once(t),
            &BTreeMap::new(),
            None,
            None,
            &policy(),
            now + Duration::seconds(11),
        );
        let reset = state.goals.get("goal#0").unwrap();
        assert_eq!(reset.backoff.failures_without_progress, 0);
        assert_eq!(reset.next_wake_at, "2026-01-01T00:00:11+00:00");
        assert!(reset.pending_action.is_none());
    }

    #[test]
    fn route_has_one_probe_and_staggers_recovery_without_fallback() {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut state = ConvergenceState::default();
        let mut health = ProviderHealth::default();
        let status = health.get_or_create_provider("pi|openrouter|b3:endpoint");
        status.is_paused = true;
        status.last_failure_at = Some(now.to_rfc3339());
        state.sync_route_health(&health, &policy(), now);
        assert!(matches!(
            state.admit_route(
                "pi|openrouter|b3:endpoint",
                "a",
                &policy(),
                now + Duration::seconds(20)
            ),
            RouteAdmission::Probe { .. }
        ));
        assert!(matches!(
            state.admit_route(
                "pi|openrouter|b3:endpoint",
                "b",
                &policy(),
                now + Duration::seconds(20)
            ),
            RouteAdmission::Deferred { .. }
        ));

        health
            .get_or_create_provider("pi|openrouter|b3:endpoint")
            .resume();
        state.sync_route_health(&health, &policy(), now + Duration::seconds(21));
        let a = state.admit_route(
            "pi|openrouter|b3:endpoint",
            "a",
            &policy(),
            now + Duration::seconds(21),
        );
        let b = state.admit_route(
            "pi|openrouter|b3:endpoint",
            "b",
            &policy(),
            now + Duration::seconds(21),
        );
        assert!(matches!(
            a,
            RouteAdmission::Allowed | RouteAdmission::Deferred { .. }
        ));
        assert!(matches!(
            b,
            RouteAdmission::Allowed | RouteAdmission::Deferred { .. }
        ));
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn stage_matrix_keeps_daemon_as_ledger_actor_not_graph_task() {
        let mut open = task("open");
        let mut working = task("working");
        working.status = Status::InProgress;
        let mut waiting = task("waiting");
        waiting.status = Status::Waiting;
        let mut eval = task("eval");
        eval.status = Status::PendingEval;
        let mut failed = task("failed");
        failed.status = Status::Failed;
        assert_eq!(
            classify_stage(&open, None).0,
            ConvergenceStage::AwaitDispatch
        );
        assert_eq!(
            classify_stage(&working, None).0,
            ConvergenceStage::ObserveOwner
        );
        working.lifecycle.pi_terminal_reservation =
            Some(crate::pi_watchdog::TerminalIntentReceipt {
                task_id: working.id.clone(),
                generation: working.lifecycle.generation,
                attempt_id: "attempt-0-1".into(),
                attempt_fence: working.lifecycle.fence,
                process_epoch: 1,
                process_identity_digest: "exact-process".into(),
                tool_call_id: "wg-done".into(),
                disposition: crate::pi_watchdog::TerminalDisposition::SuccessIntent,
                idempotency_key: "terminal-once".into(),
            });
        assert_eq!(
            classify_stage(&working, None),
            (
                ConvergenceStage::AwaitSourceFinish,
                BlockerClass::SourceFinish,
                None
            ),
            "receipted Pi completion must have a scheduled convergence wake"
        );
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut pending = ConvergenceState::default();
        pending.reconcile_goals(
            std::iter::once(working.clone()),
            &BTreeMap::new(),
            None,
            None,
            &policy(),
            now,
        );
        let record = pending.goals.get("working#0").unwrap();
        assert_eq!(record.next_wake_at, now.to_rfc3339());
        assert_eq!(
            record.finish_convergence_rank,
            Some(FinishConvergenceRank::ReceiptNoTransaction)
        );
        assert_eq!(
            record.pending_convergence_action,
            Some(FinishConvergenceAction::ResumeSameSession)
        );
        assert_eq!(
            classify_stage(&waiting, None).0,
            ConvergenceStage::AwaitWait
        );
        assert_eq!(
            classify_stage(&eval, None).0,
            ConvergenceStage::AwaitEvaluation
        );
        assert_eq!(
            classify_stage(&failed, None).0,
            ConvergenceStage::NeedsHuman
        );
        open.title = ".daemon-controller".into();
        let state = ConvergenceState::default();
        assert!(state.goals.is_empty(), "scheduler creates no graph task");
    }
}
