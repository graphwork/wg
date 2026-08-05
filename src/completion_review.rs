//! Manifest-bound FLIP then eval completion valve.
//!
//! Reviewers consume one already-resolved immutable bundle. Resolver failures
//! become `IncompleteEvidence`; model/provider failures become `Unavailable`.
//! Neither is converted into semantic rejection, and eval is never invoked
//! until FLIP passes the exact manifest and requirements binding.

use crate::completion_manifest::{
    ArtifactOutput, ArtifactStoreError, CompletionArtifactStore, ContentDigest, IncompleteEvidence,
    ResolvedReviewBundle,
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

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &ResolvedReviewBundle,
    ) -> Result<SemanticReview, ReviewerUnavailable>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewReceipt {
    pub receipt_version: u32,
    pub manifest_digest: ContentDigest,
    pub requirements_digest: ContentDigest,
    pub reviewer_kind: ReviewerKind,
    pub verdict: ReviewVerdict,
    pub findings_digest: ContentDigest,
    pub inspected_output_digests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_route: Option<String>,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredReviewReceipt {
    pub receipt: ReviewReceipt,
    pub receipt_object: ArtifactOutput,
    pub findings_object: ArtifactOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewValveStatus {
    Accepted,
    FlipRejected,
    EvalRejected,
    ReviewUnavailable,
    IncompleteEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
    #[error("persist review receipt: {0}")]
    Store(#[from] ArtifactStoreError),
    #[error("serialize review receipt: {0}")]
    Serialize(#[from] serde_json::Error),
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
    run_review_valve_at(
        artifact_store,
        manifest_digest,
        requirements_digest,
        resolved,
        flip_reviewer,
        eval_reviewer,
        &Utc::now().to_rfc3339(),
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
                    model_route: None,
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
    let flip_result = flip_reviewer.review(ReviewerKind::Flip, &bundle);
    let flip = receipt_from_reviewer_result(
        artifact_store,
        manifest_digest,
        requirements_digest,
        ReviewerKind::Flip,
        &bundle.inspected_output_digests,
        flip_reviewer.route(),
        flip_result,
        created_at,
    )?;
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
    let eval_result = eval_reviewer.review(ReviewerKind::Eval, &bundle);
    let eval = receipt_from_reviewer_result(
        artifact_store,
        manifest_digest,
        requirements_digest,
        ReviewerKind::Eval,
        &bundle.inspected_output_digests,
        eval_reviewer.route(),
        eval_result,
        created_at,
    )?;
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
    model_route: &str,
    result: Result<SemanticReview, ReviewerUnavailable>,
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
            model_route: Some(model_route.to_string()),
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
    model_route: Option<String>,
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
        model_route: material.model_route,
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
