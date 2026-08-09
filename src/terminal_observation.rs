//! Exactly-once projection of accepted terminal task outcomes into Agency.
//!
//! This is an observation boundary, not a completion controller. The module
//! only reads terminal graph state and immutable completion/review objects and
//! creates a content-bound Agency record. It has no lifecycle, publication,
//! retry, dispatch, or graph-mutation API.

use crate::agency;
use crate::completion_manifest::{
    CompletionArtifactStore, ContentDigest, OutputRef, ResolvedReviewBundle, ReviewResolver,
};
use crate::completion_review::{
    CompletionReviewBinding, ReviewCandidateState, ReviewFailureClass, ReviewReceipt, ReviewUsage,
    ReviewerKind, verified_review_activities,
};
use crate::completion_task::{
    ReviewEvidence, load_exact_review_pair, load_review_evidence, load_submission_bytes,
};
use crate::graph::{CompletionContract, CompletionDisposition, Status, Task, TokenUsage};
use crate::identity::canonical_json;
use crate::lifecycle::{ActorKind, AttemptDisposition, LifecycleEvent};
use crate::parser::load_graph;
use crate::simple_land::ReviewVerdict;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

pub const TERMINAL_OBSERVATION_SCHEMA_VERSION: u32 = 1;
pub const TERMINAL_OBSERVATION_POLICY: &str = "accepted-terminal-outcome-v1";
pub const DEFAULT_TERMINAL_OBSERVATION_BACKFILL_LIMIT: usize = 256;
const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const OBSERVATIONS_DIR: &str = "terminal-observations";

/// The deterministic exactly-once key. A later generation or attempt is a new
/// observation; replaying the same immutable completion receipt is not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalObservationKey {
    pub policy: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub completion_receipt: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAcceptanceKind {
    ReviewedCompletion,
    OperatorAccepted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewedCompletionProvenance {
    pub manifest_digest: String,
    pub requirements_digest: String,
    pub flip_receipt_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_receipt_digest: Option<String>,
    pub review_policy: String,
    pub publication_receipt: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorAcceptanceProvenance {
    pub operator: String,
    pub reason: String,
    pub status_before_accept: String,
    pub generation_before_accept: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    /// Operator acceptance is explicit adjudication, not proof that ordinary
    /// publication was verified. Keeping this false prevents later consumers
    /// from silently treating it as a normal reviewed landing.
    pub ordinary_publication_verified: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgencyAttribution {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tradeoff_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExecutionAttribution {
    pub lifecycle_actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TerminalReviewObservation {
    pub receipt_id: String,
    pub reviewer_kind: ReviewerKind,
    pub verdict: ReviewVerdict,
    pub candidate_state: ReviewCandidateState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<CompletionReviewBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<ReviewFailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ReviewUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub findings_digest: String,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalObservationScoreState {
    Unscored,
}

/// One terminal generation episode. Completion-review verdicts are retained as
/// evidence but are deliberately not converted into an Agency score.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TerminalOutcomeObservation {
    pub schema_version: u32,
    pub observation_id: String,
    pub key: TerminalObservationKey,
    pub acceptance_kind: TerminalAcceptanceKind,
    pub disposition: CompletionDisposition,
    pub completion_contract: CompletionContract,
    pub completed_at: String,
    pub agency_attribution: AgencyAttribution,
    pub execution: ExecutionAttribution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_completion: Option<ReviewedCompletionProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_acceptance: Option<OperatorAcceptanceProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<TerminalReviewObservation>,
    pub current_candidate_review_disagreement: bool,
    pub review_trajectory_disagreement: bool,
    pub invalid_review_activity_count: usize,
    /// Always `None` for this projection. `wg evaluate` remains the explicit
    /// surface that creates scored Agency evaluations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub score_state: TerminalObservationScoreState,
    pub score_semantics: String,
    pub unknown_unscored_fields: Vec<String>,
}

/// Exact immutable receipt created by ordinary reviewed completion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewedCompletionReceipt {
    pub receipt_version: u32,
    pub task_id: String,
    pub generation: u64,
    pub manifest_digest: String,
    pub requirements_digest: String,
    pub flip_receipt_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_receipt_digest: Option<String>,
    pub review_policy: String,
    pub contract: String,
    pub publication: String,
    pub completed_at: String,
}

/// Exact immutable receipt created by the reason-required operator escape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorAcceptanceReceipt {
    pub receipt_version: u32,
    pub task_id: String,
    pub generation_before_accept: u64,
    pub status_before_accept: String,
    pub reason: String,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    pub accepted_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionStatus {
    Created { observation_id: String },
    Existing { observation_id: String },
    Skipped { reason: String },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BackfillReport {
    pub limit: usize,
    pub candidates: usize,
    pub attempted: usize,
    pub created: usize,
    pub existing: usize,
    pub skipped: usize,
    pub remaining: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Error)]
pub enum TerminalObservationError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Agency error: {0}")]
    Agency(#[from] agency::AgencyError),
    #[error("graph error: {0}")]
    Graph(String),
    #[error("task is not eligible for scored evaluation: {0}")]
    Ineligible(String),
    #[error("terminal observation collision at {0}")]
    Collision(String),
}

fn observations_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("agency").join(OBSERVATIONS_DIR)
}

pub fn terminal_observations_dir(workgraph_dir: &Path) -> PathBuf {
    observations_dir(workgraph_dir)
}

fn observation_id(key: &TerminalObservationKey) -> Result<String, TerminalObservationError> {
    let value = serde_json::to_value(key)?;
    let bytes = canonical_json(&value);
    Ok(format!(
        "terminal-observation-v1:{}",
        blake3::hash(&bytes).to_hex()
    ))
}

fn observation_path(workgraph_dir: &Path, id: &str) -> Result<PathBuf, TerminalObservationError> {
    let Some(hash) = id.strip_prefix("terminal-observation-v1:") else {
        return Err(TerminalObservationError::Collision(id.to_string()));
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TerminalObservationError::Collision(id.to_string()));
    }
    Ok(observations_dir(workgraph_dir).join(format!("{hash}.json")))
}

fn verify_observation_identity(
    observation: &TerminalOutcomeObservation,
) -> Result<(), TerminalObservationError> {
    if observation.schema_version != TERMINAL_OBSERVATION_SCHEMA_VERSION
        || observation.key.policy != TERMINAL_OBSERVATION_POLICY
        || observation.observation_id != observation_id(&observation.key)?
        || observation.score.is_some()
        || observation.score_state != TerminalObservationScoreState::Unscored
    {
        return Err(TerminalObservationError::Collision(
            observation.observation_id.clone(),
        ));
    }
    Ok(())
}

fn save_observation_create_once(
    workgraph_dir: &Path,
    observation: &TerminalOutcomeObservation,
) -> Result<ProjectionStatus, TerminalObservationError> {
    verify_observation_identity(observation)?;
    let path = observation_path(workgraph_dir, &observation.observation_id)?;
    let bytes = serde_json::to_vec_pretty(observation)?;
    match crate::atomic_file::write_atomic_create_new(&path, &bytes) {
        Ok(()) => Ok(ProjectionStatus::Created {
            observation_id: observation.observation_id.clone(),
        }),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing: TerminalOutcomeObservation = serde_json::from_slice(&fs::read(&path)?)?;
            verify_observation_identity(&existing)?;
            if existing.key != observation.key {
                return Err(TerminalObservationError::Collision(
                    path.display().to_string(),
                ));
            }
            Ok(ProjectionStatus::Existing {
                observation_id: existing.observation_id,
            })
        }
        Err(error) => Err(error.into()),
    }
}

pub fn load_terminal_outcome_observations(
    workgraph_dir: &Path,
) -> Result<Vec<TerminalOutcomeObservation>, TerminalObservationError> {
    let dir = observations_dir(workgraph_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut observations = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let observation: TerminalOutcomeObservation = serde_json::from_slice(&fs::read(&path)?)?;
        verify_observation_identity(&observation)?;
        if observation_path(workgraph_dir, &observation.observation_id)? != path {
            return Err(TerminalObservationError::Collision(
                path.display().to_string(),
            ));
        }
        observations.push(observation);
    }
    observations.sort_by(|left, right| {
        left.completed_at
            .cmp(&right.completed_at)
            .then(left.observation_id.cmp(&right.observation_id))
    });
    Ok(observations)
}

pub fn count_terminal_outcome_observations(workgraph_dir: &Path) -> usize {
    load_terminal_outcome_observations(workgraph_dir)
        .map(|observations| observations.len())
        .unwrap_or(0)
}

fn expected_disposition(contract: CompletionContract) -> Option<CompletionDisposition> {
    match contract {
        CompletionContract::Land => Some(CompletionDisposition::Landed),
        CompletionContract::Report => Some(CompletionDisposition::Reported),
        CompletionContract::Explore => Some(CompletionDisposition::Explored),
        CompletionContract::Deliver => None,
    }
}

fn terminal_event<'a>(task: &'a Task, receipt: &str) -> Result<&'a LifecycleEvent, String> {
    let matches = task
        .lifecycle
        .audit
        .iter()
        .filter(|event| {
            event.generation == task.lifecycle.generation
                && event.new_state == Status::Done
                && event.evidence_refs.iter().any(|value| value == receipt)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "expected one receipt-bound terminal lifecycle event, found {}",
            matches.len()
        ));
    }
    let event = matches[0];
    let attempt_id = event
        .attempt_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "terminal lifecycle event has no attempt identity".to_string())?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .ok_or_else(|| "terminal task has no current attempt".to_string())?;
    if attempt.id != attempt_id
        || attempt.generation != event.generation
        || attempt.fence != event.fence
        || attempt.disposition != Some(AttemptDisposition::Succeeded)
    {
        return Err(
            "terminal lifecycle event does not match the successful current attempt".into(),
        );
    }
    Ok(event)
}

fn receipt_object_bytes(workgraph_dir: &Path, receipt: &str) -> Result<Vec<u8>, String> {
    let digest = ContentDigest::parse(receipt.to_string())
        .map_err(|error| format!("completion receipt is not a v3 content digest: {error}"))?;
    let name = digest
        .as_str()
        .strip_prefix("b3:")
        .ok_or_else(|| "completion receipt has no object name".to_string())?;
    let path = workgraph_dir.join("completion/v3/objects").join(name);
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "completion receipt {} is unreadable: {error}",
            path.display()
        )
    })?;
    if bytes.len() > MAX_RECEIPT_BYTES || ContentDigest::of_bytes(&bytes) != digest {
        return Err("completion receipt bytes are missing, oversized, or digest-mismatched".into());
    }
    Ok(bytes)
}

fn output_identity(output: &OutputRef) -> String {
    match output {
        OutputRef::Git(git) => git.commit_oid.clone(),
        OutputRef::Artifact(artifact) => artifact.content_digest.to_string(),
        OutputRef::External(external) => external.after_digest.to_string(),
    }
}

fn verify_publication_receipt(
    project_root: &Path,
    contract: CompletionContract,
    outputs: &[OutputRef],
    publication: &str,
) -> Result<(), String> {
    let expected = match contract {
        CompletionContract::Land => {
            let commits = outputs
                .iter()
                .filter_map(|output| match output {
                    OutputRef::Git(git) => Some(git.commit_oid.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if commits.len() != 1 {
                return Err("Land observation requires exactly one Git output".into());
            }
            let commit = commits[0];
            let suffix = format!(":{commit}");
            let integration_ref = publication
                .strip_prefix("git:")
                .and_then(|value| value.strip_suffix(&suffix))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "publication receipt does not bind the manifest Git output".to_string()
                })?;
            let status = Command::new("git")
                .args(["merge-base", "--is-ancestor", commit, integration_ref])
                .current_dir(project_root)
                .status()
                .map_err(|error| format!("publication verification failed: {error}"))?;
            if !status.success() {
                return Err(format!(
                    "reviewed commit {commit} is not reachable from {integration_ref}"
                ));
            }
            format!("git:{integration_ref}:{commit}")
        }
        CompletionContract::Report => format!(
            "artifacts:{}",
            outputs
                .iter()
                .map(output_identity)
                .collect::<Vec<_>>()
                .join(",")
        ),
        CompletionContract::Explore => format!(
            "exploration:{}",
            outputs
                .iter()
                .map(output_identity)
                .collect::<Vec<_>>()
                .join(",")
        ),
        CompletionContract::Deliver => {
            return Err("legacy Deliver has no v3 terminal observation policy".into());
        }
    };
    if publication != expected {
        return Err("completion receipt publication does not match immutable outputs".into());
    }
    Ok(())
}

fn route(executor: Option<&str>, model: Option<&str>) -> Option<String> {
    match (executor, model) {
        (Some(executor), Some(model)) if model.starts_with(&format!("{executor}:")) => {
            Some(model.to_string())
        }
        (Some(executor), Some(model)) => Some(format!("{executor}:{model}")),
        (None, Some(model)) => Some(model.to_string()),
        _ => None,
    }
}

fn agency_attribution(workgraph_dir: &Path, task: &Task) -> AgencyAttribution {
    let Some(agent_id) = task.agent.clone() else {
        return AgencyAttribution {
            state: "uncomposed_direct_dispatch".into(),
            agent_id: None,
            role_id: None,
            tradeoff_id: None,
        };
    };
    let agents = workgraph_dir.join("agency/cache/agents");
    match agency::find_agent_by_prefix(&agents, &agent_id) {
        Ok(agent) => AgencyAttribution {
            state: "resolved_composition".into(),
            agent_id: Some(agent.id),
            role_id: Some(agent.role_id),
            tradeoff_id: Some(agent.tradeoff_id),
        },
        Err(_) => AgencyAttribution {
            state: "unresolved_composition".into(),
            agent_id: Some(agent_id),
            role_id: None,
            tradeoff_id: None,
        },
    }
}

fn review_observation(
    receipt_id: String,
    receipt: &ReviewReceipt,
    candidate_state: ReviewCandidateState,
) -> TerminalReviewObservation {
    TerminalReviewObservation {
        receipt_id,
        reviewer_kind: receipt.reviewer_kind,
        verdict: receipt.verdict,
        candidate_state,
        binding: receipt.binding.clone(),
        failure_class: receipt.failure_class,
        model_route: receipt.model_route.clone(),
        executor: receipt.executor.clone(),
        usage: receipt.usage.clone(),
        duration_ms: receipt.duration_ms,
        findings_digest: receipt.findings_digest.to_string(),
        created_at: receipt.created_at.clone(),
    }
}

fn merge_reviews(
    workgraph_dir: &Path,
    task: &Task,
    current: Vec<(String, ReviewReceipt)>,
) -> (Vec<TerminalReviewObservation>, usize) {
    let verified = verified_review_activities(workgraph_dir, task);
    let mut reviews = BTreeMap::new();
    for activity in verified.activities {
        reviews.insert(
            activity.activity_id.clone(),
            TerminalReviewObservation {
                receipt_id: activity.activity_id.clone(),
                reviewer_kind: activity.reviewer_kind,
                verdict: activity.verdict,
                candidate_state: activity.candidate_state,
                binding: activity.binding.clone(),
                failure_class: activity.failure_class,
                model_route: activity.model_route.clone(),
                executor: activity.executor.clone(),
                usage: activity.usage.clone(),
                duration_ms: activity.duration_ms,
                findings_digest: activity
                    .findings_digest
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                created_at: activity.created_at.clone(),
            },
        );
    }
    for (id, receipt) in current {
        reviews.insert(
            id.clone(),
            review_observation(id, &receipt, ReviewCandidateState::Current),
        );
    }
    let mut reviews = reviews.into_values().collect::<Vec<_>>();
    reviews.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.receipt_id.cmp(&right.receipt_id))
    });
    (reviews, verified.invalid_count)
}

fn current_reviews(
    task: &Task,
    evidence: ReviewEvidence,
) -> Result<Vec<(String, ReviewReceipt)>, String> {
    let candidate = task
        .completion_candidate
        .as_ref()
        .ok_or_else(|| "terminal task has no selected completion candidate".to_string())?;
    let flip_id = candidate
        .flip_receipt
        .as_ref()
        .ok_or_else(|| "terminal task has no FLIP receipt reference".to_string())?
        .content_digest
        .to_string();
    let mut reviews = vec![(flip_id, evidence.flip)];
    match (candidate.eval_receipt.as_ref(), evidence.eval) {
        (Some(reference), Some(receipt)) => {
            reviews.push((reference.content_digest.to_string(), receipt));
        }
        (None, None) => {}
        _ => return Err("completion receipt and candidate disagree on eval evidence".into()),
    }
    Ok(reviews)
}

struct BaseObservationInput<'a> {
    workgraph_dir: &'a Path,
    task: &'a Task,
    event: &'a LifecycleEvent,
    receipt: &'a str,
    acceptance_kind: TerminalAcceptanceKind,
    completed_at: String,
    reviews: Vec<TerminalReviewObservation>,
    invalid_review_activity_count: usize,
}

fn base_observation(input: BaseObservationInput<'_>) -> Result<TerminalOutcomeObservation, String> {
    let BaseObservationInput {
        workgraph_dir,
        task,
        event,
        receipt,
        acceptance_kind,
        completed_at,
        reviews,
        invalid_review_activity_count,
    } = input;
    let disposition = expected_disposition(task.completion_contract)
        .ok_or_else(|| "legacy Deliver is not eligible for observation projection".to_string())?;
    if task.status != Status::Done
        || task.completion_disposition != Some(disposition)
        || task.completed_at.as_deref() != Some(completed_at.as_str())
    {
        return Err("task is not the exact receipt-backed terminal Done projection".into());
    }
    let attempt_id = event
        .attempt_id
        .clone()
        .ok_or_else(|| "terminal event has no attempt identity".to_string())?;
    let key = TerminalObservationKey {
        policy: TERMINAL_OBSERVATION_POLICY.to_string(),
        task_id: task.id.clone(),
        generation: event.generation,
        attempt_id,
        attempt_fence: event.fence,
        completion_receipt: receipt.to_string(),
    };
    let observation_id = observation_id(&key).map_err(|error| error.to_string())?;
    let current_candidate_review_disagreement = reviews.iter().any(|review| {
        review.candidate_state == ReviewCandidateState::Current
            && review.verdict != ReviewVerdict::Pass
    });
    let review_trajectory_disagreement = reviews
        .iter()
        .any(|review| review.verdict != ReviewVerdict::Pass);
    Ok(TerminalOutcomeObservation {
        schema_version: TERMINAL_OBSERVATION_SCHEMA_VERSION,
        observation_id,
        key,
        acceptance_kind,
        disposition,
        completion_contract: task.completion_contract,
        completed_at,
        agency_attribution: agency_attribution(workgraph_dir, task),
        execution: ExecutionAttribution {
            lifecycle_actor: event.actor_id.clone(),
            executor: task.actual_executor.clone(),
            model: task.actual_model.clone(),
            route: route(task.actual_executor.as_deref(), task.actual_model.as_deref()),
            requested_model: task.model.clone(),
            profile: task.profile.clone(),
            usage: task.token_usage.clone(),
        },
        reviewed_completion: None,
        operator_acceptance: None,
        reviews,
        current_candidate_review_disagreement,
        review_trajectory_disagreement,
        invalid_review_activity_count,
        score: None,
        score_state: TerminalObservationScoreState::Unscored,
        score_semantics: "completion review is advisory evidence, not an Agency evaluation score; use `wg evaluate` to score this task".into(),
        unknown_unscored_fields: vec![
            "quality_score".into(),
            "evaluation_dimensions".into(),
            "independent_ground_truth".into(),
            "assignment_reward".into(),
            "reviewer_calibration".into(),
        ],
    })
}

fn build_reviewed_observation_with_bundle(
    workgraph_dir: &Path,
    project_root: &Path,
    task: &Task,
    receipt_digest: &str,
    bytes: &[u8],
) -> Result<(TerminalOutcomeObservation, ResolvedReviewBundle), String> {
    let receipt: ReviewedCompletionReceipt = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid reviewed completion receipt: {error}"))?;
    if receipt.receipt_version != 1
        || receipt.task_id != task.id
        || receipt.generation != task.lifecycle.generation
        || receipt.contract != task.completion_contract.to_string()
        || !matches!(receipt.review_policy.as_str(), "strict" | "advisory")
    {
        return Err("reviewed completion receipt does not match the terminal task".into());
    }
    let event = terminal_event(task, receipt_digest)?;
    if event.actor_kind != ActorKind::Finalizer
        || event.reason_code != "reviewed_publication_committed"
        || event.event_kind != "attempt-succeeded"
    {
        return Err("terminal event is not an ordinary reviewed completion".into());
    }
    let store = CompletionArtifactStore::open(workgraph_dir.join("completion/v3"))
        .map_err(|error| format!("completion store unavailable: {error}"))?;
    let (submission, manifest, requirements, summary) = load_submission_bytes(&store, task)
        .map_err(|error| format!("completion candidate no longer verifies: {error}"))?;
    let manifest_digest = manifest
        .digest()
        .map_err(|error| format!("completion manifest invalid: {error}"))?;
    if receipt.manifest_digest != manifest_digest.to_string()
        || receipt.requirements_digest != manifest.requirements_digest.to_string()
        || receipt.flip_receipt_digest
            != submission
                .flip_receipt_ref
                .as_ref()
                .map(|reference| reference.content_digest.to_string())
                .unwrap_or_default()
        || receipt.eval_receipt_digest
            != submission
                .eval_receipt_ref
                .as_ref()
                .map(|reference| reference.content_digest.to_string())
    {
        return Err("completion receipt does not bind the selected immutable candidate".into());
    }
    let binding = submission
        .review_binding
        .as_ref()
        .ok_or_else(|| "completion candidate has no attempt-bound review identity".to_string())?;
    if binding.task_id != task.id
        || binding.generation != event.generation
        || binding.attempt_id.as_deref() != event.attempt_id.as_deref()
        || binding.attempt_fence != event.fence
    {
        return Err("completion candidate review binding is stale for the terminal attempt".into());
    }
    let dependency_outputs = &task
        .completion_candidate
        .as_ref()
        .ok_or_else(|| "terminal task has no selected completion candidate".to_string())?
        .dependency_outputs;
    let resolver = ReviewResolver::new(&store);
    let resolved = if task.completion_contract == CompletionContract::Land {
        resolver.repository(project_root).resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            dependency_outputs,
        )
    } else {
        resolver.resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            dependency_outputs,
        )
    }
    .map_err(|error| format!("completion outputs no longer resolve: {error}"))?;
    let review_evidence = if receipt.review_policy == "strict" {
        let pair = load_exact_review_pair(&store, &submission, &manifest, &resolved)
            .map_err(|error| format!("strict completion review no longer verifies: {error}"))?;
        ReviewEvidence {
            flip: pair.flip,
            eval: Some(pair.eval),
        }
    } else {
        load_review_evidence(&store, &submission, &manifest, &resolved)
            .map_err(|error| format!("advisory completion review no longer verifies: {error}"))?
    };
    verify_publication_receipt(
        project_root,
        task.completion_contract,
        &manifest.outputs,
        &receipt.publication,
    )?;
    let current = current_reviews(task, review_evidence)?;
    let (reviews, invalid_count) = merge_reviews(workgraph_dir, task, current);
    let mut observation = base_observation(BaseObservationInput {
        workgraph_dir,
        task,
        event,
        receipt: receipt_digest,
        acceptance_kind: TerminalAcceptanceKind::ReviewedCompletion,
        completed_at: receipt.completed_at.clone(),
        reviews,
        invalid_review_activity_count: invalid_count,
    })?;
    observation.reviewed_completion = Some(ReviewedCompletionProvenance {
        manifest_digest: receipt.manifest_digest,
        requirements_digest: receipt.requirements_digest,
        flip_receipt_digest: receipt.flip_receipt_digest,
        eval_receipt_digest: receipt.eval_receipt_digest,
        review_policy: receipt.review_policy,
        publication_receipt: receipt.publication,
    });
    Ok((observation, resolved))
}

fn build_operator_observation(
    workgraph_dir: &Path,
    task: &Task,
    receipt_digest: &str,
    bytes: &[u8],
) -> Result<TerminalOutcomeObservation, String> {
    let receipt: OperatorAcceptanceReceipt = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid operator acceptance receipt: {error}"))?;
    if receipt.receipt_version != 1
        || receipt.task_id != task.id
        || receipt.reason.trim().is_empty()
        || receipt.operator.trim().is_empty()
    {
        return Err("operator acceptance receipt is incomplete or cross-task".into());
    }
    let event = terminal_event(task, receipt_digest)?;
    if event.actor_kind != ActorKind::Operator
        || event.actor_id != receipt.operator
        || event.reason_code != "operator_acceptance"
        || !matches!(
            event.event_kind.as_str(),
            "attempt-succeeded" | "acceptance-satisfied"
        )
    {
        return Err("terminal event is not the exact operator acceptance".into());
    }
    let (reviews, invalid_count) = merge_reviews(workgraph_dir, task, Vec::new());
    let mut observation = base_observation(BaseObservationInput {
        workgraph_dir,
        task,
        event,
        receipt: receipt_digest,
        acceptance_kind: TerminalAcceptanceKind::OperatorAccepted,
        completed_at: receipt.accepted_at.clone(),
        reviews,
        invalid_review_activity_count: invalid_count,
    })?;
    observation.operator_acceptance = Some(OperatorAcceptanceProvenance {
        operator: receipt.operator,
        reason: receipt.reason,
        status_before_accept: receipt.status_before_accept,
        generation_before_accept: receipt.generation_before_accept,
        git_head: receipt.git_head,
        ordinary_publication_verified: false,
    });
    Ok(observation)
}

fn build_observation(
    workgraph_dir: &Path,
    task: &Task,
) -> Result<TerminalOutcomeObservation, String> {
    if task.status != Status::Done {
        return Err(format!("task status {} is not Done", task.status));
    }
    let receipt_digest = task
        .completion_receipt
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Done task has no immutable completion receipt".to_string())?;
    let bytes = receipt_object_bytes(workgraph_dir, receipt_digest)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("completion receipt is not JSON: {error}"))?;
    let project_root = workgraph_dir
        .parent()
        .ok_or_else(|| "workgraph directory has no project root".to_string())?;
    if value.get("manifest_digest").is_some() {
        build_reviewed_observation_with_bundle(
            workgraph_dir,
            project_root,
            task,
            receipt_digest,
            &bytes,
        )
        .map(|(observation, _)| observation)
    } else if value.get("generation_before_accept").is_some() {
        build_operator_observation(workgraph_dir, task, receipt_digest, &bytes)
    } else {
        Err("completion receipt kind is not eligible for terminal observation projection".into())
    }
}

/// Fully verified immutable input for the scored-evaluation observer.
///
/// This value is assembled only after re-verifying the receipt-bound terminal
/// lifecycle event, current generation/attempt/fence, selected completion
/// candidate, review receipts, resolved immutable output bytes, and current
/// publication truth. It grants no lifecycle or publication authority.
#[derive(Clone, Debug)]
pub struct VerifiedTerminalScoringEvidence {
    pub task: Task,
    pub observation: TerminalOutcomeObservation,
    pub bundle: ResolvedReviewBundle,
}

/// Re-verify one persisted terminal observation and all completion/publication
/// evidence it names. Only ordinary reviewed completions are scoreable;
/// operator-accepted and legacy terminal rows remain observable but do not
/// silently acquire ordinary-publication semantics.
pub fn verify_terminal_scoring_evidence(
    workgraph_dir: &Path,
    task_id: &str,
) -> Result<VerifiedTerminalScoringEvidence, TerminalObservationError> {
    let graph = load_graph(workgraph_dir.join("graph.jsonl"))
        .map_err(|error| TerminalObservationError::Graph(error.to_string()))?;
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| TerminalObservationError::Graph(format!("task '{task_id}' not found")))?
        .clone();
    if task.status != Status::Done {
        return Err(TerminalObservationError::Ineligible(format!(
            "task status {} is not Done",
            task.status
        )));
    }
    let receipt_digest = task
        .completion_receipt
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            TerminalObservationError::Ineligible(
                "Done task has no immutable completion receipt".to_string(),
            )
        })?;
    let bytes = receipt_object_bytes(workgraph_dir, receipt_digest)
        .map_err(TerminalObservationError::Ineligible)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value.get("manifest_digest").is_none() {
        return Err(TerminalObservationError::Ineligible(
            "only ordinary reviewed completion/publication receipts are scoreable".to_string(),
        ));
    }
    let project_root = workgraph_dir.parent().ok_or_else(|| {
        TerminalObservationError::Ineligible("workgraph directory has no project root".to_string())
    })?;
    let (observation, bundle) = build_reviewed_observation_with_bundle(
        workgraph_dir,
        project_root,
        &task,
        receipt_digest,
        &bytes,
    )
    .map_err(TerminalObservationError::Ineligible)?;

    let path = observation_path(workgraph_dir, &observation.observation_id)?;
    let stored: TerminalOutcomeObservation =
        serde_json::from_slice(&fs::read(&path).map_err(|error| {
            TerminalObservationError::Ineligible(format!(
                "source terminal observation {} is unavailable: {error}",
                observation.observation_id
            ))
        })?)?;
    verify_observation_identity(&stored)?;
    if stored != observation || observation_path(workgraph_dir, &stored.observation_id)? != path {
        return Err(TerminalObservationError::Ineligible(format!(
            "source terminal observation {} no longer matches terminal evidence",
            observation.observation_id
        )));
    }

    Ok(VerifiedTerminalScoringEvidence {
        task,
        observation,
        bundle,
    })
}

/// Project one task if and only if its accepted terminal evidence still
/// verifies. Replays return `Existing`; ineligible/stale candidates return
/// `Skipped` and never create an Agency record.
pub fn project_terminal_outcome(
    workgraph_dir: &Path,
    task_id: &str,
) -> Result<ProjectionStatus, TerminalObservationError> {
    let graph = load_graph(workgraph_dir.join("graph.jsonl"))
        .map_err(|error| TerminalObservationError::Graph(error.to_string()))?;
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| TerminalObservationError::Graph(format!("task '{task_id}' not found")))?;
    match build_observation(workgraph_dir, task) {
        Ok(observation) => save_observation_create_once(workgraph_dir, &observation),
        Err(reason) => Ok(ProjectionStatus::Skipped { reason }),
    }
}

fn preliminary_id(task: &Task) -> Option<String> {
    let receipt = task.completion_receipt.as_deref()?;
    ContentDigest::parse(receipt.to_string()).ok()?;
    let event = terminal_event(task, receipt).ok()?;
    let key = TerminalObservationKey {
        policy: TERMINAL_OBSERVATION_POLICY.to_string(),
        task_id: task.id.clone(),
        generation: event.generation,
        attempt_id: event.attempt_id.clone()?,
        attempt_fence: event.fence,
        completion_receipt: receipt.to_string(),
    };
    observation_id(&key).ok()
}

/// Bounded, idempotent migration/backfill used by daemon reconciliation and
/// explicit Agency migration. Only missing modern receipt-bound candidates use
/// the budget. Legacy/unbound rows are left untouched rather than guessed.
pub fn reconcile_terminal_outcomes(
    workgraph_dir: &Path,
    limit: usize,
) -> Result<BackfillReport, TerminalObservationError> {
    let graph = load_graph(workgraph_dir.join("graph.jsonl"))
        .map_err(|error| TerminalObservationError::Graph(error.to_string()))?;
    let existing = load_terminal_outcome_observations(workgraph_dir)?
        .into_iter()
        .map(|observation| observation.observation_id)
        .collect::<HashSet<_>>();
    let mut candidates = graph
        .tasks()
        .filter(|task| task.status == Status::Done && preliminary_id(task).is_some())
        .collect::<Vec<_>>();
    // Newest first prevents a large legacy backlog from delaying current work.
    candidates.sort_by(|left, right| {
        right
            .completed_at
            .cmp(&left.completed_at)
            .then(left.id.cmp(&right.id))
    });
    let mut report = BackfillReport {
        limit,
        candidates: candidates.len(),
        ..BackfillReport::default()
    };
    for task in candidates {
        let Some(id) = preliminary_id(task) else {
            continue;
        };
        if existing.contains(&id) {
            report.existing += 1;
            continue;
        }
        if report.attempted >= limit {
            report.remaining += 1;
            continue;
        }
        report.attempted += 1;
        match build_observation(workgraph_dir, task) {
            Ok(observation) => match save_observation_create_once(workgraph_dir, &observation) {
                Ok(ProjectionStatus::Created { .. }) => report.created += 1,
                Ok(ProjectionStatus::Existing { .. }) => report.existing += 1,
                Ok(ProjectionStatus::Skipped { .. }) => report.skipped += 1,
                Err(error) => report.errors.push(format!("{}: {error}", task.id)),
            },
            Err(_) => report.skipped += 1,
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, Task, WorkGraph};
    use crate::lifecycle::{
        FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
    };
    use crate::parser::save_graph;
    use tempfile::tempdir;

    fn terminal_task(id: &str, receipt: &str) -> Task {
        let mut task = Task {
            id: id.into(),
            title: "terminal observation fixture".into(),
            status: Status::Open,
            completion_contract: CompletionContract::Report,
            ..Task::default()
        };
        apply_transition(
            &mut task,
            TransitionRequest::new(
                TransitionKind::AttemptReserved {
                    owner_id: Some("agent-1".into()),
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "dispatcher".into(),
                },
                "fixture",
                format!("reserve:{id}"),
            ),
        )
        .unwrap();
        let request = TransitionRequest::new(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some(receipt.into()),
                manual_review: false,
            },
            LifecycleActor {
                kind: ActorKind::Finalizer,
                id: "completion-v3".into(),
            },
            "reviewed_publication_committed",
            format!("done:{id}:{receipt}"),
        )
        .expecting(FenceExpectation::current(&task))
        .with_evidence(receipt);
        apply_transition(&mut task, request).unwrap();
        task.completion_disposition = Some(CompletionDisposition::Reported);
        task.completion_receipt = Some(receipt.into());
        task.completed_at = Some("2026-08-09T00:00:00Z".into());
        task
    }

    #[test]
    fn failed_waiting_and_unverifiable_tasks_project_nothing() {
        let temp = tempdir().unwrap();
        let wg = temp.path().join(".wg");
        fs::create_dir_all(&wg).unwrap();
        let mut graph = WorkGraph::new();
        for (id, status) in [("failed", Status::Failed), ("waiting", Status::Waiting)] {
            graph.add_node(Node::Task(Task {
                id: id.into(),
                title: id.into(),
                status,
                completion_receipt: Some(format!("b3:{}", "0".repeat(64))),
                ..Task::default()
            }));
        }
        graph.add_node(Node::Task(terminal_task(
            "missing-receipt",
            &format!("b3:{}", "1".repeat(64)),
        )));
        save_graph(&graph, wg.join("graph.jsonl")).unwrap();

        for id in ["failed", "waiting", "missing-receipt"] {
            assert!(matches!(
                project_terminal_outcome(&wg, id).unwrap(),
                ProjectionStatus::Skipped { .. }
            ));
        }
        assert!(load_terminal_outcome_observations(&wg).unwrap().is_empty());
    }

    #[test]
    fn stale_generation_receipt_projects_nothing() {
        let temp = tempdir().unwrap();
        let wg = temp.path().join(".wg");
        fs::create_dir_all(&wg).unwrap();
        let receipt = ReviewedCompletionReceipt {
            receipt_version: 1,
            task_id: "stale".into(),
            generation: 9,
            manifest_digest: format!("b3:{}", "1".repeat(64)),
            requirements_digest: format!("b3:{}", "2".repeat(64)),
            flip_receipt_digest: format!("b3:{}", "3".repeat(64)),
            eval_receipt_digest: None,
            review_policy: "advisory".into(),
            contract: "report".into(),
            publication: format!("artifacts:b3:{}", "4".repeat(64)),
            completed_at: "2026-08-09T00:00:00Z".into(),
        };
        let store = CompletionArtifactStore::open(wg.join("completion/v3")).unwrap();
        let bytes = canonical_json(&serde_json::to_value(receipt).unwrap());
        let receipt_id = store
            .put_bytes(&bytes, "application/vnd.worksgood.completion-receipt+json")
            .unwrap()
            .content_digest
            .to_string();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(terminal_task("stale", &receipt_id)));
        save_graph(&graph, wg.join("graph.jsonl")).unwrap();
        assert!(matches!(
            project_terminal_outcome(&wg, "stale").unwrap(),
            ProjectionStatus::Skipped { .. }
        ));
        assert!(load_terminal_outcome_observations(&wg).unwrap().is_empty());
    }

    #[test]
    fn publication_receipt_must_match_immutable_outputs() {
        let temp = tempdir().unwrap();
        let store = CompletionArtifactStore::open(temp.path().join("completion/v3")).unwrap();
        let artifact = store.put_bytes(b"landed report", "text/plain").unwrap();
        let outputs = vec![OutputRef::Artifact(artifact.clone())];
        assert!(
            verify_publication_receipt(
                temp.path(),
                CompletionContract::Report,
                &outputs,
                &format!("artifacts:{}", artifact.content_digest)
            )
            .is_ok()
        );
        assert!(
            verify_publication_receipt(
                temp.path(),
                CompletionContract::Report,
                &outputs,
                &format!("artifacts:b3:{}", "0".repeat(64))
            )
            .is_err()
        );
    }

    #[test]
    fn observation_identity_changes_with_attempt_or_receipt() {
        let key = TerminalObservationKey {
            policy: TERMINAL_OBSERVATION_POLICY.into(),
            task_id: "task".into(),
            generation: 2,
            attempt_id: "attempt-2-1".into(),
            attempt_fence: 7,
            completion_receipt: format!("b3:{}", "a".repeat(64)),
        };
        let mut changed = key.clone();
        changed.attempt_id = "attempt-2-2".into();
        assert_ne!(
            observation_id(&key).unwrap(),
            observation_id(&changed).unwrap()
        );
        changed = key.clone();
        changed.completion_receipt = format!("b3:{}", "b".repeat(64));
        assert_ne!(
            observation_id(&key).unwrap(),
            observation_id(&changed).unwrap()
        );
    }

    #[test]
    fn operator_acceptance_projects_once_unscored_with_distinct_provenance() {
        let temp = tempdir().unwrap();
        let wg = temp.path().join(".wg");
        fs::create_dir_all(&wg).unwrap();
        let accepted_at = "2026-08-09T00:00:00Z";
        let receipt = OperatorAcceptanceReceipt {
            receipt_version: 1,
            task_id: "operator-task".into(),
            generation_before_accept: 0,
            status_before_accept: "InProgress".into(),
            reason: "human verified preserved output".into(),
            operator: "human-1".into(),
            git_head: Some("a".repeat(40)),
            accepted_at: accepted_at.into(),
        };
        let store = CompletionArtifactStore::open(wg.join("completion/v3")).unwrap();
        let bytes = canonical_json(&serde_json::to_value(&receipt).unwrap());
        let receipt_id = store
            .put_bytes(&bytes, "application/vnd.worksgood.operator-acceptance+json")
            .unwrap()
            .content_digest
            .to_string();

        let mut task = Task {
            id: "operator-task".into(),
            title: "operator observation fixture".into(),
            status: Status::Open,
            completion_contract: CompletionContract::Report,
            actual_executor: Some("pi".into()),
            actual_model: Some("openrouter:test/model".into()),
            model: Some("pi:openrouter:test/model".into()),
            token_usage: Some(TokenUsage {
                cost_usd: 0.25,
                input_tokens: 100,
                output_tokens: 20,
                cache_read_input_tokens: 10,
                cache_creation_input_tokens: 5,
            }),
            ..Task::default()
        };
        apply_transition(
            &mut task,
            TransitionRequest::new(
                TransitionKind::AttemptReserved {
                    owner_id: Some("human-1".into()),
                },
                LifecycleActor {
                    kind: ActorKind::Operator,
                    id: "human-1".into(),
                },
                "operator_recovery_attempt",
                "operator-reserve",
            ),
        )
        .unwrap();
        let request = TransitionRequest::new(
            TransitionKind::AttemptSucceeded {
                acceptance_ref: Some(receipt_id.clone()),
                manual_review: false,
            },
            LifecycleActor::operator("human-1"),
            "operator_acceptance",
            format!("operator-done:{receipt_id}"),
        )
        .expecting(FenceExpectation::current(&task))
        .with_evidence(receipt_id.clone());
        apply_transition(&mut task, request).unwrap();
        task.completion_disposition = Some(CompletionDisposition::Reported);
        task.completion_receipt = Some(receipt_id);
        task.completed_at = Some(accepted_at.into());
        // Runtime accounting is normally retained by the terminal adapter
        // after attempt reservation (which intentionally clears prior-attempt
        // accounting). Populate the exact terminal attempt evidence here.
        task.actual_executor = Some("pi".into());
        task.actual_model = Some("openrouter:test/model".into());
        task.token_usage = Some(TokenUsage {
            cost_usd: 0.25,
            input_tokens: 100,
            output_tokens: 20,
            cache_read_input_tokens: 10,
            cache_creation_input_tokens: 5,
        });

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, wg.join("graph.jsonl")).unwrap();
        let graph_before = fs::read(wg.join("graph.jsonl")).unwrap();

        assert!(matches!(
            project_terminal_outcome(&wg, "operator-task").unwrap(),
            ProjectionStatus::Created { .. }
        ));
        assert!(matches!(
            project_terminal_outcome(&wg, "operator-task").unwrap(),
            ProjectionStatus::Existing { .. }
        ));
        assert_eq!(
            fs::read(wg.join("graph.jsonl")).unwrap(),
            graph_before,
            "observation projection must have no task lifecycle authority"
        );
        let observations = load_terminal_outcome_observations(&wg).unwrap();
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(
            observation.acceptance_kind,
            TerminalAcceptanceKind::OperatorAccepted
        );
        assert_eq!(observation.score, None);
        assert_eq!(
            observation.score_state,
            TerminalObservationScoreState::Unscored
        );
        assert_eq!(
            observation.execution.route.as_deref(),
            Some("pi:openrouter:test/model")
        );
        assert_eq!(observation.execution.usage.as_ref().unwrap().cost_usd, 0.25);
        let provenance = observation.operator_acceptance.as_ref().unwrap();
        assert_eq!(provenance.reason, "human verified preserved output");
        assert!(!provenance.ordinary_publication_verified);
        assert!(observation.reviewed_completion.is_none());
    }
}
