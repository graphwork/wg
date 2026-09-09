//! Graph projection helpers for the worker-owned completion protocol.
//!
//! The graph stores only immutable object references. Git, the completion
//! object store, and exact review receipts remain the sources of truth; there
//! is no completion transaction or replay scheduler here.

use crate::completion_manifest::{
    ArtifactOutput, ArtifactStoreError, CompletionArtifactStore, CompletionManifest,
    CompletionManifestRef, ContentDigest, ResolvedReviewBundle,
};
use crate::completion_review::{ReviewReceipt, ReviewerKind};
use crate::graph::{CompletionContract as GraphContract, Task};
use crate::identity::canonical_json;
use crate::simple_land::CompletionContract;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TASK_REQUIREMENTS_VERSION: u32 = 1;
pub const MAX_COMPLETION_METADATA_BYTES: u64 = 4 * 1024 * 1024;

/// Compact graph projection of immutable completion objects. Updating this
/// value selects a new candidate; it does not schedule or authorize an action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionCandidateRefs {
    pub manifest: CompletionManifestRef,
    pub requirements: ArtifactOutput,
    pub worker_summary: ArtifactOutput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_outputs: Vec<crate::completion_manifest::EvidenceRef>,
    /// Exact source attempt/fence plus monotonic per-task candidate chronology.
    /// Review receipts repeat this binding; it never grants completion authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_binding: Option<crate::completion_review::CompletionReviewBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_receipt: Option<ArtifactOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_receipt: Option<ArtifactOutput>,
}

#[derive(Debug, Serialize)]
struct TaskRequirements<'a> {
    requirements_version: u32,
    task_id: &'a str,
    generation: u64,
    title: &'a str,
    description: &'a Option<String>,
    completion_contract: CompletionContract,
    after: &'a [String],
    requires: &'a [String],
    skills: &'a [String],
    inputs: &'a [String],
    deliverables: &'a [String],
    /// Exact deterministic commands are task requirements, not worker prose.
    /// Omit the empty field so historical requirements bytes remain stable.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    validation_commands: &'a [String],
}

pub fn completion_contract(task: &Task) -> Result<CompletionContract, CompletionTaskError> {
    match task.completion_contract {
        GraphContract::Land => Ok(CompletionContract::Land),
        GraphContract::Report => Ok(CompletionContract::Report),
        GraphContract::Explore => Ok(CompletionContract::Explore),
        GraphContract::Deliver => Err(CompletionTaskError::LegacyDeliver),
    }
}

/// Canonical exact bytes reviewed as the task requirements.
pub fn task_requirements_bytes(task: &Task) -> Result<Vec<u8>, CompletionTaskError> {
    let validation_commands = crate::completion_validation::configured_validation_commands(task);
    let requirements = TaskRequirements {
        requirements_version: TASK_REQUIREMENTS_VERSION,
        task_id: &task.id,
        generation: task.lifecycle.generation,
        title: &task.title,
        description: &task.description,
        completion_contract: completion_contract(task)?,
        after: &task.after,
        requires: &task.requires,
        skills: &task.skills,
        inputs: &task.inputs,
        deliverables: &task.deliverables,
        validation_commands: &validation_commands,
    };
    let value = serde_json::to_value(requirements)
        .map_err(|error| CompletionTaskError::Serialize(error.to_string()))?;
    Ok(canonical_json(&value))
}

pub fn requirements_digest(task: &Task) -> Result<ContentDigest, CompletionTaskError> {
    Ok(ContentDigest::of_bytes(&task_requirements_bytes(task)?))
}

#[derive(Clone, Debug)]
pub struct TaskSubmission {
    pub manifest_ref: CompletionManifestRef,
    pub requirements_ref: ArtifactOutput,
    pub summary_ref: ArtifactOutput,
    pub review_binding: Option<crate::completion_review::CompletionReviewBinding>,
    pub flip_receipt_ref: Option<ArtifactOutput>,
    pub eval_receipt_ref: Option<ArtifactOutput>,
}

pub fn task_submission(task: &Task) -> Result<TaskSubmission, CompletionTaskError> {
    let candidate = task
        .completion_candidate
        .as_ref()
        .ok_or(CompletionTaskError::Missing("completion candidate"))?;
    Ok(TaskSubmission {
        manifest_ref: candidate.manifest.clone(),
        requirements_ref: candidate.requirements.clone(),
        summary_ref: candidate.worker_summary.clone(),
        review_binding: candidate.review_binding.clone(),
        flip_receipt_ref: candidate.flip_receipt.clone(),
        eval_receipt_ref: candidate.eval_receipt.clone(),
    })
}

pub fn load_submission_bytes(
    store: &CompletionArtifactStore,
    task: &Task,
) -> Result<(TaskSubmission, CompletionManifest, Vec<u8>, Vec<u8>), CompletionTaskError> {
    let submission = task_submission(task)?;
    let manifest = store.read_manifest(&submission.manifest_ref, MAX_COMPLETION_METADATA_BYTES)?;
    let requirements =
        store.read_artifact(&submission.requirements_ref, MAX_COMPLETION_METADATA_BYTES)?;
    let summary = store.read_artifact(&submission.summary_ref, MAX_COMPLETION_METADATA_BYTES)?;

    if manifest.task_id != task.id {
        return Err(CompletionTaskError::Binding("manifest task id changed"));
    }
    if let Some(binding) = submission.review_binding.as_ref()
        && (binding.task_id != task.id
            || binding.generation != task.lifecycle.generation
            || binding.attempt_fence != task.lifecycle.fence
            || binding.attempt_id.as_deref()
                != task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str()))
    {
        return Err(CompletionTaskError::Binding(
            "completion review binding is stale for the task generation/attempt/fence",
        ));
    }
    if manifest.generation != task.lifecycle.generation {
        return Err(CompletionTaskError::Binding("manifest generation is stale"));
    }
    if manifest.completion_contract != completion_contract(task)? {
        return Err(CompletionTaskError::Binding(
            "manifest completion contract changed",
        ));
    }
    let current_requirements = task_requirements_bytes(task)?;
    if requirements != current_requirements
        || ContentDigest::of_bytes(&requirements) != manifest.requirements_digest
    {
        return Err(CompletionTaskError::Binding(
            "task requirements changed after submission",
        ));
    }
    if ContentDigest::of_bytes(&summary) != manifest.worker_summary_digest {
        return Err(CompletionTaskError::Binding(
            "worker summary does not match manifest",
        ));
    }
    Ok((submission, manifest, requirements, summary))
}

#[derive(Clone, Debug)]
pub struct ReviewEvidence {
    pub flip: ReviewReceipt,
    pub eval: Option<ReviewReceipt>,
}

#[derive(Clone, Debug)]
pub struct ExactReviewPair {
    pub flip: ReviewReceipt,
    pub eval: ReviewReceipt,
}

/// Load content-bound review evidence without granting it completion
/// authority. Advisory local review may reject or be unavailable, but its
/// receipt must still bind the exact manifest/requirements and, when outputs
/// were inspectable, the exact resolved output set.
pub fn load_review_evidence(
    store: &CompletionArtifactStore,
    submission: &TaskSubmission,
    manifest: &CompletionManifest,
    resolved: &ResolvedReviewBundle,
) -> Result<ReviewEvidence, CompletionTaskError> {
    let manifest_digest = manifest
        .digest()
        .map_err(|error| CompletionTaskError::Serialize(error.to_string()))?;
    let flip_ref = submission
        .flip_receipt_ref
        .as_ref()
        .ok_or(CompletionTaskError::Missing("FLIP receipt"))?;
    let stored_flip = crate::completion_review::load_stored_review_receipt(store, flip_ref)
        .map_err(|error| CompletionTaskError::InvalidReceipt(error.to_string()))?;
    crate::completion_review::validate_stored_flip_against_bundle(store, &stored_flip, resolved)
        .map_err(|error| CompletionTaskError::InvalidReceipt(error.to_string()))?;
    let flip = stored_flip.receipt;
    validate_review_binding(submission, &flip)?;
    validate_bound_receipt(
        store,
        &flip,
        ReviewerKind::Flip,
        &manifest_digest,
        &manifest.requirements_digest,
        resolved,
    )?;
    let eval = submission
        .eval_receipt_ref
        .as_ref()
        .map(|reference| {
            crate::completion_review::load_stored_review_receipt(store, reference)
                .map(|stored| stored.receipt)
                .map_err(|error| CompletionTaskError::InvalidReceipt(error.to_string()))
        })
        .transpose()?;
    if let Some(eval) = eval.as_ref() {
        validate_review_binding(submission, eval)?;
    }
    if let Some(eval) = eval.as_ref() {
        validate_bound_receipt(
            store,
            eval,
            ReviewerKind::Eval,
            &manifest_digest,
            &manifest.requirements_digest,
            resolved,
        )?;
    }
    Ok(ReviewEvidence { flip, eval })
}

pub fn load_exact_review_pair(
    store: &CompletionArtifactStore,
    submission: &TaskSubmission,
    manifest: &CompletionManifest,
    resolved: &ResolvedReviewBundle,
) -> Result<ExactReviewPair, CompletionTaskError> {
    let evidence = load_review_evidence(store, submission, manifest, resolved)?;
    let manifest_digest = manifest
        .digest()
        .map_err(|error| CompletionTaskError::Serialize(error.to_string()))?;
    if !evidence.flip.is_exact_pass(
        &manifest_digest,
        &manifest.requirements_digest,
        ReviewerKind::Flip,
    ) {
        return Err(CompletionTaskError::InvalidReceipt(
            "FLIP did not pass the exact manifest and requirements".to_string(),
        ));
    }
    let eval = evidence
        .eval
        .ok_or(CompletionTaskError::Missing("eval receipt"))?;
    if !eval.is_exact_pass(
        &manifest_digest,
        &manifest.requirements_digest,
        ReviewerKind::Eval,
    ) {
        return Err(CompletionTaskError::InvalidReceipt(
            "Eval did not pass the exact manifest and requirements".to_string(),
        ));
    }
    Ok(ExactReviewPair {
        flip: evidence.flip,
        eval,
    })
}

fn validate_review_binding(
    submission: &TaskSubmission,
    receipt: &ReviewReceipt,
) -> Result<(), CompletionTaskError> {
    if receipt.binding != submission.review_binding {
        return Err(CompletionTaskError::InvalidReceipt(
            "review receipt does not bind the selected candidate chronology".to_string(),
        ));
    }
    Ok(())
}

fn validate_bound_receipt(
    store: &CompletionArtifactStore,
    receipt: &ReviewReceipt,
    kind: ReviewerKind,
    manifest: &ContentDigest,
    requirements: &ContentDigest,
    resolved: &ResolvedReviewBundle,
) -> Result<(), CompletionTaskError> {
    if receipt.receipt_version != crate::completion_review::COMPLETION_REVIEW_RECEIPT_VERSION
        || &receipt.manifest_digest != manifest
        || &receipt.requirements_digest != requirements
        || receipt.reviewer_kind != kind
    {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} does not bind the exact manifest and requirements"
        )));
    }
    if !matches!(
        receipt.verdict,
        crate::simple_land::ReviewVerdict::IncompleteEvidence
    ) && receipt.model_route.as_deref().is_none_or(str::is_empty)
    {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} receipt has no exact model route"
        )));
    }
    if !matches!(
        receipt.verdict,
        crate::simple_land::ReviewVerdict::IncompleteEvidence
    ) && receipt.inspected_output_digests != resolved.inspected_output_digests
    {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} receipt inspected different outputs"
        )));
    }
    if kind == ReviewerKind::Flip
        && matches!(
            receipt.verdict,
            crate::simple_land::ReviewVerdict::Pass | crate::simple_land::ReviewVerdict::Reject
        )
    {
        if !receipt.has_genuine_flip_proof(store) {
            return Err(CompletionTaskError::InvalidReceipt(
                "FLIP receipt lacks a genuine two-phase prompt-reconstruction proof".into(),
            ));
        }
        // `load_stored_review_receipt` already reloaded and verified every
        // phase input/prompt/hypothesis object before lifecycle authority
        // reaches this exact-binding check.
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum CompletionTaskError {
    #[error("historical deliver tasks cannot enter the new completion protocol")]
    LegacyDeliver,
    #[error("missing {0}")]
    Missing(&'static str),
    #[error("completion binding invalid: {0}")]
    Binding(&'static str),
    #[error("invalid review receipt: {0}")]
    InvalidReceipt(String),
    #[error("completion metadata serialization failed: {0}")]
    Serialize(String),
    #[error(transparent)]
    Store(#[from] ArtifactStoreError),
}
