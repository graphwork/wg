//! Pure, replayable planner for correctness-critical daemon decisions.
//!
//! The planner consumes only normalized, typed observations and logical time.
//! It performs no filesystem, process, socket, Git, provider, signal, or clock
//! reads. Adapters are responsible for producing evidence; emitted effects are
//! idempotent requests that adapters execute and acknowledge separately.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub const DAEMON_PLANNER_SCHEMA_VERSION: u16 = 5;
pub const DAEMON_TRACE_SCHEMA_VERSION: u16 = 5;
const MIN_SUPPORTED_DAEMON_PLANNER_SCHEMA_VERSION: u16 = 1;
const MIN_SUPPORTED_DAEMON_TRACE_SCHEMA_VERSION: u16 = 1;
pub const MAX_TRACE_OBSERVATIONS: usize = 256;
pub const MAX_REPLAY_BUNDLES: usize = 32;
const TRACE_FILE: &str = "decision-trace-v1.json";
const STATE_FILE: &str = "planner-state-v1.json";
const EFFECT_JOURNAL_FILE: &str = "planner-effects-v1.json";
const LOCK_FILE: &str = ".planner.lock";
const EFFECT_JOURNAL_SCHEMA_VERSION: u16 = 1;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Identifier allowed on the replay wire. Free-form text, paths, endpoints,
/// credentials, prompts and provider output have no representable type here.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpaqueId(String);

impl OpaqueId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 96
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-|".contains(&byte))
        {
            bail!("planner identifier must be 1..=96 safe identifier bytes");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Normalize an external stable identity without admitting its raw bytes
    /// to the planner wire when they are not in the bounded identifier alphabet.
    pub fn normalized(value: impl AsRef<[u8]>) -> Self {
        let bytes = value.as_ref();
        let candidate = String::from_utf8_lossy(bytes).to_string();
        Self::new(candidate).unwrap_or_else(|_| {
            let digest = blake3::hash(bytes).to_hex();
            Self::new(format!("id:{digest}")).expect("digest is a safe planner identifier")
        })
    }
}

impl fmt::Display for OpaqueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for OpaqueId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OpaqueId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerRuleset {
    Historical,
    #[default]
    Corrected,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskKey {
    pub graph_id: OpaqueId,
    pub task_id: OpaqueId,
    pub generation: u64,
    pub attempt_id: OpaqueId,
    pub fence: u64,
}

impl TaskKey {
    fn stable_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.graph_id, self.task_id, self.generation, self.attempt_id, self.fence
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnerEvidence {
    None,
    AuthenticatedLive {
        actor_id: OpaqueId,
        lease_id: OpaqueId,
    },
    ProvenDead {
        actor_id: OpaqueId,
        lease_id: OpaqueId,
    },
    Unauthenticated {
        actor_id: OpaqueId,
    },
}

impl OwnerEvidence {
    fn is_authenticated_live(&self) -> bool {
        matches!(self, Self::AuthenticatedLive { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    SpawnAttempt,
    ResumeSameSession,
    ConsumeWait,
    ReconcileChatRequest,
    ReplanFinish,
    PromoteCandidate,
    CleanupFinish,
    ReleaseDeadOwner,
    MigrateServiceState,
    ProbeRoute,
    ArchiveBatch,
    /// Reopen the exact failed prerequisite as one new generation. The
    /// prerequisite binding on the effect makes this distinct from spawning
    /// the blocked descendant.
    RetryFailedPrerequisite,
    /// Persist an operator-visible, evidence-bound reconciliation issue when
    /// an automatic retry is unsafe or its one-shot budget is exhausted.
    RecordNeedsReconciliation,
    /// Fence the exact proven-dead owner while retaining its registered
    /// worktree, owner token, observer state, and dirty bytes as evidence.
    ReclaimRetainWorktree,
    FailClosedHold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    DependencySuccess,
    CorrelatedMessage,
    HumanInput,
    ArchiveConfirmation,
    ProviderRecovery,
    /// Dependency/readiness evidence must change before dispatch can proceed.
    DependencyChange,
    /// Capacity, disk, or another resource admission observation must change.
    ResourceCapacity,
    /// A per-task admission gate is waiting for its persisted cooldown.
    Admission,
    /// Source bytes/evidence require repair. A planner-owned deadline may be
    /// attached without granting the dispatch adapter mutation authority.
    SourceRepair,
    /// A semantic rejection is terminal. Its descendant waits for an explicit
    /// repair/waiver/new generation and can never turn this wait into a retry.
    SemanticPrerequisiteRepair,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWait {
    pub wait_id: OpaqueId,
    pub kind: WaitKind,
    /// Optional planner-owned re-observation deadline. This remains one
    /// external-wait forward class; reaching it never executes a stale action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledAction {
    pub action: ActionKind,
    pub deadline: u64,
}

/// Exact, redacted binding carried by a dispatch effect. `plan_id` is a digest
/// of the already-resolved SpawnPlan (including the exact model identity); the
/// adapter must recompute and match it before execution. Route/model fallback
/// is therefore not representable at the planner boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchEffectBinding {
    pub route_id: OpaqueId,
    pub plan_id: OpaqueId,
    pub retry_base_seconds: u64,
    pub retry_cap_seconds: u64,
    pub jitter_divisor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectBinding {
    Dispatch(DispatchEffectBinding),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DispatchReadiness {
    Ready,
    Waiting {
        wait_id: OpaqueId,
        kind: WaitKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DispatchAdmission {
    Admitted,
    Deferred {
        wait_id: OpaqueId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceEvidence {
    Available,
    Deferred {
        wait_id: OpaqueId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteHealthEvidence {
    Healthy,
    Unavailable { failure_id: OpaqueId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRouteObservation {
    pub route_id: OpaqueId,
    pub plan_id: OpaqueId,
    pub health: RouteHealthEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPolicy {
    pub retry_base_seconds: u64,
    pub retry_cap_seconds: u64,
    pub route_probe_base_seconds: u64,
    pub route_probe_cap_seconds: u64,
    pub action_lease_seconds: u64,
    pub jitter_divisor: u64,
}

impl DispatchPolicy {
    fn normalized(&self) -> Self {
        Self {
            retry_base_seconds: self.retry_base_seconds.max(1),
            retry_cap_seconds: self.retry_cap_seconds.max(self.retry_base_seconds.max(1)),
            route_probe_base_seconds: self.route_probe_base_seconds.max(1),
            route_probe_cap_seconds: self
                .route_probe_cap_seconds
                .max(self.route_probe_base_seconds.max(1)),
            action_lease_seconds: self.action_lease_seconds.max(1),
            jitter_divisor: self.jitter_divisor.max(1),
        }
    }
}

/// One production dispatch normalization. Gate precedence is deterministic:
/// readiness, admission, resource, then exact route health. Consequently every
/// unfinished observation projects to exactly one forward class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchObservation {
    pub key: TaskKey,
    pub progress_id: OpaqueId,
    pub readiness: DispatchReadiness,
    pub admission: DispatchAdmission,
    pub resource: ResourceEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<DispatchRouteObservation>,
    pub policy: DispatchPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerRouteState {
    Healthy,
    Unavailable,
    Probing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerRouteProbeLease {
    pub effect_id: OpaqueId,
    pub task_id: OpaqueId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub spawned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerRouteProjection {
    pub route_id: OpaqueId,
    pub epoch: u64,
    pub state: PlannerRouteState,
    pub consecutive_outages: u32,
    pub next_probe_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_lease: Option<PlannerRouteProbeLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_id: Option<OpaqueId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at: Option<u64>,
    pub policy: DispatchPolicy,
}

/// Zero-output is evidence only during this cutover. It is persisted for the
/// ownership planner; the detector cannot kill, reopen, fail, pause, reroute,
/// or schedule a retry on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroOutputObservation {
    pub task: TaskKey,
    pub owner_id: OpaqueId,
    pub evidence_id: OpaqueId,
    pub age_bucket: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<OpaqueId>,
}

/// RFC3339 timestamp retained byte-for-byte from a migrated durable scheduler.
/// Arbitrary strings are not representable in a planner trace.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlannerTimestamp(String);

impl PlannerTimestamp {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() > 64 || DateTime::parse_from_rfc3339(&value).is_err() {
            bail!("planner timestamp must be a bounded RFC3339 value");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn datetime(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.0)
            .expect("PlannerTimestamp validates on construction")
            .with_timezone(&Utc)
    }
}

impl Serialize for PlannerTimestamp {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PlannerTimestamp {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedBackoff {
    pub class: super::convergence::BlockerClass,
    pub failures_without_progress: u32,
    pub base_seconds: u64,
    pub cap_seconds: u64,
    pub jitter_seed: OpaqueId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedActionLease {
    pub action_id: OpaqueId,
    pub task_id: OpaqueId,
    pub generation: u64,
    pub attempt_id: Option<OpaqueId>,
    pub fence: u64,
    pub revision: u64,
    pub stage: super::convergence::ConvergenceStage,
    pub progress_id: OpaqueId,
    pub lease_epoch: u64,
    pub expires_at: PlannerTimestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedGoalSchedule {
    pub task_id: OpaqueId,
    pub generation: u64,
    pub priority: u32,
    pub stage: super::convergence::ConvergenceStage,
    pub blocker: super::convergence::BlockerClass,
    pub next_wake_at: PlannerTimestamp,
    pub backoff: ImportedBackoff,
    pub progress_id: OpaqueId,
    pub pending_action: Option<ImportedActionLease>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedRouteProbeLease {
    pub action_id: OpaqueId,
    pub task_id: OpaqueId,
    pub epoch: u64,
    pub expires_at: PlannerTimestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedRouteSchedule {
    pub route_id: OpaqueId,
    pub epoch: u64,
    pub state: super::convergence::RouteBreakerState,
    pub consecutive_outages: u32,
    pub next_probe_at: PlannerTimestamp,
    pub probe_lease: Option<ImportedRouteProbeLease>,
    pub last_failure_marker: Option<OpaqueId>,
    pub recovered_at: Option<PlannerTimestamp>,
}

/// Typed, redacted one-time import. It contains every legacy scheduling value
/// needed for later domain cutovers, but no free-form reason, provider output,
/// path, endpoint, prompt, or credential can enter the replay wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyConvergenceImport {
    pub source_schema_version: u32,
    pub goals: BTreeMap<OpaqueId, ImportedGoalSchedule>,
    pub routes: BTreeMap<OpaqueId, ImportedRouteSchedule>,
    pub last_reconciled_at: Option<PlannerTimestamp>,
}

impl LegacyConvergenceImport {
    fn from_legacy(value: &super::convergence::ConvergenceState) -> Result<Self> {
        let mut goals = BTreeMap::new();
        for record in value.goals.values() {
            let task_id = OpaqueId::normalized(&record.goal.task_id);
            let key = OpaqueId::normalized(format!("{}:{}", task_id, record.goal.generation));
            let pending_action = record
                .pending_action
                .as_ref()
                .map(|lease| {
                    Ok::<_, anyhow::Error>(ImportedActionLease {
                        action_id: OpaqueId::normalized(&lease.action_id),
                        task_id: OpaqueId::normalized(&lease.task_id),
                        generation: lease.generation,
                        attempt_id: lease.attempt_id.as_deref().map(OpaqueId::normalized),
                        fence: lease.fence,
                        revision: lease.revision,
                        stage: lease.stage,
                        progress_id: OpaqueId::normalized(&lease.progress_digest),
                        lease_epoch: lease.lease_epoch,
                        expires_at: PlannerTimestamp::new(lease.expires_at.clone())?,
                    })
                })
                .transpose()?;
            goals.insert(
                key,
                ImportedGoalSchedule {
                    task_id,
                    generation: record.goal.generation,
                    priority: record.priority,
                    stage: record.stage,
                    blocker: record.blocker,
                    next_wake_at: PlannerTimestamp::new(record.next_wake_at.clone())?,
                    backoff: ImportedBackoff {
                        class: record.backoff.class,
                        failures_without_progress: record.backoff.failures_without_progress,
                        base_seconds: record.backoff.base_seconds,
                        cap_seconds: record.backoff.cap_seconds,
                        jitter_seed: OpaqueId::normalized(&record.backoff.jitter_seed),
                    },
                    progress_id: OpaqueId::normalized(&record.last_authoritative_progress.digest),
                    pending_action,
                },
            );
        }
        let mut routes = BTreeMap::new();
        for breaker in value.route_breakers.values() {
            let route_id = OpaqueId::normalized(&breaker.route_id);
            let probe_lease = breaker
                .probe_lease
                .as_ref()
                .map(|lease| {
                    Ok::<_, anyhow::Error>(ImportedRouteProbeLease {
                        action_id: OpaqueId::normalized(&lease.action_id),
                        task_id: OpaqueId::normalized(&lease.task_id),
                        epoch: lease.epoch,
                        expires_at: PlannerTimestamp::new(lease.expires_at.clone())?,
                    })
                })
                .transpose()?;
            routes.insert(
                route_id.clone(),
                ImportedRouteSchedule {
                    route_id,
                    epoch: breaker.epoch,
                    state: breaker.state,
                    consecutive_outages: breaker.consecutive_outages,
                    next_probe_at: PlannerTimestamp::new(breaker.next_probe_at.clone())?,
                    probe_lease,
                    last_failure_marker: breaker
                        .last_failure_marker
                        .as_deref()
                        .map(OpaqueId::normalized),
                    recovered_at: breaker
                        .recovered_at
                        .as_ref()
                        .map(|value| PlannerTimestamp::new(value.clone()))
                        .transpose()?,
                },
            );
        }
        Ok(Self {
            source_schema_version: value.schema_version,
            goals,
            routes,
            last_reconciled_at: value
                .last_reconciled_at
                .as_ref()
                .map(|value| PlannerTimestamp::new(value.clone()))
                .transpose()?,
        })
    }

    fn earliest_wake(&self) -> Option<DateTime<Utc>> {
        self.goals
            .values()
            .filter(|record| {
                !matches!(
                    record.stage,
                    super::convergence::ConvergenceStage::ObserveOwner
                        | super::convergence::ConvergenceStage::AwaitDependency
                        | super::convergence::ConvergenceStage::AwaitDispatch
                )
            })
            .map(|record| record.next_wake_at.datetime())
            .min()
    }
}

/// Typed source of a failed dependency. These are deliberately semantic
/// categories rather than strings parsed from provider output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedPrerequisiteClass {
    ProviderUnavailableAfterDurableCandidate,
    SourceExecutionNoProgress,
    SourceExecutionWithProgress,
    OrphanBeforeSpawn,
    SemanticValidationRejected,
}

/// Exact evidence presence on the replay wire. `Absent` is an observed fact,
/// not a missing optional field, which is important for the before-spawn and
/// zero-progress incidents.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceSlot {
    Absent,
    Present { evidence_id: OpaqueId },
}

impl EvidenceSlot {
    fn is_present(&self) -> bool {
        matches!(self, Self::Present { .. })
    }
}

/// Evidence retained from the failed source tuple. IDs are opaque digests or
/// bounded stable identities; paths, logs and provider prose remain outside
/// the pure planner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedPrerequisiteEvidence {
    pub work_save: EvidenceSlot,
    pub candidate: EvidenceSlot,
    pub session: EvidenceSlot,
    pub worktree: EvidenceSlot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedPrerequisite {
    pub source: TaskKey,
    pub class: FailedPrerequisiteClass,
    pub evidence: FailedPrerequisiteEvidence,
    /// Number of already-issued automatic retries for this exact source
    /// lineage, separate from user retries and provider attempts.
    pub automatic_retries: u8,
    /// Persisted finite budget. Zero means reconcile immediately.
    pub max_automatic_retries: u8,
}

/// Actual incidents are represented as bounded codes, never copied logs or
/// task/provider content. The historical ruleset exposes the named violation;
/// the corrected ruleset deterministically chooses a safe forward class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentCode {
    ExitedWrapperRejectedStale,
    ReopenBeforeOwnerRelease,
    ParkResumeOverlap,
    ObsoleteDaemonChatCreationLostResponse,
    TargetMovedDuringFinish,
    SurpriseArchivalBacklog,
    ControlPlaneCandidateReplacement,
    DeadPiOwnerRetainingLeases,
    AbandonedDependencySatisfiedReadiness,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskObservation {
    pub key: TaskKey,
    /// Digest/identifier of authoritative progress. A new logical effect for
    /// the same task/action requires a new progress identity.
    pub progress_id: OpaqueId,
    pub unfinished: bool,
    pub owner: OwnerEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runnable: Option<ActionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_wait: Option<ExternalWait>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<ScheduledAction>,
    /// Exact adapter binding for runnable dispatch effects. Historical and
    /// non-dispatch observations omit it byte-for-byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_binding: Option<EffectBinding>,
    #[serde(default)]
    pub incidents: BTreeSet<IncidentCode>,
    /// Present only when this unfinished task is blocked by an exact failed
    /// prerequisite. Version-1 observations deserialize to `None` unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_prerequisite: Option<FailedPrerequisite>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckOutcome {
    Succeeded,
    Retryable,
    RejectedStale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observation {
    Task(Box<TaskObservation>),
    /// Typed production dispatch gates and exact route evidence.
    Dispatch(Box<DispatchObservation>),
    /// Typed observation only; ownership actions are cut over separately.
    ZeroOutput(Box<ZeroOutputObservation>),
    /// A preparation-time ownership decision. Unlike a normal unfinished-task
    /// projection, this can authorize two ordered, independently acknowledged
    /// logical effects: retain/fence the dead owner's exact tuple, then dispatch
    /// the already-selected current tuple. The adapter must not delete or edit
    /// either evidence slot while executing the reclaim effect.
    WorktreeSpawn(Box<WorktreeSpawnObservation>),
    EffectAcknowledged {
        effect_id: OpaqueId,
        outcome: AckOutcome,
    },
    Crash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationEnvelope {
    pub sequence: u64,
    pub logical_time: u64,
    pub observation: Observation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSpawnObservation {
    pub stale_owner: TaskKey,
    pub current_attempt: TaskKey,
    pub progress_id: OpaqueId,
    pub worktree_id: OpaqueId,
    pub owner: OwnerEvidence,
    pub owner_token: EvidenceSlot,
    pub observer_state: EvidenceSlot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationCode {
    NoForwardDisposition,
    MultipleForwardDispositions,
    SequenceConflict,
    SequenceRegression,
    CrossGraphIdentity,
    ExitedWrapperRejectedStale,
    ReopenBeforeOwnerRelease,
    ParkResumeOverlap,
    ObsoleteDaemonChatCreationLostResponse,
    TargetMovedDuringFinish,
    SurpriseArchivalBacklog,
    ControlPlaneCandidateReplacement,
    DeadPiOwnerRetainingLeases,
    AbandonedDependencySatisfiedReadiness,
    UnprovenWorktreeOwnership,
}

impl From<IncidentCode> for ViolationCode {
    fn from(value: IncidentCode) -> Self {
        match value {
            IncidentCode::ExitedWrapperRejectedStale => Self::ExitedWrapperRejectedStale,
            IncidentCode::ReopenBeforeOwnerRelease => Self::ReopenBeforeOwnerRelease,
            IncidentCode::ParkResumeOverlap => Self::ParkResumeOverlap,
            IncidentCode::ObsoleteDaemonChatCreationLostResponse => {
                Self::ObsoleteDaemonChatCreationLostResponse
            }
            IncidentCode::TargetMovedDuringFinish => Self::TargetMovedDuringFinish,
            IncidentCode::SurpriseArchivalBacklog => Self::SurpriseArchivalBacklog,
            IncidentCode::ControlPlaneCandidateReplacement => {
                Self::ControlPlaneCandidateReplacement
            }
            IncidentCode::DeadPiOwnerRetainingLeases => Self::DeadPiOwnerRetainingLeases,
            IncidentCode::AbandonedDependencySatisfiedReadiness => {
                Self::AbandonedDependencySatisfiedReadiness
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Issued,
    Acknowledged(AckOutcome),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedEffect {
    pub effect_id: OpaqueId,
    pub task: TaskKey,
    /// Authoritative progress identity that produced this effect. Historical
    /// effects omit it; new dispatch observations use it to reject a prior
    /// issued effect when task evidence changes without changing the tuple.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_id: Option<OpaqueId>,
    /// Exact route/model binding for dispatch and route-probe execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<EffectBinding>,
    /// Exact failed source acted on by prerequisite convergence. Absent for
    /// ordinary task-local planner effects and every v1 fixture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerequisite: Option<TaskKey>,
    pub action: ActionKind,
    pub issue_epoch: u64,
    pub status: EffectStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerState {
    pub schema_version: u16,
    pub graph_id: OpaqueId,
    pub logical_time: u64,
    pub last_sequence: Option<u64>,
    #[serde(default)]
    pub seen_observations: BTreeMap<u64, OpaqueId>,
    #[serde(default)]
    pub tasks: BTreeMap<OpaqueId, TaskObservation>,
    /// Planner-owned route breaker/probe state. Empty historical projections
    /// remain omitted so old replay fixtures stay byte-identical.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<OpaqueId, PlannerRouteProjection>,
    /// Latest typed zero-output observations, evidence only.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub zero_output: BTreeMap<OpaqueId, ZeroOutputObservation>,
    #[serde(default)]
    pub effects: BTreeMap<OpaqueId, PlannedEffect>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub effect_retry_deadlines: BTreeMap<OpaqueId, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub effect_retry_attempts: BTreeMap<OpaqueId, u32>,
    #[serde(default)]
    pub early_acknowledgements: BTreeMap<OpaqueId, AckOutcome>,
    #[serde(default)]
    pub repaired_incidents: BTreeSet<IncidentCode>,
    #[serde(default)]
    pub fail_closed: bool,
    /// Exact one-time import of the legacy convergence scheduler. It is
    /// migration evidence/read-model data only: the pure reducer never mutates
    /// it or issues an effect from it. Later authority cutovers consume the
    /// typed deadlines one domain at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_convergence: Option<Box<LegacyConvergenceImport>>,
    /// `false` remains omitted so v1-v3 offline replay bytes do not drift.
    #[serde(default, skip_serializing_if = "is_false")]
    pub convergence_import_complete: bool,
}

impl PlannerState {
    pub fn new(graph_id: OpaqueId) -> Self {
        Self {
            schema_version: DAEMON_PLANNER_SCHEMA_VERSION,
            graph_id,
            logical_time: 0,
            last_sequence: None,
            seen_observations: BTreeMap::new(),
            tasks: BTreeMap::new(),
            routes: BTreeMap::new(),
            zero_output: BTreeMap::new(),
            effects: BTreeMap::new(),
            effect_retry_deadlines: BTreeMap::new(),
            effect_retry_attempts: BTreeMap::new(),
            early_acknowledgements: BTreeMap::new(),
            repaired_incidents: BTreeSet::new(),
            fail_closed: false,
            legacy_convergence: None,
            convergence_import_complete: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerStep {
    pub sequence: u64,
    pub state: PlannerState,
    pub effects: Vec<PlannedEffect>,
    pub violations: BTreeSet<ViolationCode>,
}

fn observation_id(envelope: &ObservationEnvelope) -> OpaqueId {
    let bytes = serde_json::to_vec(envelope).expect("typed observation serializes");
    let digest = blake3::hash(&bytes).to_hex();
    OpaqueId::new(format!("obs:{digest}")).expect("digest is safe")
}

fn task_state_id(task: &TaskKey) -> OpaqueId {
    let digest = blake3::hash(task.stable_key().as_bytes()).to_hex();
    OpaqueId::new(format!("task:{digest}")).expect("digest is safe")
}

fn effect_id(
    task: &TaskKey,
    progress_id: &OpaqueId,
    binding: Option<&EffectBinding>,
    action: ActionKind,
    issue_epoch: u64,
) -> OpaqueId {
    let binding_id = binding.map_or_else(String::new, |binding| {
        let bytes = serde_json::to_vec(binding).expect("typed binding serializes");
        format!(":binding:{}", blake3::hash(&bytes).to_hex())
    });
    let material = format!(
        "{}:{}:{action:?}:{issue_epoch}{binding_id}",
        task.stable_key(),
        progress_id
    );
    let digest = blake3::hash(material.as_bytes()).to_hex();
    OpaqueId::new(format!("effect:{digest}")).expect("digest is safe")
}

fn corrected_incident_projection(task: &mut TaskObservation, now: u64) {
    for incident in task.incidents.clone() {
        task.owner = OwnerEvidence::None;
        task.runnable = None;
        task.external_wait = None;
        task.scheduled = None;
        match incident {
            IncidentCode::ExitedWrapperRejectedStale => {
                task.scheduled = Some(ScheduledAction {
                    action: ActionKind::ResumeSameSession,
                    deadline: now,
                });
            }
            IncidentCode::ReopenBeforeOwnerRelease | IncidentCode::DeadPiOwnerRetainingLeases => {
                task.scheduled = Some(ScheduledAction {
                    action: ActionKind::ReleaseDeadOwner,
                    deadline: now,
                });
            }
            IncidentCode::ParkResumeOverlap => {
                task.external_wait = Some(ExternalWait {
                    wait_id: OpaqueId::new("correlated-wait").expect("constant id"),
                    kind: WaitKind::CorrelatedMessage,
                    deadline: None,
                });
            }
            IncidentCode::ObsoleteDaemonChatCreationLostResponse => {
                task.scheduled = Some(ScheduledAction {
                    action: ActionKind::ReconcileChatRequest,
                    deadline: now,
                });
            }
            IncidentCode::TargetMovedDuringFinish => {
                task.scheduled = Some(ScheduledAction {
                    action: ActionKind::ReplanFinish,
                    deadline: now,
                });
            }
            IncidentCode::SurpriseArchivalBacklog => {
                task.external_wait = Some(ExternalWait {
                    wait_id: OpaqueId::new("archive-confirmation").expect("constant id"),
                    kind: WaitKind::ArchiveConfirmation,
                    deadline: None,
                });
            }
            IncidentCode::ControlPlaneCandidateReplacement => {
                task.scheduled = Some(ScheduledAction {
                    action: ActionKind::FailClosedHold,
                    deadline: now,
                });
            }
            IncidentCode::AbandonedDependencySatisfiedReadiness => {
                task.external_wait = Some(ExternalWait {
                    wait_id: OpaqueId::new("dependency-success").expect("constant id"),
                    kind: WaitKind::DependencySuccess,
                    deadline: None,
                });
            }
        }
    }
}

fn corrected_failed_prerequisite_projection(task: &mut TaskObservation, now: u64) {
    let Some(failure) = task.failed_prerequisite.as_ref() else {
        return;
    };
    task.owner = OwnerEvidence::None;
    task.runnable = None;
    task.external_wait = None;
    task.scheduled = None;

    if failure.class == FailedPrerequisiteClass::SemanticValidationRejected {
        task.external_wait = Some(ExternalWait {
            wait_id: OpaqueId::new(format!(
                "semantic-repair:{}:{}",
                failure.source.task_id, failure.source.generation
            ))
            .expect("typed source produces a safe wait id"),
            kind: WaitKind::SemanticPrerequisiteRepair,
            deadline: None,
        });
        return;
    }

    let retry_available = failure.automatic_retries < failure.max_automatic_retries;
    let action = if !retry_available {
        ActionKind::RecordNeedsReconciliation
    } else {
        match failure.class {
            FailedPrerequisiteClass::ProviderUnavailableAfterDurableCandidate
                if failure.evidence.work_save.is_present()
                    && failure.evidence.candidate.is_present() =>
            {
                ActionKind::ReplanFinish
            }
            FailedPrerequisiteClass::SourceExecutionNoProgress
            | FailedPrerequisiteClass::SourceExecutionWithProgress
            | FailedPrerequisiteClass::OrphanBeforeSpawn => ActionKind::RetryFailedPrerequisite,
            // Provider failures without the durable candidate asserted by the
            // typed class are contradictory evidence and must not discard WIP.
            FailedPrerequisiteClass::ProviderUnavailableAfterDurableCandidate
            | FailedPrerequisiteClass::SemanticValidationRejected => {
                ActionKind::RecordNeedsReconciliation
            }
        }
    };
    task.scheduled = Some(ScheduledAction {
        action,
        deadline: now,
    });
}

fn forward_count(task: &TaskObservation) -> usize {
    usize::from(task.runnable.is_some())
        + usize::from(task.owner.is_authenticated_live())
        + usize::from(task.external_wait.is_some())
        + usize::from(task.scheduled.is_some())
}

fn issue_bound_effect(
    state: &mut PlannerState,
    task: &TaskKey,
    progress_id: &OpaqueId,
    binding: Option<EffectBinding>,
    prerequisite: Option<TaskKey>,
    action: ActionKind,
) -> Option<PlannedEffect> {
    let issue_epoch = 1;
    let id = effect_id(task, progress_id, binding.as_ref(), action, issue_epoch);
    if state.effects.contains_key(&id) {
        return None;
    }
    let status = match state.early_acknowledgements.remove(&id) {
        Some(AckOutcome::Retryable) | None => EffectStatus::Issued,
        Some(outcome) => EffectStatus::Acknowledged(outcome),
    };
    let should_emit = status == EffectStatus::Issued;
    let effect = PlannedEffect {
        effect_id: id.clone(),
        task: task.clone(),
        progress_id: Some(progress_id.clone()),
        binding,
        prerequisite,
        action,
        issue_epoch,
        status,
    };
    state.effects.insert(id, effect.clone());
    should_emit.then_some(effect)
}

fn issue_effect(
    state: &mut PlannerState,
    task: &TaskObservation,
    action: ActionKind,
) -> Option<PlannedEffect> {
    issue_bound_effect(
        state,
        &task.key,
        &task.progress_id,
        task.effect_binding.clone(),
        task.failed_prerequisite
            .as_ref()
            .map(|failure| failure.source.clone()),
        action,
    )
}

fn unix_timestamp(value: &PlannerTimestamp) -> u64 {
    value.datetime().timestamp().max(0) as u64
}

fn stable_hash_u64(material: &str) -> u64 {
    let hash = blake3::hash(material.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn bounded_delay(base: u64, cap: u64, exponent: u32) -> u64 {
    base.max(1)
        .saturating_mul(1u64.checked_shl(exponent.min(63)).unwrap_or(u64::MAX))
        .min(cap.max(base.max(1)))
}

fn route_deadline(
    route_id: &OpaqueId,
    outage_count: u32,
    policy: &DispatchPolicy,
    now: u64,
) -> u64 {
    let exponent = outage_count.saturating_sub(1);
    let delay = bounded_delay(
        policy.route_probe_base_seconds,
        policy.route_probe_cap_seconds,
        exponent,
    );
    let jitter_window = (delay / policy.jitter_divisor.max(1)).max(1);
    let jitter = stable_hash_u64(&format!("{route_id}:{outage_count}")) % jitter_window;
    now.saturating_add(delay).saturating_add(jitter)
}

fn route_from_legacy(
    state: &PlannerState,
    route_id: &OpaqueId,
    policy: &DispatchPolicy,
    now: u64,
) -> Option<PlannerRouteProjection> {
    let imported = state.legacy_convergence.as_ref()?.routes.get(route_id)?;
    Some(PlannerRouteProjection {
        route_id: route_id.clone(),
        epoch: imported.epoch,
        state: match imported.state {
            super::convergence::RouteBreakerState::Healthy => PlannerRouteState::Healthy,
            super::convergence::RouteBreakerState::Unavailable => PlannerRouteState::Unavailable,
            super::convergence::RouteBreakerState::Probing => PlannerRouteState::Probing,
        },
        consecutive_outages: imported.consecutive_outages,
        next_probe_at: unix_timestamp(&imported.next_probe_at),
        probe_lease: imported
            .probe_lease
            .as_ref()
            .map(|lease| PlannerRouteProbeLease {
                effect_id: lease.action_id.clone(),
                task_id: lease.task_id.clone(),
                expires_at: Some(unix_timestamp(&lease.expires_at)),
                spawned: false,
            }),
        last_failure_id: imported.last_failure_marker.clone(),
        recovered_at: imported.recovered_at.as_ref().map(unix_timestamp),
        policy: policy.clone(),
    })
    .or_else(|| {
        Some(PlannerRouteProjection {
            route_id: route_id.clone(),
            epoch: 0,
            state: PlannerRouteState::Healthy,
            consecutive_outages: 0,
            next_probe_at: now,
            probe_lease: None,
            last_failure_id: None,
            recovered_at: None,
            policy: policy.clone(),
        })
    })
}

fn dispatch_wait(
    task: &mut TaskObservation,
    wait_id: OpaqueId,
    kind: WaitKind,
    deadline: Option<u64>,
) {
    task.runnable = None;
    task.scheduled = None;
    task.external_wait = Some(ExternalWait {
        wait_id,
        kind,
        deadline,
    });
}

fn project_dispatch(
    state: &mut PlannerState,
    observed: &DispatchObservation,
    now: u64,
    emitted: &mut Vec<PlannedEffect>,
    violations: &mut BTreeSet<ViolationCode>,
) {
    let policy = observed.policy.normalized();
    let mut task = TaskObservation {
        key: observed.key.clone(),
        progress_id: observed.progress_id.clone(),
        unfinished: true,
        owner: OwnerEvidence::None,
        runnable: None,
        external_wait: None,
        scheduled: None,
        effect_binding: None,
        incidents: BTreeSet::new(),
        failed_prerequisite: None,
    };

    if task.key.graph_id != state.graph_id {
        violations.insert(ViolationCode::CrossGraphIdentity);
        state.fail_closed = true;
        return;
    }

    match &observed.readiness {
        DispatchReadiness::Ready => {}
        DispatchReadiness::Waiting {
            wait_id,
            kind,
            deadline,
        } => {
            dispatch_wait(&mut task, wait_id.clone(), *kind, *deadline);
            state.tasks.insert(task_state_id(&task.key), task);
            return;
        }
    }
    match &observed.admission {
        DispatchAdmission::Admitted => {}
        DispatchAdmission::Deferred { wait_id, deadline } => {
            dispatch_wait(&mut task, wait_id.clone(), WaitKind::Admission, *deadline);
            state.tasks.insert(task_state_id(&task.key), task);
            return;
        }
    }
    match &observed.resource {
        ResourceEvidence::Available => {}
        ResourceEvidence::Deferred { wait_id, deadline } => {
            dispatch_wait(
                &mut task,
                wait_id.clone(),
                WaitKind::ResourceCapacity,
                *deadline,
            );
            state.tasks.insert(task_state_id(&task.key), task);
            return;
        }
    }

    let Some(route_observation) = observed.route.as_ref() else {
        violations.insert(ViolationCode::NoForwardDisposition);
        task.scheduled = Some(ScheduledAction {
            action: ActionKind::FailClosedHold,
            deadline: now,
        });
        state.tasks.insert(task_state_id(&task.key), task);
        state.fail_closed = true;
        return;
    };
    let binding = EffectBinding::Dispatch(DispatchEffectBinding {
        route_id: route_observation.route_id.clone(),
        plan_id: route_observation.plan_id.clone(),
        retry_base_seconds: policy.retry_base_seconds,
        retry_cap_seconds: policy.retry_cap_seconds,
        jitter_divisor: policy.jitter_divisor,
    });
    task.effect_binding = Some(binding);

    let route_id = route_observation.route_id.clone();
    let mut route = state
        .routes
        .get(&route_id)
        .cloned()
        .or_else(|| route_from_legacy(state, &route_id, &policy, now))
        .unwrap_or(PlannerRouteProjection {
            route_id: route_id.clone(),
            epoch: 0,
            state: PlannerRouteState::Healthy,
            consecutive_outages: 0,
            next_probe_at: now,
            probe_lease: None,
            last_failure_id: None,
            recovered_at: None,
            policy: policy.clone(),
        });
    route.policy = policy.clone();

    match &route_observation.health {
        RouteHealthEvidence::Healthy => {
            if route.state != PlannerRouteState::Healthy {
                route.epoch = route.epoch.saturating_add(1);
                route.recovered_at = Some(now);
            }
            route.state = PlannerRouteState::Healthy;
            route.consecutive_outages = 0;
            route.next_probe_at = now;
            route.probe_lease = None;
            route.last_failure_id = None;
            let release_at = route.recovered_at.map(|recovered_at| {
                let spread = stable_hash_u64(&format!(
                    "{}:{}:{}",
                    route.route_id, route.epoch, task.key.task_id
                )) % policy
                    .route_probe_base_seconds
                    .saturating_mul(1_000)
                    .saturating_add(1);
                recovered_at.saturating_add((spread + 999) / 1_000)
            });
            if release_at.is_some_and(|deadline| deadline > now) {
                dispatch_wait(
                    &mut task,
                    OpaqueId::normalized(format!("route-stagger:{}", route.route_id)),
                    WaitKind::ProviderRecovery,
                    release_at,
                );
            } else {
                task.runnable = Some(ActionKind::SpawnAttempt);
            }
        }
        RouteHealthEvidence::Unavailable { failure_id } => {
            if route.last_failure_id.as_ref() != Some(failure_id) {
                route.epoch = route.epoch.saturating_add(1);
                route.state = PlannerRouteState::Unavailable;
                route.consecutive_outages = route.consecutive_outages.saturating_add(1).max(1);
                route.next_probe_at =
                    route_deadline(&route.route_id, route.consecutive_outages, &policy, now);
                route.probe_lease = None;
                route.last_failure_id = Some(failure_id.clone());
                route.recovered_at = None;
            }

            if route.state == PlannerRouteState::Probing
                && let Some(lease) = route.probe_lease.as_ref()
            {
                let pending_effect = state
                    .effects
                    .get(&lease.effect_id)
                    .is_some_and(|effect| matches!(effect.status, EffectStatus::Issued));
                if lease.spawned || pending_effect {
                    let deadline = (!lease.spawned)
                        .then(|| {
                            state
                                .effect_retry_deadlines
                                .get(&lease.effect_id)
                                .copied()
                                .or(lease.expires_at)
                        })
                        .flatten();
                    dispatch_wait(
                        &mut task,
                        OpaqueId::normalized(format!("route-probe:{}", route.route_id)),
                        WaitKind::ProviderRecovery,
                        deadline,
                    );
                } else if lease.expires_at.is_some_and(|deadline| deadline > now) {
                    dispatch_wait(
                        &mut task,
                        OpaqueId::normalized(format!("route-probe:{}", route.route_id)),
                        WaitKind::ProviderRecovery,
                        lease.expires_at,
                    );
                } else {
                    route.state = PlannerRouteState::Unavailable;
                    route.probe_lease = None;
                    route.consecutive_outages = route.consecutive_outages.saturating_add(1);
                    route.next_probe_at =
                        route_deadline(&route.route_id, route.consecutive_outages, &policy, now);
                }
            }

            if task.external_wait.is_none() {
                if route.next_probe_at > now {
                    dispatch_wait(
                        &mut task,
                        OpaqueId::normalized(format!("route-unavailable:{}", route.route_id)),
                        WaitKind::ProviderRecovery,
                        Some(route.next_probe_at),
                    );
                } else {
                    task.runnable = Some(ActionKind::ProbeRoute);
                }
            }
        }
    }

    // Only the effect derived from the latest exact tuple, progress, binding,
    // and action remains executable. This also retires a pending probe as soon
    // as healthy route evidence arrives, preventing an orphaned journal record
    // from keeping the event-loop deadline hot after recovery.
    let expected_action = task.runnable;
    let expected_effect_id = expected_action.map(|action| {
        effect_id(
            &task.key,
            &task.progress_id,
            task.effect_binding.as_ref(),
            action,
            1,
        )
    });
    let active_probe_id = (route.state == PlannerRouteState::Probing)
        .then(|| {
            route
                .probe_lease
                .as_ref()
                .map(|lease| lease.effect_id.clone())
        })
        .flatten();
    for effect in state.effects.values_mut().filter(|effect| {
        let same_task =
            effect.task.graph_id == task.key.graph_id && effect.task.task_id == task.key.task_id;
        let stale_route_probe = route.state != PlannerRouteState::Probing
            && effect.action == ActionKind::ProbeRoute
            && matches!(
                effect.binding.as_ref(),
                Some(EffectBinding::Dispatch(binding)) if binding.route_id == route_id
            );
        (same_task || stale_route_probe)
            && matches!(
                effect.action,
                ActionKind::SpawnAttempt | ActionKind::ProbeRoute
            )
            && matches!(effect.status, EffectStatus::Issued)
            && expected_effect_id.as_ref() != Some(&effect.effect_id)
            && active_probe_id.as_ref() != Some(&effect.effect_id)
    }) {
        effect.status = EffectStatus::Acknowledged(AckOutcome::RejectedStale);
        state.effect_retry_deadlines.remove(&effect.effect_id);
        state.effect_retry_attempts.remove(&effect.effect_id);
    }

    if let Some(action) = expected_action {
        if let Some(effect) = issue_effect(state, &task, action) {
            if action == ActionKind::ProbeRoute {
                route.state = PlannerRouteState::Probing;
                route.probe_lease = Some(PlannerRouteProbeLease {
                    effect_id: effect.effect_id.clone(),
                    task_id: task.key.task_id.clone(),
                    expires_at: Some(now.saturating_add(policy.action_lease_seconds)),
                    spawned: false,
                });
            }
            emitted.push(effect);
        } else if action == ActionKind::ProbeRoute
            && route.probe_lease.is_none()
            && let Some(effect_id) = expected_effect_id.as_ref()
            && state
                .effects
                .get(effect_id)
                .is_some_and(|effect| matches!(effect.status, EffectStatus::Issued))
        {
            // Recover a missing projection lease from the durable logical
            // effect without re-emitting it; the journal owns execution.
            route.state = PlannerRouteState::Probing;
            route.probe_lease = Some(PlannerRouteProbeLease {
                effect_id: effect_id.clone(),
                task_id: task.key.task_id.clone(),
                expires_at: Some(now.saturating_add(policy.action_lease_seconds)),
                spawned: false,
            });
        }
    }
    if forward_count(&task) != 1 {
        violations.insert(if forward_count(&task) == 0 {
            ViolationCode::NoForwardDisposition
        } else {
            ViolationCode::MultipleForwardDispositions
        });
        state.fail_closed = true;
    }
    state.routes.insert(route_id, route);
    state.tasks.insert(task_state_id(&task.key), task);
}

/// Execute one pure planner transition. No external state is read or written.
#[must_use]
pub fn plan(
    state: &PlannerState,
    envelope: &ObservationEnvelope,
    ruleset: PlannerRuleset,
) -> PlannerStep {
    let mut next = state.clone();
    let mut emitted = Vec::new();
    let mut violations = BTreeSet::new();
    let observation_id = observation_id(envelope);

    if !(MIN_SUPPORTED_DAEMON_PLANNER_SCHEMA_VERSION..=DAEMON_PLANNER_SCHEMA_VERSION)
        .contains(&next.schema_version)
    {
        violations.insert(ViolationCode::SequenceConflict);
        next.fail_closed = true;
        return PlannerStep {
            sequence: envelope.sequence,
            state: next,
            effects: emitted,
            violations,
        };
    }

    if let Some(existing) = next.seen_observations.get(&envelope.sequence) {
        if existing != &observation_id {
            violations.insert(ViolationCode::SequenceConflict);
            next.fail_closed = true;
        }
        return PlannerStep {
            sequence: envelope.sequence,
            state: next,
            effects: emitted,
            violations,
        };
    }
    if next
        .last_sequence
        .is_some_and(|last| envelope.sequence <= last)
    {
        violations.insert(ViolationCode::SequenceRegression);
        next.fail_closed = true;
        return PlannerStep {
            sequence: envelope.sequence,
            state: next,
            effects: emitted,
            violations,
        };
    }

    next.logical_time = envelope.logical_time;
    next.last_sequence = Some(envelope.sequence);
    next.seen_observations
        .insert(envelope.sequence, observation_id);
    if next.seen_observations.len() > MAX_TRACE_OBSERVATIONS {
        let first_retained = envelope
            .sequence
            .saturating_sub(MAX_TRACE_OBSERVATIONS as u64 - 1);
        next.seen_observations
            .retain(|sequence, _| *sequence >= first_retained);
    }

    match &envelope.observation {
        Observation::Crash => {}
        Observation::Dispatch(observed) => {
            if !next.fail_closed {
                project_dispatch(
                    &mut next,
                    observed,
                    envelope.logical_time,
                    &mut emitted,
                    &mut violations,
                );
            }
        }
        Observation::ZeroOutput(observed) => {
            let observed = observed.as_ref();
            if observed.task.graph_id != next.graph_id {
                violations.insert(ViolationCode::CrossGraphIdentity);
                next.fail_closed = true;
            } else {
                next.zero_output
                    .insert(task_state_id(&observed.task), observed.clone());
            }
        }
        Observation::WorktreeSpawn(observed) => {
            if next.fail_closed {
                return PlannerStep {
                    sequence: envelope.sequence,
                    state: next,
                    effects: emitted,
                    violations,
                };
            }
            let observed = observed.as_ref();
            if observed.stale_owner.graph_id != next.graph_id
                || observed.current_attempt.graph_id != next.graph_id
            {
                violations.insert(ViolationCode::CrossGraphIdentity);
                next.fail_closed = true;
            } else {
                match &observed.owner {
                    // Authenticated liveness is a wait owned by that exact
                    // attempt. It is never converted into reclaim authority.
                    OwnerEvidence::AuthenticatedLive { .. } => {}
                    OwnerEvidence::ProvenDead { .. }
                        if observed.owner_token.is_present()
                            && observed.observer_state.is_present() =>
                    {
                        if let Some(effect) = issue_bound_effect(
                            &mut next,
                            &observed.stale_owner,
                            &observed.progress_id,
                            None,
                            None,
                            ActionKind::ReclaimRetainWorktree,
                        ) {
                            emitted.push(effect);
                        }
                        if let Some(effect) = issue_bound_effect(
                            &mut next,
                            &observed.current_attempt,
                            &observed.progress_id,
                            None,
                            Some(observed.stale_owner.clone()),
                            ActionKind::SpawnAttempt,
                        ) {
                            emitted.push(effect);
                        }
                    }
                    // Missing/unauthenticated ownership or missing retained
                    // evidence cannot be promoted into a destructive reclaim.
                    OwnerEvidence::None
                    | OwnerEvidence::Unauthenticated { .. }
                    | OwnerEvidence::ProvenDead { .. } => {
                        violations.insert(ViolationCode::UnprovenWorktreeOwnership);
                        next.fail_closed = true;
                    }
                }
            }
        }
        Observation::EffectAcknowledged { effect_id, outcome } => {
            if let Some(existing) = next.effects.get(effect_id).cloned() {
                // The first terminal acknowledgement is immutable. Delayed,
                // duplicated, or conflicting acknowledgements from an older
                // adapter execution cannot reopen or rewrite a settled effect.
                if !matches!(existing.status, EffectStatus::Acknowledged(_)) {
                    if *outcome == AckOutcome::Retryable {
                        if let Some(EffectBinding::Dispatch(binding)) = existing.binding.as_ref() {
                            let attempt = next
                                .effect_retry_attempts
                                .entry(effect_id.clone())
                                .or_insert(0);
                            let delay = bounded_delay(
                                binding.retry_base_seconds,
                                binding.retry_cap_seconds,
                                *attempt,
                            );
                            let jitter_window = (delay / binding.jitter_divisor.max(1)).max(1);
                            let jitter = stable_hash_u64(&format!("{}:{}", effect_id, *attempt))
                                % jitter_window;
                            *attempt = attempt.saturating_add(1);
                            next.effect_retry_deadlines.insert(
                                effect_id.clone(),
                                envelope
                                    .logical_time
                                    .saturating_add(delay)
                                    .saturating_add(jitter),
                            );
                            if let Some(effect) = next.effects.get_mut(effect_id) {
                                effect.status = EffectStatus::Issued;
                            }
                        } else {
                            if let Some(effect) = next.effects.get_mut(effect_id) {
                                effect.status = EffectStatus::Issued;
                                emitted.push(effect.clone());
                            }
                        }
                    } else {
                        if let Some(effect) = next.effects.get_mut(effect_id) {
                            effect.status = EffectStatus::Acknowledged(*outcome);
                        }
                        next.effect_retry_deadlines.remove(effect_id);
                        next.effect_retry_attempts.remove(effect_id);
                    }

                    if let Some(EffectBinding::Dispatch(binding)) = existing.binding.as_ref()
                        && let Some(route) = next.routes.get_mut(&binding.route_id)
                        && route
                            .probe_lease
                            .as_ref()
                            .is_some_and(|lease| lease.effect_id == *effect_id)
                    {
                        match outcome {
                            AckOutcome::Succeeded => {
                                if let Some(lease) = route.probe_lease.as_mut() {
                                    lease.spawned = true;
                                    lease.expires_at = None;
                                }
                                route.state = PlannerRouteState::Probing;
                            }
                            AckOutcome::Retryable => {
                                if let Some(lease) = route.probe_lease.as_mut() {
                                    lease.expires_at =
                                        next.effect_retry_deadlines.get(effect_id).copied();
                                }
                            }
                            AckOutcome::RejectedStale => {
                                route.state = PlannerRouteState::Unavailable;
                                route.probe_lease = None;
                                route.consecutive_outages =
                                    route.consecutive_outages.saturating_add(1);
                                route.next_probe_at = route_deadline(
                                    &route.route_id,
                                    route.consecutive_outages,
                                    &route.policy,
                                    envelope.logical_time,
                                );
                            }
                        }
                    }
                }
            } else {
                next.early_acknowledgements
                    .entry(effect_id.clone())
                    .or_insert(*outcome);
            }
        }
        Observation::Task(observed) => {
            if next.fail_closed {
                // The hold is an absorbing external-effect boundary. Adapters
                // may persist diagnostics/acks, but no later task observation
                // can authorize another mutation until an explicit future
                // recovery protocol creates a new planner checkpoint.
                return PlannerStep {
                    sequence: envelope.sequence,
                    state: next,
                    effects: emitted,
                    violations,
                };
            }
            let mut task = observed.as_ref().clone();
            if task.key.graph_id != next.graph_id {
                violations.insert(ViolationCode::CrossGraphIdentity);
                next.fail_closed = true;
                return PlannerStep {
                    sequence: envelope.sequence,
                    state: next,
                    effects: emitted,
                    violations,
                };
            }

            if ruleset == PlannerRuleset::Historical {
                violations.extend(task.incidents.iter().copied().map(ViolationCode::from));
            } else {
                next.repaired_incidents
                    .extend(task.incidents.iter().copied());
                corrected_incident_projection(&mut task, envelope.logical_time);
                corrected_failed_prerequisite_projection(&mut task, envelope.logical_time);
            }

            if task.unfinished {
                match forward_count(&task) {
                    0 => {
                        violations.insert(ViolationCode::NoForwardDisposition);
                        if ruleset == PlannerRuleset::Corrected {
                            task.scheduled = Some(ScheduledAction {
                                action: ActionKind::FailClosedHold,
                                deadline: envelope.logical_time,
                            });
                        }
                    }
                    1 => {}
                    _ => {
                        violations.insert(ViolationCode::MultipleForwardDispositions);
                        if ruleset == PlannerRuleset::Corrected {
                            task.owner = OwnerEvidence::None;
                            task.runnable = None;
                            task.external_wait = None;
                            task.scheduled = Some(ScheduledAction {
                                action: ActionKind::FailClosedHold,
                                deadline: envelope.logical_time,
                            });
                        }
                    }
                }
            }

            next.tasks.insert(task_state_id(&task.key), task.clone());
            let due_action = task.runnable.or_else(|| {
                task.scheduled
                    .as_ref()
                    .filter(|scheduled| scheduled.deadline <= envelope.logical_time)
                    .map(|scheduled| scheduled.action)
            });
            if let Some(action) = due_action
                && let Some(effect) = issue_effect(&mut next, &task, action)
            {
                emitted.push(effect);
            }
            if !violations.is_empty() {
                next.fail_closed = true;
            }
        }
    }

    PlannerStep {
        sequence: envelope.sequence,
        state: next,
        effects: emitted,
        violations,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionPolicy {
    TypedIdentifiersAndDigestsOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionTrace {
    pub trace_schema_version: u16,
    pub planner_schema_version: u16,
    pub redaction: RedactionPolicy,
    pub ruleset: PlannerRuleset,
    pub initial_state: PlannerState,
    pub observations: Vec<ObservationEnvelope>,
}

impl DecisionTrace {
    pub fn validate(&self) -> Result<()> {
        if !(MIN_SUPPORTED_DAEMON_TRACE_SCHEMA_VERSION..=DAEMON_TRACE_SCHEMA_VERSION)
            .contains(&self.trace_schema_version)
        {
            bail!(
                "unsupported daemon trace schema {}",
                self.trace_schema_version
            );
        }
        if self.planner_schema_version != self.initial_state.schema_version
            || !(MIN_SUPPORTED_DAEMON_PLANNER_SCHEMA_VERSION..=DAEMON_PLANNER_SCHEMA_VERSION)
                .contains(&self.planner_schema_version)
        {
            bail!("unsupported or mismatched daemon planner schema");
        }
        if self.trace_schema_version != self.planner_schema_version {
            bail!("daemon trace/planner schema mismatch");
        }
        if self.observations.len() > MAX_TRACE_OBSERVATIONS {
            bail!("daemon replay trace exceeds bounded observation limit");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub trace_schema_version: u16,
    pub planner_schema_version: u16,
    pub steps: Vec<PlannerStep>,
    pub final_state: PlannerState,
}

/// Replay a bounded trace entirely offline. This function has no I/O.
pub fn replay(trace: &DecisionTrace) -> Result<ReplayReport> {
    trace.validate()?;
    let mut state = trace.initial_state.clone();
    let mut steps = Vec::with_capacity(trace.observations.len());
    for observation in &trace.observations {
        let step = plan(&state, observation, trace.ruleset);
        state = step.state.clone();
        steps.push(step);
    }
    Ok(ReplayReport {
        trace_schema_version: trace.trace_schema_version,
        planner_schema_version: trace.planner_schema_version,
        steps,
        final_state: state,
    })
}

pub fn replay_bytes(trace: &DecisionTrace) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&replay(trace)?)?)
}

/// Production adapters submit this type instead of constructing sequence
/// numbers. The store turns the adapter's observed wall/logical timestamp into
/// a strictly increasing durable logical clock and allocates the next sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedObservation {
    pub observed_at: u64,
    pub observation: Observation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectExecutionPhase {
    Issued,
    Executing,
    Executed { outcome: AckOutcome },
    Acknowledged { outcome: AckOutcome },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectExecutionRecord {
    pub effect: PlannedEffect,
    pub phase: EffectExecutionPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectExecutionJournal {
    schema_version: u16,
    #[serde(default)]
    records: BTreeMap<OpaqueId, EffectExecutionRecord>,
}

impl Default for EffectExecutionJournal {
    fn default() -> Self {
        Self {
            schema_version: EFFECT_JOURNAL_SCHEMA_VERSION,
            records: BTreeMap::new(),
        }
    }
}

/// Recovery work is ordered: an already-recorded execution is acknowledged
/// without running the physical operation again; issued/executing effects are
/// retried with the same stable effect ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectReplay {
    Execute(PlannedEffect),
    Acknowledge {
        effect: PlannedEffect,
        outcome: AckOutcome,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannerStatusProjection {
    pub schema_version: u16,
    pub graph_id: OpaqueId,
    pub logical_time: u64,
    pub last_sequence: Option<u64>,
    pub next_sequence: Option<u64>,
    pub earliest_deadline: Option<String>,
    pub normalized_tasks: BTreeMap<OpaqueId, TaskObservation>,
    pub routes: BTreeMap<OpaqueId, PlannerRouteProjection>,
    pub zero_output: BTreeMap<OpaqueId, ZeroOutputObservation>,
    pub effects: BTreeMap<OpaqueId, EffectExecutionRecord>,
    pub legacy_convergence: Option<Box<LegacyConvergenceImport>>,
    pub fail_closed: bool,
}

struct PlannerMutationLock {
    file: File,
}

impl PlannerMutationLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let root = dir.join("service");
        std::fs::create_dir_all(&root)?;
        let path = root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        #[cfg(unix)]
        loop {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).with_context(|| format!("failed to lock {}", path.display()));
            }
        }
        Ok(Self { file })
    }

    fn acquire_shared_existing(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join("service").join(LOCK_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let file = OpenOptions::new()
            .read(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        #[cfg(unix)]
        loop {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
            if result == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).with_context(|| format!("failed to lock {}", path.display()));
            }
        }
        Ok(Some(Self { file }))
    }
}

impl Drop for PlannerMutationLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// Durable production boundary. The decision trace is scheduling authority,
/// the effect journal is execution/acknowledgement authority, and the state
/// file is only a rebuildable normalized cache. Every issued effect is present
/// in both durable authorities before it can be returned to an adapter.
pub struct PlannerStore {
    dir: PathBuf,
    trace: DecisionTrace,
    state: PlannerState,
    effect_journal: EffectExecutionJournal,
}

impl PlannerStore {
    pub fn open(dir: &Path, graph_id: OpaqueId) -> Result<Self> {
        let _lock = PlannerMutationLock::acquire(dir)?;
        Self::load_locked(dir, graph_id)
    }

    fn load_locked(dir: &Path, graph_id: OpaqueId) -> Result<Self> {
        let root = dir.join("service");
        let trace_path = root.join(TRACE_FILE);
        let state_path = root.join(STATE_FILE);
        let trace_existed = trace_path.exists();
        let state_existed = state_path.exists();
        let mut migrated = false;
        let mut trace = if trace_existed {
            serde_json::from_slice::<DecisionTrace>(&std::fs::read(&trace_path)?)
                .with_context(|| format!("failed to parse {}", trace_path.display()))?
        } else if state_existed {
            let state = serde_json::from_slice::<PlannerState>(&std::fs::read(&state_path)?)
                .with_context(|| format!("failed to parse {}", state_path.display()))?;
            if !(MIN_SUPPORTED_DAEMON_PLANNER_SCHEMA_VERSION..=DAEMON_PLANNER_SCHEMA_VERSION)
                .contains(&state.schema_version)
            {
                bail!("unsupported daemon planner schema {}", state.schema_version);
            }
            migrated = true;
            DecisionTrace {
                trace_schema_version: state.schema_version,
                planner_schema_version: state.schema_version,
                redaction: RedactionPolicy::TypedIdentifiersAndDigestsOnly,
                ruleset: PlannerRuleset::Corrected,
                initial_state: state,
                observations: Vec::new(),
            }
        } else {
            DecisionTrace {
                trace_schema_version: DAEMON_TRACE_SCHEMA_VERSION,
                planner_schema_version: DAEMON_PLANNER_SCHEMA_VERSION,
                redaction: RedactionPolicy::TypedIdentifiersAndDigestsOnly,
                ruleset: PlannerRuleset::Corrected,
                initial_state: PlannerState::new(graph_id.clone()),
                observations: Vec::new(),
            }
        };
        trace.validate()?;
        if trace.initial_state.graph_id != graph_id {
            bail!("daemon planner graph identity mismatch");
        }

        if trace.trace_schema_version != DAEMON_TRACE_SCHEMA_VERSION
            || trace.planner_schema_version != DAEMON_PLANNER_SCHEMA_VERSION
            || trace.initial_state.schema_version != DAEMON_PLANNER_SCHEMA_VERSION
        {
            trace.trace_schema_version = DAEMON_TRACE_SCHEMA_VERSION;
            trace.planner_schema_version = DAEMON_PLANNER_SCHEMA_VERSION;
            trace.initial_state.schema_version = DAEMON_PLANNER_SCHEMA_VERSION;
            migrated = true;
        }
        if !trace.initial_state.convergence_import_complete {
            let convergence_path = super::convergence::ConvergenceState::path(dir);
            if convergence_path.exists() {
                // Load the typed legacy schema and copy it exactly. No `now`,
                // policy, jitter, route-health read, or reconciliation occurs
                // here, so existing deadlines and backoff cannot reset.
                let convergence = super::convergence::ConvergenceState::load(dir)?;
                trace.initial_state.legacy_convergence = Some(Box::new(
                    LegacyConvergenceImport::from_legacy(&convergence)?,
                ));
                trace.initial_state.convergence_import_complete = true;
                migrated = true;
            }
        }

        let state = replay(&trace)?.final_state;
        let state_cache_changed = match std::fs::read(&state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PlannerState>(&bytes).ok())
        {
            Some(cached) => cached != state,
            None => trace_existed || state_existed || migrated,
        };
        let journal_path = root.join(EFFECT_JOURNAL_FILE);
        let journal_existed = journal_path.exists();
        let mut effect_journal = if journal_existed {
            let journal =
                serde_json::from_slice::<EffectExecutionJournal>(&std::fs::read(&journal_path)?)
                    .with_context(|| format!("failed to parse {}", journal_path.display()))?;
            if journal.schema_version != EFFECT_JOURNAL_SCHEMA_VERSION {
                bail!(
                    "unsupported planner effect journal schema {}",
                    journal.schema_version
                );
            }
            journal
        } else {
            EffectExecutionJournal::default()
        };
        let journal_changed = sync_effect_journal(&state, &mut effect_journal)?
            || (!journal_existed && (trace_existed || state_existed || migrated));
        let store = Self {
            dir: dir.to_path_buf(),
            trace,
            state,
            effect_journal,
        };
        if migrated || journal_changed || state_cache_changed {
            store.persist_all()?;
        }
        Ok(store)
    }

    fn refresh_locked(&mut self) -> Result<()> {
        let graph_id = self.state.graph_id.clone();
        *self = Self::load_locked(&self.dir, graph_id)?;
        Ok(())
    }

    fn persist_all(&self) -> Result<()> {
        let trace_path = self.trace_path();
        crate::atomic_file::write_atomic(&trace_path, serde_json::to_vec_pretty(&self.trace)?)
            .with_context(|| format!("failed to persist {}", trace_path.display()))?;
        crate::atomic_file::write_atomic(
            &self.effect_journal_path(),
            serde_json::to_vec_pretty(&self.effect_journal)?,
        )
        .with_context(|| format!("failed to persist {}", self.effect_journal_path().display()))?;
        crate::atomic_file::write_atomic(
            &self.state_path(),
            serde_json::to_vec_pretty(&self.state)?,
        )
        .with_context(|| format!("failed to persist {}", self.state_path().display()))
    }

    pub fn state(&self) -> &PlannerState {
        &self.state
    }

    pub fn trace_path(&self) -> PathBuf {
        self.dir.join("service").join(TRACE_FILE)
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join("service").join(STATE_FILE)
    }

    pub fn effect_journal_path(&self) -> PathBuf {
        self.dir.join("service").join(EFFECT_JOURNAL_FILE)
    }

    fn next_envelope(&self, input: TypedObservation) -> Result<ObservationEnvelope> {
        let sequence = match self.state.last_sequence {
            Some(last) => last.checked_add(1).context("planner sequence exhausted")?,
            None => 1,
        };
        let logical_floor = self
            .state
            .logical_time
            .checked_add(1)
            .context("planner logical time exhausted")?;
        Ok(ObservationEnvelope {
            sequence,
            logical_time: input.observed_at.max(logical_floor),
            observation: input.observation,
        })
    }

    /// Normalize a typed production observation using the current durable
    /// allocator without persisting it. `observe` repeats this under the writer
    /// lock and is the authoritative allocation path.
    pub fn normalize_observation(&self, input: TypedObservation) -> Result<ObservationEnvelope> {
        self.next_envelope(input)
    }

    /// Allocate sequence/logical time and durably reduce one typed production
    /// observation. Callers cannot reset either counter after restart.
    pub fn observe(&mut self, input: TypedObservation) -> Result<PlannerStep> {
        let _lock = PlannerMutationLock::acquire(&self.dir)?;
        self.refresh_locked()?;
        let envelope = self.next_envelope(input)?;
        self.apply_locked(envelope)
    }

    /// Low-level replay/test path for an already-normalized envelope. Production
    /// adapters should use `observe` so allocation is store-owned.
    pub fn apply(&mut self, observation: ObservationEnvelope) -> Result<PlannerStep> {
        let _lock = PlannerMutationLock::acquire(&self.dir)?;
        self.refresh_locked()?;
        self.apply_locked(observation)
    }

    fn apply_locked(&mut self, observation: ObservationEnvelope) -> Result<PlannerStep> {
        let (step, _) = plan_guarded(&self.dir, &self.state, &observation)?;
        let mut trace = self.trace.clone();
        trace.observations.push(observation.clone());
        if trace.observations.len() > MAX_TRACE_OBSERVATIONS {
            let split = trace.observations.len() - MAX_TRACE_OBSERVATIONS;
            let prefix = DecisionTrace {
                observations: trace.observations[..split].to_vec(),
                ..trace.clone()
            };
            trace.initial_state = replay(&prefix)?.final_state;
            trace.initial_state.seen_observations.clear();
            trace.observations = trace.observations[split..].to_vec();
        }
        let mut journal = self.effect_journal.clone();
        sync_effect_journal(&step.state, &mut journal)?;
        if let Observation::EffectAcknowledged { effect_id, outcome } = &observation.observation
            && *outcome == AckOutcome::Retryable
            && let Some(record) = journal.records.get_mut(effect_id)
        {
            record.phase = EffectExecutionPhase::Issued;
        }

        // Trace first: a crash here reconstructs the issued journal record on
        // open. Journal second: no effect is returned before its execution
        // state is durable. The normalized state cache remains rebuildable.
        crate::atomic_file::write_atomic(&self.trace_path(), serde_json::to_vec_pretty(&trace)?)
            .with_context(|| format!("failed to persist {}", self.trace_path().display()))?;
        crate::atomic_file::write_atomic(
            &self.effect_journal_path(),
            serde_json::to_vec_pretty(&journal)?,
        )
        .with_context(|| format!("failed to persist {}", self.effect_journal_path().display()))?;
        crate::atomic_file::write_atomic(
            &self.state_path(),
            serde_json::to_vec_pretty(&step.state)?,
        )
        .with_context(|| format!("failed to persist {}", self.state_path().display()))?;
        self.trace = trace;
        self.effect_journal = journal;
        self.state = step.state.clone();
        Ok(step)
    }

    /// Ordered restart work. `Execute` always carries the original stable ID;
    /// `Acknowledge` proves execution was already recorded and must not rerun.
    pub fn replayable_effects(&self) -> Vec<EffectReplay> {
        self.effect_journal
            .records
            .values()
            .filter(|record| {
                self.state
                    .effect_retry_deadlines
                    .get(&record.effect.effect_id)
                    .is_none_or(|deadline| *deadline <= self.state.logical_time)
            })
            .filter_map(|record| match record.phase {
                EffectExecutionPhase::Issued | EffectExecutionPhase::Executing => {
                    Some(EffectReplay::Execute(record.effect.clone()))
                }
                EffectExecutionPhase::Executed { outcome } => Some(EffectReplay::Acknowledge {
                    effect: record.effect.clone(),
                    outcome,
                }),
                EffectExecutionPhase::Acknowledged { .. } => None,
            })
            .collect()
    }

    /// Persist the execution boundary before invoking an adapter. Repeating
    /// this call after a crash is inert and returns the same effect.
    pub fn mark_effect_execution_started(&mut self, effect_id: &OpaqueId) -> Result<PlannedEffect> {
        let _lock = PlannerMutationLock::acquire(&self.dir)?;
        self.refresh_locked()?;
        let (effect, changed) = {
            let record = self
                .effect_journal
                .records
                .get_mut(effect_id)
                .with_context(|| format!("unknown planner effect {effect_id}"))?;
            let changed = match record.phase {
                EffectExecutionPhase::Issued => {
                    record.phase = EffectExecutionPhase::Executing;
                    true
                }
                EffectExecutionPhase::Executing => false,
                EffectExecutionPhase::Executed { .. } => {
                    bail!("planner effect {effect_id} already executed; acknowledge it")
                }
                EffectExecutionPhase::Acknowledged { .. } => {
                    bail!("planner effect {effect_id} is already acknowledged")
                }
            };
            (record.effect.clone(), changed)
        };
        if changed {
            crate::atomic_file::write_atomic(
                &self.effect_journal_path(),
                serde_json::to_vec_pretty(&self.effect_journal)?,
            )?;
        }
        Ok(effect)
    }

    /// Persist the physical result before acknowledgement. A duplicate result
    /// with the same outcome is inert; a conflicting result fails closed.
    pub fn record_effect_execution(
        &mut self,
        effect_id: &OpaqueId,
        outcome: AckOutcome,
    ) -> Result<()> {
        let _lock = PlannerMutationLock::acquire(&self.dir)?;
        self.refresh_locked()?;
        let record = self
            .effect_journal
            .records
            .get_mut(effect_id)
            .with_context(|| format!("unknown planner effect {effect_id}"))?;
        match record.phase {
            EffectExecutionPhase::Issued | EffectExecutionPhase::Executing => {
                record.phase = EffectExecutionPhase::Executed { outcome };
                crate::atomic_file::write_atomic(
                    &self.effect_journal_path(),
                    serde_json::to_vec_pretty(&self.effect_journal)?,
                )?;
            }
            EffectExecutionPhase::Executed { outcome: existing }
            | EffectExecutionPhase::Acknowledged { outcome: existing }
                if existing == outcome => {}
            EffectExecutionPhase::Executed { outcome: existing }
            | EffectExecutionPhase::Acknowledged { outcome: existing } => {
                bail!(
                    "conflicting execution outcome for {effect_id}: {existing:?} versus {outcome:?}"
                )
            }
        }
        Ok(())
    }

    /// Acknowledge a recorded execution through the same durable observation
    /// allocator. Trace acknowledgement is persisted before the journal is
    /// marked complete, so either crash ordering repairs deterministically.
    pub fn acknowledge_recorded_effect(
        &mut self,
        effect_id: &OpaqueId,
        observed_at: u64,
    ) -> Result<PlannerStep> {
        let _lock = PlannerMutationLock::acquire(&self.dir)?;
        self.refresh_locked()?;
        let outcome = match self.effect_journal.records.get(effect_id).map(|r| &r.phase) {
            Some(EffectExecutionPhase::Executed { outcome }) => *outcome,
            Some(EffectExecutionPhase::Acknowledged { .. }) => {
                return Ok(PlannerStep {
                    sequence: self.state.last_sequence.unwrap_or(0),
                    state: self.state.clone(),
                    effects: Vec::new(),
                    violations: BTreeSet::new(),
                });
            }
            Some(EffectExecutionPhase::Issued | EffectExecutionPhase::Executing) => {
                bail!("planner effect {effect_id} has no recorded execution")
            }
            None => bail!("unknown planner effect {effect_id}"),
        };
        let envelope = self.next_envelope(TypedObservation {
            observed_at,
            observation: Observation::EffectAcknowledged {
                effect_id: effect_id.clone(),
                outcome,
            },
        })?;
        self.apply_locked(envelope)
    }

    pub fn earliest_deadline(&self) -> Option<DateTime<Utc>> {
        earliest_deadline(&self.state, &self.effect_journal)
    }

    pub fn status_projection(&self) -> PlannerStatusProjection {
        status_projection(&self.state, &self.effect_journal)
    }

    /// Read status without creating a lock, running a migration, importing
    /// convergence, or rewriting a cache. Atomic writer renames ensure each
    /// individual source is complete; the trace remains authoritative.
    pub fn read_status(dir: &Path) -> Result<Option<PlannerStatusProjection>> {
        let _lock = PlannerMutationLock::acquire_shared_existing(dir)?;
        let root = dir.join("service");
        let trace_path = root.join(TRACE_FILE);
        let state_path = root.join(STATE_FILE);
        let state = if trace_path.exists() {
            let trace: DecisionTrace = serde_json::from_slice(&std::fs::read(&trace_path)?)
                .with_context(|| format!("failed to parse {}", trace_path.display()))?;
            replay(&trace)?.final_state
        } else if state_path.exists() {
            serde_json::from_slice(&std::fs::read(&state_path)?)
                .with_context(|| format!("failed to parse {}", state_path.display()))?
        } else {
            return Ok(None);
        };
        let journal_path = root.join(EFFECT_JOURNAL_FILE);
        let mut journal = if journal_path.exists() {
            serde_json::from_slice(&std::fs::read(&journal_path)?)
                .with_context(|| format!("failed to parse {}", journal_path.display()))?
        } else {
            EffectExecutionJournal::default()
        };
        sync_effect_journal(&state, &mut journal)?;
        Ok(Some(status_projection(&state, &journal)))
    }

    /// The daemon event loop's sole logical deadline source.
    pub fn read_earliest_deadline(dir: &Path) -> Result<Option<DateTime<Utc>>> {
        Ok(Self::read_status(dir)?
            .and_then(|status| status.earliest_deadline)
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&Utc)))
    }
}

fn canonical_issued_effect(effect: &PlannedEffect) -> PlannedEffect {
    let mut effect = effect.clone();
    effect.status = EffectStatus::Issued;
    effect
}

fn sync_effect_journal(state: &PlannerState, journal: &mut EffectExecutionJournal) -> Result<bool> {
    if journal.schema_version != EFFECT_JOURNAL_SCHEMA_VERSION {
        bail!(
            "unsupported planner effect journal schema {}",
            journal.schema_version
        );
    }
    let before = journal.clone();
    for (effect_id, record) in &journal.records {
        if effect_id != &record.effect.effect_id {
            bail!("planner effect journal key/id mismatch for {effect_id}");
        }
        if !state.effects.contains_key(effect_id) {
            bail!("planner effect journal contains unknown effect {effect_id}");
        }
    }
    for (effect_id, effect) in &state.effects {
        let canonical = canonical_issued_effect(effect);
        let record =
            journal
                .records
                .entry(effect_id.clone())
                .or_insert_with(|| EffectExecutionRecord {
                    effect: canonical.clone(),
                    phase: match effect.status {
                        EffectStatus::Issued => EffectExecutionPhase::Issued,
                        EffectStatus::Acknowledged(outcome) => {
                            EffectExecutionPhase::Acknowledged { outcome }
                        }
                    },
                });
        if record.effect != canonical {
            bail!("planner effect journal payload mismatch for {effect_id}");
        }
        if let EffectStatus::Acknowledged(outcome) = effect.status {
            record.phase = EffectExecutionPhase::Acknowledged { outcome };
        }
    }
    Ok(*journal != before)
}

fn logical_datetime(value: u64) -> Option<DateTime<Utc>> {
    i64::try_from(value)
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

fn earliest_deadline(
    state: &PlannerState,
    journal: &EffectExecutionJournal,
) -> Option<DateTime<Utc>> {
    let mut deadlines = Vec::new();
    for record in journal
        .records
        .values()
        .filter(|record| !matches!(record.phase, EffectExecutionPhase::Acknowledged { .. }))
    {
        let deadline = state
            .effect_retry_deadlines
            .get(&record.effect.effect_id)
            .copied()
            .unwrap_or(state.logical_time);
        if let Some(deadline) = logical_datetime(deadline) {
            deadlines.push(deadline);
        }
    }
    for task in state.tasks.values().filter(|task| task.unfinished) {
        if let Some(wait) = task.external_wait.as_ref()
            && let Some(deadline) = wait.deadline.and_then(logical_datetime)
        {
            deadlines.push(deadline);
        }
        if let Some(scheduled) = task.scheduled.as_ref() {
            let settled = state.effects.values().any(|effect| {
                effect.task == task.key
                    && effect.action == scheduled.action
                    && matches!(effect.status, EffectStatus::Acknowledged(_))
            });
            if !settled && let Some(deadline) = logical_datetime(scheduled.deadline) {
                deadlines.push(deadline);
            }
        }
    }
    if let Some(legacy) = state.legacy_convergence.as_ref()
        && let Some(deadline) = legacy.earliest_wake()
    {
        deadlines.push(deadline);
    }
    deadlines.into_iter().min()
}

fn status_projection(
    state: &PlannerState,
    journal: &EffectExecutionJournal,
) -> PlannerStatusProjection {
    PlannerStatusProjection {
        schema_version: state.schema_version,
        graph_id: state.graph_id.clone(),
        logical_time: state.logical_time,
        last_sequence: state.last_sequence,
        next_sequence: state
            .last_sequence
            .map_or(Some(1), |last| last.checked_add(1)),
        earliest_deadline: earliest_deadline(state, journal).map(|value| value.to_rfc3339()),
        normalized_tasks: state.tasks.clone(),
        routes: state.routes.clone(),
        zero_output: state.zero_output.clone(),
        effects: journal.records.clone(),
        legacy_convergence: state.legacy_convergence.clone(),
        fail_closed: state.fail_closed,
    }
}

fn replay_dir(dir: &Path) -> PathBuf {
    dir.join("service").join("replay")
}

/// Persist the minimal replay input before returning the fail-closed state.
/// The bundle contains only typed identifiers/enums and is retention-bounded.
pub fn plan_guarded(
    dir: &Path,
    state: &PlannerState,
    observation: &ObservationEnvelope,
) -> Result<(PlannerStep, Option<PathBuf>)> {
    let step = plan(state, observation, PlannerRuleset::Corrected);
    if step.violations.is_empty() {
        return Ok((step, None));
    }
    let trace = DecisionTrace {
        trace_schema_version: DAEMON_TRACE_SCHEMA_VERSION,
        planner_schema_version: DAEMON_PLANNER_SCHEMA_VERSION,
        redaction: RedactionPolicy::TypedIdentifiersAndDigestsOnly,
        ruleset: PlannerRuleset::Corrected,
        initial_state: state.clone(),
        observations: vec![observation.clone()],
    };
    let root = replay_dir(dir);
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create replay directory {}", root.display()))?;
    let digest = blake3::hash(&serde_json::to_vec(&trace)?).to_hex();
    let path = root.join(format!("violation-{digest}.json"));
    crate::atomic_file::write_atomic(&path, serde_json::to_vec_pretty(&trace)?)
        .with_context(|| format!("failed to persist replay bundle {}", path.display()))?;

    let mut entries = std::fs::read_dir(&root)?
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("violation-")
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let remove_count = entries.len().saturating_sub(MAX_REPLAY_BUNDLES);
    for old in entries.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(old.path());
    }
    Ok((step, Some(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).unwrap()
    }

    fn key(task: &str, attempt: &str) -> TaskKey {
        TaskKey {
            graph_id: id("graph-a"),
            task_id: id(task),
            generation: 1,
            attempt_id: id(attempt),
            fence: 7,
        }
    }

    fn runnable(sequence: u64, task: &str, attempt: &str) -> ObservationEnvelope {
        ObservationEnvelope {
            sequence,
            logical_time: sequence,
            observation: Observation::Task(Box::new(TaskObservation {
                key: key(task, attempt),
                progress_id: id("progress-1"),
                unfinished: true,
                owner: OwnerEvidence::None,
                runnable: Some(ActionKind::SpawnAttempt),
                external_wait: None,
                scheduled: None,
                effect_binding: None,
                incidents: BTreeSet::new(),
                failed_prerequisite: None,
            })),
        }
    }

    #[test]
    fn replay_is_byte_deterministic_and_duplicate_observation_is_inert() {
        let state = PlannerState::new(id("graph-a"));
        let observation = runnable(1, "task-a", "attempt-a");
        let trace = DecisionTrace {
            trace_schema_version: DAEMON_TRACE_SCHEMA_VERSION,
            planner_schema_version: DAEMON_PLANNER_SCHEMA_VERSION,
            redaction: RedactionPolicy::TypedIdentifiersAndDigestsOnly,
            ruleset: PlannerRuleset::Corrected,
            initial_state: state,
            observations: vec![observation.clone(), observation],
        };
        let first = replay_bytes(&trace).unwrap();
        let second = replay_bytes(&trace).unwrap();
        assert_eq!(first, second);
        let report = replay(&trace).unwrap();
        assert_eq!(report.steps[0].effects.len(), 1);
        assert!(report.steps[1].effects.is_empty());
    }

    #[test]
    fn crash_and_reordered_duplicate_ack_preserve_one_logical_effect() {
        let mut state = PlannerState::new(id("graph-a"));
        let issued = plan(
            &state,
            &runnable(1, "task-a", "attempt-a"),
            PlannerRuleset::Corrected,
        );
        let effect_id = issued.effects[0].effect_id.clone();
        state = issued.state;
        let crash = ObservationEnvelope {
            sequence: 2,
            logical_time: 2,
            observation: Observation::Crash,
        };
        state = plan(&state, &crash, PlannerRuleset::Corrected).state;
        let duplicate_task = runnable(3, "task-a", "attempt-a");
        let replayed = plan(&state, &duplicate_task, PlannerRuleset::Corrected);
        assert!(replayed.effects.is_empty());
        state = replayed.state;
        for sequence in [4, 5] {
            let ack = ObservationEnvelope {
                sequence,
                logical_time: sequence,
                observation: Observation::EffectAcknowledged {
                    effect_id: effect_id.clone(),
                    outcome: AckOutcome::Succeeded,
                },
            };
            state = plan(&state, &ack, PlannerRuleset::Corrected).state;
        }
        assert_eq!(state.effects.len(), 1);
        assert_eq!(
            state.effects[&effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::Succeeded)
        );
    }

    #[test]
    fn terminal_acknowledgement_is_immutable_under_reordered_conflicting_acks() {
        let issued = plan(
            &PlannerState::new(id("graph-a")),
            &runnable(1, "task-a", "attempt-a"),
            PlannerRuleset::Corrected,
        );
        let effect_id = issued.effects[0].effect_id.clone();
        let mut state = issued.state;
        for (sequence, outcome) in [
            (2, AckOutcome::Succeeded),
            (3, AckOutcome::Retryable),
            (4, AckOutcome::RejectedStale),
        ] {
            let step = plan(
                &state,
                &ObservationEnvelope {
                    sequence,
                    logical_time: sequence,
                    observation: Observation::EffectAcknowledged {
                        effect_id: effect_id.clone(),
                        outcome,
                    },
                },
                PlannerRuleset::Corrected,
            );
            if sequence > 2 {
                assert!(step.effects.is_empty());
            }
            state = step.state;
        }
        assert_eq!(state.effects.len(), 1);
        assert_eq!(
            state.effects[&effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::Succeeded)
        );
    }

    #[test]
    fn bounded_two_task_two_attempt_enumeration_preserves_forward_exhaustiveness() {
        for live_owner in [false, true] {
            for runnable_bit in [false, true] {
                for waiting in [false, true] {
                    for scheduled in [false, true] {
                        let mut state = PlannerState::new(id("graph-a"));
                        for (index, attempt) in ["attempt-a", "attempt-b"].iter().enumerate() {
                            let envelope = ObservationEnvelope {
                                sequence: index as u64 + 1,
                                logical_time: 10,
                                observation: Observation::Task(Box::new(TaskObservation {
                                    key: key(if index == 0 { "task-a" } else { "task-b" }, attempt),
                                    progress_id: id("progress-enumerated"),
                                    unfinished: true,
                                    owner: if live_owner {
                                        OwnerEvidence::AuthenticatedLive {
                                            actor_id: id("actor"),
                                            lease_id: id("lease"),
                                        }
                                    } else {
                                        OwnerEvidence::None
                                    },
                                    runnable: runnable_bit.then_some(ActionKind::SpawnAttempt),
                                    external_wait: waiting.then(|| ExternalWait {
                                        wait_id: id("wait"),
                                        kind: WaitKind::HumanInput,
                                        deadline: None,
                                    }),
                                    scheduled: scheduled.then_some(ScheduledAction {
                                        action: ActionKind::CleanupFinish,
                                        deadline: 11,
                                    }),
                                    effect_binding: None,
                                    incidents: BTreeSet::new(),
                                    failed_prerequisite: None,
                                })),
                            };
                            state = plan(&state, &envelope, PlannerRuleset::Corrected).state;
                        }
                        for task in state.tasks.values() {
                            assert_eq!(forward_count(task), 1);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn acknowledgement_reordered_before_issue_is_reconciled_without_execution() {
        let mut state = PlannerState::new(id("graph-a"));
        let task = match runnable(2, "task-a", "attempt-a").observation {
            Observation::Task(task) => task,
            _ => unreachable!(),
        };
        let expected_id = effect_id(
            &task.key,
            &task.progress_id,
            None,
            ActionKind::SpawnAttempt,
            1,
        );
        state = plan(
            &state,
            &ObservationEnvelope {
                sequence: 1,
                logical_time: 1,
                observation: Observation::EffectAcknowledged {
                    effect_id: expected_id.clone(),
                    outcome: AckOutcome::Succeeded,
                },
            },
            PlannerRuleset::Corrected,
        )
        .state;
        let issued = plan(
            &state,
            &ObservationEnvelope {
                sequence: 2,
                logical_time: 2,
                observation: Observation::Task(task),
            },
            PlannerRuleset::Corrected,
        );
        assert!(issued.effects.is_empty());
        assert_eq!(
            issued.state.effects[&expected_id].status,
            EffectStatus::Acknowledged(AckOutcome::Succeeded)
        );
    }

    #[test]
    fn retryable_ack_reissues_same_physical_effect_without_duplicate_logical_effect() {
        let mut state = PlannerState::new(id("graph-a"));
        let issued = plan(
            &state,
            &runnable(1, "task-a", "attempt-a"),
            PlannerRuleset::Corrected,
        );
        let effect = issued.effects[0].clone();
        state = issued.state;
        let retry = plan(
            &state,
            &ObservationEnvelope {
                sequence: 2,
                logical_time: 2,
                observation: Observation::EffectAcknowledged {
                    effect_id: effect.effect_id.clone(),
                    outcome: AckOutcome::Retryable,
                },
            },
            PlannerRuleset::Corrected,
        );
        assert_eq!(retry.effects.len(), 1);
        assert_eq!(retry.effects[0].effect_id, effect.effect_id);
        assert_eq!(retry.state.effects.len(), 1);
    }

    #[test]
    fn persisted_issue_boundary_replays_after_cache_loss() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let step = store.apply(runnable(1, "task-a", "attempt-a")).unwrap();
        assert_eq!(step.effects.len(), 1);
        std::fs::remove_file(store.state_path()).unwrap();
        let reopened = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(reopened.state(), &step.state);
        assert_eq!(reopened.state().effects.len(), 1);
    }

    #[test]
    fn persisted_issue_execute_ack_boundary_matrix_is_exactly_once_logically() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert!(store.state().effects.is_empty()); // killed before issue

        let issued = store.apply(runnable(1, "task-a", "attempt-a")).unwrap();
        let effect = issued.effects[0].clone();
        std::fs::remove_file(store.state_path()).unwrap(); // kill after issue, before execute
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(store.state().effects.len(), 1);

        let mut same = runnable(2, "task-a", "attempt-a");
        same.logical_time = 2;
        assert!(store.apply(same).unwrap().effects.is_empty()); // lost execute response
        let acked = store
            .apply(ObservationEnvelope {
                sequence: 3,
                logical_time: 3,
                observation: Observation::EffectAcknowledged {
                    effect_id: effect.effect_id.clone(),
                    outcome: AckOutcome::Succeeded,
                },
            })
            .unwrap();
        assert!(acked.effects.is_empty());
        std::fs::remove_file(store.state_path()).unwrap(); // kill after ack persistence
        let store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(store.state().effects.len(), 1);
        assert_eq!(
            store.state().effects[&effect.effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::Succeeded)
        );
    }

    #[test]
    fn authoritative_trace_rebuilds_missing_empty_journal_and_state_cache() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        store
            .observe(TypedObservation {
                observed_at: 50,
                observation: Observation::Crash,
            })
            .unwrap();
        let expected = store.state().clone();
        std::fs::remove_file(store.state_path()).unwrap();
        std::fs::remove_file(store.effect_journal_path()).unwrap();
        drop(store);

        let reopened = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(reopened.state(), &expected);
        assert!(reopened.state_path().exists());
        assert!(reopened.effect_journal_path().exists());
    }

    #[test]
    fn durable_trace_compacts_to_bounded_checkpoint_and_replays_identically() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        for sequence in 1..=(MAX_TRACE_OBSERVATIONS as u64 + 3) {
            store
                .apply(runnable(sequence, "task-a", "attempt-a"))
                .unwrap();
        }
        let trace: DecisionTrace =
            serde_json::from_slice(&std::fs::read(store.trace_path()).unwrap()).unwrap();
        assert_eq!(trace.observations.len(), MAX_TRACE_OBSERVATIONS);
        let reopened = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(reopened.state(), store.state());
    }

    #[test]
    fn invalid_identifier_cannot_deserialize_secret_or_path_content() {
        let raw = r#"{"trace_schema_version":1,"planner_schema_version":1,"redaction":"typed_identifiers_and_digests_only","ruleset":"corrected","initial_state":{"schema_version":1,"graph_id":"https://user:secret@example.test/x","logical_time":0,"last_sequence":null,"seen_observations":{},"tasks":{},"effects":{},"early_acknowledgements":{},"repaired_incidents":[],"fail_closed":false},"observations":[]}"#;
        assert!(serde_json::from_str::<DecisionTrace>(raw).is_err());
        let unknown = r#"{"trace_schema_version":1,"planner_schema_version":1,"redaction":"typed_identifiers_and_digests_only","ruleset":"corrected","initial_state":{"schema_version":1,"graph_id":"graph-a","logical_time":0,"last_sequence":null,"seen_observations":{},"tasks":{},"effects":{},"early_acknowledgements":{},"repaired_incidents":[],"fail_closed":false},"observations":[],"secret":"must-not-be-representable"}"#;
        assert!(serde_json::from_str::<DecisionTrace>(unknown).is_err());
    }

    #[test]
    fn schema_v1_state_and_convergence_fixture_migrate_without_reset_and_restart_byte_stable() {
        let temp = tempfile::tempdir().unwrap();
        let service = temp.path().join("service");
        std::fs::create_dir_all(&service).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/planner_runtime");
        let convergence_bytes = std::fs::read(fixtures.join("convergence-state-v1.json")).unwrap();
        std::fs::write(service.join("convergence-state.json"), &convergence_bytes).unwrap();
        std::fs::copy(
            fixtures.join("planner-state-v1.json"),
            service.join(STATE_FILE),
        )
        .unwrap();

        let expected_convergence: super::super::convergence::ConvergenceState =
            serde_json::from_slice(&convergence_bytes).unwrap();
        let expected_import = LegacyConvergenceImport::from_legacy(&expected_convergence).unwrap();
        let store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(store.state().schema_version, DAEMON_PLANNER_SCHEMA_VERSION);
        assert_eq!(store.state().last_sequence, Some(7));
        assert_eq!(store.state().logical_time, 1_700_000_000);
        assert_eq!(
            store.state().legacy_convergence.as_deref(),
            Some(&expected_import)
        );
        assert_eq!(
            store.status_projection().earliest_deadline.as_deref(),
            // The schema-v5 dispatch cutover deliberately retires imported
            // AwaitDispatch timing authority; the future SourceRepair deadline
            // remains visible and prevents a false exhaustiveness hold.
            Some("2039-09-18T23:06:40+00:00")
        );
        assert!(matches!(
            store
                .status_projection()
                .effects
                .values()
                .next()
                .unwrap()
                .phase,
            EffectExecutionPhase::Acknowledged {
                outcome: AckOutcome::Succeeded
            }
        ));
        assert_eq!(
            store
                .normalize_observation(TypedObservation {
                    observed_at: 1,
                    observation: Observation::Crash,
                })
                .unwrap(),
            ObservationEnvelope {
                sequence: 8,
                logical_time: 1_700_000_001,
                observation: Observation::Crash,
            }
        );

        let paths = [
            store.trace_path(),
            store.state_path(),
            store.effect_journal_path(),
            service.join("convergence-state.json"),
        ];
        let before = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        drop(store);
        let reopened = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let after = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        assert_eq!(
            reopened.state().legacy_convergence.as_deref(),
            Some(&expected_import)
        );
    }

    #[test]
    fn concurrent_store_handles_refresh_monotonic_allocator_under_lock() {
        let temp = tempfile::tempdir().unwrap();
        let mut first = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let mut stale = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        first
            .observe(TypedObservation {
                observed_at: 500,
                observation: Observation::Crash,
            })
            .unwrap();
        stale
            .observe(TypedObservation {
                observed_at: 1,
                observation: Observation::Crash,
            })
            .unwrap();
        assert_eq!(stale.state().last_sequence, Some(2));
        assert_eq!(stale.state().logical_time, 501);
        let reopened = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert_eq!(reopened.state().last_sequence, Some(2));
        assert_eq!(reopened.state().logical_time, 501);
    }

    #[test]
    fn issue_execute_ack_fault_boundaries_replay_one_stable_effect_id() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert!(store.replayable_effects().is_empty()); // before issue persistence

        let observation = match runnable(1, "task-a", "attempt-a").observation {
            Observation::Task(task) => Observation::Task(task),
            _ => unreachable!(),
        };
        let issued = store
            .observe(TypedObservation {
                observed_at: 100,
                observation,
            })
            .unwrap();
        let effect_id = issued.effects[0].effect_id.clone();
        assert!(store.trace_path().exists());
        assert!(store.effect_journal_path().exists());
        drop(store); // after issue, before execution

        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let EffectReplay::Execute(effect) = &store.replayable_effects()[0] else {
            panic!("issued effect must replay for execution")
        };
        assert_eq!(effect.effect_id, effect_id);
        assert_eq!(
            store
                .mark_effect_execution_started(&effect_id)
                .unwrap()
                .effect_id,
            effect_id
        );

        // The adapter's physical consequence is idempotent by effect ID. A
        // crash after execution but before recording may call it again, but
        // cannot produce a second logical/physical consequence.
        let mut physical_consequences = BTreeSet::new();
        assert!(physical_consequences.insert(effect_id.clone()));
        drop(store);
        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let EffectReplay::Execute(replayed) = &store.replayable_effects()[0] else {
            panic!("unrecorded execution must replay")
        };
        assert_eq!(replayed.effect_id, effect_id);
        assert!(!physical_consequences.insert(effect_id.clone()));
        store
            .record_effect_execution(&effect_id, AckOutcome::Succeeded)
            .unwrap();
        drop(store); // after execution record, before acknowledgement

        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        assert!(matches!(
            &store.replayable_effects()[0],
            EffectReplay::Acknowledge { effect, outcome }
                if effect.effect_id == effect_id && *outcome == AckOutcome::Succeeded
        ));
        store.acknowledge_recorded_effect(&effect_id, 101).unwrap();
        assert!(store.replayable_effects().is_empty());
        let acknowledged_sequence = store.state().last_sequence;
        let duplicate = store.acknowledge_recorded_effect(&effect_id, 999).unwrap();
        assert!(duplicate.effects.is_empty());
        assert_eq!(duplicate.state.last_sequence, acknowledged_sequence);
        let bytes_before = [
            std::fs::read(store.trace_path()).unwrap(),
            std::fs::read(store.state_path()).unwrap(),
            std::fs::read(store.effect_journal_path()).unwrap(),
        ];
        drop(store); // after acknowledgement

        let store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        let bytes_after = [
            std::fs::read(store.trace_path()).unwrap(),
            std::fs::read(store.state_path()).unwrap(),
            std::fs::read(store.effect_journal_path()).unwrap(),
        ];
        assert_eq!(bytes_before, bytes_after);
        assert!(store.replayable_effects().is_empty());
        assert_eq!(store.state().effects.len(), 1);
        assert_eq!(
            store.state().effects[&effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::Succeeded)
        );
        assert_eq!(physical_consequences.len(), 1);
    }

    #[test]
    fn all_existing_trace_schema_fixtures_migrate_with_stable_effect_ids() {
        let fixtures = [
            "formal/fixtures/daemon/v1/target_moved_during_finish.json",
            "formal/fixtures/daemon/v2/zero_progress_source_failure.json",
            "formal/fixtures/daemon/v3/stale_worktree_spawn_owner.json",
        ];
        let convergence_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/planner_runtime/convergence-state-v1.json");
        for fixture in fixtures {
            let wrapper: serde_json::Value = serde_json::from_slice(
                &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture)).unwrap(),
            )
            .unwrap();
            let mut trace: DecisionTrace =
                serde_json::from_value(wrapper.get("trace").unwrap().clone()).unwrap();
            trace.ruleset = PlannerRuleset::Corrected;
            let graph_id = trace.initial_state.graph_id.clone();
            let expected = replay(&trace)
                .unwrap()
                .final_state
                .effects
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let temp = tempfile::tempdir().unwrap();
            let service = temp.path().join("service");
            std::fs::create_dir_all(&service).unwrap();
            std::fs::write(
                service.join(TRACE_FILE),
                serde_json::to_vec_pretty(&trace).unwrap(),
            )
            .unwrap();
            std::fs::copy(&convergence_fixture, service.join("convergence-state.json")).unwrap();
            let store = PlannerStore::open(temp.path(), graph_id).unwrap();
            assert_eq!(
                store.state().effects.keys().cloned().collect::<Vec<_>>(),
                expected,
                "effect identity drifted while migrating {fixture}"
            );
            assert_eq!(
                store.state().legacy_convergence.as_ref().unwrap().goals[&id("legacy-goal:3")]
                    .next_wake_at
                    .as_str(),
                "2031-02-03T04:05:06.123456789+00:00"
            );
            let migrated: DecisionTrace =
                serde_json::from_slice(&std::fs::read(store.trace_path()).unwrap()).unwrap();
            assert_eq!(migrated.trace_schema_version, DAEMON_TRACE_SCHEMA_VERSION);
            assert_eq!(
                migrated.planner_schema_version,
                DAEMON_PLANNER_SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn read_only_status_projection_does_not_create_or_rewrite_runtime_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(PlannerStore::read_status(temp.path()).unwrap().is_none());
        assert!(!temp.path().join("service").exists());

        let mut store = PlannerStore::open(temp.path(), id("graph-a")).unwrap();
        store
            .observe(TypedObservation {
                observed_at: 42,
                observation: runnable(1, "task-a", "attempt-a").observation,
            })
            .unwrap();
        let paths = [
            store.trace_path(),
            store.state_path(),
            store.effect_journal_path(),
        ];
        let before = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        drop(store);
        let status = PlannerStore::read_status(temp.path()).unwrap().unwrap();
        assert_eq!(status.last_sequence, Some(1));
        assert_eq!(status.next_sequence, Some(2));
        assert_eq!(status.effects.len(), 1);
        let after = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
    }

    fn dispatch_observation(
        task: &str,
        plan: &str,
        readiness: DispatchReadiness,
        admission: DispatchAdmission,
        resource: ResourceEvidence,
        health: RouteHealthEvidence,
    ) -> DispatchObservation {
        DispatchObservation {
            key: key(task, "dispatch-ready"),
            progress_id: id(&format!("progress-{task}-{plan}")),
            readiness,
            admission,
            resource,
            route: Some(DispatchRouteObservation {
                route_id: id("route-a"),
                plan_id: id(plan),
                health,
            }),
            policy: DispatchPolicy {
                retry_base_seconds: 5,
                retry_cap_seconds: 40,
                route_probe_base_seconds: 10,
                route_probe_cap_seconds: 80,
                action_lease_seconds: 5,
                jitter_divisor: 1_000_000,
            },
        }
    }

    #[test]
    fn dispatch_readiness_admission_resource_and_ready_each_have_one_forward_class() {
        let cases = [
            dispatch_observation(
                "dependency",
                "plan-a",
                DispatchReadiness::Waiting {
                    wait_id: id("dependency-wait"),
                    kind: WaitKind::DependencyChange,
                    deadline: None,
                },
                DispatchAdmission::Admitted,
                ResourceEvidence::Available,
                RouteHealthEvidence::Healthy,
            ),
            dispatch_observation(
                "admission",
                "plan-a",
                DispatchReadiness::Ready,
                DispatchAdmission::Deferred {
                    wait_id: id("admission-wait"),
                    deadline: Some(120),
                },
                ResourceEvidence::Available,
                RouteHealthEvidence::Healthy,
            ),
            dispatch_observation(
                "resource",
                "plan-a",
                DispatchReadiness::Ready,
                DispatchAdmission::Admitted,
                ResourceEvidence::Deferred {
                    wait_id: id("resource-wait"),
                    deadline: None,
                },
                RouteHealthEvidence::Healthy,
            ),
            dispatch_observation(
                "ready",
                "plan-a",
                DispatchReadiness::Ready,
                DispatchAdmission::Admitted,
                ResourceEvidence::Available,
                RouteHealthEvidence::Healthy,
            ),
        ];

        for (index, observation) in cases.into_iter().enumerate() {
            let step = plan(
                &PlannerState::new(id("graph-a")),
                &ObservationEnvelope {
                    sequence: 1,
                    logical_time: 100,
                    observation: Observation::Dispatch(Box::new(observation)),
                },
                PlannerRuleset::Corrected,
            );
            assert!(step.violations.is_empty(), "case {index}");
            let task = step.state.tasks.values().next().unwrap();
            assert_eq!(forward_count(task), 1, "case {index}");
            assert_eq!(step.effects.len(), usize::from(index == 3));
        }
    }

    #[test]
    fn route_outage_issues_one_exact_probe_and_persists_lease_without_storm() {
        let first = dispatch_observation(
            "task-a",
            "plan-exact",
            DispatchReadiness::Ready,
            DispatchAdmission::Admitted,
            ResourceEvidence::Available,
            RouteHealthEvidence::Unavailable {
                failure_id: id("failure-1"),
            },
        );
        let step1 = plan(
            &PlannerState::new(id("graph-a")),
            &ObservationEnvelope {
                sequence: 1,
                logical_time: 100,
                observation: Observation::Dispatch(Box::new(first.clone())),
            },
            PlannerRuleset::Corrected,
        );
        assert!(step1.effects.is_empty());
        let deadline = step1.state.routes[&id("route-a")].next_probe_at;
        assert!(deadline > 100);

        let step2 = plan(
            &step1.state,
            &ObservationEnvelope {
                sequence: 2,
                logical_time: deadline,
                observation: Observation::Dispatch(Box::new(first)),
            },
            PlannerRuleset::Corrected,
        );
        assert_eq!(step2.effects.len(), 1);
        let probe = step2.effects[0].clone();
        assert_eq!(probe.action, ActionKind::ProbeRoute);
        assert!(matches!(
            probe.binding.as_ref(),
            Some(EffectBinding::Dispatch(DispatchEffectBinding {
                route_id,
                plan_id,
                ..
            })) if route_id == &id("route-a") && plan_id == &id("plan-exact")
        ));

        let second = dispatch_observation(
            "task-b",
            "plan-exact",
            DispatchReadiness::Ready,
            DispatchAdmission::Admitted,
            ResourceEvidence::Available,
            RouteHealthEvidence::Unavailable {
                failure_id: id("failure-1"),
            },
        );
        let step3 = plan(
            &step2.state,
            &ObservationEnvelope {
                sequence: 3,
                logical_time: deadline,
                observation: Observation::Dispatch(Box::new(second.clone())),
            },
            PlannerRuleset::Corrected,
        );
        assert!(step3.effects.is_empty());
        assert!(
            step3
                .state
                .tasks
                .values()
                .all(|task| forward_count(task) == 1)
        );
        assert_eq!(step3.state.routes.len(), 1);

        let ack = plan(
            &step3.state,
            &ObservationEnvelope {
                sequence: 4,
                logical_time: deadline + 1,
                observation: Observation::EffectAcknowledged {
                    effect_id: probe.effect_id.clone(),
                    outcome: AckOutcome::Succeeded,
                },
            },
            PlannerRuleset::Corrected,
        );
        let lease = ack.state.routes[&id("route-a")]
            .probe_lease
            .as_ref()
            .unwrap();
        assert!(lease.spawned);
        assert_eq!(lease.expires_at, None);

        let restarted =
            serde_json::from_slice::<PlannerState>(&serde_json::to_vec(&ack.state).unwrap())
                .unwrap();
        let after_restart = plan(
            &restarted,
            &ObservationEnvelope {
                sequence: 5,
                logical_time: deadline + 10_000,
                observation: Observation::Dispatch(Box::new(second)),
            },
            PlannerRuleset::Corrected,
        );
        assert!(after_restart.effects.is_empty());
        assert!(
            after_restart.state.routes[&id("route-a")]
                .probe_lease
                .as_ref()
                .unwrap()
                .spawned
        );
    }

    #[test]
    fn healthy_route_rejects_pending_probe_and_changed_progress_rejects_old_spawn() {
        let outage = dispatch_observation(
            "task-a",
            "plan-exact",
            DispatchReadiness::Ready,
            DispatchAdmission::Admitted,
            ResourceEvidence::Available,
            RouteHealthEvidence::Unavailable {
                failure_id: id("failure-1"),
            },
        );
        let first = plan(
            &PlannerState::new(id("graph-a")),
            &ObservationEnvelope {
                sequence: 1,
                logical_time: 100,
                observation: Observation::Dispatch(Box::new(outage.clone())),
            },
            PlannerRuleset::Corrected,
        );
        let deadline = first.state.routes[&id("route-a")].next_probe_at;
        let probe_step = plan(
            &first.state,
            &ObservationEnvelope {
                sequence: 2,
                logical_time: deadline,
                observation: Observation::Dispatch(Box::new(outage)),
            },
            PlannerRuleset::Corrected,
        );
        let probe = probe_step.effects[0].clone();

        let mut healthy = dispatch_observation(
            "task-a",
            "plan-exact",
            DispatchReadiness::Ready,
            DispatchAdmission::Admitted,
            ResourceEvidence::Available,
            RouteHealthEvidence::Healthy,
        );
        let recovered = plan(
            &probe_step.state,
            &ObservationEnvelope {
                sequence: 3,
                logical_time: deadline + 1,
                observation: Observation::Dispatch(Box::new(healthy.clone())),
            },
            PlannerRuleset::Corrected,
        );
        assert_eq!(
            recovered.state.effects[&probe.effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::RejectedStale)
        );
        let ready = plan(
            &recovered.state,
            &ObservationEnvelope {
                sequence: 4,
                logical_time: deadline + 100,
                observation: Observation::Dispatch(Box::new(healthy.clone())),
            },
            PlannerRuleset::Corrected,
        );
        let first_spawn = ready.effects[0].clone();
        assert_eq!(first_spawn.action, ActionKind::SpawnAttempt);

        healthy.progress_id = id("new-authoritative-progress");
        let changed = plan(
            &ready.state,
            &ObservationEnvelope {
                sequence: 5,
                logical_time: deadline + 101,
                observation: Observation::Dispatch(Box::new(healthy)),
            },
            PlannerRuleset::Corrected,
        );
        assert_eq!(
            changed.state.effects[&first_spawn.effect_id].status,
            EffectStatus::Acknowledged(AckOutcome::RejectedStale)
        );
        assert_eq!(changed.effects.len(), 1);
        assert_ne!(changed.effects[0].effect_id, first_spawn.effect_id);
    }

    #[test]
    fn dispatch_retry_and_future_source_repair_deadlines_are_planner_owned() {
        let observation = dispatch_observation(
            "task-a",
            "plan-exact",
            DispatchReadiness::Ready,
            DispatchAdmission::Admitted,
            ResourceEvidence::Available,
            RouteHealthEvidence::Healthy,
        );
        let issued = plan(
            &PlannerState::new(id("graph-a")),
            &ObservationEnvelope {
                sequence: 1,
                logical_time: 100,
                observation: Observation::Dispatch(Box::new(observation)),
            },
            PlannerRuleset::Corrected,
        );
        let effect_id = issued.effects[0].effect_id.clone();
        let retry = plan(
            &issued.state,
            &ObservationEnvelope {
                sequence: 2,
                logical_time: 101,
                observation: Observation::EffectAcknowledged {
                    effect_id: effect_id.clone(),
                    outcome: AckOutcome::Retryable,
                },
            },
            PlannerRuleset::Corrected,
        );
        assert!(retry.effects.is_empty());
        let retry_deadline = retry.state.effect_retry_deadlines[&effect_id];
        assert!(retry_deadline > 101);

        let source_repair = TaskObservation {
            key: key("repair", "attempt-a"),
            progress_id: id("repair-progress"),
            unfinished: true,
            owner: OwnerEvidence::None,
            runnable: None,
            external_wait: Some(ExternalWait {
                wait_id: id("source-repair"),
                kind: WaitKind::SourceRepair,
                deadline: Some(500),
            }),
            scheduled: None,
            effect_binding: None,
            incidents: BTreeSet::new(),
            failed_prerequisite: None,
        };
        let with_repair = plan(
            &retry.state,
            &ObservationEnvelope {
                sequence: 3,
                logical_time: 102,
                observation: Observation::Task(Box::new(source_repair)),
            },
            PlannerRuleset::Corrected,
        );
        let mut journal = EffectExecutionJournal::default();
        sync_effect_journal(&with_repair.state, &mut journal).unwrap();
        let projection = status_projection(&with_repair.state, &journal);
        assert_eq!(
            projection.earliest_deadline,
            logical_datetime(retry_deadline).map(|value| value.to_rfc3339())
        );
    }

    #[test]
    fn zero_output_is_persisted_evidence_and_never_an_action() {
        let state = PlannerState::new(id("graph-a"));
        let step = plan(
            &state,
            &ObservationEnvelope {
                sequence: 1,
                logical_time: 100,
                observation: Observation::ZeroOutput(Box::new(ZeroOutputObservation {
                    task: key("task-a", "attempt-a"),
                    owner_id: id("agent-a"),
                    evidence_id: id("zero-output-evidence"),
                    age_bucket: 5,
                    route_id: Some(id("route-a")),
                })),
            },
            PlannerRuleset::Corrected,
        );
        assert!(step.effects.is_empty());
        assert_eq!(step.state.zero_output.len(), 1);
    }

    #[test]
    fn monitor_persists_replay_before_returning_hold() {
        let temp = tempfile::tempdir().unwrap();
        let state = PlannerState::new(id("graph-a"));
        let observation = ObservationEnvelope {
            sequence: 1,
            logical_time: 10,
            observation: Observation::Task(Box::new(TaskObservation {
                key: key("task-a", "attempt-a"),
                progress_id: id("progress-1"),
                unfinished: true,
                owner: OwnerEvidence::None,
                runnable: None,
                external_wait: None,
                scheduled: None,
                effect_binding: None,
                incidents: BTreeSet::new(),
                failed_prerequisite: None,
            })),
        };
        let (step, bundle) = plan_guarded(temp.path(), &state, &observation).unwrap();
        assert!(step.state.fail_closed);
        assert_eq!(step.effects[0].action, ActionKind::FailClosedHold);
        let bundle = bundle.unwrap();
        assert!(bundle.exists());
        let trace: DecisionTrace = serde_json::from_slice(&std::fs::read(bundle).unwrap()).unwrap();
        assert_eq!(replay(&trace).unwrap().final_state, step.state);
    }
}
