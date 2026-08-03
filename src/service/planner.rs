//! Pure, replayable planner for correctness-critical daemon decisions.
//!
//! The planner consumes only normalized, typed observations and logical time.
//! It performs no filesystem, process, socket, Git, provider, signal, or clock
//! reads. Adapters are responsible for producing evidence; emitted effects are
//! idempotent requests that adapters execute and acknowledge separately.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub const DAEMON_PLANNER_SCHEMA_VERSION: u16 = 1;
pub const DAEMON_TRACE_SCHEMA_VERSION: u16 = 1;
pub const MAX_TRACE_OBSERVATIONS: usize = 256;
pub const MAX_REPLAY_BUNDLES: usize = 32;
const TRACE_FILE: &str = "decision-trace-v1.json";
const STATE_FILE: &str = "planner-state-v1.json";

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
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            bail!("planner identifier must be 1..=96 safe identifier bytes");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWait {
    pub wait_id: OpaqueId,
    pub kind: WaitKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledAction {
    pub action: ActionKind,
    pub deadline: u64,
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
    #[serde(default)]
    pub incidents: BTreeSet<IncidentCode>,
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
    #[serde(default)]
    pub effects: BTreeMap<OpaqueId, PlannedEffect>,
    #[serde(default)]
    pub early_acknowledgements: BTreeMap<OpaqueId, AckOutcome>,
    #[serde(default)]
    pub repaired_incidents: BTreeSet<IncidentCode>,
    #[serde(default)]
    pub fail_closed: bool,
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
            effects: BTreeMap::new(),
            early_acknowledgements: BTreeMap::new(),
            repaired_incidents: BTreeSet::new(),
            fail_closed: false,
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
    action: ActionKind,
    issue_epoch: u64,
) -> OpaqueId {
    let material = format!(
        "{}:{}:{action:?}:{issue_epoch}",
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
                });
            }
        }
    }
}

fn forward_count(task: &TaskObservation) -> usize {
    usize::from(task.runnable.is_some())
        + usize::from(task.owner.is_authenticated_live())
        + usize::from(task.external_wait.is_some())
        + usize::from(task.scheduled.is_some())
}

fn issue_effect(
    state: &mut PlannerState,
    task: &TaskObservation,
    action: ActionKind,
) -> Option<PlannedEffect> {
    let issue_epoch = 1;
    let id = effect_id(&task.key, &task.progress_id, action, issue_epoch);
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
        task: task.key.clone(),
        action,
        issue_epoch,
        status,
    };
    state.effects.insert(id, effect.clone());
    should_emit.then_some(effect)
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

    if next.schema_version != DAEMON_PLANNER_SCHEMA_VERSION {
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
        Observation::EffectAcknowledged { effect_id, outcome } => {
            if let Some(effect) = next.effects.get_mut(effect_id) {
                if *outcome == AckOutcome::Retryable {
                    // A physical retry reuses the same logical effect ID. The
                    // effect map remains cardinality-one while the adapter is
                    // explicitly asked to retry its idempotent operation.
                    effect.status = EffectStatus::Issued;
                    emitted.push(effect.clone());
                } else {
                    effect.status = EffectStatus::Acknowledged(*outcome);
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
        if self.trace_schema_version != DAEMON_TRACE_SCHEMA_VERSION {
            bail!(
                "unsupported daemon trace schema {}",
                self.trace_schema_version
            );
        }
        if self.planner_schema_version != DAEMON_PLANNER_SCHEMA_VERSION
            || self.initial_state.schema_version != DAEMON_PLANNER_SCHEMA_VERSION
        {
            bail!("unsupported daemon planner schema");
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

/// Durable adapter boundary. The trace is scheduling authority; the state file
/// is a rebuildable normalized cache. Each observation is persisted in the
/// trace before the new cache/effects are returned to an adapter.
pub struct PlannerStore {
    dir: PathBuf,
    trace: DecisionTrace,
    state: PlannerState,
}

impl PlannerStore {
    pub fn open(dir: &Path, graph_id: OpaqueId) -> Result<Self> {
        let root = dir.join("service");
        let trace_path = root.join(TRACE_FILE);
        let trace = if trace_path.exists() {
            serde_json::from_slice::<DecisionTrace>(&std::fs::read(&trace_path)?)
                .with_context(|| format!("failed to parse {}", trace_path.display()))?
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
        let state = replay(&trace)?.final_state;
        Ok(Self {
            dir: dir.to_path_buf(),
            trace,
            state,
        })
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

    /// Persist one observation and return newly issued/retried physical effects.
    /// On an invariant failure, the minimal replay bundle is written first.
    pub fn apply(&mut self, observation: ObservationEnvelope) -> Result<PlannerStep> {
        let (step, _) = plan_guarded(&self.dir, &self.state, &observation)?;
        self.trace.observations.push(observation);
        if self.trace.observations.len() > MAX_TRACE_OBSERVATIONS {
            let split = self.trace.observations.len() - MAX_TRACE_OBSERVATIONS;
            let prefix = DecisionTrace {
                observations: self.trace.observations[..split].to_vec(),
                ..self.trace.clone()
            };
            self.trace.initial_state = replay(&prefix)?.final_state;
            self.trace.initial_state.seen_observations.clear();
            self.trace.observations = self.trace.observations[split..].to_vec();
        }
        let trace_path = self.trace_path();
        if let Some(parent) = trace_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic_file::write_atomic(&trace_path, serde_json::to_vec_pretty(&self.trace)?)
            .with_context(|| format!("failed to persist {}", trace_path.display()))?;
        // The trace above is authoritative after a crash. Only now publish the
        // cache and return effects for execution.
        crate::atomic_file::write_atomic(
            &self.state_path(),
            serde_json::to_vec_pretty(&step.state)?,
        )
        .with_context(|| format!("failed to persist {}", self.state_path().display()))?;
        self.state = step.state.clone();
        Ok(step)
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
                incidents: BTreeSet::new(),
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
                                    }),
                                    scheduled: scheduled.then_some(ScheduledAction {
                                        action: ActionKind::CleanupFinish,
                                        deadline: 11,
                                    }),
                                    incidents: BTreeSet::new(),
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
        let expected_id = effect_id(&task.key, &task.progress_id, ActionKind::SpawnAttempt, 1);
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
                incidents: BTreeSet::new(),
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
