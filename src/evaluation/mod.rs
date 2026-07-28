//! Attempt-bound lazy evaluation records.
//!
//! Evaluation is evidence attached to one immutable candidate, not graph work.
//! This module owns only policy selection and create-once record minting. It
//! deliberately does not execute a model, mutate source lifecycle state, or
//! allocate worker/build capacity.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub mod bounded;
pub mod deep;
pub mod rollout;

use crate::config::{Config, ReasoningLevel};
use crate::eval_lifecycle::{
    AgencyStage, DispatchSelectionSource, EvaluationGateApplicability, EvaluationGatePolicy,
    FlipVerdictPolicy,
};
use crate::graph::{Status, Task, WorkGraph, is_system_task};

pub const EVALUATION_RECORD_SCHEMA: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationProduct {
    Bounded,
    DeepReadonlyFlip,
}

impl EvaluationProduct {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bounded => "bounded-evaluation",
            Self::DeepReadonlyFlip => "deep-readonly-flip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationState {
    PreparingBundle,
    Queued,
    Running,
    EvidenceAvailable,
    Consumed,
    RetryBackoff,
    TimedOut,
    Malformed,
    RouteDrift,
    ProcessFailed,
    Unavailable,
    Cancelled,
}

/// Versioned, executor-neutral usage reported by the selected evaluation
/// adapter. Costs are never estimated here: Pi (or an explicitly selected
/// native adapter) is the usage authority for its own call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluationFailureKind {
    AdapterUnavailable,
    ProcessFailure,
    Timeout,
    MalformedOutput,
    RouteDrift,
    EvidenceUnavailable,
    ResourceDeferred,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationFailure {
    pub kind: EvaluationFailureKind,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_usage: Option<EvaluationUsage>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationAttempt {
    pub attempt_id: String,
    pub executor: String,
    pub exact_route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    pub renderer_version: u16,
    pub verdict_schema_version: u16,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<EvaluationUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<EvaluationFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoundedVerdictOutcome {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundedVerdict {
    pub schema_version: u16,
    pub verdict_id: String,
    pub score: f64,
    pub outcome: BoundedVerdictOutcome,
    pub dimensions: BTreeMap<String, f64>,
    pub summary: String,
    pub evidence_manifest_id: String,
    pub route_digest: String,
    pub received_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCandidateRef {
    pub task_id: String,
    pub generation: u64,
    pub source_attempt_id: String,
    pub source_fence: u64,
    pub finalization_round: u64,
    pub candidate_digest: String,
    pub candidate_manifest_digest: String,
    pub dependency_revision_digest: String,
    pub validation_result_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationPolicySnapshot {
    pub product: EvaluationProduct,
    pub applicability: EvaluationGateApplicability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    pub selector: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationRouteCall {
    pub stage: AgencyStage,
    pub exact_route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    pub handler: String,
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationRouteSnapshot {
    pub adapter: String,
    pub calls: Vec<EvaluationRouteCall>,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    pub schema: u16,
    pub evaluation_id: String,
    pub product: EvaluationProduct,
    pub source: SourceCandidateRef,
    pub policy: EvaluationPolicySnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<EvaluationRouteSnapshot>,
    pub route_digest: String,
    pub state: EvaluationState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_attempts: Vec<String>,
    /// Structured attempt provenance for the dedicated lane. The string list
    /// above remains a historical compatibility projection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<EvaluationAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_manifest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<BoundedVerdict>,
    /// Evidence-linked high-fidelity report produced only by the separately
    /// selected deep-readonly FLIP lane. Keeping this distinct from the
    /// bounded verdict prevents a summary grader from masquerading as a
    /// system-level investigation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_report: Option<deep::DeepFlipReport>,
    /// Reports superseded only by an explicit same-candidate FLIP retry. Each
    /// remains immutable, inspectable evidence; it is never averaged with or
    /// allowed to satisfy a later acceptance decision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_deep_reports: Vec<deep::DeepFlipReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_verdict_id: Option<String>,
    pub created_by_event: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LazyEvaluationSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounded: Option<EvaluationPolicySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_readonly_flip: Option<EvaluationPolicySnapshot>,
}

impl LazyEvaluationSelection {
    /// Resolve bounded and deep policy independently. `auto_evaluate` selects
    /// only bounded work. Deep FLIP requires its separate explicit switch or
    /// an explicit per-task high-risk/deep tag.
    pub fn resolve(task: &Task, config: &Config) -> Result<Self> {
        const EXCLUDED_TAGS: [&str; 10] = [
            "system",
            "meta",
            "evaluation",
            "message-only",
            "reconciliation-only",
            "draft",
            "unpublished",
            "cancelled",
            "skipped",
            "preparation-failed",
        ];
        if is_system_task(&task.id)
            || task.exec.is_some()
            || task.exec_mode.as_deref() == Some("shell")
            || task.paused
            || task
                .tags
                .iter()
                .any(|tag| EXCLUDED_TAGS.contains(&tag.as_str()))
        {
            return Ok(Self {
                bounded: None,
                deep_readonly_flip: None,
            });
        }

        // Bounded selection and deep selection are intentionally independent.
        // The managed FLIP-required stage never enables bounded auto-evaluation
        // and never consults eval_gate_all.
        let bounded_required = !config.evaluation.managed_rollout
            && config.agency.auto_evaluate
            && config.agency.eval_gate_threshold.is_some()
            && (config.agency.eval_gate_all || has_declared_deliverables(task));
        let bounded = config.agency.auto_evaluate.then(|| {
            policy_snapshot(
                EvaluationProduct::Bounded,
                if bounded_required {
                    EvaluationGateApplicability::Required
                } else {
                    EvaluationGateApplicability::Advisory
                },
                bounded_required
                    .then_some(config.agency.eval_gate_threshold)
                    .flatten(),
                if bounded_required {
                    "bounded:explicit-hard-gate"
                } else {
                    "bounded:optional-secondary"
                },
            )
        });

        let managed_flip_required = config.evaluation.managed_rollout
            && config.evaluation.rollout_stage
                == crate::config::EvaluationRolloutStage::FlipRequired;
        let explicit_deep = config.agency.flip_enabled
            || task.tags.iter().any(|tag| {
                matches!(
                    tag.as_str(),
                    "deep-readonly-flip" | "high-risk-evaluation" | "evaluation-high-risk"
                )
            });
        let deep_readonly_flip = explicit_deep.then(|| {
            let required = managed_flip_required || bounded_required;
            let threshold = required.then(|| {
                config
                    .agency
                    .flip_verification_threshold
                    .or(config.agency.eval_gate_threshold)
                    .unwrap_or(0.8)
            });
            policy_snapshot(
                EvaluationProduct::DeepReadonlyFlip,
                if required {
                    EvaluationGateApplicability::Required
                } else {
                    EvaluationGateApplicability::Advisory
                },
                threshold,
                if managed_flip_required {
                    "deep:managed-flip-required"
                } else if config.agency.flip_enabled {
                    "deep:explicit-policy"
                } else {
                    "deep:high-risk-task-policy"
                },
            )
        });

        Ok(Self {
            bounded,
            deep_readonly_flip,
        })
    }

    pub fn gate_policy(&self) -> Option<EvaluationGatePolicy> {
        let bounded = self.bounded.as_ref();
        let deep = self.deep_readonly_flip.as_ref();
        if bounded.is_none() && deep.is_none() {
            return None;
        }
        let required = bounded
            .is_some_and(|policy| policy.applicability == EvaluationGateApplicability::Required)
            || deep.is_some_and(|policy| {
                policy.applicability == EvaluationGateApplicability::Required
            });
        Some(EvaluationGatePolicy {
            applicability: if required {
                EvaluationGateApplicability::Required
            } else {
                EvaluationGateApplicability::Advisory
            },
            // None is authoritative for a deep-only gate: bounded evaluation
            // is absent, not silently assigned a default threshold.
            evaluator_threshold: bounded.and_then(|policy| policy.threshold),
            flip_policy: match deep {
                None => FlipVerdictPolicy::NotScheduled,
                Some(policy) if policy.applicability == EvaluationGateApplicability::Required => {
                    FlipVerdictPolicy::Required
                }
                Some(_) => FlipVerdictPolicy::Advisory,
            },
            flip_threshold: deep.and_then(|policy| policy.threshold),
            flip_threshold_source: None,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.bounded.is_none() && self.deep_readonly_flip.is_none()
    }
}

/// Stable, bounded operator projection for the required FLIP gate. It exposes
/// only content IDs and closed finding codes—never prompts, raw model text,
/// secrets, or chain-of-thought.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlipGateProjection {
    pub state: String,
    pub candidate_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_id: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finding_codes: Vec<String>,
    pub inspect_command: String,
    pub retry_flip_command: String,
    pub repair_command: String,
    pub waiver_command: String,
}

pub fn flip_gate_projection(task: &Task) -> Option<FlipGateProjection> {
    let record = task
        .evaluation_records
        .iter()
        .filter(|record| {
            record.product == EvaluationProduct::DeepReadonlyFlip
                && record.policy.applicability == EvaluationGateApplicability::Required
                && source_candidate_is_current(task, &record.source)
        })
        .max_by_key(|record| (record.source.generation, record.source.finalization_round))?;
    let report = record.deep_report.as_ref();
    let semantic_pass = report.is_some_and(|report| {
        report.outcome == BoundedVerdictOutcome::Pass
            && report.score >= record.policy.threshold.unwrap_or(1.0)
    });
    let state = match record.state {
        EvaluationState::PreparingBundle | EvaluationState::Queued => "flip-queued",
        EvaluationState::Running => "flip-running",
        EvaluationState::Consumed if semantic_pass && task.status == Status::Done => {
            "flip-passed-merged"
        }
        EvaluationState::Consumed if semantic_pass => "flip-passed-merging",
        EvaluationState::Consumed => "flip-rejected-repair-needed",
        EvaluationState::Unavailable
        | EvaluationState::TimedOut
        | EvaluationState::Malformed
        | EvaluationState::RouteDrift
        | EvaluationState::ProcessFailed => "flip-infrastructure-unavailable",
        _ => "waiting-on-required-flip",
    };
    let updated_at = record
        .attempts
        .last()
        .and_then(|attempt| {
            attempt
                .completed_at
                .clone()
                .or_else(|| Some(attempt.started_at.clone()))
        })
        .unwrap_or_else(|| record.created_at.clone());
    let mut finding_codes = report
        .map(|report| {
            report
                .findings
                .iter()
                .map(|finding| finding.finding_code.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    finding_codes.sort();
    finding_codes.dedup();
    Some(FlipGateProjection {
        state: state.into(),
        candidate_id: record.source.candidate_digest.clone(),
        report_id: report.map(|report| report.report_id.clone()),
        updated_at,
        finding_codes,
        inspect_command: format!("wg show {}", task.id),
        retry_flip_command: format!("wg evaluate run {} --flip", task.id),
        repair_command: format!("wg candidate repair {}", record.source.candidate_digest),
        waiver_command: format!(
            "wg candidate waive {} --report {} --reason '<operator-reason>'",
            record.source.candidate_digest,
            report
                .map(|report| report.report_id.as_str())
                .unwrap_or("<report-id>")
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MintSummary {
    pub created: usize,
    pub existing: usize,
}

/// Explicitly request one bounded advisory evaluation for the latest immutable
/// candidate. This is the pre-enable canary surface: it never changes global
/// policy, creates graph work, or permits a hard gate.
pub fn request_manual_bounded(dir: &Path, task_id: &str, config: &Config) -> Result<String> {
    let graph_path = dir.join("graph.jsonl");
    if !graph_path.exists() {
        bail!("WG not initialized. Run `wg init` first.");
    }
    let graph = crate::parser::load_graph(&graph_path)?;
    let task = graph.get_task_or_err(task_id)?;
    if !matches!(task.status, Status::Done | Status::Failed) {
        bail!("bounded evaluation canary requires a terminal candidate-completed source");
    }
    let source = task
        .evaluation_records
        .iter()
        .max_by_key(|record| (record.source.generation, record.source.finalization_round))
        .map(|record| record.source.clone())
        .or_else(|| source_from_candidate_event(dir, &graph, task).ok())
        .context(
            "bounded evaluation canary requires an immutable candidate-checkpointed source attempt",
        )?;
    if let Some(existing) = task
        .evaluation_records
        .iter()
        .find(|record| record.product == EvaluationProduct::Bounded && record.source == source)
    {
        return Ok(existing.evaluation_id.clone());
    }
    let policy = policy_snapshot(
        EvaluationProduct::Bounded,
        EvaluationGateApplicability::Advisory,
        None,
        "bounded:explicit-pre-enable-canary",
    );
    let route = route_snapshot(config, task, EvaluationProduct::Bounded)?;
    let route_digest = route.digest.clone();
    let evaluation_id = evaluation_id(
        EvaluationProduct::Bounded,
        &source,
        &policy.digest,
        &route_digest,
    )?;
    let created_by_event = task
        .lifecycle
        .audit
        .iter()
        .rev()
        .find(|event| {
            event.event_kind == "candidate-checkpointed"
                && event.attempt_id.as_deref() == Some(source.source_attempt_id.as_str())
                && event
                    .evidence_refs
                    .iter()
                    .any(|value| value == &source.candidate_digest)
        })
        .map(|event| event.event_id.clone())
        .context("bounded canary candidate checkpoint event is missing")?;
    let record = EvaluationRecord {
        schema: EVALUATION_RECORD_SCHEMA,
        evaluation_id: evaluation_id.clone(),
        product: EvaluationProduct::Bounded,
        source: source.clone(),
        policy,
        route: Some(route),
        route_digest,
        state: EvaluationState::PreparingBundle,
        runner_attempts: Vec::new(),
        attempts: Vec::new(),
        evidence_ids: vec![
            source.candidate_digest.clone(),
            source.candidate_manifest_digest.clone(),
            source.validation_result_id.clone(),
        ],
        evidence_manifest_id: None,
        verdict: None,
        deep_report: None,
        prior_deep_reports: Vec::new(),
        consumed_verdict_id: None,
        created_by_event,
        created_at: Utc::now().to_rfc3339(),
        diagnostic: Some(
            "Explicit bounded advisory canary requested; awaiting dedicated lane".into(),
        ),
    };
    crate::parser::modify_graph(&graph_path, |fresh| {
        let Some(task) = fresh.get_task_mut(task_id) else {
            return false;
        };
        if task.evaluation_records.iter().any(|existing| {
            existing.product == EvaluationProduct::Bounded && existing.source == source
        }) {
            return false;
        }
        task.evaluation_records.push(record.clone());
        task.log.push(crate::graph::LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("manual-bounded-canary".into()),
            user: None,
            message: format!(
                "Explicitly requested bounded advisory canary {} after immutable candidate completion",
                evaluation_id
            ),
        });
        true
    })?;
    Ok(evaluation_id)
}

/// Explicitly request a deep-readonly FLIP for the latest immutable candidate.
/// This is the manual trigger surface: it never changes global policy and it
/// refuses status-only or evidence-free tasks. High-risk policy uses the normal
/// completion-time minting path instead.
pub fn request_manual_deep(dir: &Path, task_id: &str, config: &Config) -> Result<String> {
    let graph_path = dir.join("graph.jsonl");
    if !graph_path.exists() {
        bail!("WG not initialized. Run `wg init` first.");
    }
    let graph = crate::parser::load_graph(&graph_path)?;
    let task = graph.get_task_or_err(task_id)?;
    if !matches!(
        task.status,
        Status::Done | Status::PendingEval | Status::FailedPendingEval | Status::Failed
    ) {
        bail!("deep FLIP requires a source attempt at candidate completion");
    }
    let source = task
        .evaluation_records
        .iter()
        .max_by_key(|record| (record.source.generation, record.source.finalization_round))
        .map(|record| record.source.clone())
        .or_else(|| source_from_candidate_event(dir, &graph, task).ok())
        .context("deep FLIP requires an immutable candidate-checkpointed source attempt")?;
    if let Some(existing) = task.evaluation_records.iter().find(|record| {
        record.product == EvaluationProduct::DeepReadonlyFlip && record.source == source
    }) {
        return Ok(existing.evaluation_id.clone());
    }
    let policy = policy_snapshot(
        EvaluationProduct::DeepReadonlyFlip,
        EvaluationGateApplicability::Advisory,
        None,
        "deep:explicit-manual-after-candidate",
    );
    let route = route_snapshot(config, task, EvaluationProduct::DeepReadonlyFlip)?;
    let route_digest = route.digest.clone();
    let evaluation_id = evaluation_id(
        EvaluationProduct::DeepReadonlyFlip,
        &source,
        &policy.digest,
        &route_digest,
    )?;
    let created_by_event = task
        .lifecycle
        .audit
        .iter()
        .rev()
        .find(|event| {
            event.event_kind == "candidate-checkpointed"
                && event.attempt_id.as_deref() == Some(source.source_attempt_id.as_str())
                && event
                    .evidence_refs
                    .iter()
                    .any(|value| value == &source.candidate_digest)
        })
        .map(|event| event.event_id.clone())
        .context("deep FLIP candidate checkpoint event is missing")?;
    let record = EvaluationRecord {
        schema: EVALUATION_RECORD_SCHEMA,
        evaluation_id: evaluation_id.clone(),
        product: EvaluationProduct::DeepReadonlyFlip,
        source: source.clone(),
        policy,
        route: Some(route),
        route_digest,
        state: EvaluationState::PreparingBundle,
        runner_attempts: Vec::new(),
        attempts: Vec::new(),
        evidence_ids: vec![
            source.candidate_digest.clone(),
            source.candidate_manifest_digest.clone(),
            source.validation_result_id.clone(),
        ],
        evidence_manifest_id: None,
        verdict: None,
        deep_report: None,
        prior_deep_reports: Vec::new(),
        consumed_verdict_id: None,
        created_by_event,
        created_at: Utc::now().to_rfc3339(),
        diagnostic: Some("Explicit deep-readonly FLIP requested; awaiting observation lane".into()),
    };
    crate::parser::modify_graph(&graph_path, |fresh| {
        let Some(task) = fresh.get_task_mut(task_id) else {
            return false;
        };
        if task.evaluation_records.iter().any(|existing| {
            existing.product == EvaluationProduct::DeepReadonlyFlip && existing.source == source
        }) {
            return false;
        }
        task.evaluation_records.push(record.clone());
        task.evaluation_records.sort_by(|a, b| {
            a.source
                .generation
                .cmp(&b.source.generation)
                .then_with(|| a.product.label().cmp(b.product.label()))
        });
        task.log.push(crate::graph::LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("manual-deep-flip".into()),
            user: None,
            message: format!(
                "Explicitly requested deep-readonly FLIP {} after immutable candidate completion",
                evaluation_id
            ),
        });
        true
    })?;
    Ok(evaluation_id)
}

fn source_from_candidate_event(
    dir: &Path,
    graph: &WorkGraph,
    task: &Task,
) -> Result<SourceCandidateRef> {
    let event = task
        .lifecycle
        .audit
        .iter()
        .rev()
        .find(|event| {
            event.event_kind == "candidate-checkpointed"
                && event.evidence_refs.len() >= 3
                && event.attempt_id.is_some()
        })
        .context("candidate checkpoint unavailable")?;
    let candidate_digest = event.evidence_refs[0].clone();
    let descriptor_path = dir
        .join("finalization/objects")
        .join(candidate_digest.replace(':', "_"));
    let descriptor: crate::finalization::CandidateDescriptor =
        serde_json::from_slice(&fs::read(descriptor_path)?)?;
    Ok(SourceCandidateRef {
        task_id: task.id.clone(),
        generation: event.generation,
        source_attempt_id: event.attempt_id.clone().unwrap_or_default(),
        source_fence: event.fence,
        finalization_round: descriptor.candidate_version,
        candidate_digest,
        candidate_manifest_digest: event.evidence_refs[1].clone(),
        dependency_revision_digest: dependency_revision_digest(graph, task)?,
        validation_result_id: event.evidence_refs[2].clone(),
    })
}

/// True only when the current generation/fence has the authoritative proof
/// emitted after the serialized launch gate was released. Reservations,
/// admission deferrals, preparation failures and reconciler status writes do
/// not satisfy this predicate.
pub fn has_authenticated_running_attempt(task: &Task) -> bool {
    let Some(attempt) = task.lifecycle.current_attempt.as_ref() else {
        return false;
    };
    attempt.disposition.is_none()
        && attempt.generation == task.lifecycle.generation
        && attempt.fence == task.lifecycle.fence
        && task.lifecycle.audit.iter().any(|event| {
            event.event_kind == "attempt-running"
                && event.generation == attempt.generation
                && event.attempt_id.as_deref() == Some(attempt.id.as_str())
                && event.fence == attempt.fence
        })
}

/// True only while this immutable candidate still names the authoritative,
/// successfully completed source generation/attempt/fence. Evaluation reports
/// for older tuples remain evidence, but may never reject, waive, or merge the
/// current source.
pub fn source_candidate_is_current(task: &Task, source: &SourceCandidateRef) -> bool {
    use crate::lifecycle::AttemptDisposition;

    task.id == source.task_id
        && task.lifecycle.generation == source.generation
        && task.lifecycle.fence == source.source_fence
        && task
            .lifecycle
            .current_attempt
            .as_ref()
            .is_some_and(|attempt| {
                attempt.id == source.source_attempt_id
                    && attempt.generation == source.generation
                    && attempt.fence == source.source_fence
                    && attempt.disposition == Some(AttemptDisposition::Succeeded)
            })
}

/// Mint the selected records exactly once for an authenticated running source
/// candidate. Callers perform this while holding the graph lock, in the same
/// commit as `CandidateCheckpointed` and the completion disposition.
pub fn mint_for_candidate(
    task: &mut Task,
    source: &SourceCandidateRef,
    selection: &LazyEvaluationSelection,
    config: &Config,
) -> Result<MintSummary> {
    validate_creation_predicate(task, source)?;
    let created_event = task
        .lifecycle
        .audit
        .iter()
        .rev()
        .find(|event| {
            event.event_kind == "candidate-checkpointed"
                && event.generation == source.generation
                && event.attempt_id.as_deref() == Some(source.source_attempt_id.as_str())
                && event.fence == source.source_fence
                && event
                    .evidence_refs
                    .iter()
                    .any(|evidence| evidence == &source.candidate_digest)
        })
        .context("lazy-evaluation.candidate-event-missing")?
        .event_id
        .clone();

    let mut created = 0usize;
    let mut existing = 0usize;
    for policy in [
        selection.bounded.as_ref(),
        selection.deep_readonly_flip.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        // Product + immutable source tuple is the semantic uniqueness key.
        // It intentionally wins over ambient config after restart.
        if task
            .evaluation_records
            .iter()
            .any(|record| record.product == policy.product && record.source == *source)
        {
            existing += 1;
            continue;
        }

        let route_result = route_snapshot(config, task, policy.product);
        let (route, route_digest, state, diagnostic) = match route_result {
            Ok(route) => {
                let digest = route.digest.clone();
                (Some(route), digest, EvaluationState::PreparingBundle, None)
            }
            Err(error) => {
                let diagnostic = format!("route unavailable: {error:#}");
                let digest = digest_json(&serde_json::json!({
                    "schema": 1,
                    "product": policy.product,
                    "diagnostic": diagnostic,
                }))?;
                (None, digest, EvaluationState::Unavailable, Some(diagnostic))
            }
        };
        let evaluation_id = evaluation_id(policy.product, source, &policy.digest, &route_digest)?;
        task.evaluation_records.push(EvaluationRecord {
            schema: EVALUATION_RECORD_SCHEMA,
            evaluation_id,
            product: policy.product,
            source: source.clone(),
            policy: policy.clone(),
            route,
            route_digest,
            state,
            runner_attempts: Vec::new(),
            attempts: Vec::new(),
            evidence_ids: vec![
                source.candidate_digest.clone(),
                source.candidate_manifest_digest.clone(),
                source.validation_result_id.clone(),
            ],
            evidence_manifest_id: None,
            verdict: None,
            deep_report: None,
            prior_deep_reports: Vec::new(),
            consumed_verdict_id: None,
            created_by_event: created_event.clone(),
            created_at: Utc::now().to_rfc3339(),
            diagnostic,
        });
        created += 1;
    }
    task.evaluation_records.sort_by(|a, b| {
        a.source
            .generation
            .cmp(&b.source.generation)
            .then_with(|| {
                a.source
                    .finalization_round
                    .cmp(&b.source.finalization_round)
            })
            .then_with(|| a.product.label().cmp(b.product.label()))
            .then_with(|| a.evaluation_id.cmp(&b.evaluation_id))
    });
    Ok(MintSummary { created, existing })
}

/// Stable digest of the exact dependency revisions observed at candidate
/// completion. This is metadata only; evaluation cannot follow live edges.
pub fn dependency_revision_digest(graph: &WorkGraph, task: &Task) -> Result<String> {
    let mut dependencies: Vec<serde_json::Value> = task
        .after
        .iter()
        .map(|id| {
            graph.get_task(id).map_or_else(
                || serde_json::json!({"id": id, "missing": true}),
                |dependency| {
                    serde_json::json!({
                        "id": id,
                        "generation": dependency.lifecycle.generation,
                        "revision": dependency.lifecycle.revision,
                        "status": dependency.status,
                    })
                },
            )
        })
        .collect();
    dependencies.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    digest_json(&serde_json::json!({"schema": 1, "dependencies": dependencies}))
}

fn validate_creation_predicate(task: &Task, source: &SourceCandidateRef) -> Result<()> {
    if task.id != source.task_id || task.status != Status::InProgress {
        bail!("lazy-evaluation.source-not-running");
    }
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("lazy-evaluation.attempt-missing")?;
    if attempt.id != source.source_attempt_id
        || attempt.generation != source.generation
        || attempt.fence != source.source_fence
        || task.lifecycle.generation != source.generation
        || task.lifecycle.fence != source.source_fence
        || attempt.disposition != None
    {
        bail!("lazy-evaluation.source-fence-mismatch");
    }
    if !has_authenticated_running_attempt(task) {
        bail!("lazy-evaluation.attempt-never-ran");
    }
    if source.candidate_digest.trim().is_empty()
        || source.candidate_manifest_digest.trim().is_empty()
        || source.validation_result_id.trim().is_empty()
    {
        bail!("lazy-evaluation.candidate-binding-incomplete");
    }
    Ok(())
}

fn route_snapshot(
    config: &Config,
    task: &Task,
    product: EvaluationProduct,
) -> Result<EvaluationRouteSnapshot> {
    let virtual_id = match product {
        EvaluationProduct::Bounded => format!(".evaluate-{}", task.id),
        EvaluationProduct::DeepReadonlyFlip => format!(".flip-{}", task.id),
    };
    let effective =
        crate::dispatch::effective_config_owned(task.profile.as_deref(), config.clone());
    let plan = crate::eval_lifecycle::build_plan(
        &effective,
        task,
        &virtual_id,
        DispatchSelectionSource::ScaffoldConfig,
    )?;
    let calls: Vec<EvaluationRouteCall> = plan
        .calls
        .into_iter()
        .map(|call| EvaluationRouteCall {
            stage: call.stage,
            exact_route: call.route,
            endpoint: call.endpoint,
            reasoning: call.reasoning,
            handler: call.system.handler,
            provider: call.system.provider,
        })
        .collect();
    let adapter = calls
        .first()
        .map(|call| format!("{}-evaluation-v1", call.handler))
        .context("evaluation route contains no calls")?;
    let digest = digest_json(&serde_json::json!({
        "schema": 1,
        "adapter": adapter,
        "calls": calls,
    }))?;
    Ok(EvaluationRouteSnapshot {
        adapter,
        calls,
        digest,
    })
}

fn policy_snapshot(
    product: EvaluationProduct,
    applicability: EvaluationGateApplicability,
    threshold: Option<f64>,
    selector: &str,
) -> EvaluationPolicySnapshot {
    let value = serde_json::json!({
        "schema": 1,
        "product": product,
        "applicability": applicability,
        "threshold": threshold,
        "selector": selector,
    });
    EvaluationPolicySnapshot {
        product,
        applicability,
        threshold,
        selector: selector.to_string(),
        digest: digest_json(&value).expect("policy snapshot is JSON serializable"),
    }
}

fn evaluation_id(
    product: EvaluationProduct,
    source: &SourceCandidateRef,
    policy_digest: &str,
    route_digest: &str,
) -> Result<String> {
    let value = serde_json::json!({
        "domain": "wg-evaluation-v1",
        "product": product,
        "source": source,
        "policy_digest": policy_digest,
        "route_digest": route_digest,
    });
    Ok(format!(
        "eval-{}",
        digest_json(&value)?.trim_start_matches("b3:")
    ))
}

fn digest_json(value: &serde_json::Value) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
}

fn has_declared_deliverables(task: &Task) -> bool {
    if !task.deliverables.is_empty() {
        return true;
    }
    let Some(description) = task.description.as_deref() else {
        return false;
    };
    let mut in_section = false;
    for line in description.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            in_section = trimmed.eq_ignore_ascii_case("## Deliverables");
            continue;
        }
        if in_section && !trimmed.is_empty() {
            return true;
        }
    }
    false
}
