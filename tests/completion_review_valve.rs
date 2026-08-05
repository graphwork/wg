use tempfile::TempDir;
use worksgood::completion_manifest::{
    ArtifactOutput, COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest,
    ContentDigest, EvidenceRef, IncompleteEvidence, IncompleteEvidenceKind, OutputRef,
    ResolvedReviewBundle, ReviewResolver,
};
use worksgood::completion_review::{
    ManifestReviewer, ReviewFinding, ReviewValveError, ReviewValveStatus, ReviewerKind,
    ReviewerUnavailable, SemanticReview, SemanticVerdict, run_review_valve_at,
};
use worksgood::identity::canonical_json;
use worksgood::simple_land::{CompletionContract, ReviewVerdict};

const REQUIREMENTS: &[u8] = b"produce reviewed output";
const SUMMARY: &[u8] = b"output produced and validated";
const NOW: &str = "2026-08-05T12:00:00Z";

struct Fixture {
    _temp: TempDir,
    store: CompletionArtifactStore,
    manifest_digest: ContentDigest,
    requirements_digest: ContentDigest,
    bundle: ResolvedReviewBundle,
}

fn evidence(store: &CompletionArtifactStore) -> EvidenceRef {
    store
        .evidence_from_bytes(b"tests passed", "test-log", "text/plain")
        .unwrap()
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let store = CompletionArtifactStore::open(temp.path().join("objects")).unwrap();
    let output = store.put_bytes(b"reviewed report", "text/plain").unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: "review-task".to_string(),
        generation: 7,
        completion_contract: CompletionContract::Report,
        requirements_digest: ContentDigest::of_bytes(REQUIREMENTS),
        source_revision: "main@review".to_string(),
        outputs: vec![OutputRef::Artifact(output)],
        validation_evidence: vec![evidence(&store)],
        worker_summary_digest: ContentDigest::of_bytes(SUMMARY),
    };
    let submission = store.put_manifest(&manifest).unwrap();
    let bundle = ReviewResolver::new(&store)
        .resolve_submission(&submission, REQUIREMENTS, SUMMARY, &[])
        .unwrap();
    Fixture {
        _temp: temp,
        store,
        manifest_digest: manifest.digest().unwrap(),
        requirements_digest: manifest.requirements_digest,
        bundle,
    }
}

#[derive(Clone)]
struct FakeReviewer {
    route: String,
    result: Result<SemanticReview, ReviewerUnavailable>,
    calls: Vec<ReviewerKind>,
}

impl FakeReviewer {
    fn pass(route: &str) -> Self {
        Self {
            route: route.to_string(),
            result: Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::new(),
            }),
            calls: Vec::new(),
        }
    }

    fn reject(route: &str, code: &str) -> Self {
        Self {
            route: route.to_string(),
            result: Ok(SemanticReview {
                verdict: SemanticVerdict::Reject,
                findings: vec![ReviewFinding::new(code, "repair this exact output")],
            }),
            calls: Vec::new(),
        }
    }

    fn unavailable(route: &str) -> Self {
        Self {
            route: route.to_string(),
            result: Err(ReviewerUnavailable {
                code: "provider.timeout".to_string(),
                message: "exact reviewer route timed out".to_string(),
            }),
            calls: Vec::new(),
        }
    }
}

impl ManifestReviewer for FakeReviewer {
    fn route(&self) -> &str {
        &self.route
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        _bundle: &ResolvedReviewBundle,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        self.calls.push(kind);
        self.result.clone()
    }
}

#[test]
fn flip_then_eval_pass_opens_the_exact_manifest_valve() {
    let fixture = fixture();
    let mut flip = FakeReviewer::pass("pi:openrouter:anthropic/claude-opus-4.7");
    let mut eval = FakeReviewer::pass("codex:gpt-5.5");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::Accepted);
    assert!(outcome.accepted_exactly(&fixture.manifest_digest, &fixture.requirements_digest));
    assert_eq!(flip.calls, vec![ReviewerKind::Flip]);
    assert_eq!(eval.calls, vec![ReviewerKind::Eval]);
    assert_eq!(
        outcome.flip.receipt.model_route.as_deref(),
        Some("pi:openrouter:anthropic/claude-opus-4.7")
    );
    assert_eq!(
        outcome
            .eval
            .as_ref()
            .unwrap()
            .receipt
            .model_route
            .as_deref(),
        Some("codex:gpt-5.5")
    );
    assert_eq!(outcome.flip.receipt.created_at, NOW);
    assert!(!outcome.accepted_exactly(
        &ContentDigest::of_bytes(b"changed manifest"),
        &fixture.requirements_digest
    ));
}

#[test]
fn flip_rejection_returns_to_source_without_invoking_eval() {
    let fixture = fixture();
    let mut flip = FakeReviewer::reject("pi:reviewer", "requirements.missing");
    let mut eval = FakeReviewer::pass("pi:evaluator");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::FlipRejected);
    assert_eq!(outcome.flip.receipt.verdict, ReviewVerdict::Reject);
    assert!(outcome.eval.is_none());
    assert!(eval.calls.is_empty());
}

#[test]
fn eval_rejection_closes_the_valve_after_exact_flip_pass() {
    let fixture = fixture();
    let mut flip = FakeReviewer::pass("pi:reviewer");
    let mut eval = FakeReviewer::reject("pi:evaluator", "regression.detected");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::EvalRejected);
    assert_eq!(
        outcome.eval.as_ref().unwrap().receipt.verdict,
        ReviewVerdict::Reject
    );
    assert!(!outcome.accepted_exactly(&fixture.manifest_digest, &fixture.requirements_digest));
}

#[test]
fn reviewer_failure_is_unavailable_not_reject_and_never_falls_back() {
    let fixture = fixture();
    let mut flip = FakeReviewer::unavailable("pi:exact-route");
    let mut eval = FakeReviewer::pass("pi:must-not-run");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::ReviewUnavailable);
    assert_eq!(outcome.flip.receipt.verdict, ReviewVerdict::Unavailable);
    assert_eq!(
        outcome.flip.receipt.model_route.as_deref(),
        Some("pi:exact-route")
    );
    assert!(eval.calls.is_empty());
}

#[test]
fn eval_failure_is_unavailable_without_reclassifying_flip() {
    let fixture = fixture();
    let mut flip = FakeReviewer::pass("pi:flip");
    let mut eval = FakeReviewer::unavailable("pi:eval");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::ReviewUnavailable);
    assert_eq!(outcome.flip.receipt.verdict, ReviewVerdict::Pass);
    assert_eq!(
        outcome.eval.as_ref().unwrap().receipt.verdict,
        ReviewVerdict::Unavailable
    );
}

#[test]
fn resolver_failure_records_incomplete_evidence_without_model_calls() {
    let fixture = fixture();
    let incomplete = IncompleteEvidence {
        kind: IncompleteEvidenceKind::DigestMismatch,
        reference: "artifact output".to_string(),
        detail: "observed b3:bad, expected b3:good".to_string(),
    };
    let mut flip = FakeReviewer::pass("pi:must-not-run");
    let mut eval = FakeReviewer::pass("pi:must-not-run");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Err(incomplete),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    assert_eq!(outcome.status, ReviewValveStatus::IncompleteEvidence);
    assert_eq!(
        outcome.flip.receipt.verdict,
        ReviewVerdict::IncompleteEvidence
    );
    assert!(outcome.flip.receipt.model_route.is_none());
    assert!(outcome.eval.is_none());
    assert!(flip.calls.is_empty());
    assert!(eval.calls.is_empty());
}

#[test]
fn reviewer_without_an_exact_route_is_refused_before_model_call() {
    let fixture = fixture();
    let mut flip = FakeReviewer::pass("");
    let mut eval = FakeReviewer::pass("pi:eval");

    let error = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ReviewValveError::MissingExactRoute(ReviewerKind::Flip)
    ));
    assert!(flip.calls.is_empty());
    assert!(eval.calls.is_empty());
}

#[test]
fn mismatched_resolved_bundle_is_an_integrity_error_before_review() {
    let mut fixture = fixture();
    fixture.bundle.manifest_digest = ContentDigest::of_bytes(b"different manifest");
    let mut flip = FakeReviewer::pass("pi:must-not-run");
    let mut eval = FakeReviewer::pass("pi:must-not-run");

    let error = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap_err();

    assert!(matches!(error, ReviewValveError::BindingMismatch));
    assert!(flip.calls.is_empty());
    assert!(eval.calls.is_empty());
}

fn object_bytes(store: &CompletionArtifactStore, output: &ArtifactOutput) -> Vec<u8> {
    let name = output.content_digest.as_str().strip_prefix("b3:").unwrap();
    std::fs::read(store.root().join("objects").join(name)).unwrap()
}

#[test]
fn findings_and_receipts_are_immutable_content_addressed_objects() {
    let fixture = fixture();
    let mut flip = FakeReviewer::reject("pi:reviewer", "specific.failure");
    let mut eval = FakeReviewer::pass("pi:must-not-run");

    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();

    let findings = object_bytes(&fixture.store, &outcome.flip.findings_object);
    assert_eq!(
        ContentDigest::of_bytes(&findings),
        outcome.flip.receipt.findings_digest
    );
    let receipt = object_bytes(&fixture.store, &outcome.flip.receipt_object);
    let expected = canonical_json(&serde_json::to_value(&outcome.flip.receipt).unwrap());
    assert_eq!(receipt, expected);
    assert_eq!(
        ContentDigest::of_bytes(&receipt),
        outcome.flip.receipt_object.content_digest
    );
}
