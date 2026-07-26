//! Durable, route-stable evaluation lifecycle primitives.
//!
//! Agency satellites are part of the evaluation gate, not ordinary work.  This
//! module gives each source attempt one pipeline identity, persists the exact
//! handler-first routes selected at scaffold time, and records semantic verdicts
//! before any graph transition.  Dispatcher reconciliation can therefore link
//! and consume a verdict idempotently after a crash without invoking a model
//! again.

use crate::agency::Evaluation;
use crate::config::{
    Config, DispatchRole, ExecutionSystemKey, ReasoningLevel, ReasoningProvenance,
    execution_system_key, parse_exact_pi_route,
};
use crate::graph::{LogEntry, Status, Task, WorkGraph};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const AGENCY_PLAN_SCHEMA: u16 = 1;
pub const EVAL_LIFECYCLE_SCHEMA: u16 = 1;
/// Schema 1 hashed an in-memory `Evaluation`. Its `HashMap` field order was
/// process-random, so the durable JSON could deserialize to different bytes.
/// Schema 2 pins the exact durable evaluation file instead.
pub const EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA: u16 = 2;
pub const MAX_EXECUTION_ATTEMPTS_PER_ROUTE_GENERATION: u32 = 2;
pub const AGENCY_REASONING_MIGRATION_SCHEMA: u16 = 1;
const MAX_REASONING_MIGRATIONS_PER_SOURCE_ATTEMPT: usize = 1;

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgencyStage {
    FlipInference,
    FlipComparison,
    Evaluate,
}

impl AgencyStage {
    pub fn role(self) -> DispatchRole {
        match self {
            Self::FlipInference => DispatchRole::FlipInference,
            Self::FlipComparison => DispatchRole::FlipComparison,
            Self::Evaluate => DispatchRole::Evaluator,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DispatchSelectionSource {
    ScaffoldConfig,
    PersistedPlan,
    LegacyHandlerFirst,
    LegacyCodexSplit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgencyCallPlan {
    pub stage: AgencyStage,
    /// Canonical handler-first route. Invocation must never reconstruct this
    /// value from the compatibility `Task.model` / `Task.provider` mirrors.
    pub route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    pub system: ExecutionSystemKey,
    pub source: DispatchSelectionSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgencyDispatchPlan {
    pub schema: u16,
    pub pipeline_id: String,
    pub source_task: String,
    pub source_attempt: u32,
    /// A route/reasoning migration mints a new generation without pretending
    /// that the completed source worker ran again. Generation zero is omitted
    /// so pre-migration plan hashes remain byte-for-byte verifiable.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub route_generation: u32,
    pub task_id: String,
    pub calls: Vec<AgencyCallPlan>,
    pub plan_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgencyReasoningResolution {
    pub stage: AgencyStage,
    pub route: String,
    pub reasoning: ReasoningLevel,
    pub was_missing: bool,
    pub provenance: ReasoningProvenance,
    pub config_source: String,
}

/// Immutable audit row for the explicit pre-Pi reasoning migration boundary.
/// The original plan and its failed producer identity remain embedded even
/// after the mutable satellite is rearmed with the new executable plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgencyPlanMigration {
    pub schema: u16,
    pub boundary: String,
    pub migrated_at: String,
    pub source_task: String,
    pub source_attempt: u32,
    pub route_generation: u32,
    pub task_id: String,
    pub old_pipeline_id: String,
    pub new_pipeline_id: String,
    pub old_plan: AgencyDispatchPlan,
    pub new_plan: AgencyDispatchPlan,
    pub reasoning: Vec<AgencyReasoningResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_producer_run_id: Option<String>,
    pub prior_status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_source_diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationExecutionState {
    #[default]
    Ready,
    Claimed,
    Waiting,
    Blocked,
    VerdictDurable,
    Consumed,
}

/// Whether an evaluator is informational or is allowed to decide source
/// completion. This is snapshotted on the source attempt before `wg done`
/// returns, so a daemon reload cannot silently change the meaning of an
/// already-visible evaluation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationGateApplicability {
    Advisory,
    Required,
}

/// FLIP's contribution to the source gate. A persisted FLIP dependency is a
/// required independent verdict for a hard gate; scores are never averaged
/// with the evaluator and a successful system-task execution is never a
/// substitute for this attempt-bound verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlipVerdictPolicy {
    NotScheduled,
    Advisory,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlipThresholdSource {
    EvaluatorThreshold,
    FlipVerificationThreshold,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationGatePolicy {
    pub applicability: EvaluationGateApplicability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_threshold: Option<f64>,
    pub flip_policy: FlipVerdictPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_threshold_source: Option<FlipThresholdSource>,
}

impl EvaluationGatePolicy {
    pub fn validate(&self) -> Result<()> {
        let valid_threshold = |name: &str, value: Option<f64>, required: bool| -> Result<()> {
            let Some(value) = value else {
                if required {
                    anyhow::bail!("error[WG-EVAL-GATE-POLICY]: required {name} is missing");
                }
                return Ok(());
            };
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                anyhow::bail!(
                    "error[WG-EVAL-GATE-POLICY]: {name} must be finite and in [0, 1], got {value}"
                );
            }
            Ok(())
        };
        let required = self.applicability == EvaluationGateApplicability::Required;
        valid_threshold("evaluator threshold", self.evaluator_threshold, required)?;
        valid_threshold(
            "FLIP threshold",
            self.flip_threshold,
            self.flip_policy == FlipVerdictPolicy::Required,
        )?;
        if self.flip_policy == FlipVerdictPolicy::Required && !required {
            anyhow::bail!("error[WG-EVAL-GATE-POLICY]: an advisory evaluation cannot require FLIP");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationGateOutcome {
    AwaitingEvidence,
    AdvisoryCompleted,
    Passed,
    RescueRetry,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationOutcomeProvenance {
    pub outcome: EvaluationGateOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_verdict: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationLifecycle {
    pub schema: u16,
    pub pipeline_id: String,
    pub source_attempt: u32,
    #[serde(default)]
    pub route_generation: u32,
    #[serde(default)]
    pub schedule_attempts: u32,
    #[serde(default)]
    pub transport_attempts: u32,
    #[serde(default)]
    pub semantic_attempts: u32,
    #[serde(default)]
    pub execution_state: EvaluationExecutionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_flip_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_eval_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_verdict: Option<String>,
    /// Attempt-pinned applicability and thresholds. `None` is historical and
    /// is migrated only while the source is still in a soft evaluation state;
    /// completed historical rows remain immutable and are surfaced by audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_policy: Option<EvaluationGatePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_provenance: Option<EvaluationOutcomeProvenance>,
    #[serde(default)]
    pub repair_version: u16,
    /// Number of coordinator plumbing repairs performed for this source
    /// attempt. A repair may rearm one or both satellites, but the budget is
    /// consumed once for the atomic repair transaction.
    #[serde(default)]
    pub repair_attempts: u16,
    /// Append-only audit history for explicit reasoning migrations. A source
    /// attempt may cross this boundary at most once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_migrations: Vec<AgencyPlanMigration>,
    /// Stable, actionable fail-closed diagnostic. This is deliberately kept
    /// separate from `Task.failure_reason`: FailedPendingEval still needs to
    /// retain the worker's original failure evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl EvaluationLifecycle {
    pub fn for_source(task: &Task) -> Self {
        // This derivation is the migration/default path. Once a source attempt
        // has been explicitly minted, its stored `source_attempt` is
        // authoritative even if legacy retry counters are later reset.
        let source_attempt = task
            .retry_count
            .saturating_add(task.rescue_count)
            .saturating_add(1);
        Self::for_source_attempt(task, source_attempt)
    }

    fn for_source_attempt(task: &Task, source_attempt: u32) -> Self {
        Self {
            schema: EVAL_LIFECYCLE_SCHEMA,
            pipeline_id: pipeline_id(&task.id, source_attempt, task.loop_iteration),
            source_attempt,
            route_generation: 0,
            schedule_attempts: 0,
            transport_attempts: 0,
            semantic_attempts: 0,
            execution_state: EvaluationExecutionState::Ready,
            linked_flip_verdict: None,
            linked_eval_verdict: None,
            consumed_verdict: None,
            gate_policy: None,
            outcome_provenance: None,
            repair_version: 0,
            repair_attempts: 0,
            plan_migrations: Vec::new(),
            diagnostic: None,
        }
    }

    /// Reserve one claimed transport run within this immutable route
    /// generation. The caller performs this while atomically claiming the task.
    pub fn reserve_transport_attempt(&mut self) -> Result<u32> {
        if self.transport_attempts >= MAX_EXECUTION_ATTEMPTS_PER_ROUTE_GENERATION {
            self.execution_state = EvaluationExecutionState::Blocked;
            anyhow::bail!(
                "error[WG-EXEC-AGENCY-EXECUTION-EXHAUSTED]: {} claimed transport attempts exhausted",
                self.transport_attempts
            );
        }
        self.transport_attempts = self.transport_attempts.saturating_add(1);
        self.execution_state = EvaluationExecutionState::Claimed;
        Ok(self.transport_attempts)
    }
}

/// Ensure a source entering a soft evaluation state has the lifecycle identity
/// minted when its current execution attempt began. Older code derived the id
/// again at completion, which is precisely how a resumed worker and its
/// already-scaffolded satellites acquired different pipelines.
pub fn refresh_source_lifecycle(task: &mut Task) {
    let derived = EvaluationLifecycle::for_source(task);
    let replacement_attempt = match task.evaluation_lifecycle.as_ref() {
        None => Some(derived.source_attempt),
        Some(current) if current.consumed_verdict.is_some() => Some(
            derived
                .source_attempt
                .max(current.source_attempt.saturating_add(1)),
        ),
        Some(current)
            if current.pipeline_id
                != pipeline_id(&task.id, current.source_attempt, task.loop_iteration) =>
        {
            Some(current.source_attempt.max(derived.source_attempt))
        }
        Some(_) => None,
    };
    if let Some(source_attempt) = replacement_attempt {
        task.evaluation_lifecycle = Some(EvaluationLifecycle::for_source_attempt(
            task,
            source_attempt,
        ));
    }
}

/// Snapshot the effective gate contract on the current source attempt. Existing
/// attempt policy wins: retries preserve the policy that the user saw rather
/// than inheriting a later daemon/config reload.
pub fn snapshot_source_gate(
    task: &mut Task,
    policy: EvaluationGatePolicy,
    outcome: EvaluationGateOutcome,
) -> Result<()> {
    policy.validate()?;
    refresh_source_lifecycle(task);
    let lifecycle = task
        .evaluation_lifecycle
        .as_mut()
        .expect("refresh_source_lifecycle always installs a lifecycle");
    if let Some(existing) = lifecycle.gate_policy.as_ref() {
        existing.validate()?;
    } else {
        lifecycle.gate_policy = Some(policy);
    }
    if lifecycle.outcome_provenance.is_none() {
        lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
            outcome,
            evaluator_verdict: None,
            flip_verdict: None,
            summary: match outcome {
                EvaluationGateOutcome::AdvisoryCompleted => {
                    "source completed directly; evaluator execution is advisory evidence, not a quality pass"
                }
                EvaluationGateOutcome::AwaitingEvidence => {
                    "source is hard-gated pending exact attempt-bound required verdicts"
                }
                _ => "source gate outcome pending reconciliation",
            }
            .to_string(),
        });
    }
    Ok(())
}

fn source_attempt_for_plan(task: &Task) -> u32 {
    task.evaluation_lifecycle
        .as_ref()
        .filter(|lifecycle| lifecycle.consumed_verdict.is_none())
        .map(|lifecycle| lifecycle.source_attempt)
        .unwrap_or_else(|| EvaluationLifecycle::for_source(task).source_attempt)
}

pub fn pipeline_id(source_task: &str, source_attempt: u32, loop_iteration: u32) -> String {
    let material = format!("wg-eval-v1\0{source_task}\0{source_attempt}\0{loop_iteration}");
    format!(
        "evalp-{}",
        &blake3::hash(material.as_bytes()).to_hex()[..24]
    )
}

pub fn stages_for_task(task_id: &str) -> Result<Vec<AgencyStage>> {
    if task_id.starts_with(".flip-") {
        Ok(vec![
            AgencyStage::FlipInference,
            AgencyStage::FlipComparison,
        ])
    } else if task_id.starts_with(".evaluate-") {
        Ok(vec![AgencyStage::Evaluate])
    } else {
        anyhow::bail!("{task_id:?} is not an evaluation lifecycle satellite")
    }
}

pub fn build_plan(
    config: &Config,
    source_task: &Task,
    task_id: &str,
    source: DispatchSelectionSource,
) -> Result<AgencyDispatchPlan> {
    let source_attempt = source_attempt_for_plan(source_task);
    let mut calls = Vec::new();
    for stage in stages_for_task(task_id)? {
        let role = stage.role();
        let dispatch = crate::service::llm::resolve_agency_dispatch(config, role)
            .with_context(|| format!("selecting agency route for stage {stage:?}"))?;
        let system = execution_system_key(&dispatch.raw_spec)?;
        let fallbacks = config.execution.models_for(&dispatch.raw_spec).to_vec();
        validate_fallbacks(&system, &fallbacks)?;
        calls.push(AgencyCallPlan {
            stage,
            route: dispatch.raw_spec,
            endpoint: config
                .models
                .get_role(role)
                .and_then(|model| model.endpoint.clone()),
            reasoning: dispatch.reasoning,
            system,
            source,
            fallbacks,
        });
    }
    let mut plan = AgencyDispatchPlan {
        schema: AGENCY_PLAN_SCHEMA,
        pipeline_id: pipeline_id(&source_task.id, source_attempt, source_task.loop_iteration),
        source_task: source_task.id.clone(),
        source_attempt,
        route_generation: 0,
        task_id: task_id.to_string(),
        calls,
        plan_hash: String::new(),
    };
    plan.plan_hash = compute_plan_hash(&plan)?;
    validate_plan(&plan)?;
    Ok(plan)
}

/// Lossless migration for a historical satellite. Bare OpenRouter is
/// deliberately ambiguous (Pi vs Nex) and therefore fails closed.
pub fn migrate_legacy_plan(source_task: &Task, satellite: &Task) -> Result<AgencyDispatchPlan> {
    let raw = satellite
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("error[WG-EXEC-AGENCY-ROUTE-UNSELECTED]: no persisted route")
        })?;
    let (route, source) = match execution_system_key(raw) {
        Ok(_) => (raw.to_string(), DispatchSelectionSource::LegacyHandlerFirst),
        Err(_) if satellite.provider.as_deref() == Some("codex") => (
            format!("codex:{raw}"),
            DispatchSelectionSource::LegacyCodexSplit,
        ),
        Err(_) if satellite.provider.as_deref() == Some("openrouter") => anyhow::bail!(
            "error[WG-EXEC-AGENCY-ROUTE-AMBIGUOUS]: legacy provider=openrouter cannot identify pi versus nex"
        ),
        Err(error) => anyhow::bail!(
            "error[WG-EXEC-AGENCY-ROUTE-AMBIGUOUS]: historical route {raw:?} is not handler-first: {error}"
        ),
    };
    let system = execution_system_key(&route)?;
    let source_attempt = source_attempt_for_plan(source_task);
    let calls = stages_for_task(&satellite.id)?
        .into_iter()
        .map(|stage| AgencyCallPlan {
            stage,
            route: route.clone(),
            endpoint: satellite.endpoint.clone(),
            reasoning: satellite.reasoning,
            system: system.clone(),
            source,
            fallbacks: Vec::new(),
        })
        .collect();
    let mut plan = AgencyDispatchPlan {
        schema: AGENCY_PLAN_SCHEMA,
        pipeline_id: pipeline_id(&source_task.id, source_attempt, source_task.loop_iteration),
        source_task: source_task.id.clone(),
        source_attempt,
        route_generation: 0,
        task_id: satellite.id.clone(),
        calls,
        plan_hash: String::new(),
    };
    plan.plan_hash = compute_plan_hash(&plan)?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub fn validate_plan(plan: &AgencyDispatchPlan) -> Result<()> {
    if plan.schema != AGENCY_PLAN_SCHEMA {
        anyhow::bail!("unsupported agency plan schema {}", plan.schema);
    }
    if plan.calls.is_empty() {
        anyhow::bail!("agency plan contains no calls");
    }
    let expected_hash = compute_plan_hash(plan)?;
    if expected_hash != plan.plan_hash {
        anyhow::bail!(
            "error[WG-EXEC-AGENCY-PLAN-HASH]: stored={} computed={}",
            plan.plan_hash,
            expected_hash
        );
    }
    for call in &plan.calls {
        let actual = execution_system_key(&call.route)?;
        if actual != call.system {
            anyhow::bail!(
                "error[WG-EXEC-AGENCY-SYSTEM-MISMATCH]: route {:?} is {}, plan recorded {}",
                call.route,
                actual,
                call.system
            );
        }
        validate_fallbacks(&call.system, &call.fallbacks)?;
    }
    Ok(())
}

/// Validate the stricter model-plane boundary immediately before a persisted
/// plan can be claimed or invoked. Structural validation intentionally still
/// accepts a pre-Pi plan so the explicit migration boundary can authenticate
/// its old hash; executable validation never does.
pub fn validate_executable_plan(plan: &AgencyDispatchPlan) -> Result<()> {
    validate_plan(plan)?;
    for call in &plan.calls {
        parse_exact_pi_route(&call.route).map_err(|error| {
            anyhow::anyhow!(
                "error[WG-PI-ROUTE-REQUIRED]: persisted agency plan route {:?} is not an exact Pi route: {error}",
                call.route
            )
        })?;
        if call.reasoning.is_none() {
            anyhow::bail!(
                "error[WG-PI-REASONING-MISSING]: persisted agency plan route {:?} has no reasoning; coordinator migration must resolve the authoritative role/tier value before execution",
                call.route
            );
        }
    }
    Ok(())
}

fn validate_fallbacks(primary: &ExecutionSystemKey, fallbacks: &[String]) -> Result<()> {
    for fallback in fallbacks {
        let system = execution_system_key(fallback)?;
        if &system != primary {
            anyhow::bail!(
                "error[WG-EXEC-FALLBACK-CROSS-SYSTEM]: primary={} fallback={fallback:?} fallback_system={system}",
                primary
            );
        }
    }
    Ok(())
}

fn compute_plan_hash(plan: &AgencyDispatchPlan) -> Result<String> {
    let mut material = plan.clone();
    material.plan_hash.clear();
    let bytes = serde_json::to_vec(&material)?;
    Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
}

pub fn call<'a>(plan: &'a AgencyDispatchPlan, stage: AgencyStage) -> Result<&'a AgencyCallPlan> {
    validate_plan(plan)?;
    plan.calls
        .iter()
        .find(|call| call.stage == stage)
        .ok_or_else(|| anyhow::anyhow!("agency plan has no {stage:?} call"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurableEvalVerdict {
    pub schema: u16,
    pub verdict_id: String,
    /// Digest of this verdict record with this field blank. The filename,
    /// record and separately persisted Evaluation are all verified on load.
    #[serde(default)]
    pub verdict_digest: String,
    pub evaluation_id: String,
    pub pipeline_id: String,
    pub source_task: String,
    pub source_attempt: u32,
    pub stage: AgencyStage,
    pub producer_run_id: String,
    pub score: f64,
    /// Digest scheme for `evaluation_digest`. Missing means the historical
    /// schema 1 compact-JSON digest; schema 2 hashes the durable file bytes.
    #[serde(
        default = "legacy_evaluation_digest_schema",
        skip_serializing_if = "is_legacy_evaluation_digest_schema"
    )]
    pub evaluation_digest_schema: u16,
    pub evaluation_digest: String,
    pub created_at: String,
}

fn legacy_evaluation_digest_schema() -> u16 {
    1
}

fn is_legacy_evaluation_digest_schema(schema: &u16) -> bool {
    *schema == 1
}

pub fn verdicts_dir(dir: &Path) -> PathBuf {
    dir.join("agency").join("eval-lifecycle").join("verdicts")
}

/// Persist semantic evidence create-if-absent. Replaying the same verdict is a
/// no-op; a different body at the same id is corruption and fails closed.
pub fn write_durable_verdict(
    dir: &Path,
    source_task: &Task,
    satellite: &Task,
    stage: AgencyStage,
    evaluation: &Evaluation,
) -> Result<PathBuf> {
    let plan = satellite.agency_dispatch.as_ref().ok_or_else(|| {
        anyhow::anyhow!("satellite {} has no persisted agency plan", satellite.id)
    })?;
    validate_plan(plan)?;
    if plan.source_task != source_task.id {
        anyhow::bail!("agency plan source mismatch");
    }
    if evaluation.task_id != source_task.id
        || !evaluation.score.is_finite()
        || !(0.0..=1.0).contains(&evaluation.score)
    {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: evaluation source/score is malformed or non-finite"
        );
    }
    let stage_source_matches = match stage {
        AgencyStage::FlipComparison => evaluation.source == crate::agency::eval_source::FLIP,
        AgencyStage::Evaluate => {
            evaluation.source != crate::agency::eval_source::FLIP && evaluation.source != "system"
        }
        AgencyStage::FlipInference => false,
    };
    if !stage_source_matches {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: evaluation source {:?} cannot produce {:?} source-gate evidence",
            evaluation.source,
            stage
        );
    }
    // The evaluation writer has already atomically renamed its JSON before
    // reaching this call. Pin those exact durable bytes rather than serializing
    // the in-memory `HashMap` again: HashMap iteration order is not stable
    // across the evaluator and daemon processes.
    let evidence = load_evaluation_evidence(dir, &evaluation.id)?;
    if canonical_evaluation_bytes(&evidence.evaluation)? != canonical_evaluation_bytes(evaluation)?
    {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: durable evaluation {:?} differs from writer value",
            evaluation.id
        );
    }
    let evaluation_digest = digest_bytes(&evidence.bytes);
    let verdict_id = format!(
        "verdict-{}-{}-{}",
        plan.pipeline_id,
        match stage {
            AgencyStage::FlipInference | AgencyStage::FlipComparison => "flip",
            AgencyStage::Evaluate => "evaluate",
        },
        &blake3::hash(evaluation.id.as_bytes()).to_hex()[..16]
    );
    let verdict = DurableEvalVerdict {
        schema: EVAL_LIFECYCLE_SCHEMA,
        verdict_id: verdict_id.clone(),
        verdict_digest: String::new(),
        evaluation_id: evaluation.id.clone(),
        pipeline_id: plan.pipeline_id.clone(),
        source_task: source_task.id.clone(),
        source_attempt: plan.source_attempt,
        stage,
        producer_run_id: satellite
            .assigned
            .clone()
            .unwrap_or_else(|| "manual".to_string()),
        score: evaluation.score,
        evaluation_digest_schema: EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA,
        evaluation_digest,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut verdict = verdict;
    verdict.verdict_digest = compute_verdict_digest(&verdict)?;
    let bytes = serde_json::to_vec_pretty(&verdict)?;
    let directory = verdicts_dir(dir);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{verdict_id}.json"));
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&path)?;
            let parsed: DurableEvalVerdict = serde_json::from_slice(&existing)?;
            // `created_at` and the current wrapper assignment are observational;
            // semantic identity is the pipeline/stage/evaluation digest. Verify
            // the immutable record we already have, then compare only canonical
            // semantic content. A crash replaying the same completed model result
            // is therefore a no-op even when time/run identity changed, while a
            // different result at the same key is quarantined.
            if parsed.verdict_digest != compute_verdict_digest(&parsed)? {
                anyhow::bail!(
                    "error[WG-EVAL-VERDICT-INTEGRITY]: verdict digest mismatch at {}",
                    path.display()
                );
            }
            // Independently validate the existing record against the durable
            // evaluation. This also makes an upgrade replay from legacy scheme
            // 1 to scheme 2 a no-op: both schemes must pin the same bytes, but
            // observational timestamps/run ids and the digest encoding itself
            // are not semantic conflicts.
            verify_evaluation_digest(dir, &parsed)?;
            if parsed.schema != verdict.schema
                || parsed.verdict_id != verdict.verdict_id
                || parsed.evaluation_id != verdict.evaluation_id
                || parsed.pipeline_id != verdict.pipeline_id
                || parsed.source_task != verdict.source_task
                || parsed.source_attempt != verdict.source_attempt
                || parsed.stage != verdict.stage
                || parsed.score != verdict.score
            {
                anyhow::bail!(
                    "error[WG-EVAL-VERDICT-CONFLICT]: verdict id {} has conflicting content",
                    verdict_id
                );
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn compute_verdict_digest(verdict: &DurableEvalVerdict) -> Result<String> {
    let mut material = verdict.clone();
    material.verdict_digest.clear();
    Ok(format!(
        "b3:{}",
        blake3::hash(&serde_json::to_vec(&material)?).to_hex()
    ))
}

struct EvaluationEvidence {
    evaluation: Evaluation,
    bytes: Vec<u8>,
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

/// Serialize through `Value`, whose default map representation is ordered.
/// This is used only to prove the just-written file has the same semantics as
/// the writer value; integrity itself pins the exact durable file bytes.
fn canonical_evaluation_bytes(evaluation: &Evaluation) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(evaluation)?)?)
}

/// Remove JSON formatting whitespace while preserving object member order and
/// every byte inside strings. The schema-1 writer hashed compact serde JSON and
/// saved pretty serde JSON from the same object, so this exactly reconstructs
/// its pre-restart byte sequence without guessing HashMap order.
fn compact_durable_json(bytes: &[u8]) -> Vec<u8> {
    let mut compact = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut escaped = false;
    for byte in bytes {
        if in_string {
            compact.push(*byte);
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else if *byte == b'"' {
            in_string = true;
            compact.push(*byte);
        } else if !byte.is_ascii_whitespace() {
            compact.push(*byte);
        }
    }
    compact
}

fn load_evaluation_evidence(dir: &Path, evaluation_id: &str) -> Result<EvaluationEvidence> {
    let directory = dir.join("agency/evaluations");
    let mut matching = Vec::new();
    if directory.exists() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path)?;
            let evaluation: Evaluation = serde_json::from_slice(&bytes)
                .with_context(|| format!("loading evaluation evidence {}", path.display()))?;
            if evaluation.id == evaluation_id {
                matching.push(EvaluationEvidence { evaluation, bytes });
            }
        }
    }
    if matching.len() != 1 {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: evaluation {:?} has {} durable matches",
            evaluation_id,
            matching.len()
        );
    }
    Ok(matching.pop().expect("one matching evaluation"))
}

fn verify_evaluation_digest(dir: &Path, verdict: &DurableEvalVerdict) -> Result<()> {
    let evidence = load_evaluation_evidence(dir, &verdict.evaluation_id).map_err(|error| {
        anyhow::anyhow!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: verdict {}: {error:#}",
            verdict.verdict_id
        )
    })?;
    let stage_source_matches = match verdict.stage {
        AgencyStage::FlipComparison => {
            evidence.evaluation.source == crate::agency::eval_source::FLIP
        }
        AgencyStage::Evaluate => {
            evidence.evaluation.source != crate::agency::eval_source::FLIP
                && evidence.evaluation.source != "system"
        }
        AgencyStage::FlipInference => false,
    };
    if evidence.evaluation.task_id != verdict.source_task
        || evidence.evaluation.score != verdict.score
        || !verdict.score.is_finite()
        || !(0.0..=1.0).contains(&verdict.score)
        || !stage_source_matches
    {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: verdict {} source/stage/score evaluation mismatch or non-finite score",
            verdict.verdict_id
        );
    }
    let digest = match verdict.evaluation_digest_schema {
        1 => digest_bytes(&compact_durable_json(&evidence.bytes)),
        EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA => digest_bytes(&evidence.bytes),
        schema => anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: verdict {} uses unsupported evaluation digest schema {}",
            verdict.verdict_id,
            schema
        ),
    };
    if digest != verdict.evaluation_digest {
        anyhow::bail!(
            "error[WG-EVAL-VERDICT-EVIDENCE]: verdict {} evaluation digest mismatch",
            verdict.verdict_id
        );
    }
    Ok(())
}

pub fn load_durable_verdicts(dir: &Path) -> Result<Vec<DurableEvalVerdict>> {
    let directory = verdicts_dir(dir);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut verdicts = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let verdict: DurableEvalVerdict = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("loading durable verdict {}", path.display()))?;
        let expected_file = format!("{}.json", verdict.verdict_id);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_file.as_str()) {
            anyhow::bail!(
                "error[WG-EVAL-VERDICT-INTEGRITY]: verdict id/filename mismatch at {}",
                path.display()
            );
        }
        if verdict.verdict_digest != compute_verdict_digest(&verdict)? {
            anyhow::bail!(
                "error[WG-EVAL-VERDICT-INTEGRITY]: verdict digest mismatch at {}",
                path.display()
            );
        }
        verify_evaluation_digest(dir, &verdict)?;
        verdicts.push(verdict);
    }
    verdicts.sort_by(|a, b| a.verdict_id.cmp(&b.verdict_id));
    Ok(verdicts)
}

/// Upgrade an unambiguous pre-schema Evaluation into durable pipeline evidence.
/// Missing source timestamps and zero/multiple candidates are deliberately left
/// untouched for operator review; this function never chooses "latest".
pub fn migrate_unambiguous_legacy_verdicts(dir: &Path) -> Result<usize> {
    let existing = load_durable_verdicts(dir)?;
    let graph = crate::parser::load_graph(&dir.join("graph.jsonl"))?;
    let evaluations = crate::agency::load_all_evaluations_or_warn(&dir.join("agency/evaluations"));
    let mut migrated = 0;

    for source in graph
        .tasks()
        .filter(|task| matches!(task.status, Status::PendingEval | Status::FailedPendingEval))
    {
        let Some(started_at) = source
            .started_at
            .as_deref()
            .and_then(|value| value.parse::<chrono::DateTime<Utc>>().ok())
        else {
            continue;
        };
        for (task_id, stage, is_candidate) in [
            (
                format!(".flip-{}", source.id),
                AgencyStage::FlipComparison,
                true,
            ),
            (
                format!(".evaluate-{}", source.id),
                AgencyStage::Evaluate,
                false,
            ),
        ] {
            let Some(satellite) = graph.get_task(&task_id) else {
                continue;
            };
            let plan = if let Some(plan) = satellite.agency_dispatch.clone() {
                validate_plan(&plan)?;
                plan
            } else {
                // A completed, claimed pre-schema evaluator is execution evidence,
                // not a route-retry candidate. We may backfill its plan only when
                // its display route is losslessly handler-qualified; this never
                // reopens or invokes the historical row.
                if satellite.status != Status::Done
                    || satellite.assigned.is_none()
                    || satellite.started_at.is_none()
                {
                    continue;
                }
                let Ok(plan) = migrate_legacy_plan(source, satellite) else {
                    continue;
                };
                plan
            };
            if existing.iter().any(|verdict| {
                verdict.pipeline_id == plan.pipeline_id
                    && verdict.source_attempt == plan.source_attempt
                    && verdict.stage == stage
            }) {
                continue;
            }
            let evidence_started_at = satellite
                .started_at
                .as_deref()
                .and_then(|value| value.parse::<chrono::DateTime<Utc>>().ok())
                .map_or(started_at, |satellite_start| {
                    started_at.max(satellite_start)
                });
            let candidates: Vec<_> = evaluations
                .iter()
                .filter(|evaluation| evaluation.task_id == source.id)
                .filter(|evaluation| evaluation.loop_iteration == source.loop_iteration)
                .filter(|evaluation| {
                    evaluation
                        .timestamp
                        .parse::<chrono::DateTime<Utc>>()
                        .is_ok_and(|timestamp| timestamp >= evidence_started_at)
                })
                .filter(|evaluation| {
                    if is_candidate {
                        evaluation.source == crate::agency::eval_source::FLIP
                    } else {
                        evaluation.source != crate::agency::eval_source::FLIP
                            && evaluation.source != "system"
                    }
                })
                .collect();
            if candidates.len() != 1 {
                continue;
            }
            let mut planned_satellite = satellite.clone();
            planned_satellite.agency_dispatch = Some(plan);
            write_durable_verdict(dir, source, &planned_satellite, stage, candidates[0])?;
            migrated += 1;
        }
    }
    Ok(migrated)
}

fn lifecycle_for_plan(plan: &AgencyDispatchPlan) -> EvaluationLifecycle {
    EvaluationLifecycle {
        schema: EVAL_LIFECYCLE_SCHEMA,
        pipeline_id: plan.pipeline_id.clone(),
        source_attempt: plan.source_attempt,
        route_generation: 0,
        schedule_attempts: 0,
        transport_attempts: 0,
        semantic_attempts: 0,
        execution_state: EvaluationExecutionState::Ready,
        linked_flip_verdict: None,
        linked_eval_verdict: None,
        consumed_verdict: None,
        gate_policy: None,
        outcome_provenance: None,
        repair_version: 0,
        repair_attempts: 0,
        plan_migrations: Vec::new(),
        diagnostic: None,
    }
}

fn lifecycle_conflict(task: &mut Task, message: String) -> bool {
    let already_recorded = if let Some(lifecycle) = task.evaluation_lifecycle.as_mut() {
        if lifecycle.diagnostic.as_deref() == Some(message.as_str()) {
            true
        } else {
            lifecycle.diagnostic = Some(message.clone());
            lifecycle.execution_state = EvaluationExecutionState::Blocked;
            false
        }
    } else if task.failure_reason.as_deref() == Some(message.as_str()) {
        true
    } else {
        task.failure_reason = Some(message.clone());
        false
    };
    if already_recorded {
        return false;
    }
    task.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: Some("eval-lifecycle-reconcile".to_string()),
        user: None,
        message,
    });
    true
}

/// Repair historical pre-claim rows using only lossless evidence already in
/// the graph. A legacy Codex split is canonical; an OpenRouter provider without
/// a handler is deliberately parked because it cannot distinguish Pi from Nex.
/// Each row is rearmed at most once per lifecycle schema.
pub fn repair_historical_rows(graph: &mut WorkGraph) -> bool {
    let satellite_ids: Vec<String> = graph
        .tasks()
        .filter(|task| task.id.starts_with(".flip-") || task.id.starts_with(".evaluate-"))
        .filter(|task| task.agency_dispatch.is_none())
        // Never rewrite an active or previously claimed legacy run. Route
        // repair is automatic only for rows with pre-claim evidence.
        .filter(|task| task.assigned.is_none() && task.started_at.is_none())
        .map(|task| task.id.clone())
        .collect();
    let mut modified = false;

    for satellite_id in satellite_ids {
        let source_id = satellite_id
            .strip_prefix(".flip-")
            .or_else(|| satellite_id.strip_prefix(".evaluate-"))
            .expect("filtered satellite id");
        let Some(source) = graph.get_task(source_id).cloned() else {
            continue;
        };
        if !matches!(
            source.status,
            Status::PendingEval | Status::FailedPendingEval
        ) {
            continue;
        }
        let satellite_snapshot = graph
            .get_task(&satellite_id)
            .expect("collected satellite")
            .clone();
        match migrate_legacy_plan(&source, &satellite_snapshot) {
            Ok(plan) => {
                let satellite = graph
                    .get_task_mut(&satellite_id)
                    .expect("collected satellite");
                satellite.model = Some(plan.calls[0].route.clone());
                satellite.provider = Some(plan.calls[0].system.handler.clone());
                satellite.endpoint = plan.calls[0].endpoint.clone();
                satellite.reasoning = plan.calls[0].reasoning;
                satellite.agency_dispatch = Some(plan.clone());
                let lifecycle = satellite
                    .evaluation_lifecycle
                    .get_or_insert_with(|| lifecycle_for_plan(&plan));
                if satellite.status == Status::Incomplete
                    && satellite.assigned.is_none()
                    && satellite.started_at.is_none()
                    && satellite.spawn_failures > 0
                    && lifecycle.repair_version < EVAL_LIFECYCLE_SCHEMA
                {
                    satellite.status = Status::Open;
                    satellite.spawn_failures = 0;
                    satellite.failure_reason = None;
                    lifecycle.repair_version = EVAL_LIFECYCLE_SCHEMA;
                    lifecycle.execution_state = EvaluationExecutionState::Ready;
                }
                satellite.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("eval-lifecycle-repair".to_string()),
                    user: None,
                    message: format!(
                        "Installed lossless historical plan {}; route={}",
                        plan.plan_hash, plan.calls[0].route
                    ),
                });
                modified = true;
            }
            Err(error) => {
                let diagnostic = format!("Lifecycle route repair required: {error:#}");
                let satellite = graph
                    .get_task_mut(&satellite_id)
                    .expect("collected satellite");
                if satellite.status != Status::Blocked
                    || satellite.failure_reason.as_deref() != Some(diagnostic.as_str())
                {
                    satellite.status = Status::Blocked;
                    satellite.wait_condition = None;
                    modified |= lifecycle_conflict(satellite, diagnostic);
                }
            }
        }
    }
    modified
}

/// Backfill a plan on a completed, claimed pre-schema satellite only after a
/// verified durable verdict proves that its semantic call already completed.
/// This is deliberately separate from `repair_historical_rows`: it never
/// rearms claimed work and cannot cause another model invocation.
fn install_completed_legacy_plan(
    graph: &mut WorkGraph,
    task_id: &str,
    verdict: &DurableEvalVerdict,
) -> bool {
    let Some(satellite_snapshot) = graph.get_task(task_id).cloned() else {
        return false;
    };
    if satellite_snapshot.agency_dispatch.is_some()
        || satellite_snapshot.status != Status::Done
        || satellite_snapshot.assigned.is_none()
        || satellite_snapshot.started_at.is_none()
    {
        return false;
    }
    let Some(source) = graph.get_task(&verdict.source_task).cloned() else {
        return false;
    };
    let Ok(plan) = migrate_legacy_plan(&source, &satellite_snapshot) else {
        return false;
    };
    if plan.pipeline_id != verdict.pipeline_id
        || plan.source_attempt != verdict.source_attempt
        || plan.source_task != verdict.source_task
        || !plan.calls.iter().any(|call| call.stage == verdict.stage)
    {
        return false;
    }

    let satellite = graph
        .get_task_mut(task_id)
        .expect("legacy satellite snapshot came from graph");
    satellite.model = Some(plan.calls[0].route.clone());
    satellite.provider = Some(plan.calls[0].system.handler.clone());
    satellite.endpoint = plan.calls[0].endpoint.clone();
    satellite.reasoning = plan.calls[0].reasoning;
    satellite.agency_dispatch = Some(plan.clone());
    satellite.evaluation_lifecycle = Some(lifecycle_for_plan(&plan));
    satellite.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: Some("eval-lifecycle-reconcile".to_string()),
        user: None,
        message: format!(
            "Backfilled completed historical plan {} from verified verdict {}; no semantic rerun",
            plan.plan_hash, verdict.verdict_id
        ),
    });
    true
}

fn mark_satellite_verdict(
    graph: &mut WorkGraph,
    task_id: &str,
    verdict: &DurableEvalVerdict,
) -> bool {
    let Some(task) = graph.get_task_mut(task_id) else {
        return false;
    };
    let Some(plan) = task.agency_dispatch.as_ref() else {
        return lifecycle_conflict(
            task,
            format!(
                "Durable verdict {} has no persisted agency plan",
                verdict.verdict_id
            ),
        );
    };
    if plan.pipeline_id != verdict.pipeline_id || plan.source_attempt != verdict.source_attempt {
        return lifecycle_conflict(
            task,
            format!(
                "Durable verdict {} does not match persisted pipeline {}",
                verdict.verdict_id, plan.pipeline_id
            ),
        );
    }
    let plan = plan.clone();
    task.evaluation_lifecycle
        .get_or_insert_with(|| lifecycle_for_plan(&plan));
    let existing = task.evaluation_lifecycle.as_ref().and_then(|lifecycle| {
        if verdict.stage == AgencyStage::Evaluate {
            lifecycle.linked_eval_verdict.clone()
        } else {
            lifecycle.linked_flip_verdict.clone()
        }
    });
    if let Some(existing) = existing
        && existing != verdict.verdict_id
    {
        return lifecycle_conflict(
            task,
            format!(
                "error[WG-EVAL-CONSUMPTION-CONFLICT]: stage linked {} but found {}",
                existing, verdict.verdict_id
            ),
        );
    }
    let lifecycle = task
        .evaluation_lifecycle
        .as_mut()
        .expect("inserted lifecycle");
    let slot = if verdict.stage == AgencyStage::Evaluate {
        &mut lifecycle.linked_eval_verdict
    } else {
        &mut lifecycle.linked_flip_verdict
    };
    let mut modified = false;
    if slot.is_none() {
        *slot = Some(verdict.verdict_id.clone());
        modified = true;
    }
    if task.status != Status::Done {
        task.status = Status::Done;
        task.assigned = None;
        task.completed_at
            .get_or_insert_with(|| Utc::now().to_rfc3339());
        modified = true;
    }
    if lifecycle.semantic_attempts == 0 {
        lifecycle.semantic_attempts = 1;
        modified = true;
    }
    if lifecycle.execution_state != EvaluationExecutionState::VerdictDurable {
        lifecycle.execution_state = EvaluationExecutionState::VerdictDurable;
        modified = true;
    }
    if modified {
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("eval-lifecycle-reconcile".to_string()),
            user: None,
            message: format!(
                "Linked durable {:?} verdict {} without semantic rerun",
                verdict.stage, verdict.verdict_id
            ),
        });
    }
    modified
}

fn rebind_plan_to_source(plan: &AgencyDispatchPlan, source: &Task) -> Result<AgencyDispatchPlan> {
    validate_plan(plan)?;
    if plan.source_task != source.id {
        anyhow::bail!(
            "error[WG-EVAL-PIPELINE-SOURCE]: plan for {:?} names source {:?}",
            plan.task_id,
            plan.source_task
        );
    }
    let mut rebound = plan.clone();
    rebound.source_attempt = source_attempt_for_plan(source);
    if let Some(lifecycle) = source.evaluation_lifecycle.as_ref()
        && lifecycle.source_attempt == rebound.source_attempt
        && lifecycle.consumed_verdict.is_none()
    {
        rebound.route_generation = lifecycle.route_generation;
        rebound.pipeline_id = lifecycle.pipeline_id.clone();
    } else {
        rebound.route_generation = 0;
        rebound.pipeline_id =
            pipeline_id(&source.id, rebound.source_attempt, source.loop_iteration);
    }
    rebound.plan_hash = compute_plan_hash(&rebound)?;
    validate_plan(&rebound)?;
    Ok(rebound)
}

fn prepare_rearm_plan(
    graph: &WorkGraph,
    task_id: &str,
    source: &Task,
) -> Result<AgencyDispatchPlan> {
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| anyhow::anyhow!("evaluation satellite {task_id:?} is missing"))?;
    let previous = match task.agency_dispatch.as_ref() {
        Some(plan) => {
            validate_plan(plan)?;
            plan.clone()
        }
        None => migrate_legacy_plan(source, task)?,
    };
    rebind_plan_to_source(&previous, source)
}

fn apply_rearm_plan(task: &mut Task, plan: AgencyDispatchPlan, actor: &str) {
    let primary = &plan.calls[0];
    task.status = Status::Open;
    task.assigned = None;
    task.started_at = None;
    task.completed_at = None;
    task.failure_reason = None;
    task.wait_condition = None;
    task.spawn_failures = 0;
    // Compatibility mirrors follow the persisted plan; no route is resolved
    // again from ambient config during retry or repair.
    task.model = Some(primary.route.clone());
    task.provider = Some(primary.system.handler.clone());
    task.endpoint = primary.endpoint.clone();
    task.reasoning = primary.reasoning;
    task.agency_dispatch = Some(plan.clone());
    task.evaluation_lifecycle = Some(lifecycle_for_plan(&plan));
    task.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: Some(actor.to_string()),
        user: None,
        message: format!(
            "Rearmed exact persisted route for source attempt {}; plan={}",
            plan.source_attempt, plan.plan_hash
        ),
    });
}

fn reasoning_migration_pipeline_id(
    old_pipeline_id: &str,
    source_attempt: u32,
    route_generation: u32,
    plans: &[(String, AgencyDispatchPlan, Vec<AgencyReasoningResolution>)],
) -> String {
    let mut identity = plans
        .iter()
        .map(|(task_id, plan, resolutions)| {
            let calls = resolutions
                .iter()
                .map(|resolution| {
                    format!(
                        "{:?}\0{}\0{}",
                        resolution.stage, resolution.route, resolution.reasoning
                    )
                })
                .collect::<Vec<_>>()
                .join("\0");
            format!("{task_id}\0{}\0{calls}", plan.plan_hash)
        })
        .collect::<Vec<_>>();
    identity.sort();
    let material = format!(
        "wg-eval-pi-reasoning-migration-v1\0{old_pipeline_id}\0{source_attempt}\0{route_generation}\0{}",
        identity.join("\0")
    );
    format!(
        "evalp-{}",
        &blake3::hash(material.as_bytes()).to_hex()[..24]
    )
}

fn reasoning_migration_error(graph: &mut WorkGraph, source_id: &str, error: anyhow::Error) -> bool {
    let source = graph
        .get_task_mut(source_id)
        .expect("reasoning migration source snapshot came from graph");
    if source.evaluation_lifecycle.is_none() {
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(source));
    }
    lifecycle_conflict(
        source,
        format!(
            "error[WG-EVAL-PI-REASONING-MIGRATION-AMBIGUOUS]: operator action required; no agency call was rearmed: {error:#}"
        ),
    )
}

/// Explicit, bounded migration boundary for pre-Pi persisted agency plans.
///
/// This runs in the coordinator's single graph transaction before ordinary
/// pipeline repair. It authenticates every old plan hash, accepts only exact
/// `pi:<provider>:<model>` routes, resolves each absent effort from the
/// authoritative stage role/tier configuration, and then atomically moves the
/// source plus all of its satellites to one newly hashed generation. No model
/// call is made here. Original plans, producer ids and failures are retained in
/// append-only `plan_migrations` audit rows.
pub fn migrate_missing_pi_reasoning(graph: &mut WorkGraph, config: &Config) -> bool {
    let source_ids = graph
        .tasks()
        .filter(|task| matches!(task.status, Status::PendingEval | Status::FailedPendingEval))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let mut modified = false;

    for source_id in source_ids {
        let source_snapshot = graph
            .get_task(&source_id)
            .expect("collected reasoning migration source")
            .clone();
        let mut source_lifecycle = source_snapshot
            .evaluation_lifecycle
            .clone()
            .unwrap_or_else(|| EvaluationLifecycle::for_source(&source_snapshot));
        if source_lifecycle.consumed_verdict.is_some() {
            continue;
        }

        // Do not overwrite an unrelated verdict or active-run ambiguity. The
        // historical repair-exhausted diagnostic is specifically recoverable
        // because it was caused by replaying these same invalid bytes.
        if let Some(diagnostic) = source_lifecycle.diagnostic.as_deref()
            && !diagnostic.contains("WG-EVAL-PIPELINE-REPAIR-EXHAUSTED")
            && !diagnostic.contains("WG-PI-REASONING-MISSING")
            && !diagnostic.contains("WG-EVAL-PI-REASONING-MIGRATION")
        {
            continue;
        }

        let satellite_ids = [
            format!(".flip-{source_id}"),
            format!(".evaluate-{source_id}"),
        ];
        let mut old_rows = Vec::new();
        let mut has_missing_reasoning = false;
        let mut preparation_error = None;

        for task_id in satellite_ids {
            let Some(task) = graph.get_task(&task_id).cloned() else {
                if task_id.starts_with(".evaluate-") {
                    preparation_error = Some(anyhow::anyhow!(
                        "required evaluator satellite {task_id:?} is missing"
                    ));
                }
                continue;
            };
            let Some(old_plan) = task.agency_dispatch.clone() else {
                preparation_error = Some(anyhow::anyhow!(
                    "satellite {task_id:?} has no persisted agency plan"
                ));
                break;
            };
            if let Err(error) = validate_plan(&old_plan) {
                preparation_error = Some(anyhow::anyhow!(
                    "satellite {task_id:?} has an invalid historical plan: {error:#}"
                ));
                break;
            }
            if old_plan.task_id != task_id
                || old_plan.source_task != source_id
                || old_plan.source_attempt != source_lifecycle.source_attempt
                || old_plan.pipeline_id != source_lifecycle.pipeline_id
                || old_plan.route_generation != source_lifecycle.route_generation
            {
                preparation_error = Some(anyhow::anyhow!(
                    "satellite {task_id:?} plan identity does not match authoritative pipeline {} attempt {} generation {}",
                    source_lifecycle.pipeline_id,
                    source_lifecycle.source_attempt,
                    source_lifecycle.route_generation
                ));
                break;
            }
            if task.status == Status::InProgress {
                preparation_error = Some(anyhow::anyhow!(
                    "satellite {task_id:?} is still active; refusing to relabel its producer run"
                ));
                break;
            }
            has_missing_reasoning |= old_plan.calls.iter().any(|call| call.reasoning.is_none());
            old_rows.push((task, old_plan));
        }

        if !has_missing_reasoning {
            continue;
        }
        if source_lifecycle
            .plan_migrations
            .iter()
            .filter(|migration| migration.source_attempt == source_lifecycle.source_attempt)
            .count()
            >= MAX_REASONING_MIGRATIONS_PER_SOURCE_ATTEMPT
        {
            preparation_error = Some(anyhow::anyhow!(
                "source attempt {} already crossed its one allowed reasoning migration boundary",
                source_lifecycle.source_attempt
            ));
        }
        if let Some(error) = preparation_error {
            modified |= reasoning_migration_error(graph, &source_id, error);
            continue;
        }

        let mut prepared = Vec::new();
        for (task, old_plan) in &old_rows {
            let mut new_plan = old_plan.clone();
            let mut resolutions = Vec::new();
            for call in &mut new_plan.calls {
                if let Err(error) = parse_exact_pi_route(&call.route) {
                    preparation_error = Some(anyhow::anyhow!(
                        "satellite {:?} stage {:?} route {:?} is not an exact Pi route: {error}; legacy non-Pi/malformed plans cannot be migrated",
                        task.id,
                        call.stage,
                        call.route
                    ));
                    break;
                }
                let was_missing = call.reasoning.is_none();
                let (reasoning, provenance, config_source) = if let Some(reasoning) = call.reasoning
                {
                    (
                        reasoning,
                        ReasoningProvenance::Explicit,
                        "persisted-plan".to_string(),
                    )
                } else {
                    let resolved = config.resolve_reasoning_detail(call.stage.role());
                    let Some(reasoning) = resolved.level else {
                        preparation_error = Some(anyhow::anyhow!(
                            "stage {:?} route {:?} has no authoritative reasoning; set models.{}.reasoning or tiers.{}_reasoning, then retry the coordinator tick",
                            call.stage,
                            call.route,
                            call.stage.role(),
                            call.stage.role().default_tier()
                        ));
                        break;
                    };
                    let Some(config_source) = resolved.source else {
                        preparation_error = Some(anyhow::anyhow!(
                            "stage {:?} resolved reasoning without auditable configuration provenance",
                            call.stage
                        ));
                        break;
                    };
                    (reasoning, resolved.provenance, config_source)
                };
                call.reasoning = Some(reasoning);
                resolutions.push(AgencyReasoningResolution {
                    stage: call.stage,
                    route: call.route.clone(),
                    reasoning,
                    was_missing,
                    provenance,
                    config_source,
                });
            }
            if preparation_error.is_some() {
                break;
            }
            prepared.push((task.id.clone(), new_plan, resolutions));
        }
        if let Some(error) = preparation_error {
            modified |= reasoning_migration_error(graph, &source_id, error);
            continue;
        }

        let route_generation = source_lifecycle.route_generation.saturating_add(1);
        let new_pipeline_id = reasoning_migration_pipeline_id(
            &source_lifecycle.pipeline_id,
            source_lifecycle.source_attempt,
            route_generation,
            &prepared,
        );
        for (_, plan, _) in &mut prepared {
            plan.route_generation = route_generation;
            plan.pipeline_id = new_pipeline_id.clone();
            match compute_plan_hash(plan).and_then(|hash| {
                plan.plan_hash = hash;
                validate_executable_plan(plan)
            }) {
                Ok(()) => {}
                Err(error) => {
                    preparation_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = preparation_error {
            modified |= reasoning_migration_error(graph, &source_id, error);
            continue;
        }

        let migrated_at = Utc::now().to_rfc3339();
        let boundary = format!(
            "evalm-{}",
            &blake3::hash(new_pipeline_id.as_bytes()).to_hex()[..24]
        );
        let prior_source_diagnostic = source_lifecycle.diagnostic.clone();
        let old_pipeline_id = source_lifecycle.pipeline_id.clone();
        let mut audit_rows = Vec::new();
        for ((old_task, old_plan), (_, new_plan, resolutions)) in
            old_rows.into_iter().zip(prepared.into_iter())
        {
            let audit = AgencyPlanMigration {
                schema: AGENCY_REASONING_MIGRATION_SCHEMA,
                boundary: boundary.clone(),
                migrated_at: migrated_at.clone(),
                source_task: source_id.clone(),
                source_attempt: source_lifecycle.source_attempt,
                route_generation,
                task_id: old_task.id.clone(),
                old_pipeline_id: old_pipeline_id.clone(),
                new_pipeline_id: new_pipeline_id.clone(),
                old_plan,
                new_plan: new_plan.clone(),
                reasoning: resolutions,
                prior_producer_run_id: old_task.assigned.clone(),
                prior_status: old_task.status,
                prior_started_at: old_task.started_at.clone(),
                prior_completed_at: old_task.completed_at.clone(),
                prior_failure_reason: old_task.failure_reason.clone(),
                prior_source_diagnostic: prior_source_diagnostic.clone(),
            };
            let task = graph
                .get_task_mut(&old_task.id)
                .expect("prepared reasoning migration satellite");
            apply_rearm_plan(task, new_plan.clone(), "eval-reasoning-migration");
            if let Some(lifecycle) = task.evaluation_lifecycle.as_mut() {
                lifecycle.route_generation = route_generation;
            }
            task.log.push(LogEntry {
                timestamp: migrated_at.clone(),
                actor: Some("eval-reasoning-migration".to_string()),
                user: None,
                message: format!(
                    "Migrated pre-Pi reasoning atomically: boundary={boundary} generation={route_generation} old_plan={} new_plan={} old_pipeline={old_pipeline_id} new_pipeline={new_pipeline_id}; prior producer/failure retained in source audit",
                    audit.old_plan.plan_hash, audit.new_plan.plan_hash
                ),
            });
            audit_rows.push(audit);
        }

        source_lifecycle.pipeline_id = new_pipeline_id.clone();
        source_lifecycle.route_generation = route_generation;
        source_lifecycle.schedule_attempts = 0;
        source_lifecycle.transport_attempts = 0;
        source_lifecycle.semantic_attempts = 0;
        source_lifecycle.execution_state = EvaluationExecutionState::Ready;
        source_lifecycle.linked_flip_verdict = None;
        source_lifecycle.linked_eval_verdict = None;
        source_lifecycle.consumed_verdict = None;
        source_lifecycle.repair_attempts = 0;
        source_lifecycle.repair_version = EVAL_LIFECYCLE_SCHEMA;
        source_lifecycle.diagnostic = None;
        source_lifecycle.plan_migrations.extend(audit_rows);
        // A re-armed source carries a complete gate identity so downstream
        // reconciliation stays verdict-driven: a stale old-generation verdict
        // is then a clean no-op rather than a normalization that flips the
        // reconcile return value. PendingEval is always a hard gate.
        if source_lifecycle.gate_policy.is_none() {
            let threshold = config.agency.eval_gate_threshold.unwrap_or(0.7);
            let policy = hard_gate_policy_for(graph, &source_id, threshold);
            if policy.validate().is_ok() {
                source_lifecycle.gate_policy = Some(policy);
                source_lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
                    outcome: EvaluationGateOutcome::AwaitingEvidence,
                    evaluator_verdict: None,
                    flip_verdict: None,
                    summary:
                        "re-armed migrated pipeline as a required gate; awaiting exact attempt-bound required verdicts"
                            .to_string(),
                });
            }
        }
        let source = graph
            .get_task_mut(&source_id)
            .expect("prepared reasoning migration source");
        source.evaluation_lifecycle = Some(source_lifecycle);
        source.log.push(LogEntry {
            timestamp: migrated_at,
            actor: Some("eval-reasoning-migration".to_string()),
            user: None,
            message: format!(
                "Migrated/rearmed agency reasoning exactly once without rerunning source work: boundary={boundary} source_attempt={} generation={route_generation} old_pipeline={old_pipeline_id} new_pipeline={new_pipeline_id}",
                source
                    .evaluation_lifecycle
                    .as_ref()
                    .expect("installed migration lifecycle")
                    .source_attempt
            ),
        });
        modified = true;
    }

    modified
}

fn reset_satellite_for_source(graph: &mut WorkGraph, task_id: &str, source: &Task) -> Result<bool> {
    if graph.get_task(task_id).is_none() {
        return Ok(false);
    }
    let plan = prepare_rearm_plan(graph, task_id, source)?;
    let task = graph.get_task_mut(task_id).expect("plan came from task");
    apply_rearm_plan(task, plan, "eval-lifecycle-reconcile");
    Ok(true)
}

/// Rearm an existing evaluation chain while preserving its exact prior call
/// identities. This low-level helper assumes the caller has already minted the
/// authoritative lifecycle on `source`.
pub fn rearm_satellites_for_source(graph: &mut WorkGraph, source: &Task) -> bool {
    if source.id.starts_with('.') {
        return false;
    }
    let mut modified = false;
    for task_id in [
        format!(".flip-{}", source.id),
        format!(".evaluate-{}", source.id),
    ] {
        match reset_satellite_for_source(graph, &task_id, source) {
            Ok(changed) => modified |= changed,
            Err(error) => {
                if let Some(task) = graph.get_task_mut(&task_id) {
                    lifecycle_conflict(task, format!("error[WG-EVAL-PIPELINE-REARM]: {error:#}"));
                    modified = true;
                }
            }
        }
    }
    modified
}

/// Atomically begin a new source execution attempt and rearm every existing
/// evaluation satellite to that exact attempt. Callers invoke this from the
/// same graph transaction that resets the source to a dispatchable state.
/// Durable verdict files are never touched; only the mutable execution plans
/// are rebound to the newly minted pipeline.
pub fn begin_source_attempt(graph: &mut WorkGraph, source_id: &str, reason: &str) -> bool {
    let Some(snapshot) = graph.get_task(source_id).cloned() else {
        return false;
    };
    if snapshot.id.starts_with('.') {
        return false;
    }
    let satellite_ids = [
        format!(".flip-{source_id}"),
        format!(".evaluate-{source_id}"),
    ];
    let has_pipeline = snapshot.evaluation_lifecycle.is_some()
        || satellite_ids
            .iter()
            .any(|task_id| graph.get_task(task_id).is_some());
    if !has_pipeline {
        return false;
    }

    let derived = EvaluationLifecycle::for_source(&snapshot).source_attempt;
    let highest_existing = std::iter::once(
        snapshot
            .evaluation_lifecycle
            .as_ref()
            .map(|lifecycle| lifecycle.source_attempt),
    )
    .chain(satellite_ids.iter().map(|task_id| {
        graph
            .get_task(task_id)
            .and_then(|task| task.agency_dispatch.as_ref())
            .map(|plan| plan.source_attempt)
    }))
    .flatten()
    .max()
    .unwrap_or(0);
    let source_attempt = derived.max(highest_existing.saturating_add(1));

    let prior_gate_policy = snapshot
        .evaluation_lifecycle
        .as_ref()
        .and_then(|lifecycle| lifecycle.gate_policy.clone());
    let mut minted = snapshot.clone();
    let mut next_lifecycle = EvaluationLifecycle::for_source_attempt(&minted, source_attempt);
    next_lifecycle.gate_policy = prior_gate_policy;
    if next_lifecycle.gate_policy.is_some() {
        next_lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
            outcome: EvaluationGateOutcome::AwaitingEvidence,
            evaluator_verdict: None,
            flip_verdict: None,
            summary: "in-place rescue retained the prior attempt's exact gate policy; awaiting new attempt-bound verdicts".to_string(),
        });
    }
    minted.evaluation_lifecycle = Some(next_lifecycle);
    if let Some(source) = graph.get_task_mut(source_id) {
        source.evaluation_lifecycle = minted.evaluation_lifecycle.clone();
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("eval-lifecycle-attempt".to_string()),
            user: None,
            message: format!(
                "Minted evaluation pipeline {} for source attempt {} ({reason})",
                source
                    .evaluation_lifecycle
                    .as_ref()
                    .expect("just minted")
                    .pipeline_id,
                source_attempt
            ),
        });
    }

    // Prepare every plan before opening any row. A retry must never expose a
    // half-rearmed pipeline, nor relabel an old satellite that is still live.
    let mut prepared = Vec::new();
    let mut conflicts = Vec::new();
    for task_id in &satellite_ids {
        let Some(satellite) = graph.get_task(task_id) else {
            continue;
        };
        if satellite.status == Status::InProgress || satellite.assigned.is_some() {
            conflicts.push(format!(
                "{task_id} is still active on the previous source attempt"
            ));
            continue;
        }
        match prepare_rearm_plan(graph, task_id, &minted) {
            Ok(plan) => prepared.push((task_id.clone(), plan)),
            Err(error) => conflicts.push(format!("{task_id}: {error:#}")),
        }
    }

    if conflicts.is_empty() {
        for (task_id, plan) in prepared {
            apply_rearm_plan(
                graph.get_task_mut(&task_id).expect("prepared satellite"),
                plan,
                "eval-lifecycle-attempt",
            );
        }
    } else {
        let diagnostic = format!(
            "error[WG-EVAL-PIPELINE-REARM]: source attempt {} could not atomically rearm its evaluation pipeline: {}",
            source_attempt,
            conflicts.join("; ")
        );
        if let Some(source) = graph.get_task_mut(source_id) {
            lifecycle_conflict(source, diagnostic.clone());
        }
        for task_id in satellite_ids {
            if let Some(satellite) = graph.get_task_mut(&task_id)
                && satellite.status != Status::InProgress
                && satellite.assigned.is_none()
            {
                satellite.status = Status::Blocked;
                lifecycle_conflict(satellite, diagnostic.clone());
            }
        }
    }
    true
}

const MAX_PIPELINE_REPAIRS_PER_SOURCE_ATTEMPT: u16 = 1;

fn satellite_has_linked_stage(task: &Task) -> bool {
    let Some(lifecycle) = task.evaluation_lifecycle.as_ref() else {
        return false;
    };
    if task.id.starts_with(".evaluate-") {
        lifecycle.linked_eval_verdict.is_some()
    } else {
        lifecycle.linked_flip_verdict.is_some()
    }
}

/// Public read-only lifecycle health used by `status`, `show`, and
/// `why-blocked`. It intentionally relies only on the atomically persisted
/// graph. Durable evidence ambiguity is first recorded on the source by the
/// coordinator transaction and then appears here as operator-required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationHealthState {
    MigrationRequired,
    MigratedRearmed,
    ActiveEvaluation,
    RepairablePipelineDrift,
    OperatorRequiredAmbiguity,
}

impl std::fmt::Display for EvaluationHealthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::MigrationRequired => "migration-required",
            Self::MigratedRearmed => "migrated-rearmed",
            Self::ActiveEvaluation => "active-evaluation",
            Self::RepairablePipelineDrift => "repairable-pipeline-drift",
            Self::OperatorRequiredAmbiguity => "operator-required-ambiguity",
        };
        formatter.write_str(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationHealth {
    pub state: EvaluationHealthState,
    pub pipeline_id: String,
    pub source_attempt: u32,
    pub route_generation: u32,
    pub migration_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_verdict: Option<String>,
    pub diagnostic: String,
}

/// Read-only gate diagnostics used by `wg show` and `wg status`. Historical
/// completions are never rewritten; exact consumed evidence is instead audited
/// against the currently configured thresholds and surfaced loudly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvaluationGateDiagnostics {
    pub pipeline_id: String,
    pub source_attempt: u32,
    pub applicability: String,
    pub evaluator_threshold: Option<f64>,
    pub flip_policy: String,
    pub flip_threshold: Option<f64>,
    pub outcome_provenance: Option<EvaluationOutcomeProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<String>,
    #[serde(default)]
    pub audit_alert: bool,
}

pub fn evaluation_gate_diagnostics(
    source: &Task,
    verdicts: std::result::Result<&[DurableEvalVerdict], &str>,
    current_evaluator_threshold: Option<f64>,
    current_flip_threshold: Option<f64>,
) -> Option<EvaluationGateDiagnostics> {
    let lifecycle = source.evaluation_lifecycle.as_ref()?;
    let durable_evidence_error = verdicts.as_ref().err().copied();
    if let Some(policy) = lifecycle.gate_policy.as_ref() {
        let audit = lifecycle.diagnostic.clone().or_else(|| {
            durable_evidence_error
                .map(|error| format!("durable gate evidence unavailable (fail-closed): {error}"))
        });
        return Some(EvaluationGateDiagnostics {
            pipeline_id: lifecycle.pipeline_id.clone(),
            source_attempt: lifecycle.source_attempt,
            applicability: match policy.applicability {
                EvaluationGateApplicability::Advisory => "advisory".to_string(),
                EvaluationGateApplicability::Required => "required".to_string(),
            },
            evaluator_threshold: policy.evaluator_threshold,
            flip_policy: match policy.flip_policy {
                FlipVerdictPolicy::NotScheduled => "not-scheduled".to_string(),
                FlipVerdictPolicy::Advisory => "advisory".to_string(),
                FlipVerdictPolicy::Required => "required-strict".to_string(),
            },
            flip_threshold: policy.flip_threshold,
            outcome_provenance: lifecycle.outcome_provenance.clone(),
            audit: audit.clone(),
            audit_alert: audit.is_some(),
        });
    }

    let consumed = lifecycle.consumed_verdict.as_deref()?;
    let evaluator_threshold = current_evaluator_threshold.unwrap_or(0.7);
    let flip_threshold = current_flip_threshold.unwrap_or(evaluator_threshold);
    let mut alert = false;
    let audit = match verdicts {
        Err(error) => {
            alert = true;
            format!(
                "HISTORICAL AUDIT ALERT: immutable Done outcome has no persisted gate policy and verdict evidence is unavailable: {error}"
            )
        }
        Ok(verdicts) => {
            let evals: Vec<_> = verdicts
                .iter()
                .filter(|verdict| {
                    verdict.verdict_id == consumed
                        && verdict.source_task == source.id
                        && verdict.pipeline_id == lifecycle.pipeline_id
                        && verdict.source_attempt == lifecycle.source_attempt
                        && verdict.stage == AgencyStage::Evaluate
                })
                .collect();
            let flips: Vec<_> = lifecycle
                .linked_flip_verdict
                .as_deref()
                .map(|flip_id| {
                    verdicts
                        .iter()
                        .filter(|verdict| {
                            verdict.verdict_id == flip_id
                                && verdict.source_task == source.id
                                && verdict.pipeline_id == lifecycle.pipeline_id
                                && verdict.source_attempt == lifecycle.source_attempt
                                && verdict.stage == AgencyStage::FlipComparison
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if evals.len() != 1 || (lifecycle.linked_flip_verdict.is_some() && flips.len() != 1) {
                alert = true;
                format!(
                    "HISTORICAL AUDIT ALERT: immutable Done outcome consumed ambiguous/missing exact evidence (evaluator matches={}, FLIP matches={}); operator review required",
                    evals.len(),
                    flips.len()
                )
            } else {
                let eval = evals[0];
                let eval_failed = !eval.score.is_finite() || eval.score < evaluator_threshold;
                let flip_failed = flips
                    .first()
                    .is_some_and(|flip| !flip.score.is_finite() || flip.score < flip_threshold);
                if eval_failed || flip_failed {
                    alert = true;
                    format!(
                        "HISTORICAL AUDIT ALERT: immutable Done outcome was accepted below current strict thresholds; evaluator={:.2}/{:.2}, FLIP={}; history was not rewritten",
                        eval.score,
                        evaluator_threshold,
                        flips.first().map_or_else(
                            || "not-linked".to_string(),
                            |flip| format!("{:.2}/{:.2}", flip.score, flip_threshold)
                        )
                    )
                } else {
                    format!(
                        "historical immutable outcome has no persisted policy; exact evidence meets current thresholds (evaluator={:.2}/{:.2}, FLIP={})",
                        eval.score,
                        evaluator_threshold,
                        flips.first().map_or_else(
                            || "not-linked".to_string(),
                            |flip| format!("{:.2}/{:.2}", flip.score, flip_threshold)
                        )
                    )
                }
            }
        }
    };
    Some(EvaluationGateDiagnostics {
        pipeline_id: lifecycle.pipeline_id.clone(),
        source_attempt: lifecycle.source_attempt,
        applicability: "historical-unclassified".to_string(),
        evaluator_threshold: Some(evaluator_threshold),
        flip_policy: if lifecycle.linked_flip_verdict.is_some() {
            "historical-linked-audit".to_string()
        } else {
            "historical-not-linked".to_string()
        },
        flip_threshold: lifecycle
            .linked_flip_verdict
            .as_ref()
            .map(|_| flip_threshold),
        outcome_provenance: lifecycle.outcome_provenance.clone(),
        audit: Some(audit),
        audit_alert: alert,
    })
}

pub fn evaluation_health(graph: &WorkGraph, source_id: &str) -> Option<EvaluationHealth> {
    let source = graph.get_task(source_id)?;
    if !matches!(
        source.status,
        Status::PendingEval | Status::FailedPendingEval
    ) {
        return None;
    }
    let Some(lifecycle) = source.evaluation_lifecycle.as_ref() else {
        return Some(EvaluationHealth {
            state: EvaluationHealthState::RepairablePipelineDrift,
            pipeline_id: "unminted".to_string(),
            source_attempt: 0,
            route_generation: 0,
            migration_count: 0,
            consumed_verdict: None,
            diagnostic: "source lifecycle is missing; coordinator repair must mint it".to_string(),
        });
    };

    let mut migration_required = Vec::new();
    let mut migration_rejected = Vec::new();
    for task_id in [
        format!(".flip-{source_id}"),
        format!(".evaluate-{source_id}"),
    ] {
        let Some(plan) = graph
            .get_task(&task_id)
            .and_then(|task| task.agency_dispatch.as_ref())
        else {
            continue;
        };
        if !plan.calls.iter().any(|call| call.reasoning.is_none()) {
            continue;
        }
        let exact_pi = validate_plan(plan).is_ok()
            && plan
                .calls
                .iter()
                .all(|call| parse_exact_pi_route(&call.route).is_ok());
        if exact_pi {
            migration_required.push(format!(
                "{task_id} plan {} has exact Pi route(s) but missing reasoning",
                plan.plan_hash
            ));
        } else {
            migration_rejected.push(format!(
                "{task_id} missing-reasoning plan is non-Pi, malformed, or hash-invalid"
            ));
        }
    }
    if !migration_rejected.is_empty() {
        return Some(EvaluationHealth {
            state: EvaluationHealthState::OperatorRequiredAmbiguity,
            pipeline_id: lifecycle.pipeline_id.clone(),
            source_attempt: lifecycle.source_attempt,
            route_generation: lifecycle.route_generation,
            migration_count: lifecycle.plan_migrations.len(),
            consumed_verdict: lifecycle.consumed_verdict.clone(),
            diagnostic: migration_rejected.join("; "),
        });
    }
    if !migration_required.is_empty()
        && lifecycle.diagnostic.as_deref().is_none_or(|diagnostic| {
            diagnostic.contains("WG-EVAL-PIPELINE-REPAIR-EXHAUSTED")
                || diagnostic.contains("WG-PI-REASONING-MISSING")
        })
    {
        return Some(EvaluationHealth {
            state: EvaluationHealthState::MigrationRequired,
            pipeline_id: lifecycle.pipeline_id.clone(),
            source_attempt: lifecycle.source_attempt,
            route_generation: lifecycle.route_generation,
            migration_count: lifecycle.plan_migrations.len(),
            consumed_verdict: lifecycle.consumed_verdict.clone(),
            diagnostic: format!(
                "{}; coordinator must resolve authoritative role/tier reasoning at the explicit migration boundary before execution",
                migration_required.join("; ")
            ),
        });
    }
    if let Some(diagnostic) = lifecycle.diagnostic.as_ref() {
        return Some(EvaluationHealth {
            state: EvaluationHealthState::OperatorRequiredAmbiguity,
            pipeline_id: lifecycle.pipeline_id.clone(),
            source_attempt: lifecycle.source_attempt,
            route_generation: lifecycle.route_generation,
            migration_count: lifecycle.plan_migrations.len(),
            consumed_verdict: lifecycle.consumed_verdict.clone(),
            diagnostic: diagnostic.clone(),
        });
    }

    let flip_required = lifecycle
        .gate_policy
        .as_ref()
        .is_some_and(|policy| policy.flip_policy == FlipVerdictPolicy::Required)
        || (lifecycle.gate_policy.is_none()
            && graph
                .get_task(&format!(".evaluate-{source_id}"))
                .is_some_and(|task| task.after.contains(&format!(".flip-{source_id}"))));
    if let Some(policy) = lifecycle.gate_policy.as_ref() {
        if policy.applicability != EvaluationGateApplicability::Required {
            return Some(EvaluationHealth {
                state: EvaluationHealthState::OperatorRequiredAmbiguity,
                pipeline_id: lifecycle.pipeline_id.clone(),
                source_attempt: lifecycle.source_attempt,
                route_generation: lifecycle.route_generation,
                migration_count: lifecycle.plan_migrations.len(),
                consumed_verdict: lifecycle.consumed_verdict.clone(),
                diagnostic:
                    "advisory policy cannot inhabit PendingEval/FailedPendingEval; refusing quality promotion"
                        .to_string(),
            });
        }
        if let Err(error) = policy.validate() {
            return Some(EvaluationHealth {
                state: EvaluationHealthState::OperatorRequiredAmbiguity,
                pipeline_id: lifecycle.pipeline_id.clone(),
                source_attempt: lifecycle.source_attempt,
                route_generation: lifecycle.route_generation,
                migration_count: lifecycle.plan_migrations.len(),
                consumed_verdict: lifecycle.consumed_verdict.clone(),
                diagnostic: format!("invalid persisted gate policy: {error:#}"),
            });
        }
    }

    let mut repairable = Vec::new();
    let mut operator = Vec::new();
    for (task_id, required) in [
        (format!(".flip-{source_id}"), flip_required),
        (format!(".evaluate-{source_id}"), true),
    ] {
        let Some(satellite) = graph.get_task(&task_id) else {
            if required {
                operator.push(format!("required gate satellite {task_id} is missing"));
            }
            continue;
        };
        let plan = match satellite.agency_dispatch.as_ref() {
            Some(plan) => plan,
            None => {
                if migrate_legacy_plan(source, satellite).is_ok()
                    && satellite.status != Status::InProgress
                    && satellite.assigned.is_none()
                {
                    repairable.push(format!(
                        "{task_id} has a losslessly recoverable legacy plan"
                    ));
                } else {
                    operator.push(format!("{task_id} has no unambiguous persisted route"));
                }
                continue;
            }
        };
        let matches = plan.source_task == source.id
            && plan.pipeline_id == lifecycle.pipeline_id
            && plan.source_attempt == lifecycle.source_attempt;
        if !matches {
            if satellite.status == Status::InProgress || satellite.assigned.is_some() {
                operator.push(format!(
                    "{task_id} is active on stale pipeline {}",
                    plan.pipeline_id
                ));
            } else {
                repairable.push(format!(
                    "{task_id} is on stale pipeline {}",
                    plan.pipeline_id
                ));
            }
        } else if matches!(
            satellite.status,
            Status::Done
                | Status::Failed
                | Status::Abandoned
                | Status::Blocked
                | Status::Incomplete
                | Status::PendingValidation
                | Status::PendingEval
                | Status::FailedPendingEval
        ) && !satellite_has_linked_stage(satellite)
        {
            if lifecycle.repair_attempts < MAX_PIPELINE_REPAIRS_PER_SOURCE_ATTEMPT {
                repairable.push(format!(
                    "{task_id} is terminal without durable current-attempt evidence"
                ));
            } else {
                operator.push(format!("{task_id} exhausted bounded pipeline repair"));
            }
        }
    }

    let freshly_rearmed = lifecycle
        .plan_migrations
        .iter()
        .any(|migration| migration.source_attempt == lifecycle.source_attempt)
        && [
            format!(".flip-{source_id}"),
            format!(".evaluate-{source_id}"),
        ]
        .into_iter()
        .filter_map(|task_id| graph.get_task(&task_id))
        .all(|task| task.status == Status::Open && task.assigned.is_none());
    let (state, diagnostic) = if !operator.is_empty() {
        (
            EvaluationHealthState::OperatorRequiredAmbiguity,
            operator.join("; "),
        )
    } else if !repairable.is_empty() {
        (
            EvaluationHealthState::RepairablePipelineDrift,
            repairable.join("; "),
        )
    } else if freshly_rearmed {
        (
            EvaluationHealthState::MigratedRearmed,
            "reasoning migration committed atomically; repaired satellites are ready and the source worker was not rerun".to_string(),
        )
    } else {
        (
            EvaluationHealthState::ActiveEvaluation,
            "current-attempt evaluation is queued, running, or durably linking".to_string(),
        )
    };
    Some(EvaluationHealth {
        state,
        pipeline_id: lifecycle.pipeline_id.clone(),
        source_attempt: lifecycle.source_attempt,
        route_generation: lifecycle.route_generation,
        migration_count: lifecycle.plan_migrations.len(),
        consumed_verdict: lifecycle.consumed_verdict.clone(),
        diagnostic,
    })
}

fn repair_pending_pipeline(
    graph: &mut WorkGraph,
    source_id: &str,
    has_flip_evidence: bool,
    has_eval_evidence: bool,
    flip_required: bool,
) -> bool {
    let Some(source_snapshot) = graph.get_task(source_id).cloned() else {
        return false;
    };
    if !matches!(
        source_snapshot.status,
        Status::PendingEval | Status::FailedPendingEval
    ) {
        return false;
    }
    let Some(source_lifecycle) = source_snapshot.evaluation_lifecycle.as_ref() else {
        return false;
    };
    if source_lifecycle.consumed_verdict.is_some() {
        return false;
    }

    let mut prepared = Vec::<(String, AgencyDispatchPlan)>::new();
    let mut conflicts = Vec::new();
    for (task_id, has_evidence, required) in [
        (
            format!(".flip-{source_id}"),
            has_flip_evidence,
            flip_required,
        ),
        (format!(".evaluate-{source_id}"), has_eval_evidence, true),
    ] {
        let Some(satellite) = graph.get_task(&task_id) else {
            if required && !has_evidence {
                conflicts.push(format!(
                    "required gate satellite {task_id} is missing and no exact verdict exists"
                ));
            }
            continue;
        };
        let plan_matches = satellite.agency_dispatch.as_ref().is_some_and(|plan| {
            plan.source_task == source_id
                && plan.pipeline_id == source_lifecycle.pipeline_id
                && plan.source_attempt == source_lifecycle.source_attempt
        });
        // Verified durable evidence is allowed to backfill a claimed,
        // completed pre-schema row through `install_completed_legacy_plan`.
        // It must never be rearmed or relabeled as an unexecuted run.
        if has_evidence && satellite.status == Status::Done && satellite.agency_dispatch.is_none() {
            continue;
        }
        let terminal_without_evidence = matches!(
            satellite.status,
            Status::Done
                | Status::Failed
                | Status::Abandoned
                | Status::Blocked
                | Status::Incomplete
                | Status::PendingValidation
                | Status::PendingEval
                | Status::FailedPendingEval
        ) && !has_evidence;
        if plan_matches && !terminal_without_evidence {
            continue;
        }
        if (satellite.status == Status::InProgress || satellite.assigned.is_some()) && !plan_matches
        {
            conflicts.push(format!(
                "{task_id} is still active on a mismatched pipeline; refusing to relabel its run"
            ));
            continue;
        }
        match prepare_rearm_plan(graph, &task_id, &source_snapshot) {
            Ok(plan) => prepared.push((task_id, plan)),
            Err(error) => conflicts.push(format!("{task_id}: {error:#}")),
        }
    }

    if !conflicts.is_empty() {
        let source = graph.get_task_mut(source_id).expect("source snapshot");
        return lifecycle_conflict(
            source,
            format!(
                "error[WG-EVAL-PIPELINE-AMBIGUOUS]: operator action required: {}",
                conflicts.join("; ")
            ),
        );
    }
    if prepared.is_empty() {
        return false;
    }
    if source_lifecycle.repair_attempts >= MAX_PIPELINE_REPAIRS_PER_SOURCE_ATTEMPT {
        let ids = prepared
            .iter()
            .map(|(task_id, _)| task_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let source = graph.get_task_mut(source_id).expect("source snapshot");
        return lifecycle_conflict(
            source,
            format!(
                "error[WG-EVAL-PIPELINE-REPAIR-EXHAUSTED]: bounded repair already ran for {}; terminal satellites still lack evidence: {ids}",
                source_lifecycle.pipeline_id
            ),
        );
    }

    for (task_id, plan) in prepared {
        apply_rearm_plan(
            graph.get_task_mut(&task_id).expect("prepared satellite"),
            plan,
            "eval-lifecycle-repair",
        );
    }
    let source = graph.get_task_mut(source_id).expect("source snapshot");
    let lifecycle = source
        .evaluation_lifecycle
        .as_mut()
        .expect("pending source lifecycle");
    lifecycle.repair_attempts = lifecycle.repair_attempts.saturating_add(1);
    lifecycle.repair_version = EVAL_LIFECYCLE_SCHEMA;
    lifecycle.diagnostic = None;
    lifecycle.execution_state = EvaluationExecutionState::Ready;
    source.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: Some("eval-lifecycle-repair".to_string()),
        user: None,
        message: format!(
            "Repaired evaluation pipeline drift for authoritative source attempt {} ({})",
            lifecycle.source_attempt, lifecycle.pipeline_id
        ),
    });
    true
}

/// Compute the hard-gate policy a `PendingEval`/`FailedPendingEval` source
/// must carry once it is presented to a user as evaluation-gated. Shared by
/// historical soft-state normalization in [`reconcile_durable_verdicts`] and
/// by pre-Pi reasoning migration so a re-armed source already carries a
/// complete gate identity (downstream reconciliation then stays verdict-driven
/// and a stale old-generation verdict is a clean no-op).
///
/// `PendingEval` is *always* a real hard gate; advisory evaluators never
/// enter that state.
fn hard_gate_policy_for(
    graph: &WorkGraph,
    source_id: &str,
    threshold: f64,
) -> EvaluationGatePolicy {
    let flip_id = format!(".flip-{source_id}");
    let flip_required = graph
        .get_task(&format!(".evaluate-{source_id}"))
        .is_some_and(|task| task.after.contains(&flip_id));
    EvaluationGatePolicy {
        applicability: EvaluationGateApplicability::Required,
        evaluator_threshold: Some(threshold),
        flip_policy: if flip_required {
            FlipVerdictPolicy::Required
        } else {
            FlipVerdictPolicy::NotScheduled
        },
        flip_threshold: flip_required.then_some(threshold),
        flip_threshold_source: flip_required.then_some(FlipThresholdSource::EvaluatorThreshold),
    }
}

/// Link durable stage evidence and atomically consume all required verdicts
/// into their source task. The caller runs this inside the graph's single
/// `modify_graph` transaction, so the exact-attempt consumption fence and the
/// source transition always land in the same atomic rename.
///
/// Every `PendingEval`/`FailedPendingEval` source is a real hard gate. Advisory
/// evaluations never enter these states and are not consumed into source
/// quality outcomes. Required FLIP and evaluator verdicts are checked
/// independently (strictest-required-verdict semantics); they are never
/// averaged and system-task self-evaluations cannot substitute for either.
pub fn reconcile_durable_verdicts<F>(
    graph: &mut WorkGraph,
    verdicts: &[DurableEvalVerdict],
    threshold: f64,
    auto_rescue: bool,
    max_rescues: u32,
    _legacy_advisory_predicate: F,
) -> bool
where
    F: Fn(&Task) -> bool,
{
    let source_ids: Vec<String> = graph
        .tasks()
        .filter(|task| {
            matches!(task.status, Status::PendingEval | Status::FailedPendingEval)
                || task
                    .evaluation_lifecycle
                    .as_ref()
                    .and_then(|lifecycle| lifecycle.consumed_verdict.as_ref())
                    .is_some()
        })
        .map(|task| task.id.clone())
        .collect();
    let mut modified = false;

    for source_id in source_ids {
        let mut source_snapshot = graph
            .get_task(&source_id)
            .expect("collected source")
            .clone();
        let mut source_lifecycle = source_snapshot
            .evaluation_lifecycle
            .clone()
            .unwrap_or_else(|| EvaluationLifecycle::for_source(&source_snapshot));
        let source_is_pending = matches!(
            source_snapshot.status,
            Status::PendingEval | Status::FailedPendingEval
        );

        // Historical soft-state migration is fail-closed: PendingEval itself
        // is the user-visible assertion that a gate exists. Never reinterpret
        // it as advisory, even when the ambient `eval_gate_all` setting is off.
        //
        // An already-diagnosed lifecycle is different: its gate is known to be
        // unsatisfiable by automation. Do not keep synthesizing/pinning a
        // historical required policy on every coordinator tick. Preserve the
        // stable operator-required diagnostic until the sanctioned `wg retry`
        // or `wg recover` path resets the source to Open with a fresh attempt.
        if source_is_pending
            && source_lifecycle.gate_policy.is_none()
            && source_lifecycle.diagnostic.is_some()
        {
            continue;
        }
        if source_is_pending && source_lifecycle.gate_policy.is_none() {
            let policy = hard_gate_policy_for(graph, &source_id, threshold);
            if let Err(error) = policy.validate() {
                let source = graph.get_task_mut(&source_id).expect("collected source");
                source.evaluation_lifecycle = Some(source_lifecycle);
                modified = true;
                modified |= lifecycle_conflict(
                    source,
                    format!("invalid effective evaluation gate (fail-closed): {error:#}"),
                );
                continue;
            }
            source_lifecycle.gate_policy = Some(policy);
            source_lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
                outcome: EvaluationGateOutcome::AwaitingEvidence,
                evaluator_verdict: None,
                flip_verdict: None,
                summary: "migrated historical soft state as a hard gate; awaiting exact attempt-bound required verdicts".to_string(),
            });
            let source = graph.get_task_mut(&source_id).expect("collected source");
            source.evaluation_lifecycle = Some(source_lifecycle.clone());
            source.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("eval-lifecycle-reconcile".to_string()),
                user: None,
                message: format!(
                    "Pinned historical PendingEval as a required gate at evaluator threshold {:.2}",
                    threshold
                ),
            });
            source_snapshot = source.clone();
            modified = true;
        } else if source_is_pending && source_snapshot.evaluation_lifecycle.is_none() {
            graph
                .get_task_mut(&source_id)
                .expect("collected source")
                .evaluation_lifecycle = Some(source_lifecycle.clone());
            source_snapshot.evaluation_lifecycle = Some(source_lifecycle.clone());
            modified = true;
        }

        if source_is_pending {
            let Some(policy) = source_lifecycle.gate_policy.as_ref() else {
                continue;
            };
            if policy.applicability != EvaluationGateApplicability::Required {
                let source = graph.get_task_mut(&source_id).expect("collected source");
                modified |= lifecycle_conflict(
                    source,
                    "error[WG-EVAL-GATE-POLICY]: advisory evaluation found in PendingEval; refusing quality promotion".to_string(),
                );
                continue;
            }
            if let Err(error) = policy.validate() {
                let source = graph.get_task_mut(&source_id).expect("collected source");
                modified |= lifecycle_conflict(
                    source,
                    format!("invalid persisted evaluation gate (fail-closed): {error:#}"),
                );
                continue;
            }
        }

        let matching: Vec<&DurableEvalVerdict> = verdicts
            .iter()
            .filter(|verdict| {
                verdict.source_task == source_id
                    && verdict.pipeline_id == source_lifecycle.pipeline_id
                    && verdict.source_attempt == source_lifecycle.source_attempt
            })
            .collect();
        // Completed consumed rows are immutable history. Diagnostics audit
        // them read-only; reconciliation never retrofits policy, selects among
        // old evidence, or rewrites their Done status/logs.
        if !source_is_pending && source_lifecycle.consumed_verdict.is_some() {
            continue;
        }
        if let Some(malformed) = matching.iter().find(|verdict| {
            verdict.schema != EVAL_LIFECYCLE_SCHEMA
                || !verdict.score.is_finite()
                || !(0.0..=1.0).contains(&verdict.score)
                || verdict.stage == AgencyStage::FlipInference
        }) {
            let source = graph.get_task_mut(&source_id).expect("collected source");
            modified |= lifecycle_conflict(
                source,
                format!(
                    "error[WG-EVAL-VERDICT-MALFORMED]: verdict {} has invalid schema/stage/non-finite-or-out-of-range score",
                    malformed.verdict_id
                ),
            );
            continue;
        }
        let flips: Vec<_> = matching
            .iter()
            .copied()
            .filter(|verdict| verdict.stage == AgencyStage::FlipComparison)
            .collect();
        let evals: Vec<_> = matching
            .iter()
            .copied()
            .filter(|verdict| verdict.stage == AgencyStage::Evaluate)
            .collect();

        if flips.len() > 1 || evals.len() > 1 {
            let ids = matching
                .iter()
                .map(|verdict| verdict.verdict_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let source = graph.get_task_mut(&source_id).expect("collected source");
            modified |= lifecycle_conflict(
                source,
                format!(
                    "error[WG-EVAL-VERDICT-AMBIGUOUS]: multiple stage verdicts require operator selection: {ids}"
                ),
            );
            continue;
        }

        if let Some(consumed) = source_lifecycle.consumed_verdict.as_deref() {
            if let Some(eval) = evals.first()
                && eval.verdict_id != consumed
            {
                let source = graph.get_task_mut(&source_id).expect("collected source");
                modified |= lifecycle_conflict(
                    source,
                    format!(
                        "error[WG-EVAL-CONSUMPTION-CONFLICT]: source consumed {} but found {}",
                        consumed, eval.verdict_id
                    ),
                );
            }
            continue;
        }

        let flip_required = source_lifecycle
            .gate_policy
            .as_ref()
            .is_some_and(|policy| policy.flip_policy == FlipVerdictPolicy::Required);
        modified |= repair_pending_pipeline(
            graph,
            &source_id,
            !flips.is_empty(),
            !evals.is_empty(),
            flip_required,
        );
        if graph
            .get_task(&source_id)
            .and_then(|source| source.evaluation_lifecycle.as_ref())
            .and_then(|lifecycle| lifecycle.diagnostic.as_ref())
            .is_some()
        {
            continue;
        }

        if let Some(flip) = flips.first() {
            let task_id = format!(".flip-{source_id}");
            modified |= install_completed_legacy_plan(graph, &task_id, flip);
            modified |= mark_satellite_verdict(graph, &task_id, flip);
        }
        if let Some(eval) = evals.first() {
            let task_id = format!(".evaluate-{source_id}");
            modified |= install_completed_legacy_plan(graph, &task_id, eval);
            modified |= mark_satellite_verdict(graph, &task_id, eval);
        }
        if graph
            .get_task(&source_id)
            .and_then(|source| source.evaluation_lifecycle.as_ref())
            .and_then(|lifecycle| lifecycle.diagnostic.as_ref())
            .is_some()
        {
            continue;
        }

        let Some(eval) = evals.first() else {
            continue;
        };
        let flip = flips.first().copied();
        if flip_required && flip.is_none() {
            continue;
        }
        if !source_is_pending {
            continue;
        }

        let policy = source_lifecycle
            .gate_policy
            .as_ref()
            .expect("pending sources were pinned above");
        let evaluator_threshold = policy
            .evaluator_threshold
            .expect("validated required evaluator threshold");
        let flip_threshold = policy.flip_threshold;
        let evaluator_failed = eval.score < evaluator_threshold;
        let flip_failed = flip_required
            && flip.is_some_and(|verdict| {
                verdict.score < flip_threshold.expect("validated required FLIP threshold")
            });
        let hard_reject = evaluator_failed || flip_failed;
        let retry_source = hard_reject
            && source_snapshot.status == Status::PendingEval
            && auto_rescue
            && max_rescues > 0
            && source_snapshot.rescue_count < max_rescues;

        let mut evidence = vec![format!(
            "evaluator {} score={:.2} threshold={:.2} {}",
            eval.verdict_id,
            eval.score,
            evaluator_threshold,
            if evaluator_failed { "FAIL" } else { "PASS" }
        )];
        if flip_required {
            let flip = flip.expect("required FLIP checked above");
            let threshold = flip_threshold.expect("validated required FLIP threshold");
            evidence.push(format!(
                "FLIP {} score={:.2} threshold={:.2} {}",
                flip.verdict_id,
                flip.score,
                threshold,
                if flip_failed { "FAIL" } else { "PASS" }
            ));
        }
        let evidence_summary = evidence.join("; ");

        let source = graph.get_task_mut(&source_id).expect("collected source");
        let lifecycle = source
            .evaluation_lifecycle
            .as_mut()
            .expect("pending source lifecycle was installed");
        lifecycle.linked_flip_verdict = flip.map(|verdict| verdict.verdict_id.clone());
        lifecycle.linked_eval_verdict = Some(eval.verdict_id.clone());
        lifecycle.consumed_verdict = Some(eval.verdict_id.clone());
        lifecycle.execution_state = EvaluationExecutionState::Consumed;
        lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
            outcome: if retry_source {
                EvaluationGateOutcome::RescueRetry
            } else if hard_reject {
                EvaluationGateOutcome::Rejected
            } else {
                EvaluationGateOutcome::Passed
            },
            evaluator_verdict: Some(eval.verdict_id.clone()),
            flip_verdict: flip.map(|verdict| verdict.verdict_id.clone()),
            summary: evidence_summary.clone(),
        });

        if retry_source {
            source.status = Status::Open;
            source.rescue_count = source.rescue_count.saturating_add(1);
            source.assigned = None;
            source.started_at = None;
            source.completed_at = None;
            source.failure_reason = None;
        } else if hard_reject {
            source.status = Status::Failed;
            source.retry_count = source.retry_count.saturating_add(1);
            source.failure_reason = Some(format!(
                "required evaluation gate rejected: {evidence_summary}"
            ));
            source.completed_at = Some(Utc::now().to_rfc3339());
        } else {
            source.status = Status::Done;
            source.rescued |= source_snapshot.status == Status::FailedPendingEval;
            source.completed_at = Some(Utc::now().to_rfc3339());
        }
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("eval-lifecycle-reconcile".to_string()),
            user: None,
            message: format!(
                "Consumed durable verdict {} exactly once under strict required-gate policy: {}; outcome={}",
                eval.verdict_id, evidence_summary, source.status
            ),
        });
        modified = true;

        if retry_source {
            modified |= begin_source_attempt(
                graph,
                &source_id,
                "automatic in-place rescue after rejected required evaluation gate",
            );
        }
    }
    modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ReasoningLevel, RoleModelConfig};

    fn source() -> Task {
        Task {
            id: "source".into(),
            title: "source".into(),
            ..Task::default()
        }
    }

    #[test]
    fn handler_first_plan_round_trips_for_supported_systems() {
        for route in ["pi:openai-codex:gpt-5.6-sol", "pi:openrouter:z-ai/glm-5.2"] {
            let mut config = Config::default();
            config.models.evaluator = Some(RoleModelConfig {
                provider: None,
                model: Some(route.into()),
                tier: None,
                endpoint: Some("named-endpoint".into()),
                reasoning: Some(ReasoningLevel::High),
            });
            let plan = build_plan(
                &config,
                &source(),
                ".evaluate-source",
                DispatchSelectionSource::ScaffoldConfig,
            )
            .unwrap();
            assert_eq!(plan.calls[0].route, route);
            assert_eq!(plan.calls[0].endpoint.as_deref(), Some("named-endpoint"));
            assert_eq!(plan.calls[0].reasoning, Some(ReasoningLevel::High));
            validate_plan(&serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap())
                .unwrap();
        }
    }

    #[test]
    fn legacy_explicit_codex_role_is_rejected_before_persistence() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            provider: Some("codex".into()),
            model: Some("gpt-5.4-mini".into()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let error = build_plan(
            &config,
            &source(),
            ".evaluate-source",
            DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("WG-PI-ROUTE-REQUIRED"));
    }

    #[test]
    fn flip_plan_keeps_distinct_routes() {
        let mut config = Config::default();
        config.models.flip_inference = Some(RoleModelConfig {
            provider: None,
            model: Some("pi:openai-codex:gpt-5.5".into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::High),
        });
        config.models.flip_comparison = Some(RoleModelConfig {
            provider: None,
            model: Some("pi:openai-codex:gpt-5.6-sol".into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::High),
        });
        let plan = build_plan(
            &config,
            &source(),
            ".flip-source",
            DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap();
        assert_eq!(plan.calls[0].route, "pi:openai-codex:gpt-5.5");
        assert_eq!(plan.calls[1].route, "pi:openai-codex:gpt-5.6-sol");
    }

    #[test]
    fn ambiguous_openrouter_split_fails_closed() {
        let satellite = Task {
            id: ".evaluate-source".into(),
            title: "eval".into(),
            model: Some("z-ai/glm-5.2".into()),
            provider: Some("openrouter".into()),
            ..Task::default()
        };
        let error = migrate_legacy_plan(&source(), &satellite)
            .unwrap_err()
            .to_string();
        assert!(error.contains("AMBIGUOUS"));
    }

    fn planned_satellite(id: &str, source: &Task) -> Task {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            provider: None,
            model: Some("pi:openai-codex:gpt-5.5".into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::High),
        });
        config.models.flip_inference = config.models.evaluator.clone();
        config.models.flip_comparison = config.models.evaluator.clone();
        let plan =
            build_plan(&config, source, id, DispatchSelectionSource::ScaffoldConfig).unwrap();
        Task {
            id: id.into(),
            title: id.into(),
            status: Status::InProgress,
            agency_dispatch: Some(plan),
            ..Task::default()
        }
    }

    fn verdict(source: &Task, stage: AgencyStage, score: f64) -> DurableEvalVerdict {
        let pipeline = source
            .evaluation_lifecycle
            .clone()
            .unwrap_or_else(|| EvaluationLifecycle::for_source(source));
        let suffix = if stage == AgencyStage::Evaluate {
            "eval"
        } else {
            "flip"
        };
        DurableEvalVerdict {
            schema: EVAL_LIFECYCLE_SCHEMA,
            verdict_id: format!("verdict-{suffix}"),
            verdict_digest: String::new(),
            evaluation_id: format!("evaluation-{suffix}"),
            pipeline_id: pipeline.pipeline_id,
            source_task: source.id.clone(),
            source_attempt: pipeline.source_attempt,
            stage,
            producer_run_id: "run-1".into(),
            score,
            evaluation_digest_schema: EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA,
            evaluation_digest: format!("b3:{suffix}"),
            created_at: Utc::now().to_rfc3339(),
        }
    }

    fn pin_required_gate(source: &mut Task, evaluator_threshold: f64, flip_threshold: Option<f64>) {
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(source));
        let lifecycle = source.evaluation_lifecycle.as_mut().unwrap();
        lifecycle.gate_policy = Some(EvaluationGatePolicy {
            applicability: EvaluationGateApplicability::Required,
            evaluator_threshold: Some(evaluator_threshold),
            flip_policy: if flip_threshold.is_some() {
                FlipVerdictPolicy::Required
            } else {
                FlipVerdictPolicy::NotScheduled
            },
            flip_threshold,
            flip_threshold_source: flip_threshold.map(|_| FlipThresholdSource::EvaluatorThreshold),
        });
        lifecycle.outcome_provenance = Some(EvaluationOutcomeProvenance {
            outcome: EvaluationGateOutcome::AwaitingEvidence,
            evaluator_verdict: None,
            flip_verdict: None,
            summary: "test gate".into(),
        });
    }

    #[test]
    fn durable_verdict_consumption_is_atomic_and_idempotent() {
        let mut source = source();
        source.status = Status::FailedPendingEval;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let flip = planned_satellite(".flip-source", &source);
        let eval = planned_satellite(".evaluate-source", &source);
        let flip_verdict = verdict(&source, AgencyStage::FlipComparison, 1.0);
        let eval_verdict = verdict(&source, AgencyStage::Evaluate, 0.9);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict.clone(), eval_verdict.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Done);
        assert_eq!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict
                .as_deref(),
            Some(eval_verdict.verdict_id.as_str())
        );
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict, eval_verdict],
            0.7,
            true,
            3,
            |_| true,
        ));
    }

    #[test]
    fn unsatisfiable_historical_gate_is_not_re_pinned_in_a_loop() {
        let mut source = source();
        source.status = Status::PendingEval;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let exhausted =
            "error[WG-EVAL-PIPELINE-REPAIR-EXHAUSTED]: bounded repair already ran".to_string();
        source.evaluation_lifecycle.as_mut().unwrap().diagnostic = Some(exhausted.clone());
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));

        // Repeated coordinator reconciliation must be a stable no-op. Before
        // the fix, each tick appended another "Pinned historical PendingEval"
        // row even though the gate had already been declared unsatisfiable.
        for _ in 0..3 {
            assert!(!reconcile_durable_verdicts(
                &mut graph,
                &[],
                0.7,
                true,
                3,
                |_| true,
            ));
        }
        let source = graph.get_task("source").unwrap();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert!(lifecycle.gate_policy.is_none());
        assert_eq!(lifecycle.diagnostic.as_deref(), Some(exhausted.as_str()));
        assert!(!source.log.iter().any(|entry| {
            entry
                .message
                .contains("Pinned historical PendingEval as a required gate")
        }));
        assert_eq!(
            evaluation_health(&graph, "source").unwrap().state,
            EvaluationHealthState::OperatorRequiredAmbiguity
        );
    }

    #[test]
    fn pending_low_score_never_passes_and_retries_exact_plan() {
        // The legacy callback says "advisory", but PendingEval itself is now
        // an unambiguous hard-gate contract and must fail closed.
        let mut legacy_pending = source();
        legacy_pending.status = Status::PendingEval;
        legacy_pending.evaluation_lifecycle =
            Some(EvaluationLifecycle::for_source(&legacy_pending));
        let legacy_eval = planned_satellite(".evaluate-source", &legacy_pending);
        let low = verdict(&legacy_pending, AgencyStage::Evaluate, 0.2);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(legacy_pending));
        graph.add_node(crate::graph::Node::Task(legacy_eval));
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[low],
            0.7,
            true,
            3,
            |_| false,
        ));
        assert_eq!(graph.get_task("source").unwrap().status, Status::Open);

        let mut gated = source();
        gated.status = Status::PendingEval;
        gated.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&gated));
        let old_pipeline = gated
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .pipeline_id
            .clone();
        let gated_eval = planned_satellite(".evaluate-source", &gated);
        let old_route = gated_eval.agency_dispatch.as_ref().unwrap().calls[0]
            .route
            .clone();
        let low = verdict(&gated, AgencyStage::Evaluate, 0.2);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(gated));
        graph.add_node(crate::graph::Node::Task(gated_eval));
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[low.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Open);
        assert_eq!(source.rescue_count, 1);
        let eval = graph.get_task(".evaluate-source").unwrap();
        let rebound = eval.agency_dispatch.as_ref().unwrap();
        assert_eq!(rebound.calls[0].route, old_route);
        assert_ne!(rebound.pipeline_id, old_pipeline);
        assert_eq!(eval.status, Status::Open);
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[low],
            0.7,
            true,
            3,
            |_| true,
        ));
    }

    #[test]
    fn strict_required_flip_and_evaluator_threshold_matrix() {
        for (name, flip_score, eval_score, expected) in [
            ("incident-both-low", 0.18, 0.20, Status::Failed),
            ("low-flip", 0.69, 0.95, Status::Failed),
            ("low-evaluator", 0.95, 0.69, Status::Failed),
            ("exact-threshold", 0.70, 0.70, Status::Done),
        ] {
            let mut source = source();
            source.status = Status::PendingEval;
            pin_required_gate(&mut source, 0.70, Some(0.70));
            let flip = planned_satellite(".flip-source", &source);
            let eval = planned_satellite(".evaluate-source", &source);
            let flip_verdict = verdict(&source, AgencyStage::FlipComparison, flip_score);
            let eval_verdict = verdict(&source, AgencyStage::Evaluate, eval_score);
            let mut graph = WorkGraph::new();
            graph.add_node(crate::graph::Node::Task(source));
            graph.add_node(crate::graph::Node::Task(flip));
            graph.add_node(crate::graph::Node::Task(eval));

            assert!(
                reconcile_durable_verdicts(
                    &mut graph,
                    &[flip_verdict, eval_verdict],
                    0.99, // persisted 0.70 must win over ambient reload
                    false,
                    0,
                    |_| false,
                ),
                "{name} did not reconcile"
            );
            let source = graph.get_task("source").unwrap();
            assert_eq!(source.status, expected, "{name}");
            let provenance = source
                .evaluation_lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.outcome_provenance.as_ref())
                .unwrap();
            assert!(provenance.summary.contains("evaluator"), "{name}");
            assert!(provenance.summary.contains("FLIP"), "{name}");
        }
    }

    #[test]
    fn system_task_success_scores_cannot_mask_source_gate_failures() {
        let mut source = source();
        source.status = Status::PendingEval;
        pin_required_gate(&mut source, 0.70, Some(0.70));
        let flip = planned_satellite(".flip-source", &source);
        let eval = planned_satellite(".evaluate-source", &source);
        let source_flip = verdict(&source, AgencyStage::FlipComparison, 0.64);
        let source_eval = verdict(&source, AgencyStage::Evaluate, 0.18);
        let mut system_flip = source_flip.clone();
        system_flip.verdict_id = "verdict-system-flip-success".into();
        system_flip.source_task = ".flip-source".into();
        system_flip.score = 1.0;
        let mut system_eval = source_eval.clone();
        system_eval.verdict_id = "verdict-system-eval-success".into();
        system_eval.source_task = ".evaluate-source".into();
        system_eval.score = 1.0;
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[system_flip, system_eval, source_flip, source_eval],
            0.70,
            false,
            0,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Failed);
        assert!(
            source.failure_reason.as_deref().is_some_and(
                |reason| reason.contains("score=0.18") && reason.contains("score=0.64")
            )
        );
    }

    #[test]
    fn two_below_threshold_attempts_never_complete_or_unblock_dependents() {
        let mut source = source();
        source.status = Status::PendingEval;
        pin_required_gate(&mut source, 0.70, Some(0.70));
        let flip = planned_satellite(".flip-source", &source);
        let eval = planned_satellite(".evaluate-source", &source);
        let old_flip = verdict(&source, AgencyStage::FlipComparison, 0.18);
        let old_eval = verdict(&source, AgencyStage::Evaluate, 0.20);
        let dependent = Task {
            id: "dependent".into(),
            title: "must remain blocked".into(),
            after: vec!["source".into()],
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        graph.add_node(crate::graph::Node::Task(dependent));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[old_flip.clone(), old_eval.clone()],
            0.70,
            true,
            1,
            |_| true,
        ));
        let attempt_two = graph.get_task("source").unwrap().clone();
        assert_eq!(attempt_two.status, Status::Open);
        assert_eq!(attempt_two.rescue_count, 1);
        let attempt_two_id = attempt_two
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .pipeline_id
            .clone();
        assert_ne!(attempt_two_id, old_eval.pipeline_id);
        assert!(
            !crate::query::ready_tasks(&graph)
                .iter()
                .any(|task| task.id == "dependent")
        );

        graph.get_task_mut("source").unwrap().status = Status::PendingEval;
        let current_source = graph.get_task("source").unwrap().clone();
        let mut current_flip = verdict(&current_source, AgencyStage::FlipComparison, 0.19);
        current_flip.verdict_id = "verdict-attempt-2-flip".into();
        let mut current_eval = verdict(&current_source, AgencyStage::Evaluate, 0.12);
        current_eval.verdict_id = "verdict-attempt-2-eval".into();
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[old_flip, old_eval, current_flip, current_eval],
            0.70,
            true,
            1,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Failed);
        assert_eq!(
            source.evaluation_lifecycle.as_ref().unwrap().source_attempt,
            2
        );
        assert!(
            !crate::query::ready_tasks(&graph)
                .iter()
                .any(|task| task.id == "dependent")
        );
    }

    #[test]
    fn non_finite_current_attempt_verdict_fails_closed() {
        let mut source = source();
        source.status = Status::PendingEval;
        pin_required_gate(&mut source, 0.70, None);
        let eval = planned_satellite(".evaluate-source", &source);
        let malformed = verdict(&source, AgencyStage::Evaluate, f64::NAN);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[malformed],
            0.70,
            false,
            0,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::PendingEval);
        assert!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict
                .is_none()
        );
        assert!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("MALFORMED"))
        );
    }

    #[test]
    fn historical_below_threshold_done_is_audited_without_rewrite() {
        let mut source = source();
        source.status = Status::Done;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let flip = verdict(&source, AgencyStage::FlipComparison, 0.64);
        let eval = verdict(&source, AgencyStage::Evaluate, 0.18);
        {
            let lifecycle = source.evaluation_lifecycle.as_mut().unwrap();
            lifecycle.linked_flip_verdict = Some(flip.verdict_id.clone());
            lifecycle.linked_eval_verdict = Some(eval.verdict_id.clone());
            lifecycle.consumed_verdict = Some(eval.verdict_id.clone());
        }
        let diagnostic = evaluation_gate_diagnostics(
            &source,
            Ok(&[flip.clone(), eval.clone()]),
            Some(0.70),
            None,
        )
        .unwrap();
        assert!(diagnostic.audit_alert);
        assert!(
            diagnostic
                .audit
                .as_deref()
                .unwrap()
                .contains("HISTORICAL AUDIT ALERT")
        );

        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[flip, eval],
            0.70,
            true,
            3,
            |_| true,
        ));
        assert_eq!(graph.get_task("source").unwrap().status, Status::Done);
        assert!(
            graph
                .get_task("source")
                .unwrap()
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .gate_policy
                .is_none()
        );
    }

    #[test]
    fn explicit_source_retry_rebinds_existing_satellites_without_route_drift() {
        let mut old_source = source();
        old_source.status = Status::Failed;
        old_source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&old_source));
        old_source
            .evaluation_lifecycle
            .as_mut()
            .unwrap()
            .consumed_verdict = Some("verdict-old".into());
        let eval = planned_satellite(".evaluate-source", &old_source);
        let old_plan = eval.agency_dispatch.as_ref().unwrap().clone();
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(old_source.clone()));
        graph.add_node(crate::graph::Node::Task(eval));

        let mut retry_source = old_source;
        retry_source.status = Status::Open;
        retry_source.retry_count = 1;
        assert!(rearm_satellites_for_source(&mut graph, &retry_source));
        let rebound = graph
            .get_task(".evaluate-source")
            .unwrap()
            .agency_dispatch
            .as_ref()
            .unwrap();
        assert_eq!(rebound.calls, old_plan.calls);
        assert_ne!(rebound.pipeline_id, old_plan.pipeline_id);
        assert_eq!(rebound.source_attempt, 2);
    }

    #[test]
    fn preempted_attempt_rearms_before_resume_and_only_current_verdicts_promote() {
        let mut attempt_one = source();
        attempt_one.status = Status::InProgress;
        let mut flip = planned_satellite(".flip-source", &attempt_one);
        let mut eval = planned_satellite(".evaluate-source", &attempt_one);
        let flip_calls = flip.agency_dispatch.as_ref().unwrap().calls.clone();
        let eval_calls = eval.agency_dispatch.as_ref().unwrap().calls.clone();
        let old_flip = verdict(&attempt_one, AgencyStage::FlipComparison, 0.96);
        let old_eval = verdict(&attempt_one, AgencyStage::Evaluate, 0.95);
        flip.status = Status::Done;
        flip.assigned = None;
        eval.status = Status::Done;
        eval.assigned = None;

        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(attempt_one));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        // Coordinator preemption starts attempt 2. This is the atomic boundary
        // that the live incident lacked: parent + both plans move together.
        {
            let source = graph.get_task_mut("source").unwrap();
            source.status = Status::Open;
            source.retry_count = 1;
            source.assigned = None;
        }
        assert!(begin_source_attempt(
            &mut graph,
            "source",
            "test preemption"
        ));
        let authoritative = graph
            .get_task("source")
            .unwrap()
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(authoritative.source_attempt, 2);
        for (task_id, calls) in [
            (".flip-source", flip_calls),
            (".evaluate-source", eval_calls),
        ] {
            let task = graph.get_task(task_id).unwrap();
            let plan = task.agency_dispatch.as_ref().unwrap();
            assert_eq!(plan.pipeline_id, authoritative.pipeline_id);
            assert_eq!(plan.source_attempt, 2);
            assert_eq!(plan.calls, calls, "route/reasoning identity drifted");
            assert_eq!(task.status, Status::Open);
        }

        // Daemon restart boundary: source completes after graph round-trip.
        let restart_dir = tempfile::tempdir().unwrap();
        let graph_path = restart_dir.path().join("graph.jsonl");
        crate::parser::save_graph(&graph, &graph_path).unwrap();
        let mut graph = crate::parser::load_graph(&graph_path).unwrap();
        {
            let source = graph.get_task_mut("source").unwrap();
            source.status = Status::PendingEval;
            refresh_source_lifecycle(source);
        }
        assert_eq!(
            graph
                .get_task("source")
                .unwrap()
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .pipeline_id,
            authoritative.pipeline_id
        );

        // Attempt-1 evidence remains visible to the reconciler but cannot
        // mutate or score attempt 2.
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[old_flip.clone(), old_eval.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let pending = graph.get_task("source").unwrap();
        assert_eq!(pending.status, Status::PendingEval);
        assert_eq!(
            pending
                .evaluation_lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.gate_policy.as_ref())
                .map(|policy| policy.applicability),
            Some(EvaluationGateApplicability::Required)
        );

        let current_source = graph.get_task("source").unwrap().clone();
        let current_flip = verdict(&current_source, AgencyStage::FlipComparison, 0.91);
        let current_eval = verdict(&current_source, AgencyStage::Evaluate, 0.93);
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[
                old_flip.clone(),
                old_eval.clone(),
                current_flip.clone(),
                current_eval.clone(),
            ],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::Done);
        assert_eq!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict
                .as_deref(),
            Some(current_eval.verdict_id.as_str())
        );
        assert_eq!(old_eval.pipeline_id, pipeline_id("source", 1, 0));
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[old_flip, old_eval, current_flip, current_eval],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(
            graph
                .get_task("source")
                .unwrap()
                .log
                .iter()
                .filter(|entry| entry.message.contains("Consumed durable verdict"))
                .count(),
            1
        );
    }

    #[test]
    fn attempt_mint_fails_closed_without_exposing_half_rearmed_pipeline() {
        let mut source = source();
        source.status = Status::Open;
        let mut flip = planned_satellite(".flip-source", &source);
        flip.status = Status::Done;
        let mut eval = planned_satellite(".evaluate-source", &source);
        eval.status = Status::Done;
        eval.agency_dispatch = None;
        eval.provider = Some("openrouter".into());
        eval.model = Some("ambiguous/model".into());

        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(begin_source_attempt(&mut graph, "source", "test ambiguity"));
        let source = graph.get_task("source").unwrap();
        assert!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .diagnostic
                .as_deref()
                .unwrap()
                .contains("could not atomically rearm")
        );
        assert_eq!(
            graph.get_task(".flip-source").unwrap().status,
            Status::Blocked
        );
        assert_eq!(
            graph.get_task(".evaluate-source").unwrap().status,
            Status::Blocked
        );
    }

    #[test]
    fn pending_parent_repairs_live_incident_stale_terminal_satellites_once() {
        let attempt_one = source();
        let mut flip = planned_satellite(".flip-source", &attempt_one);
        let mut eval = planned_satellite(".evaluate-source", &attempt_one);
        flip.status = Status::Done;
        flip.assigned = None;
        eval.status = Status::Done;
        eval.assigned = None;
        let stale_flip = verdict(&attempt_one, AgencyStage::FlipComparison, 0.96);
        let stale_eval = verdict(&attempt_one, AgencyStage::Evaluate, 0.95);

        let mut attempt_two = attempt_one;
        attempt_two.retry_count = 1;
        attempt_two.status = Status::PendingEval;
        attempt_two.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&attempt_two));
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(attempt_two));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        let before = evaluation_health(&graph, "source").unwrap();
        assert_eq!(before.state, EvaluationHealthState::RepairablePipelineDrift);
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[stale_flip.clone(), stale_eval.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::PendingEval);
        assert_eq!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .repair_attempts,
            1
        );
        for task_id in [".flip-source", ".evaluate-source"] {
            let task = graph.get_task(task_id).unwrap();
            assert_eq!(task.status, Status::Open);
            assert_eq!(task.agency_dispatch.as_ref().unwrap().source_attempt, 2);
        }
        assert_eq!(
            evaluation_health(&graph, "source").unwrap().state,
            EvaluationHealthState::ActiveEvaluation
        );
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[stale_flip, stale_eval],
            0.7,
            true,
            3,
            |_| true,
        ));
    }

    #[test]
    fn partial_flip_evidence_rearms_only_failed_evaluator_and_is_bounded() {
        let mut source = source();
        source.status = Status::PendingEval;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let mut flip = planned_satellite(".flip-source", &source);
        let mut eval = planned_satellite(".evaluate-source", &source);
        flip.status = Status::Done;
        flip.assigned = None;
        eval.status = Status::Failed;
        eval.assigned = None;
        let flip_verdict = verdict(&source, AgencyStage::FlipComparison, 0.9);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(graph.get_task(".flip-source").unwrap().status, Status::Done);
        assert_eq!(
            graph.get_task(".evaluate-source").unwrap().status,
            Status::Open
        );
        assert_eq!(
            graph
                .get_task(".flip-source")
                .unwrap()
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .linked_flip_verdict
                .as_deref(),
            Some(flip_verdict.verdict_id.as_str())
        );

        graph.get_task_mut(".evaluate-source").unwrap().status = Status::Failed;
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .diagnostic
                .as_deref()
                .unwrap()
                .contains("REPAIR-EXHAUSTED")
        );
        let logs = source.log.len();
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(graph.get_task("source").unwrap().log.len(), logs);
    }

    #[test]
    fn multiple_current_evaluator_verdicts_fail_closed_without_consumption() {
        let mut source = source();
        source.status = Status::PendingEval;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let eval = planned_satellite(".evaluate-source", &source);
        let first = verdict(&source, AgencyStage::Evaluate, 0.9);
        let mut second = first.clone();
        second.verdict_id = "verdict-eval-duplicate".into();
        second.evaluation_id = "evaluation-eval-duplicate".into();
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(eval));

        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[first.clone(), second.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let source = graph.get_task("source").unwrap();
        assert_eq!(source.status, Status::PendingEval);
        assert!(
            source
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict
                .is_none()
        );
        assert_eq!(
            evaluation_health(&graph, "source").unwrap().state,
            EvaluationHealthState::OperatorRequiredAmbiguity
        );
        let logs = source.log.len();
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[first, second],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(graph.get_task("source").unwrap().log.len(), logs);
    }

    #[test]
    fn claimed_transport_retry_budget_is_bounded() {
        let source = source();
        let mut lifecycle = EvaluationLifecycle::for_source(&source);
        assert_eq!(lifecycle.reserve_transport_attempt().unwrap(), 1);
        lifecycle.execution_state = EvaluationExecutionState::Waiting;
        assert_eq!(lifecycle.reserve_transport_attempt().unwrap(), 2);
        assert!(lifecycle.reserve_transport_attempt().is_err());
        assert_eq!(lifecycle.transport_attempts, 2);
        assert_eq!(lifecycle.execution_state, EvaluationExecutionState::Blocked);
    }

    #[test]
    fn historical_claimed_row_is_never_rearmed_as_preclaim() {
        let mut source = source();
        source.status = Status::FailedPendingEval;
        let satellite = Task {
            id: ".evaluate-source".into(),
            title: "eval".into(),
            status: Status::Incomplete,
            model: Some("gpt-5.5".into()),
            provider: Some("codex".into()),
            spawn_failures: 5,
            started_at: Some(Utc::now().to_rfc3339()),
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(satellite));
        assert!(!repair_historical_rows(&mut graph));
        let row = graph.get_task(".evaluate-source").unwrap();
        assert_eq!(row.status, Status::Incomplete);
        assert!(row.agency_dispatch.is_none());
    }

    #[test]
    fn historical_codex_preclaim_repair_is_bounded_and_idempotent() {
        let mut source = source();
        source.status = Status::FailedPendingEval;
        let satellite = Task {
            id: ".evaluate-source".into(),
            title: "eval".into(),
            status: Status::Incomplete,
            model: Some("gpt-5.5".into()),
            provider: Some("codex".into()),
            spawn_failures: 5,
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(satellite));
        assert!(repair_historical_rows(&mut graph));
        let repaired = graph.get_task(".evaluate-source").unwrap();
        assert_eq!(repaired.status, Status::Open);
        assert_eq!(repaired.spawn_failures, 0);
        assert_eq!(repaired.model.as_deref(), Some("codex:gpt-5.5"));
        assert!(!repair_historical_rows(&mut graph));
    }

    #[test]
    fn unambiguous_legacy_evaluation_migrates_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = source();
        source.status = Status::FailedPendingEval;
        source.started_at = Some((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&source));
        let satellite = planned_satellite(".evaluate-source", &source);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source.clone()));
        graph.add_node(crate::graph::Node::Task(satellite));
        crate::parser::save_graph(&graph, &dir.path().join("graph.jsonl")).unwrap();
        let evaluation = Evaluation {
            id: "legacy-eval-source".into(),
            task_id: source.id.clone(),
            agent_id: "agent-legacy".into(),
            role_id: "role".into(),
            tradeoff_id: "tradeoff".into(),
            score: 0.9,
            dimensions: std::collections::HashMap::new(),
            notes: "legacy but unambiguous".into(),
            evaluator: "codex:gpt-5.5".into(),
            timestamp: Utc::now().to_rfc3339(),
            model: Some("codex:gpt-5.5".into()),
            source: "llm".into(),
            loop_iteration: 0,
        };
        crate::agency::save_evaluation(&evaluation, &dir.path().join("agency/evaluations"))
            .unwrap();
        assert_eq!(migrate_unambiguous_legacy_verdicts(dir.path()).unwrap(), 1);
        assert_eq!(migrate_unambiguous_legacy_verdicts(dir.path()).unwrap(), 0);
        assert_eq!(load_durable_verdicts(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn durable_verdict_replay_ignores_observational_time_and_run_identity() {
        let dir = tempfile::tempdir().unwrap();
        let source = source();
        let mut satellite = planned_satellite(".evaluate-source", &source);
        satellite.assigned = Some("agent-original".into());
        let evaluation = Evaluation {
            id: "eval-source-replay".into(),
            task_id: source.id.clone(),
            agent_id: "agent-original".into(),
            role_id: "role".into(),
            tradeoff_id: "tradeoff".into(),
            score: 0.9,
            dimensions: std::collections::HashMap::new(),
            notes: "same semantic evidence".into(),
            evaluator: "codex:gpt-5.5".into(),
            timestamp: Utc::now().to_rfc3339(),
            model: Some("codex:gpt-5.5".into()),
            source: "llm".into(),
            loop_iteration: 0,
        };
        crate::agency::save_evaluation(&evaluation, &dir.path().join("agency/evaluations"))
            .unwrap();
        let first = write_durable_verdict(
            dir.path(),
            &source,
            &satellite,
            AgencyStage::Evaluate,
            &evaluation,
        )
        .unwrap();
        // Re-encode the record exactly as a pre-schema-2 writer did. The
        // durable evaluation's pretty member order reconstructs the original
        // compact digest losslessly.
        let evaluation_path = dir
            .path()
            .join("agency/evaluations")
            .join(format!("{}.json", evaluation.id));
        let mut legacy: DurableEvalVerdict =
            serde_json::from_slice(&fs::read(&first).unwrap()).unwrap();
        legacy.evaluation_digest_schema = 1;
        legacy.evaluation_digest =
            digest_bytes(&compact_durable_json(&fs::read(evaluation_path).unwrap()));
        legacy.verdict_digest = compute_verdict_digest(&legacy).unwrap();
        fs::write(&first, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        let first_bytes = fs::read(&first).unwrap();

        satellite.assigned = Some("agent-restarted-wrapper".into());
        let replay = write_durable_verdict(
            dir.path(),
            &source,
            &satellite,
            AgencyStage::Evaluate,
            &evaluation,
        )
        .unwrap();
        assert_eq!(replay, first);
        assert_eq!(fs::read(&replay).unwrap(), first_bytes);
        assert_eq!(load_durable_verdicts(dir.path()).unwrap().len(), 1);
    }

    #[test]
    fn completed_claimed_legacy_evaluator_migrates_once_without_semantic_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let mut source = source();
        source.status = Status::PendingEval;
        source.started_at = Some((now - chrono::Duration::seconds(30)).to_rfc3339());
        let satellite = Task {
            id: ".evaluate-source".into(),
            title: "legacy completed evaluator".into(),
            status: Status::Done,
            model: Some("pi:openai-codex:gpt-5.6-terra".into()),
            assigned: Some("agent-legacy".into()),
            started_at: Some((now - chrono::Duration::seconds(20)).to_rfc3339()),
            completed_at: Some((now - chrono::Duration::seconds(5)).to_rfc3339()),
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source.clone()));
        graph.add_node(crate::graph::Node::Task(satellite));
        crate::parser::save_graph(&graph, &dir.path().join("graph.jsonl")).unwrap();
        let evaluation = Evaluation {
            id: "legacy-completed-eval".into(),
            task_id: source.id.clone(),
            agent_id: "agent-legacy".into(),
            role_id: "role".into(),
            tradeoff_id: "tradeoff".into(),
            score: 0.91,
            dimensions: std::collections::HashMap::new(),
            notes: "one post-start evaluation".into(),
            evaluator: "pi:openai-codex:gpt-5.6-terra".into(),
            timestamp: (now - chrono::Duration::seconds(4)).to_rfc3339(),
            model: Some("pi:openai-codex:gpt-5.6-terra".into()),
            source: "llm".into(),
            loop_iteration: 0,
        };
        crate::agency::save_evaluation(&evaluation, &dir.path().join("agency/evaluations"))
            .unwrap();

        assert_eq!(migrate_unambiguous_legacy_verdicts(dir.path()).unwrap(), 1);
        let verdicts = load_durable_verdicts(dir.path()).unwrap();
        assert_eq!(verdicts.len(), 1);

        // Simulate a daemon restart after durable migration but before the graph
        // transaction. Claimed-row preflight repair remains correctly disabled;
        // verified evidence performs metadata backfill and consumption instead.
        let mut restarted = crate::parser::load_graph(&dir.path().join("graph.jsonl")).unwrap();
        assert!(!repair_historical_rows(&mut restarted));
        assert!(reconcile_durable_verdicts(
            &mut restarted,
            &verdicts,
            0.7,
            true,
            3,
            |_| false,
        ));
        let source = restarted.get_task("source").unwrap();
        assert_eq!(source.status, Status::Done);
        assert_eq!(
            source
                .evaluation_lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.consumed_verdict.as_deref()),
            Some(verdicts[0].verdict_id.as_str())
        );
        let evaluator = restarted.get_task(".evaluate-source").unwrap();
        assert!(evaluator.agency_dispatch.is_some());
        assert_eq!(
            evaluator
                .evaluation_lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.linked_eval_verdict.as_deref()),
            Some(verdicts[0].verdict_id.as_str())
        );

        crate::parser::save_graph(&restarted, &dir.path().join("graph.jsonl")).unwrap();
        assert_eq!(migrate_unambiguous_legacy_verdicts(dir.path()).unwrap(), 0);
        let mut second_restart =
            crate::parser::load_graph(&dir.path().join("graph.jsonl")).unwrap();
        assert!(!reconcile_durable_verdicts(
            &mut second_restart,
            &load_durable_verdicts(dir.path()).unwrap(),
            0.7,
            true,
            3,
            |_| false,
        ));
        assert_eq!(
            crate::agency::load_all_evaluations_or_warn(&dir.path().join("agency/evaluations"))
                .len(),
            1
        );
    }

    #[test]
    fn incident_legacy_verdict_verifies_from_durable_member_order() {
        // Exact bytes from the 2026-07-19 Pi/Terra live incident. The writer's
        // dimensions order was correctness, completeness, ...; deserializing
        // into a newly seeded HashMap changed that order, so schema 1's old
        // `to_vec(&evaluation)` reader rejected its own valid verdict.
        let dir = tempfile::tempdir().unwrap();
        let evaluations = dir.path().join("agency/evaluations");
        let verdicts = verdicts_dir(dir.path());
        fs::create_dir_all(&evaluations).unwrap();
        fs::create_dir_all(&verdicts).unwrap();
        fs::write(
            evaluations.join("evaluation.json"),
            include_bytes!("../tests/fixtures/live_eval_digest_mismatch/evaluation.json"),
        )
        .unwrap();
        fs::write(
            verdicts.join("verdict-evalp-499fd5ddac13a90963448679-evaluate-97e39ad55786b39b.json"),
            include_bytes!("../tests/fixtures/live_eval_digest_mismatch/verdict.json"),
        )
        .unwrap();

        let loaded = load_durable_verdicts(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].evaluation_digest_schema, 1);
        assert_eq!(loaded[0].score, 0.88);
    }

    #[test]
    fn durable_verdict_load_verifies_record_and_evaluation_digests() {
        let dir = tempfile::tempdir().unwrap();
        let source = source();
        let satellite = planned_satellite(".evaluate-source", &source);
        let mut evaluation = Evaluation {
            id: "eval-source-fixed".into(),
            task_id: source.id.clone(),
            agent_id: "agent-1".into(),
            role_id: "role".into(),
            tradeoff_id: "tradeoff".into(),
            score: 0.9,
            dimensions: std::collections::HashMap::new(),
            notes: "valid".into(),
            evaluator: "codex:gpt-5.5".into(),
            timestamp: Utc::now().to_rfc3339(),
            model: Some("codex:gpt-5.5".into()),
            source: "llm".into(),
            loop_iteration: 0,
        };
        crate::agency::save_evaluation(&evaluation, &dir.path().join("agency/evaluations"))
            .unwrap();
        let verdict_path = write_durable_verdict(
            dir.path(),
            &source,
            &satellite,
            AgencyStage::Evaluate,
            &evaluation,
        )
        .unwrap();
        let loaded = load_durable_verdicts(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].evaluation_digest_schema,
            EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA
        );

        let original = fs::read(&verdict_path).unwrap();
        let mut tampered: serde_json::Value = serde_json::from_slice(&original).unwrap();
        tampered["score"] = serde_json::json!(0.1);
        fs::write(&verdict_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
        assert!(
            load_durable_verdicts(dir.path())
                .unwrap_err()
                .to_string()
                .contains("INTEGRITY")
        );

        fs::write(&verdict_path, original).unwrap();
        evaluation.notes = "tampered after verdict".into();
        crate::agency::save_evaluation(&evaluation, &dir.path().join("agency/evaluations"))
            .unwrap();
        assert!(
            load_durable_verdicts(dir.path())
                .unwrap_err()
                .to_string()
                .contains("EVIDENCE")
        );
    }

    // ---- pre-Pi reasoning migration coverage ----

    /// A source stranded exactly like the live `make-hashed-project` incident:
    /// `PendingEval`, a `REPAIR-EXHAUSTED` diagnostic, and the authoritative
    /// lifecycle minted for its current source attempt.
    fn stranded_source(id: &str) -> Task {
        let mut task = Task {
            id: id.into(),
            title: id.into(),
            status: Status::PendingEval,
            ..Task::default()
        };
        let mut lifecycle = EvaluationLifecycle::for_source(&task);
        lifecycle.diagnostic = Some(
            "error[WG-EVAL-PIPELINE-REPAIR-EXHAUSTED]: bounded repair replayed \
             invalid missing-reasoning bytes"
                .to_string(),
        );
        lifecycle.execution_state = EvaluationExecutionState::Blocked;
        task.evaluation_lifecycle = Some(lifecycle);
        task
    }

    /// A satellite whose persisted plan carries an exact `pi:<provider>:<model>`
    /// route but no reasoning — the exact shape of pre-Pi scaffolding. The plan
    /// is structurally valid (`validate_plan`) but never executable
    /// (`validate_executable_plan`).
    fn pre_pi_satellite(id: &str, source: &Task, route: &str) -> Task {
        let mut config = Config::default();
        let role_model = RoleModelConfig {
            provider: None,
            model: Some(route.into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::High),
        };
        config.models.evaluator = Some(role_model.clone());
        config.models.flip_inference = Some(role_model.clone());
        config.models.flip_comparison = Some(role_model);
        let mut plan =
            build_plan(&config, source, id, DispatchSelectionSource::PersistedPlan).unwrap();
        // Strip reasoning to simulate a plan scaffolded before Pi reasoning
        // became mandatory, then re-hash so the historical bytes are authentic.
        for call in &mut plan.calls {
            call.reasoning = None;
        }
        plan.plan_hash = compute_plan_hash(&plan).unwrap();
        validate_plan(&plan).expect("structural validation still accepts pre-Pi plan");
        Task {
            id: id.into(),
            title: id.into(),
            status: Status::Failed,
            assigned: Some("producer-run-prior".into()),
            failure_reason: Some(
                "error[WG-PI-REASONING-MISSING]: persisted agency plan route has no reasoning"
                    .into(),
            ),
            agency_dispatch: Some(plan),
            ..Task::default()
        }
    }

    /// Config whose agency roles resolve explicit `High` reasoning at `route`.
    fn migration_config(route: &str) -> Config {
        let mut config = Config::default();
        let role_model = RoleModelConfig {
            provider: None,
            model: Some(route.into()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::High),
        };
        config.models.evaluator = Some(role_model.clone());
        config.models.flip_inference = Some(role_model.clone());
        config.models.flip_comparison = Some(role_model);
        config
    }

    fn generation_verdict(source: &Task, stage: AgencyStage, score: f64) -> DurableEvalVerdict {
        let lifecycle = source
            .evaluation_lifecycle
            .as_ref()
            .expect("source has authoritative lifecycle after migration");
        DurableEvalVerdict {
            schema: EVAL_LIFECYCLE_SCHEMA,
            verdict_id: format!("verdict-gen-{stage:?}"),
            verdict_digest: String::new(),
            evaluation_id: format!("evaluation-gen-{stage:?}"),
            pipeline_id: lifecycle.pipeline_id.clone(),
            source_task: source.id.clone(),
            source_attempt: lifecycle.source_attempt,
            stage,
            producer_run_id: format!("run-gen-{}", lifecycle.route_generation),
            score,
            evaluation_digest_schema: EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA,
            evaluation_digest: format!("b3:gen-{stage:?}-{score}"),
            created_at: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn validate_executable_plan_boundary_separates_structural_from_executable() {
        let source = stranded_source("source");
        let satellite = pre_pi_satellite(".evaluate-source", &source, "pi:openrouter:z-ai/glm-5.2");
        let plan = satellite.agency_dispatch.unwrap();
        // Structural validation authenticates the historical hash; it must NOT
        // reject the pre-Pi plan, or the migration boundary could never audit it.
        validate_plan(&plan).expect("structural validation accepts pre-Pi plan");
        let error = validate_executable_plan(&plan).unwrap_err().to_string();
        assert!(error.contains("WG-PI-REASONING-MISSING"), "{error}");
        // A reasoning-armed exact Pi plan clears the executable boundary.
        let armed = migration_config("pi:openrouter:z-ai/glm-5.2");
        let armed_plan = build_plan(
            &armed,
            &source,
            ".evaluate-source",
            DispatchSelectionSource::PersistedPlan,
        )
        .unwrap();
        validate_executable_plan(&armed_plan).expect("armed exact Pi plan is executable");
    }

    #[test]
    fn migrate_missing_pi_reasoning_resolves_route_preservingly_and_rearms_without_rerunning_source()
     {
        let route = "pi:openrouter:z-ai/glm-5.2";
        let source = stranded_source("source");
        let flip = pre_pi_satellite(".flip-source", &source, route);
        let eval = pre_pi_satellite(".evaluate-source", &source, route);
        let old_flip_plan = flip.agency_dispatch.clone().unwrap();
        let old_eval_plan = eval.agency_dispatch.clone().unwrap();
        let old_pipeline = source
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .pipeline_id
            .clone();
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        let config = migration_config(route);

        assert!(
            migrate_missing_pi_reasoning(&mut graph, &config),
            "the migration boundary must report a graph modification"
        );

        let source = graph.get_task("source").unwrap();
        // The completed source worker is never rerun: its status, assignee, and
        // retry counters are unchanged. Only the lifecycle identity advanced.
        assert_eq!(source.status, Status::PendingEval);
        assert_eq!(source.retry_count, 0);
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.route_generation, 1);
        assert_eq!(lifecycle.diagnostic, None);
        assert_eq!(lifecycle.execution_state, EvaluationExecutionState::Ready);
        assert_eq!(lifecycle.consumed_verdict, None);
        assert_ne!(lifecycle.pipeline_id, old_pipeline);
        assert_eq!(lifecycle.plan_migrations.len(), 2);
        assert!(lifecycle.schedule_attempts == 0 && lifecycle.transport_attempts == 0);

        for (task_id, old_plan, stages) in [
            (
                ".flip-source",
                old_flip_plan.clone(),
                [AgencyStage::FlipInference, AgencyStage::FlipComparison],
            ),
            (
                ".evaluate-source",
                old_eval_plan.clone(),
                [AgencyStage::Evaluate, AgencyStage::Evaluate],
            ),
        ] {
            let task = graph.get_task(task_id).unwrap();
            assert_eq!(task.status, Status::Open, "{task_id} rearmed to Open");
            assert_eq!(task.assigned, None, "{task_id} cleared prior producer");
            assert_eq!(task.failure_reason, None, "{task_id} cleared prior failure");
            let plan = task.agency_dispatch.as_ref().unwrap();
            assert_eq!(plan.route_generation, 1, "{task_id} carries new generation");
            assert_eq!(plan.pipeline_id, lifecycle.pipeline_id);
            assert_eq!(plan.source_attempt, lifecycle.source_attempt);
            assert_eq!(plan.calls.len(), old_plan.calls.len());
            for (idx, call) in plan.calls.iter().enumerate() {
                assert_eq!(call.route, old_plan.calls[idx].route, "route preserved");
                assert_eq!(
                    call.reasoning,
                    Some(ReasoningLevel::High),
                    "reasoning resolved"
                );
                assert_eq!(call.stage, stages[idx], "stage identity preserved");
            }
            assert_ne!(plan.plan_hash, old_plan.plan_hash, "plan re-hashed");
            validate_executable_plan(plan).unwrap();
        }

        // Audit rows retain the original plan, producer, and prior failure so
        // the pre-migration history stays immutable and queryable.
        let flip_audit = lifecycle
            .plan_migrations
            .iter()
            .find(|row| row.task_id == ".flip-source")
            .unwrap();
        assert_eq!(flip_audit.old_plan.plan_hash, old_flip_plan.plan_hash);
        assert_eq!(flip_audit.old_plan.calls[0].reasoning, None);
        assert_eq!(
            flip_audit.prior_producer_run_id.as_deref(),
            Some("producer-run-prior")
        );
        assert_eq!(flip_audit.prior_status, Status::Failed);
        assert!(
            flip_audit
                .prior_failure_reason
                .as_deref()
                .unwrap()
                .contains("WG-PI-REASONING-MISSING")
        );
        assert!(flip_audit.source_task == "source" && flip_audit.source_attempt == 1);
        assert_eq!(flip_audit.old_pipeline_id, old_pipeline);
        assert_eq!(flip_audit.new_pipeline_id, lifecycle.pipeline_id);
        assert_eq!(flip_audit.reasoning.len(), 2);
        assert!(
            flip_audit
                .reasoning
                .iter()
                .all(|r| r.was_missing && r.reasoning == ReasoningLevel::High)
        );
        assert!(flip_audit.reasoning.iter().all(|r| r.route == route));
    }

    #[test]
    fn migrate_missing_pi_reasoning_is_bounded_and_idempotent_across_restart() {
        let route = "pi:openai-codex:gpt-5.6-sol";
        let source = stranded_source("source");
        let flip = pre_pi_satellite(".flip-source", &source, route);
        let eval = pre_pi_satellite(".evaluate-source", &source, route);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        let config = migration_config(route);

        assert!(migrate_missing_pi_reasoning(&mut graph, &config));
        let after_first = graph
            .get_task("source")
            .unwrap()
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .clone();
        let flip_plan_after_first = graph
            .get_task(".flip-source")
            .unwrap()
            .agency_dispatch
            .clone()
            .unwrap();
        let logs_after_first = graph.get_task("source").unwrap().log.len();

        // Daemon restart: round-trip through the graph store, then re-tick.
        let restart_dir = tempfile::tempdir().unwrap();
        let graph_path = restart_dir.path().join("graph.jsonl");
        crate::parser::save_graph(&graph, &graph_path).unwrap();
        let mut graph = crate::parser::load_graph(&graph_path).unwrap();

        // A second tick (concurrent coordinator / retry) must be a no-op: the
        // source already crossed its one allowed boundary, and no call is
        // re-armed or duplicated.
        assert!(
            !migrate_missing_pi_reasoning(&mut graph, &config),
            "re-tick after a completed migration must not modify the graph"
        );
        let after_second = graph
            .get_task("source")
            .unwrap()
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(after_second.pipeline_id, after_first.pipeline_id);
        assert_eq!(after_second.route_generation, after_first.route_generation);
        assert_eq!(
            after_second.plan_migrations.len(),
            after_first.plan_migrations.len()
        );
        let flip_plan_after_second = graph
            .get_task(".flip-source")
            .unwrap()
            .agency_dispatch
            .clone()
            .unwrap();
        assert_eq!(
            flip_plan_after_second.plan_hash, flip_plan_after_first.plan_hash,
            "plan identity is stable across restart; no duplicate migration"
        );
        assert_eq!(
            graph.get_task("source").unwrap().log.len(),
            logs_after_first,
            "idempotent re-tick logs nothing"
        );
    }

    #[test]
    fn migrate_missing_pi_reasoning_fails_closed_without_authoritative_reasoning() {
        let route = "pi:openrouter:z-ai/glm-5.2";
        let source = stranded_source("source");
        let flip = pre_pi_satellite(".flip-source", &source, route);
        let eval = pre_pi_satellite(".evaluate-source", &source, route);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        // Config resolves NO reasoning for any role/tier — the operator forgot
        // to set it. Migration must fail closed, never synthesize a default.
        let config = Config::default();

        assert!(migrate_missing_pi_reasoning(&mut graph, &config));

        let source = graph.get_task("source").unwrap();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(
            lifecycle.execution_state,
            EvaluationExecutionState::Blocked,
            "ambiguous migration parks fail-closed"
        );
        let diagnostic = lifecycle.diagnostic.as_deref().unwrap();
        assert!(
            diagnostic.contains("WG-EVAL-PI-REASONING-MIGRATION-AMBIGUOUS"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("models.") && diagnostic.contains("_reasoning"),
            "diagnostic must name the actionable config key: {diagnostic}"
        );
        assert_eq!(
            lifecycle.plan_migrations.len(),
            0,
            "no audit row is minted when reasoning cannot be resolved"
        );
        assert_eq!(
            lifecycle.route_generation, 0,
            "generation must not advance on failure"
        );

        // Satellites are NOT re-armed: the prior producer/failure is intact.
        for task_id in [".flip-source", ".evaluate-source"] {
            let task = graph.get_task(task_id).unwrap();
            assert_eq!(task.status, Status::Failed, "{task_id} left as-is");
            assert_eq!(task.assigned.as_deref(), Some("producer-run-prior"));
            assert_eq!(task.agency_dispatch.as_ref().unwrap().route_generation, 0);
        }
        let health = evaluation_health(&graph, "source").unwrap();
        assert_eq!(
            health.state,
            EvaluationHealthState::OperatorRequiredAmbiguity
        );
    }

    #[test]
    fn migrate_missing_pi_reasoning_rejects_non_pi_and_malformed_legacy_plans() {
        // A legacy non-Pi plan (codex handler) with missing reasoning must not
        // be migrated or silently rerouted; there is no cross-system fallback.
        let source = stranded_source("source");
        let lifecycle = source.evaluation_lifecycle.clone().unwrap();
        let non_pi_system = crate::config::execution_system_key("codex:gpt-5").unwrap();
        let build_non_pi = |task_id: &str| {
            let mut plan = AgencyDispatchPlan {
                schema: AGENCY_PLAN_SCHEMA,
                pipeline_id: lifecycle.pipeline_id.clone(),
                source_task: "source".into(),
                source_attempt: lifecycle.source_attempt,
                route_generation: 0,
                task_id: task_id.into(),
                calls: vec![AgencyCallPlan {
                    stage: if task_id.starts_with(".flip-") {
                        AgencyStage::FlipInference
                    } else {
                        AgencyStage::Evaluate
                    },
                    route: "codex:gpt-5".into(),
                    endpoint: None,
                    reasoning: None,
                    system: non_pi_system.clone(),
                    source: DispatchSelectionSource::LegacyCodexSplit,
                    fallbacks: Vec::new(),
                }],
                plan_hash: String::new(),
            };
            plan.plan_hash = compute_plan_hash(&plan).unwrap();
            Task {
                id: task_id.into(),
                title: task_id.into(),
                status: Status::Failed,
                agency_dispatch: Some(plan),
                ..Task::default()
            }
        };
        let flip = build_non_pi(".flip-source");
        let eval = build_non_pi(".evaluate-source");
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        let config = migration_config("pi:openrouter:z-ai/glm-5.2");

        assert!(migrate_missing_pi_reasoning(&mut graph, &config));
        let source = graph.get_task("source").unwrap();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.route_generation, 0, "non-Pi plan never migrates");
        assert_eq!(lifecycle.plan_migrations.len(), 0);
        let diagnostic = lifecycle.diagnostic.as_deref().unwrap();
        assert!(
            diagnostic.contains("WG-EVAL-PI-REASONING-MIGRATION-AMBIGUOUS"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("not an exact Pi route"), "{diagnostic}");
        // No satellite was rerouted to a synthesized Pi/Claude/Nex route.
        for task_id in [".flip-source", ".evaluate-source"] {
            assert_eq!(
                graph
                    .get_task(task_id)
                    .unwrap()
                    .agency_dispatch
                    .as_ref()
                    .unwrap()
                    .calls[0]
                    .route,
                "codex:gpt-5",
                "non-Pi route is never rewritten by the migration"
            );
        }
        let health = evaluation_health(&graph, "source").unwrap();
        assert_eq!(
            health.state,
            EvaluationHealthState::OperatorRequiredAmbiguity
        );
    }

    #[test]
    fn migrate_missing_pi_reasoning_health_distinguishes_all_states() {
        let route = "pi:openrouter:z-ai/glm-5.2";

        // (1) migration-required: exact Pi routes, missing reasoning, parked on
        // the recoverable REPAIR-EXHAUSTED diagnostic.
        let source = stranded_source("source");
        let flip = pre_pi_satellite(".flip-source", &source, route);
        let eval = pre_pi_satellite(".evaluate-source", &source, route);
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        let health = evaluation_health(&graph, "source").unwrap();
        assert_eq!(health.state, EvaluationHealthState::MigrationRequired);
        assert_eq!(health.route_generation, 0);
        assert_eq!(health.migration_count, 0);

        // (2) migrated/rearmed: satellites re-armed Open + unassigned.
        let config = migration_config(route);
        migrate_missing_pi_reasoning(&mut graph, &config);
        let health = evaluation_health(&graph, "source").unwrap();
        assert_eq!(health.state, EvaluationHealthState::MigratedRearmed);
        assert_eq!(health.route_generation, 1);
        assert_eq!(health.migration_count, 2);

        // (3) active evaluation: once a satellite is claimed, the state leaves
        // the freshly-rearmed band and reports active evaluation.
        {
            let eval = graph.get_task_mut(".evaluate-source").unwrap();
            eval.status = Status::InProgress;
            eval.assigned = Some("producer-run-new".into());
        }
        let health = evaluation_health(&graph, "source").unwrap();
        assert_eq!(health.state, EvaluationHealthState::ActiveEvaluation);

        // (4) operator-required ambiguity is the fail-closed terminal for a
        // missing-reasoning plan whose route is non-Pi (no fallback).
        let source_b = stranded_source("source-b");
        let lc = source_b.evaluation_lifecycle.clone().unwrap();
        let non_pi_system = crate::config::execution_system_key("codex:gpt-5").unwrap();
        let mut malformed = AgencyDispatchPlan {
            schema: AGENCY_PLAN_SCHEMA,
            pipeline_id: lc.pipeline_id.clone(),
            source_task: "source-b".into(),
            source_attempt: lc.source_attempt,
            route_generation: 0,
            task_id: ".evaluate-source-b".into(),
            calls: vec![AgencyCallPlan {
                stage: AgencyStage::Evaluate,
                route: "codex:gpt-5".into(),
                endpoint: None,
                reasoning: None,
                system: non_pi_system,
                source: DispatchSelectionSource::LegacyCodexSplit,
                fallbacks: Vec::new(),
            }],
            plan_hash: String::new(),
        };
        malformed.plan_hash = compute_plan_hash(&malformed).unwrap();
        let eval_b = Task {
            id: ".evaluate-source-b".into(),
            title: ".evaluate-source-b".into(),
            status: Status::Failed,
            agency_dispatch: Some(malformed),
            ..Task::default()
        };
        let mut graph_b = WorkGraph::new();
        graph_b.add_node(crate::graph::Node::Task(source_b));
        graph_b.add_node(crate::graph::Node::Task(eval_b));
        let health = evaluation_health(&graph_b, "source-b").unwrap();
        assert_eq!(
            health.state,
            EvaluationHealthState::OperatorRequiredAmbiguity
        );
    }

    #[test]
    fn migrate_missing_pi_reasoning_consumes_new_generation_verdict_exactly_once() {
        let route = "pi:openrouter:z-ai/glm-5.2";
        let source = stranded_source("source");
        let flip = pre_pi_satellite(".flip-source", &source, route);
        let eval = pre_pi_satellite(".evaluate-source", &source, route);
        let old_pipeline = source
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .pipeline_id
            .clone();
        let mut graph = WorkGraph::new();
        graph.add_node(crate::graph::Node::Task(source));
        graph.add_node(crate::graph::Node::Task(flip));
        graph.add_node(crate::graph::Node::Task(eval));
        let config = migration_config(route);
        migrate_missing_pi_reasoning(&mut graph, &config);

        let source = graph.get_task("source").unwrap().clone();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        let new_pipeline = lifecycle.pipeline_id.clone();
        assert_ne!(new_pipeline, old_pipeline);

        // A stale verdict carrying the pre-migration pipeline id must NEVER
        // score the repaired attempt.
        let stale = DurableEvalVerdict {
            schema: EVAL_LIFECYCLE_SCHEMA,
            verdict_id: "verdict-stale".into(),
            verdict_digest: String::new(),
            evaluation_id: "evaluation-stale".into(),
            pipeline_id: old_pipeline,
            source_task: "source".into(),
            source_attempt: lifecycle.source_attempt,
            stage: AgencyStage::Evaluate,
            producer_run_id: "run-prior".into(),
            score: 1.0,
            evaluation_digest_schema: EVALUATION_DIGEST_DURABLE_BYTES_SCHEMA,
            evaluation_digest: "b3:stale".into(),
            created_at: Utc::now().to_rfc3339(),
        };
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[stale],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(
            graph
                .get_task("source")
                .unwrap()
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict,
            None,
            "stale old-pipeline verdict must never be consumed"
        );

        // New-pipeline evidence under the migrated generation promotes once.
        let flip_verdict = generation_verdict(
            graph.get_task("source").unwrap(),
            AgencyStage::FlipComparison,
            0.96,
        );
        let eval_verdict = generation_verdict(
            graph.get_task("source").unwrap(),
            AgencyStage::Evaluate,
            0.92,
        );
        assert!(reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict.clone(), eval_verdict.clone()],
            0.7,
            true,
            3,
            |_| true,
        ));
        let consumed = graph
            .get_task("source")
            .unwrap()
            .evaluation_lifecycle
            .as_ref()
            .unwrap()
            .consumed_verdict
            .clone();
        assert!(consumed.is_some(), "verdict consumed after migration");
        assert_eq!(
            graph.get_task("source").unwrap().status,
            Status::Done,
            "high-scoring migrated evidence promotes the source"
        );

        // Re-feeding the same verdicts is idempotent: consumed exactly once.
        assert!(!reconcile_durable_verdicts(
            &mut graph,
            &[flip_verdict, eval_verdict],
            0.7,
            true,
            3,
            |_| true,
        ));
        assert_eq!(
            graph
                .get_task("source")
                .unwrap()
                .evaluation_lifecycle
                .as_ref()
                .unwrap()
                .consumed_verdict,
            consumed,
            "verdict is consumed exactly once across re-ticks"
        );
    }
}
