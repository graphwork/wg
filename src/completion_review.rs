//! Manifest-bound FLIP then eval completion valve.
//!
//! Reviewers consume one already-resolved immutable bundle. Resolver failures
//! become `IncompleteEvidence`; model/provider failures become `Unavailable`.
//! Neither is converted into semantic rejection, and eval is never invoked
//! until FLIP passes the exact manifest and requirements binding.

use crate::completion_manifest::{
    ArtifactOutput, ArtifactStoreError, CompletionArtifactStore, ContentDigest, ImmutableLocator,
    IncompleteEvidence, ResolvedReviewBundle,
};
use crate::identity::canonical_json;
use crate::simple_land::ReviewVerdict;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const COMPLETION_REVIEW_RECEIPT_VERSION: u32 = 1;
const MAX_FINDINGS: usize = 32;
const MAX_CODE_CHARS: usize = 96;
const MAX_MESSAGE_CHARS: usize = 2_000;
const MAX_EVIDENCE_CHARS: usize = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Flip,
    Eval,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewFinding {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

impl ReviewFinding {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            evidence: None,
        }
    }
}

/// A semantic reviewer may return only pass or reject. Infrastructure and
/// evidence failures have separate types so they cannot be mislabeled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticReview {
    pub verdict: SemanticVerdict,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticVerdict {
    Pass,
    Reject,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("reviewer unavailable ({code}): {message}")]
pub struct ReviewerUnavailable {
    pub code: String,
    pub message: String,
}

/// An exact-route reviewer. The valve never substitutes another implementation
/// or route when this reviewer fails.
pub trait ManifestReviewer {
    fn route(&self) -> &str;

    /// Return execution metadata for the immediately preceding review call.
    /// Test/static reviewers may omit it; model adapters expose provider usage.
    fn take_execution(&mut self) -> Option<ReviewExecution> {
        None
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &ResolvedReviewBundle,
    ) -> Result<SemanticReview, ReviewerUnavailable>;
}

/// Narrow append-only observer for a live review invocation. It receives no
/// graph/task/lifecycle/publication handle. A start is recorded before the
/// external call; a finish is linked only after the immutable receipt exists.
pub trait ReviewAttemptObserver {
    fn attempt_started(
        &mut self,
        reviewer_kind: ReviewerKind,
        exact_route: &str,
    ) -> Result<String, String>;

    fn attempt_finished(
        &mut self,
        observer_token: &str,
        receipt: &StoredReviewReceipt,
    ) -> Result<(), String>;
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

/// Factual execution metadata emitted by a reviewer adapter. This is captured
/// separately from its semantic verdict so receipt visibility never grants the
/// review lane graph-task authority.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewExecution {
    pub executor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ReviewUsage>,
}

/// Exact source/candidate chronology covered by a semantic-review receipt.
/// The review remains advisory: this binding prevents cross-task, stale
/// generation, and stale-fence attribution but grants no lifecycle authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionReviewBinding {
    pub task_id: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub attempt_fence: u64,
    pub candidate_sequence: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFailureClass {
    SemanticRejection,
    ReviewerUnavailable,
    IncompleteEvidence,
}

/// Durable task projection used by `wg list --all`, `wg show`, and `wg spend`.
/// `activity_id` is the immutable receipt object's content digest, making
/// replay/content-bound recording idempotent. All copied metadata is checked
/// against that object before it reaches a user-facing projection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompletionReviewActivity {
    pub activity_id: String,
    pub reviewer_kind: ReviewerKind,
    pub verdict: ReviewVerdict,
    pub manifest_digest: ContentDigest,
    pub requirements_digest: ContentDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<CompletionReviewBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_digest: Option<ContentDigest>,
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
    pub created_at: String,
}

impl CompletionReviewActivity {
    /// Concise state for human activity surfaces. `ReviewerKind::Flip` is
    /// named as the current single-call compatibility reviewer, not genuine
    /// blind inference + reveal/comparison FLIP. An unavailable verdict proves
    /// an invocation was attempted and failed; it is not absent/skipped.
    pub fn display_state(&self) -> &'static str {
        match (self.reviewer_kind, self.verdict) {
            (ReviewerKind::Flip, ReviewVerdict::Pass) => "FLIP-compat single-call reviewer pass",
            (ReviewerKind::Eval, ReviewVerdict::Pass) => "Eval reviewer pass",
            (ReviewerKind::Flip, ReviewVerdict::Reject) => {
                "FLIP-compat single-call reviewer rejected"
            }
            (ReviewerKind::Eval, ReviewVerdict::Reject) => "Eval reviewer rejected",
            (ReviewerKind::Flip, ReviewVerdict::Unavailable) => {
                "FLIP-compat single-call reviewer attempted+failed"
            }
            (ReviewerKind::Eval, ReviewVerdict::Unavailable) => "Eval reviewer attempted+failed",
            (ReviewerKind::Flip, ReviewVerdict::IncompleteEvidence) => {
                "FLIP-compat single-call reviewer incomplete evidence"
            }
            (ReviewerKind::Eval, ReviewVerdict::IncompleteEvidence) => {
                "Eval reviewer incomplete evidence"
            }
            (ReviewerKind::Flip, ReviewVerdict::Absent) => {
                "FLIP-compat single-call reviewer not attempted"
            }
            (ReviewerKind::Eval, ReviewVerdict::Absent) => "Eval reviewer not attempted",
        }
    }
}

fn legacy_attempt_is_projected(
    activity_ids: &std::collections::HashSet<&str>,
    attempt_id: &str,
    response_digest: Option<&str>,
) -> bool {
    activity_ids.contains(attempt_id) || response_digest.is_some_and(|id| activity_ids.contains(id))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCandidateState {
    Current,
    Superseded,
    LegacyUnbound,
}

/// Fully verified user-facing view. Findings are reloaded from their immutable
/// object rather than trusted from the mutable graph projection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VerifiedCompletionReviewActivity {
    #[serde(flatten)]
    pub activity: CompletionReviewActivity,
    pub candidate_state: ReviewCandidateState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<ReviewFinding>,
}

impl std::ops::Deref for VerifiedCompletionReviewActivity {
    type Target = CompletionReviewActivity;

    fn deref(&self) -> &Self::Target {
        &self.activity
    }
}

#[derive(Clone, Debug, Default)]
pub struct VerifiedReviewActivityProjection {
    pub activities: Vec<VerifiedCompletionReviewActivity>,
    pub invalid_count: usize,
}

/// One bounded, receipt-verified repair decision. Repair is deliberately
/// limited to the selected current candidate: immutable receipts can prove
/// that identity, but cannot prove missing superseded projection history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewProjectionRepairRow {
    pub task_id: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binding_restored: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity_ids_restored: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ReviewProjectionRepairReport {
    pub dry_run: bool,
    pub limit: usize,
    pub affected_candidates: usize,
    pub examined: usize,
    pub repaired: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub invalid: usize,
    pub remaining: usize,
    pub rows: Vec<ReviewProjectionRepairRow>,
}

impl ReviewProjectionRepairReport {
    pub fn changed(&self) -> bool {
        self.repaired > 0
    }
}

/// Generic writers and the coordinator use a fixed cap so reconciliation can
/// never turn a graph save into an unbounded object-store scan. Operators may
/// inspect or repair a larger explicit batch with `wg migrate review-identity`.
pub const DEFAULT_REVIEW_PROJECTION_REPAIR_LIMIT: usize = 256;

#[derive(Deserialize)]
struct ReviewedCompletionBindingReceipt {
    receipt_version: u32,
    task_id: String,
    generation: u64,
    manifest_digest: String,
    requirements_digest: String,
    flip_receipt_digest: String,
    #[serde(default)]
    eval_receipt_digest: Option<String>,
}

fn read_content_object(objects: &std::path::Path, digest: &ContentDigest) -> Option<Vec<u8>> {
    let name = digest.as_str().strip_prefix("b3:")?;
    let bytes = std::fs::read(objects.join(name)).ok()?;
    (bytes.len() <= 1024 * 1024 && ContentDigest::of_bytes(&bytes) == *digest).then_some(bytes)
}

fn activity_from_receipt(receipt_id: String, receipt: &ReviewReceipt) -> CompletionReviewActivity {
    CompletionReviewActivity {
        activity_id: receipt_id,
        reviewer_kind: receipt.reviewer_kind,
        verdict: receipt.verdict,
        manifest_digest: receipt.manifest_digest.clone(),
        requirements_digest: receipt.requirements_digest.clone(),
        binding: receipt.binding.clone(),
        findings_digest: Some(receipt.findings_digest.clone()),
        failure_class: receipt.failure_class,
        model_route: receipt.model_route.clone(),
        executor: receipt.executor.clone(),
        usage: receipt.usage.clone(),
        duration_ms: receipt.duration_ms,
        created_at: receipt.created_at.clone(),
    }
}

fn candidate_state(
    task: &crate::graph::Task,
    activity: &CompletionReviewActivity,
) -> ReviewCandidateState {
    let Some(binding) = activity.binding.as_ref() else {
        return ReviewCandidateState::LegacyUnbound;
    };
    let current = task.completion_candidate.as_ref().is_some_and(|candidate| {
        let selected_receipt = match activity.reviewer_kind {
            ReviewerKind::Flip => candidate.flip_receipt.as_ref(),
            ReviewerKind::Eval => candidate.eval_receipt.as_ref(),
        };
        candidate.manifest.content_digest == activity.manifest_digest
            && candidate.review_binding.as_ref() == Some(binding)
            && selected_receipt
                .is_some_and(|reference| reference.content_digest.as_str() == activity.activity_id)
    });
    if current {
        ReviewCandidateState::Current
    } else {
        ReviewCandidateState::Superseded
    }
}

/// Reload and content-verify the immutable receipt, findings, and manifest
/// behind every mutable task projection. Invalid, missing, stale, cross-task,
/// or mismatched rows fail closed. Review evidence is observation only; this
/// read path never changes task lifecycle state.
pub fn verified_review_activities(
    dir: &std::path::Path,
    task: &crate::graph::Task,
) -> VerifiedReviewActivityProjection {
    let mut projection = VerifiedReviewActivityProjection::default();
    let objects = dir.join("completion/v3/objects");
    for activity in &task.completion_review_activity {
        let Ok(activity_digest) = ContentDigest::parse(&activity.activity_id) else {
            projection.invalid_count += 1;
            continue;
        };
        let Some(bytes) = read_content_object(&objects, &activity_digest) else {
            projection.invalid_count += 1;
            continue;
        };
        let Ok(receipt) = serde_json::from_slice::<ReviewReceipt>(&bytes) else {
            projection.invalid_count += 1;
            continue;
        };
        let Some(manifest_bytes) = read_content_object(&objects, &receipt.manifest_digest) else {
            projection.invalid_count += 1;
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<crate::completion_manifest::CompletionManifest>(
            &manifest_bytes,
        ) else {
            projection.invalid_count += 1;
            continue;
        };
        let Some(findings_bytes) = read_content_object(&objects, &receipt.findings_digest) else {
            projection.invalid_count += 1;
            continue;
        };
        let Ok(findings) = serde_json::from_slice::<Vec<ReviewFinding>>(&findings_bytes) else {
            projection.invalid_count += 1;
            continue;
        };
        let binding_exact = match (&receipt.binding, &activity.binding) {
            (Some(receipt_binding), Some(activity_binding)) => {
                receipt_binding == activity_binding
                    && receipt_binding.task_id == task.id
                    && manifest.task_id == task.id
                    && manifest.generation == receipt_binding.generation
            }
            (None, None) => manifest.task_id == task.id,
            _ => false,
        };
        let exact = receipt.receipt_version == COMPLETION_REVIEW_RECEIPT_VERSION
            && receipt.reviewer_kind == activity.reviewer_kind
            && receipt.verdict == activity.verdict
            && receipt.manifest_digest == activity.manifest_digest
            && receipt.requirements_digest == activity.requirements_digest
            && receipt.requirements_digest == manifest.requirements_digest
            && receipt.binding == activity.binding
            && activity
                .findings_digest
                .as_ref()
                .is_none_or(|digest| digest == &receipt.findings_digest)
            && receipt.failure_class == activity.failure_class
            && receipt.model_route == activity.model_route
            && receipt.executor == activity.executor
            && receipt.usage == activity.usage
            && receipt.duration_ms == activity.duration_ms
            && receipt.created_at == activity.created_at
            && binding_exact;
        if exact {
            let mut verified = activity.clone();
            // Hydrate old projections from immutable receipt metadata while
            // continuing to reject a conflicting projected value.
            verified.findings_digest = Some(receipt.findings_digest);
            projection
                .activities
                .push(VerifiedCompletionReviewActivity {
                    candidate_state: candidate_state(task, &verified),
                    activity: verified,
                    findings,
                });
        } else {
            projection.invalid_count += 1;
        }
    }
    projection
}

fn exact_current_receipts(
    workgraph_dir: &std::path::Path,
    task: &crate::graph::Task,
) -> Result<Vec<(String, ReviewReceipt)>, String> {
    let candidate = task
        .completion_candidate
        .as_ref()
        .ok_or_else(|| "no current completion candidate".to_string())?;
    let refs = candidate
        .flip_receipt
        .iter()
        .chain(candidate.eval_receipt.iter())
        .collect::<Vec<_>>();
    if refs.is_empty() {
        return Err("current candidate has no review receipt references".into());
    }
    let store_root = workgraph_dir.join("completion/v3");
    if !store_root.join("objects").is_dir() {
        return Err("completion object store is unavailable".into());
    }
    let store = CompletionArtifactStore::open(&store_root)
        .map_err(|error| format!("completion object store is invalid: {error}"))?;
    let (_, manifest, _, _) = crate::completion_task::load_submission_bytes(&store, task)
        .map_err(|error| format!("current candidate does not verify: {error}"))?;
    let manifest_digest = manifest
        .digest()
        .map_err(|error| format!("current manifest is invalid: {error}"))?;

    let mut receipts = Vec::with_capacity(refs.len());
    for reference in refs {
        let bytes = store
            .read_artifact(
                reference,
                crate::completion_task::MAX_COMPLETION_METADATA_BYTES,
            )
            .map_err(|error| format!("review receipt object does not verify: {error}"))?;
        let receipt: ReviewReceipt = serde_json::from_slice(&bytes)
            .map_err(|error| format!("review receipt JSON is invalid: {error}"))?;
        let expected_kind = if candidate
            .flip_receipt
            .as_ref()
            .is_some_and(|flip| flip.content_digest == reference.content_digest)
        {
            ReviewerKind::Flip
        } else {
            ReviewerKind::Eval
        };
        if receipt.receipt_version != COMPLETION_REVIEW_RECEIPT_VERSION
            || receipt.reviewer_kind != expected_kind
            || receipt.manifest_digest != manifest_digest
            || receipt.requirements_digest != manifest.requirements_digest
        {
            return Err(format!(
                "{:?} receipt does not bind the selected manifest and requirements",
                expected_kind
            ));
        }
        let binding = receipt
            .binding
            .as_ref()
            .ok_or_else(|| format!("{:?} receipt has no attempt-bound identity", expected_kind))?;
        let current_attempt = task.lifecycle.current_attempt.as_ref();
        if binding.task_id != task.id
            || binding.generation != task.lifecycle.generation
            || binding.attempt_fence != task.lifecycle.fence
            || binding.candidate_sequence == 0
            || binding.attempt_id.is_none()
            || current_attempt.is_none()
            || binding.attempt_id.as_deref() != current_attempt.map(|attempt| attempt.id.as_str())
        {
            return Err(format!(
                "{:?} receipt binding is stale or cross-task",
                expected_kind
            ));
        }
        if read_content_object(&store_root.join("objects"), &receipt.findings_digest).is_none() {
            return Err(format!(
                "{:?} findings object does not verify",
                expected_kind
            ));
        }
        receipts.push((reference.content_digest.to_string(), receipt));
    }
    if receipts
        .windows(2)
        .any(|pair| pair[0].1.binding != pair[1].1.binding)
    {
        return Err("current FLIP and Eval receipts disagree on candidate identity".into());
    }
    if let Some(projected) = candidate.review_binding.as_ref()
        && receipts[0].1.binding.as_ref() != Some(projected)
    {
        return Err("projected candidate identity conflicts with immutable receipt".into());
    }

    if task.status == crate::graph::Status::Done {
        let completion_id = task
            .completion_receipt
            .as_deref()
            .ok_or_else(|| "terminal current candidate has no completion receipt".to_string())?;
        let completion_digest = ContentDigest::parse(completion_id.to_string())
            .map_err(|error| format!("completion receipt id is invalid: {error}"))?;
        let completion_bytes = read_content_object(&store_root.join("objects"), &completion_digest)
            .ok_or_else(|| "completion receipt object does not verify".to_string())?;
        let completion: ReviewedCompletionBindingReceipt =
            serde_json::from_slice(&completion_bytes)
                .map_err(|error| format!("completion receipt JSON is invalid: {error}"))?;
        let flip_id = candidate
            .flip_receipt
            .as_ref()
            .map(|reference| reference.content_digest.to_string())
            .unwrap_or_default();
        let eval_id = candidate
            .eval_receipt
            .as_ref()
            .map(|reference| reference.content_digest.to_string());
        if completion.receipt_version != 1
            || completion.task_id != task.id
            || completion.generation != task.lifecycle.generation
            || completion.manifest_digest != manifest_digest.to_string()
            || completion.requirements_digest != manifest.requirements_digest.to_string()
            || completion.flip_receipt_digest != flip_id
            || completion.eval_receipt_digest != eval_id
        {
            return Err(
                "completion receipt does not bind the selected current review receipts".into(),
            );
        }
    }
    Ok(receipts)
}

/// Reconstruct only exactly verifiable fields for selected current candidates.
/// Missing superseded history is intentionally unrecoverable here: without a
/// surviving candidate/receipt reference, guessing chronology would turn a
/// mutable projection into false evidence.
pub fn repair_current_review_projections(
    workgraph_dir: &std::path::Path,
    graph: &mut crate::graph::WorkGraph,
    limit: usize,
) -> ReviewProjectionRepairReport {
    let affected = graph
        .tasks()
        .filter(|task| {
            task.completion_candidate.as_ref().is_some_and(|candidate| {
                let receipt_ids = candidate
                    .flip_receipt
                    .iter()
                    .chain(candidate.eval_receipt.iter())
                    .map(|reference| reference.content_digest.as_str())
                    .collect::<Vec<_>>();
                candidate.review_binding.is_none()
                    || receipt_ids.iter().any(|id| {
                        !task
                            .completion_review_activity
                            .iter()
                            .any(|activity| activity.activity_id == *id)
                    })
            })
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let mut report = ReviewProjectionRepairReport {
        limit,
        affected_candidates: affected.len(),
        remaining: affected.len().saturating_sub(limit),
        ..ReviewProjectionRepairReport::default()
    };
    for task_id in affected.into_iter().take(limit) {
        report.examined += 1;
        let verification = graph
            .get_task(&task_id)
            .ok_or_else(|| "task disappeared during repair".to_string())
            .and_then(|task| exact_current_receipts(workgraph_dir, task));
        let receipts = match verification {
            Ok(receipts) => receipts,
            Err(reason) => {
                let outcome = if reason.contains("no review receipt references") {
                    report.skipped += 1;
                    "skipped"
                } else {
                    report.invalid += 1;
                    "invalid"
                };
                report.rows.push(ReviewProjectionRepairRow {
                    task_id,
                    outcome: outcome.into(),
                    reason: Some(reason),
                    binding_restored: false,
                    activity_ids_restored: Vec::new(),
                });
                continue;
            }
        };
        let binding = receipts[0]
            .1
            .binding
            .clone()
            .expect("exact current receipts require a binding");
        let task = graph
            .get_task_mut(&task_id)
            .expect("repair task was selected from this graph");
        let binding_restored = task.completion_candidate.as_mut().is_some_and(|candidate| {
            if candidate.review_binding.is_none() {
                candidate.review_binding = Some(binding.clone());
                true
            } else {
                false
            }
        });
        let mut restored = Vec::new();
        let mut conflict = None;
        for (receipt_id, receipt) in receipts {
            let activity = activity_from_receipt(receipt_id.clone(), &receipt);
            match task
                .completion_review_activity
                .iter()
                .find(|existing| existing.activity_id == receipt_id)
            {
                Some(existing) if existing != &activity => {
                    conflict = Some(format!(
                        "projected activity {} conflicts with immutable receipt",
                        receipt_id
                    ));
                    break;
                }
                Some(_) => {}
                None => {
                    task.completion_review_activity.push(activity);
                    restored.push(receipt_id);
                }
            }
        }
        if let Some(reason) = conflict {
            // Do not partly bless a row set. Undo only fields introduced by
            // this repair; pre-existing projection/history stays untouched.
            if binding_restored && let Some(candidate) = task.completion_candidate.as_mut() {
                candidate.review_binding = None;
            }
            task.completion_review_activity
                .retain(|activity| !restored.contains(&activity.activity_id));
            report.invalid += 1;
            report.rows.push(ReviewProjectionRepairRow {
                task_id,
                outcome: "invalid".into(),
                reason: Some(reason),
                binding_restored: false,
                activity_ids_restored: Vec::new(),
            });
        } else if binding_restored || !restored.is_empty() {
            report.repaired += 1;
            report.rows.push(ReviewProjectionRepairRow {
                task_id,
                outcome: "repaired".into(),
                reason: None,
                binding_restored,
                activity_ids_restored: restored,
            });
        } else {
            report.unchanged += 1;
            report.rows.push(ReviewProjectionRepairRow {
                task_id,
                outcome: "unchanged".into(),
                reason: None,
                binding_restored: false,
                activity_ids_restored: Vec::new(),
            });
        }
    }
    report
}

/// Return historical evaluation records that are not already represented by a
/// content-bound completion-review activity. Mixed-version tasks keep older
/// attempts; only exact stable-ID aliases are removed.
pub fn unprojected_legacy_evaluation_records(
    task: &crate::graph::Task,
    verified_activities: &[VerifiedCompletionReviewActivity],
) -> Vec<crate::evaluation::EvaluationRecord> {
    let activity_ids = verified_activities
        .iter()
        .map(|activity| activity.activity_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    task.evaluation_records
        .iter()
        .filter_map(|record| {
            let consumed_is_projected = record
                .consumed_verdict_id
                .as_deref()
                .is_some_and(|id| activity_ids.contains(id));
            if record.attempts.is_empty() {
                return (!consumed_is_projected).then(|| record.clone());
            }
            let mut retained = record.clone();
            retained.attempts = record
                .attempts
                .iter()
                .filter(|attempt| {
                    !legacy_attempt_is_projected(
                        &activity_ids,
                        &attempt.attempt_id,
                        attempt.response_digest.as_deref(),
                    )
                })
                .cloned()
                .collect();
            if consumed_is_projected {
                retained.consumed_verdict_id = None;
            }
            (!retained.attempts.is_empty()).then_some(retained)
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewReceipt {
    pub receipt_version: u32,
    pub manifest_digest: ContentDigest,
    pub requirements_digest: ContentDigest,
    pub reviewer_kind: ReviewerKind,
    pub verdict: ReviewVerdict,
    pub findings_digest: ContentDigest,
    pub inspected_output_digests: Vec<String>,
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
    pub created_at: String,
}

impl ReviewReceipt {
    pub fn is_exact_pass(
        &self,
        manifest: &ContentDigest,
        requirements: &ContentDigest,
        kind: ReviewerKind,
    ) -> bool {
        self.receipt_version == COMPLETION_REVIEW_RECEIPT_VERSION
            && &self.manifest_digest == manifest
            && &self.requirements_digest == requirements
            && self.reviewer_kind == kind
            && self.verdict == ReviewVerdict::Pass
    }

    /// Whether this immutable receipt already contains the semantic decision
    /// for one exact candidate/reviewer route. Infrastructure outcomes are not
    /// semantic decisions and intentionally remain retryable.
    pub fn is_reusable_semantic(
        &self,
        manifest: &ContentDigest,
        requirements: &ContentDigest,
        kind: ReviewerKind,
        route: &str,
        binding: Option<&CompletionReviewBinding>,
        inspected_output_digests: &[String],
    ) -> bool {
        self.receipt_version == COMPLETION_REVIEW_RECEIPT_VERSION
            && &self.manifest_digest == manifest
            && &self.requirements_digest == requirements
            && self.reviewer_kind == kind
            && matches!(self.verdict, ReviewVerdict::Pass | ReviewVerdict::Reject)
            && self.model_route.as_deref() == Some(route)
            && self.binding.as_ref() == binding
            && self.inspected_output_digests == inspected_output_digests
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredReviewReceipt {
    pub receipt: ReviewReceipt,
    pub receipt_object: ArtifactOutput,
    pub findings_object: ArtifactOutput,
}

/// Reload and verify a stored review receipt plus its immutable findings.
/// Candidate replay uses this rather than invoking the same semantic reviewer
/// again after a lost response or process restart.
pub fn load_stored_review_receipt(
    artifact_store: &CompletionArtifactStore,
    receipt_object: &ArtifactOutput,
) -> Result<StoredReviewReceipt, ReviewValveError> {
    let receipt_bytes = artifact_store.read_artifact(
        receipt_object,
        crate::completion_task::MAX_COMPLETION_METADATA_BYTES,
    )?;
    let receipt: ReviewReceipt = serde_json::from_slice(&receipt_bytes)?;
    if receipt.receipt_version != COMPLETION_REVIEW_RECEIPT_VERSION {
        return Err(ReviewValveError::InvalidReceipt(format!(
            "unsupported receipt version {}",
            receipt.receipt_version
        )));
    }
    let object_name = receipt
        .findings_digest
        .as_str()
        .strip_prefix("b3:")
        .ok_or_else(|| ReviewValveError::InvalidReceipt("invalid findings digest".into()))?;
    let findings_path = artifact_store.root().join("objects").join(object_name);
    let findings_size = std::fs::metadata(&findings_path)
        .map_err(ArtifactStoreError::Io)?
        .len();
    let findings_object = ArtifactOutput {
        content_digest: receipt.findings_digest.clone(),
        immutable_locator: ImmutableLocator::CompletionObject {
            digest: receipt.findings_digest.clone(),
        },
        media_type: "application/vnd.worksgood.review-findings+json".to_string(),
        size: findings_size,
        review_projection: None,
    };
    let findings_bytes = artifact_store.read_artifact(
        &findings_object,
        crate::completion_task::MAX_COMPLETION_METADATA_BYTES,
    )?;
    serde_json::from_slice::<Vec<ReviewFinding>>(&findings_bytes)?;
    Ok(StoredReviewReceipt {
        receipt,
        receipt_object: receipt_object.clone(),
        findings_object,
    })
}

/// Reload a historical receipt from its content digest. Mutable task
/// projections retain only the selected receipt reference for each reviewer;
/// this helper lets candidate replay recover an older route-specific receipt
/// without scanning or trusting mutable filenames.
pub fn load_stored_review_receipt_by_digest(
    artifact_store: &CompletionArtifactStore,
    digest: &ContentDigest,
) -> Result<StoredReviewReceipt, ReviewValveError> {
    let object_name = digest
        .as_str()
        .strip_prefix("b3:")
        .ok_or_else(|| ReviewValveError::InvalidReceipt("invalid receipt digest".into()))?;
    let size = std::fs::metadata(artifact_store.root().join("objects").join(object_name))
        .map_err(ArtifactStoreError::Io)?
        .len();
    let receipt_object = ArtifactOutput {
        content_digest: digest.clone(),
        immutable_locator: ImmutableLocator::CompletionObject {
            digest: digest.clone(),
        },
        media_type: "application/vnd.worksgood.review-receipt+json".to_string(),
        size,
        review_projection: None,
    };
    load_stored_review_receipt(artifact_store, &receipt_object)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewValveStatus {
    Accepted,
    FlipRejected,
    EvalRejected,
    ReviewUnavailable,
    IncompleteEvidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewValveOutcome {
    pub status: ReviewValveStatus,
    pub flip: StoredReviewReceipt,
    pub eval: Option<StoredReviewReceipt>,
}

impl ReviewValveOutcome {
    pub fn accepted_exactly(&self, manifest: &ContentDigest, requirements: &ContentDigest) -> bool {
        self.status == ReviewValveStatus::Accepted
            && self
                .flip
                .receipt
                .is_exact_pass(manifest, requirements, ReviewerKind::Flip)
            && self.eval.as_ref().is_some_and(|receipt| {
                receipt
                    .receipt
                    .is_exact_pass(manifest, requirements, ReviewerKind::Eval)
            })
    }
}

#[derive(Debug, Error)]
pub enum ReviewValveError {
    #[error("resolved review bundle binding does not match requested manifest")]
    BindingMismatch,
    #[error("{0:?} reviewer did not declare an exact model route")]
    MissingExactRoute(ReviewerKind),
    #[error("invalid immutable review receipt: {0}")]
    InvalidReceipt(String),
    #[error("persist review receipt: {0}")]
    Store(#[from] ArtifactStoreError),
    #[error("serialize review receipt: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("append adaptive review attempt: {0}")]
    Observer(String),
}

/// Run FLIP then eval over one exact resolved bundle.
pub fn run_review_valve(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    run_review_valve_bound(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        None,
    )
}

/// Production entry point carrying the exact task/generation/attempt/fence and
/// candidate chronology into every immutable review receipt.
#[allow(clippy::too_many_arguments)]
pub fn run_review_valve_bound(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
    binding: Option<&CompletionReviewBinding>,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    run_review_valve_at_bound(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        binding,
        None,
        None,
        &Utc::now().to_rfc3339(),
        None,
    )
}

/// Run the valve while reusing exact semantic receipts selected for this same
/// immutable candidate. A receipt is reused only when kind, route, candidate
/// binding, and inspected output digests all match. Unavailable/incomplete
/// receipts never suppress an infrastructure retry.
#[allow(clippy::too_many_arguments)]
pub fn run_review_valve_bound_reusing(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
    binding: Option<&CompletionReviewBinding>,
    prior_flip: Option<StoredReviewReceipt>,
    prior_eval: Option<StoredReviewReceipt>,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    run_review_valve_at_bound(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        binding,
        prior_flip,
        prior_eval,
        &Utc::now().to_rfc3339(),
        None,
    )
}

/// Production variant that records a create-once attempt start before each
/// external invocation and links its immutable terminal receipt afterward.
#[allow(clippy::too_many_arguments)]
pub fn run_review_valve_bound_reusing_observed(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
    binding: Option<&CompletionReviewBinding>,
    prior_flip: Option<StoredReviewReceipt>,
    prior_eval: Option<StoredReviewReceipt>,
    observer: &mut dyn ReviewAttemptObserver,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    run_review_valve_at_bound(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        binding,
        prior_flip,
        prior_eval,
        &Utc::now().to_rfc3339(),
        Some(observer),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_review_valve_at(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
    created_at: &str,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    run_review_valve_at_bound(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        None,
        None,
        None,
        created_at,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_review_valve_at_bound(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    resolved: Result<ResolvedReviewBundle, IncompleteEvidence>,
    flip_reviewer: &mut dyn ManifestReviewer,
    eval_reviewer: &mut dyn ManifestReviewer,
    binding: Option<&CompletionReviewBinding>,
    prior_flip: Option<StoredReviewReceipt>,
    prior_eval: Option<StoredReviewReceipt>,
    created_at: &str,
    mut observer: Option<&mut dyn ReviewAttemptObserver>,
) -> Result<ReviewValveOutcome, ReviewValveError> {
    let bundle = match resolved {
        Ok(bundle) => bundle,
        Err(incomplete) => {
            let findings = vec![ReviewFinding {
                code: format!("resolver.{:?}", incomplete.kind).to_ascii_lowercase(),
                message: incomplete.detail,
                evidence: Some(incomplete.reference),
            }];
            let flip = persist_receipt(
                artifact_store,
                ReceiptMaterial {
                    manifest_digest,
                    requirements_digest,
                    reviewer_kind: ReviewerKind::Flip,
                    verdict: ReviewVerdict::IncompleteEvidence,
                    findings,
                    inspected_output_digests: Vec::new(),
                    binding,
                    model_route: None,
                    execution: None,
                    duration_ms: Some(0),
                    created_at,
                },
            )?;
            return Ok(ReviewValveOutcome {
                status: ReviewValveStatus::IncompleteEvidence,
                flip,
                eval: None,
            });
        }
    };

    if &bundle.manifest_digest != manifest_digest
        || &bundle.requirements_digest != requirements_digest
    {
        return Err(ReviewValveError::BindingMismatch);
    }

    if flip_reviewer.route().trim().is_empty() {
        return Err(ReviewValveError::MissingExactRoute(ReviewerKind::Flip));
    }
    let flip = match prior_flip.filter(|stored| {
        stored.receipt.is_reusable_semantic(
            manifest_digest,
            requirements_digest,
            ReviewerKind::Flip,
            flip_reviewer.route(),
            binding,
            &bundle.inspected_output_digests,
        )
    }) {
        Some(stored) => stored,
        None => {
            let observer_token = observer
                .as_deref_mut()
                .map(|observer| observer.attempt_started(ReviewerKind::Flip, flip_reviewer.route()))
                .transpose()
                .map_err(ReviewValveError::Observer)?;
            let flip_started = std::time::Instant::now();
            let flip_result = flip_reviewer.review(ReviewerKind::Flip, &bundle);
            let flip_duration_ms =
                u64::try_from(flip_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let flip_execution = flip_reviewer.take_execution();
            let stored = receipt_from_reviewer_result(
                artifact_store,
                manifest_digest,
                requirements_digest,
                ReviewerKind::Flip,
                &bundle.inspected_output_digests,
                binding,
                flip_reviewer.route(),
                flip_result,
                flip_execution,
                Some(flip_duration_ms),
                created_at,
            )?;
            if let Some(token) = observer_token.as_deref()
                && let Some(observer) = observer.as_mut()
            {
                observer
                    .attempt_finished(token, &stored)
                    .map_err(ReviewValveError::Observer)?;
            }
            stored
        }
    };
    match flip.receipt.verdict {
        ReviewVerdict::Reject => {
            return Ok(ReviewValveOutcome {
                status: ReviewValveStatus::FlipRejected,
                flip,
                eval: None,
            });
        }
        ReviewVerdict::Unavailable => {
            return Ok(ReviewValveOutcome {
                status: ReviewValveStatus::ReviewUnavailable,
                flip,
                eval: None,
            });
        }
        ReviewVerdict::Pass => {}
        ReviewVerdict::Absent | ReviewVerdict::IncompleteEvidence => {
            unreachable!("semantic reviewer result maps only to pass/reject/unavailable")
        }
    }

    // The exact-pass predicate is checked rather than inferred from control
    // flow, keeping the eval gate aligned with the pure reducer.
    if !flip
        .receipt
        .is_exact_pass(manifest_digest, requirements_digest, ReviewerKind::Flip)
    {
        return Err(ReviewValveError::BindingMismatch);
    }

    if eval_reviewer.route().trim().is_empty() {
        return Err(ReviewValveError::MissingExactRoute(ReviewerKind::Eval));
    }
    let eval = match prior_eval.filter(|stored| {
        stored.receipt.is_reusable_semantic(
            manifest_digest,
            requirements_digest,
            ReviewerKind::Eval,
            eval_reviewer.route(),
            binding,
            &bundle.inspected_output_digests,
        )
    }) {
        Some(stored) => stored,
        None => {
            let observer_token = observer
                .as_deref_mut()
                .map(|observer| observer.attempt_started(ReviewerKind::Eval, eval_reviewer.route()))
                .transpose()
                .map_err(ReviewValveError::Observer)?;
            let eval_started = std::time::Instant::now();
            let eval_result = eval_reviewer.review(ReviewerKind::Eval, &bundle);
            let eval_duration_ms =
                u64::try_from(eval_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let eval_execution = eval_reviewer.take_execution();
            let stored = receipt_from_reviewer_result(
                artifact_store,
                manifest_digest,
                requirements_digest,
                ReviewerKind::Eval,
                &bundle.inspected_output_digests,
                binding,
                eval_reviewer.route(),
                eval_result,
                eval_execution,
                Some(eval_duration_ms),
                created_at,
            )?;
            if let Some(token) = observer_token.as_deref()
                && let Some(observer) = observer.as_mut()
            {
                observer
                    .attempt_finished(token, &stored)
                    .map_err(ReviewValveError::Observer)?;
            }
            stored
        }
    };
    let status = match eval.receipt.verdict {
        ReviewVerdict::Pass => ReviewValveStatus::Accepted,
        ReviewVerdict::Reject => ReviewValveStatus::EvalRejected,
        ReviewVerdict::Unavailable => ReviewValveStatus::ReviewUnavailable,
        ReviewVerdict::Absent | ReviewVerdict::IncompleteEvidence => {
            unreachable!("semantic reviewer result maps only to pass/reject/unavailable")
        }
    };
    Ok(ReviewValveOutcome {
        status,
        flip,
        eval: Some(eval),
    })
}

#[allow(clippy::too_many_arguments)]
fn receipt_from_reviewer_result(
    artifact_store: &CompletionArtifactStore,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    reviewer_kind: ReviewerKind,
    inspected_output_digests: &[String],
    binding: Option<&CompletionReviewBinding>,
    model_route: &str,
    result: Result<SemanticReview, ReviewerUnavailable>,
    execution: Option<ReviewExecution>,
    duration_ms: Option<u64>,
    created_at: &str,
) -> Result<StoredReviewReceipt, ReviewValveError> {
    let (verdict, findings) = match result {
        Ok(review) => {
            let verdict = match review.verdict {
                SemanticVerdict::Pass => ReviewVerdict::Pass,
                SemanticVerdict::Reject => ReviewVerdict::Reject,
            };
            let findings = if verdict == ReviewVerdict::Reject && review.findings.is_empty() {
                vec![ReviewFinding::new(
                    "review.reject_without_detail",
                    "reviewer rejected the submission without an actionable finding",
                )]
            } else {
                review.findings
            };
            (verdict, findings)
        }
        Err(unavailable) => (
            ReviewVerdict::Unavailable,
            vec![ReviewFinding::new(unavailable.code, unavailable.message)],
        ),
    };
    persist_receipt(
        artifact_store,
        ReceiptMaterial {
            manifest_digest,
            requirements_digest,
            reviewer_kind,
            verdict,
            findings,
            inspected_output_digests: inspected_output_digests.to_vec(),
            binding,
            model_route: Some(model_route.to_string()),
            execution,
            duration_ms,
            created_at,
        },
    )
}

struct ReceiptMaterial<'a> {
    manifest_digest: &'a ContentDigest,
    requirements_digest: &'a ContentDigest,
    reviewer_kind: ReviewerKind,
    verdict: ReviewVerdict,
    findings: Vec<ReviewFinding>,
    inspected_output_digests: Vec<String>,
    binding: Option<&'a CompletionReviewBinding>,
    model_route: Option<String>,
    execution: Option<ReviewExecution>,
    duration_ms: Option<u64>,
    created_at: &'a str,
}

fn persist_receipt(
    artifact_store: &CompletionArtifactStore,
    material: ReceiptMaterial<'_>,
) -> Result<StoredReviewReceipt, ReviewValveError> {
    let findings = normalize_findings(material.findings);
    let findings_value = serde_json::to_value(&findings)?;
    let findings_bytes = canonical_json(&findings_value);
    let findings_object = artifact_store.put_bytes(
        &findings_bytes,
        "application/vnd.worksgood.review-findings+json",
    )?;
    let receipt = ReviewReceipt {
        receipt_version: COMPLETION_REVIEW_RECEIPT_VERSION,
        manifest_digest: material.manifest_digest.clone(),
        requirements_digest: material.requirements_digest.clone(),
        reviewer_kind: material.reviewer_kind,
        verdict: material.verdict,
        findings_digest: findings_object.content_digest.clone(),
        inspected_output_digests: material.inspected_output_digests,
        binding: material.binding.cloned(),
        failure_class: match material.verdict {
            ReviewVerdict::Reject => Some(ReviewFailureClass::SemanticRejection),
            ReviewVerdict::Unavailable => Some(ReviewFailureClass::ReviewerUnavailable),
            ReviewVerdict::IncompleteEvidence => Some(ReviewFailureClass::IncompleteEvidence),
            ReviewVerdict::Pass | ReviewVerdict::Absent => None,
        },
        model_route: material.model_route,
        executor: material
            .execution
            .as_ref()
            .map(|value| value.executor.clone()),
        usage: material.execution.and_then(|value| value.usage),
        duration_ms: material.duration_ms,
        created_at: material.created_at.to_string(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(&receipt)?);
    let receipt_object = artifact_store.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.review-receipt+json",
    )?;
    Ok(StoredReviewReceipt {
        receipt,
        receipt_object,
        findings_object,
    })
}

fn normalize_findings(findings: Vec<ReviewFinding>) -> Vec<ReviewFinding> {
    findings
        .into_iter()
        .take(MAX_FINDINGS)
        .map(|finding| ReviewFinding {
            code: bounded(&finding.code, MAX_CODE_CHARS),
            message: bounded(&finding.message, MAX_MESSAGE_CHARS),
            evidence: finding
                .evidence
                .as_deref()
                .map(|value| bounded(value, MAX_EVIDENCE_CHARS)),
        })
        .collect()
}

fn bounded(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod projection_tests {
    use super::*;

    #[test]
    fn missing_immutable_receipt_fails_projection_closed() {
        let dir = tempfile::tempdir().unwrap();
        let digest = format!("b3:{}", "0".repeat(64));
        let mut task = crate::graph::Task::default();
        task.completion_review_activity
            .push(CompletionReviewActivity {
                activity_id: digest,
                reviewer_kind: ReviewerKind::Flip,
                verdict: ReviewVerdict::Pass,
                manifest_digest: ContentDigest::of_bytes(b"manifest"),
                requirements_digest: ContentDigest::of_bytes(b"requirements"),
                binding: None,
                findings_digest: None,
                failure_class: None,
                model_route: Some("pi:test:model".into()),
                executor: Some("pi".into()),
                usage: None,
                duration_ms: None,
                created_at: "2026-08-08T00:00:00Z".into(),
            });
        let projection = verified_review_activities(dir.path(), &task);
        assert!(projection.activities.is_empty());
        assert_eq!(projection.invalid_count, 1);
    }

    #[test]
    fn mixed_version_projection_deduplicates_only_stable_aliases() {
        let activity_ids = ["receipt-new"]
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert!(!legacy_attempt_is_projected(
            &activity_ids,
            "old-failed-attempt",
            None,
        ));
        assert!(!legacy_attempt_is_projected(
            &activity_ids,
            "unmatched-success",
            None,
        ));
        assert!(legacy_attempt_is_projected(
            &activity_ids,
            "other-attempt",
            Some("receipt-new"),
        ));
    }

    struct PassingReviewer(&'static str);

    impl ManifestReviewer for PassingReviewer {
        fn route(&self) -> &str {
            self.0
        }

        fn review(
            &mut self,
            _kind: ReviewerKind,
            _bundle: &ResolvedReviewBundle,
        ) -> Result<SemanticReview, ReviewerUnavailable> {
            Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::new(),
            })
        }
    }

    fn stripped_terminal_fixture(
        dir: &std::path::Path,
    ) -> (
        crate::graph::WorkGraph,
        CompletionReviewBinding,
        Vec<String>,
    ) {
        use crate::completion_manifest::{
            COMPLETION_MANIFEST_VERSION, CompletionManifest, EvidenceRef, OutputRef,
        };
        use crate::completion_task::CompletionCandidateRefs;
        use crate::graph::{CompletionContract, CompletionDisposition, Node, Status, Task};
        use crate::lifecycle::{AttemptDisposition, AttemptRef};
        use crate::simple_land::CompletionContract as ManifestContract;

        let store = CompletionArtifactStore::open(dir.join("completion/v3")).unwrap();
        let mut task = Task {
            id: "receipt-repair".into(),
            title: "Receipt repair".into(),
            status: Status::Done,
            completion_contract: CompletionContract::Report,
            completion_disposition: Some(CompletionDisposition::Reported),
            ..Task::default()
        };
        task.lifecycle.generation = 3;
        task.lifecycle.fence = 9;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-3-2".into(),
            generation: 3,
            fence: 9,
            actor_id: "worker".into(),
            disposition: Some(AttemptDisposition::Succeeded),
        });
        let requirements_bytes = crate::completion_task::task_requirements_bytes(&task).unwrap();
        let requirements = store
            .put_bytes(
                &requirements_bytes,
                "application/vnd.worksgood.requirements+json",
            )
            .unwrap();
        let summary = store.put_bytes(b"summary", "text/plain").unwrap();
        let output = store.put_bytes(b"result", "text/plain").unwrap();
        let evidence = store.put_bytes(b"validation", "application/json").unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: ManifestContract::Report,
            requirements_digest: requirements.content_digest.clone(),
            source_revision: "fixture-source".into(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![EvidenceRef {
                content_digest: evidence.content_digest,
                immutable_locator: evidence.immutable_locator,
                evidence_kind: "deterministic-validation/baseline/v1".into(),
                media_type: evidence.media_type,
                size: evidence.size,
                review_projection: None,
            }],
            worker_summary_digest: summary.content_digest.clone(),
        };
        let manifest_ref = store.put_manifest(&manifest).unwrap();
        let manifest_digest = manifest.digest().unwrap();
        let binding = CompletionReviewBinding {
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: Some("attempt-3-2".into()),
            attempt_fence: task.lifecycle.fence,
            candidate_sequence: 4,
        };
        let bundle = ResolvedReviewBundle {
            manifest_digest: manifest_digest.clone(),
            requirements_digest: requirements.content_digest.clone(),
            manifest_bytes: manifest.canonical_bytes().unwrap(),
            requirements_bytes,
            worker_summary_bytes: b"summary".to_vec(),
            dependency_outputs: Vec::new(),
            outputs: Vec::new(),
            validation_evidence: Vec::new(),
            inspected_output_digests: Vec::new(),
        };
        let outcome = run_review_valve_bound(
            &store,
            &manifest_digest,
            &requirements.content_digest,
            Ok(bundle),
            &mut PassingReviewer("pi:test:flip"),
            &mut PassingReviewer("pi:test:eval"),
            Some(&binding),
        )
        .unwrap();
        let eval = outcome.eval.unwrap();
        let receipt_ids = vec![
            outcome.flip.receipt_object.content_digest.to_string(),
            eval.receipt_object.content_digest.to_string(),
        ];
        task.completion_candidate = Some(CompletionCandidateRefs {
            manifest: manifest_ref,
            requirements,
            worker_summary: summary,
            dependency_outputs: Vec::new(),
            review_binding: None,
            flip_receipt: Some(outcome.flip.receipt_object),
            eval_receipt: Some(eval.receipt_object),
        });
        let completed = serde_json::json!({
            "receipt_version": 1,
            "task_id": task.id,
            "generation": task.lifecycle.generation,
            "manifest_digest": manifest_digest.to_string(),
            "requirements_digest": task.completion_candidate.as_ref().unwrap().requirements.content_digest.to_string(),
            "flip_receipt_digest": receipt_ids[0],
            "eval_receipt_digest": receipt_ids[1],
            "review_policy": "strict",
            "contract": "report",
            "publication": "artifacts:fixture",
            "completed_at": "2026-08-10T00:00:00Z"
        });
        let completion = store
            .put_bytes(
                &canonical_json(&completed),
                "application/vnd.worksgood.completion-receipt+json",
            )
            .unwrap();
        task.completion_receipt = Some(completion.content_digest.to_string());
        let mut graph = crate::graph::WorkGraph::new();
        graph.add_node(Node::Task(task));
        (graph, binding, receipt_ids)
    }

    #[test]
    fn receipt_bounded_repair_restores_only_current_identity_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut graph, binding, receipt_ids) = stripped_terminal_fixture(dir.path());
        let report = repair_current_review_projections(dir.path(), &mut graph, 1);
        assert_eq!(report.repaired, 1);
        assert_eq!(report.invalid, 0);
        let task = graph.get_task("receipt-repair").unwrap();
        assert_eq!(
            task.completion_candidate
                .as_ref()
                .unwrap()
                .review_binding
                .as_ref(),
            Some(&binding)
        );
        assert_eq!(
            task.completion_review_activity
                .iter()
                .map(|activity| activity.activity_id.clone())
                .collect::<Vec<_>>(),
            receipt_ids
        );
        assert!(
            task.completion_review_activity
                .iter()
                .all(|activity| activity.binding.as_ref() == Some(&binding))
        );

        let second = repair_current_review_projections(dir.path(), &mut graph, 1);
        assert!(!second.changed());
        assert_eq!(second.affected_candidates, 0);
        assert_eq!(
            graph
                .get_task("receipt-repair")
                .unwrap()
                .completion_review_activity
                .len(),
            2
        );
    }

    #[test]
    fn repair_reports_unreviewed_current_candidate_as_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut graph, _, _) = stripped_terminal_fixture(dir.path());
        let task = graph.get_task_mut("receipt-repair").unwrap();
        let candidate = task.completion_candidate.as_mut().unwrap();
        candidate.flip_receipt = None;
        candidate.eval_receipt = None;
        task.completion_receipt = None;
        task.status = crate::graph::Status::InProgress;

        let report = repair_current_review_projections(dir.path(), &mut graph, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.invalid, 0);
        assert_eq!(report.rows[0].outcome, "skipped");
        assert!(!report.changed());
    }

    #[test]
    fn repair_refuses_cross_task_receipt_and_never_guesses_superseded_history() {
        let dir = tempfile::tempdir().unwrap();
        let (mut graph, _, _) = stripped_terminal_fixture(dir.path());
        let task = graph.get_task_mut("receipt-repair").unwrap();
        task.lifecycle.current_attempt.as_mut().unwrap().id = "different-attempt".into();
        task.completion_review_activity
            .push(CompletionReviewActivity {
                activity_id: format!("b3:{}", "f".repeat(64)),
                reviewer_kind: ReviewerKind::Flip,
                verdict: ReviewVerdict::Reject,
                manifest_digest: ContentDigest::of_bytes(b"superseded"),
                requirements_digest: ContentDigest::of_bytes(b"old"),
                binding: None,
                findings_digest: None,
                failure_class: Some(ReviewFailureClass::SemanticRejection),
                model_route: None,
                executor: None,
                usage: None,
                duration_ms: None,
                created_at: "2026-08-09T00:00:00Z".into(),
            });
        let report = repair_current_review_projections(dir.path(), &mut graph, 1);
        assert_eq!(report.invalid, 1);
        let task = graph.get_task("receipt-repair").unwrap();
        assert!(
            task.completion_candidate
                .as_ref()
                .unwrap()
                .review_binding
                .is_none()
        );
        assert_eq!(task.completion_review_activity.len(), 1);
        assert_eq!(
            task.completion_review_activity[0].created_at,
            "2026-08-09T00:00:00Z"
        );
    }
}
