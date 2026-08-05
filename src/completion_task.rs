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
pub struct ExactReviewPair {
    pub flip: ReviewReceipt,
    pub eval: ReviewReceipt,
}

pub fn load_exact_review_pair(
    store: &CompletionArtifactStore,
    submission: &TaskSubmission,
    manifest: &CompletionManifest,
    resolved: &ResolvedReviewBundle,
) -> Result<ExactReviewPair, CompletionTaskError> {
    let manifest_digest = manifest
        .digest()
        .map_err(|error| CompletionTaskError::Serialize(error.to_string()))?;
    let flip_ref = submission
        .flip_receipt_ref
        .as_ref()
        .ok_or(CompletionTaskError::Missing("FLIP receipt"))?;
    let eval_ref = submission
        .eval_receipt_ref
        .as_ref()
        .ok_or(CompletionTaskError::Missing("eval receipt"))?;
    let flip = read_receipt(store, flip_ref)?;
    let eval = read_receipt(store, eval_ref)?;
    validate_receipt(
        &flip,
        ReviewerKind::Flip,
        &manifest_digest,
        &manifest.requirements_digest,
        resolved,
    )?;
    validate_receipt(
        &eval,
        ReviewerKind::Eval,
        &manifest_digest,
        &manifest.requirements_digest,
        resolved,
    )?;
    Ok(ExactReviewPair { flip, eval })
}

fn read_receipt(
    store: &CompletionArtifactStore,
    reference: &ArtifactOutput,
) -> Result<ReviewReceipt, CompletionTaskError> {
    let bytes = store.read_artifact(reference, MAX_COMPLETION_METADATA_BYTES)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CompletionTaskError::InvalidReceipt(error.to_string()))
}

fn validate_receipt(
    receipt: &ReviewReceipt,
    kind: ReviewerKind,
    manifest: &ContentDigest,
    requirements: &ContentDigest,
    resolved: &ResolvedReviewBundle,
) -> Result<(), CompletionTaskError> {
    if !receipt.is_exact_pass(manifest, requirements, kind) {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} did not pass the exact manifest and requirements"
        )));
    }
    if receipt.model_route.as_deref().is_none_or(str::is_empty) {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} receipt has no exact model route"
        )));
    }
    if receipt.inspected_output_digests != resolved.inspected_output_digests {
        return Err(CompletionTaskError::InvalidReceipt(format!(
            "{kind:?} receipt inspected different outputs"
        )));
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
