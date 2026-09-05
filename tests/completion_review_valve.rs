use tempfile::TempDir;
use worksgood::completion_manifest::{
    ArtifactOutput, COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest,
    ContentDigest, EvidenceRef, ImmutableLocator, IncompleteEvidence, IncompleteEvidenceKind,
    OutputRef, ResolvedReviewBundle, ReviewResolver,
};
use worksgood::completion_review::{
    CompletionReviewBinding, FLIP_BLIND_INPUT_SCHEMA, FLIP_COMPARISON_INPUT_SCHEMA,
    FLIP_HYPOTHESIS_MEDIA_TYPE, FLIP_INPUT_MEDIA_TYPE, FLIP_PHASE_RECORD_VERSION,
    FLIP_PROMPT_MEDIA_TYPE, FLIP_PROTOCOL, FLIP_RAW_OUTPUT_MEDIA_TYPE, FlipLatentHypothesis,
    FlipPhase, FlipPhaseExecution, FlipPhaseOutcome, FlipProof, FlipRouteSnapshot,
    ManifestReviewer, ReviewFinding, ReviewValveError, ReviewValveStatus, ReviewerKind,
    ReviewerUnavailable, SemanticReview, SemanticVerdict, flip_candidate_evidence_digest,
    flip_comparison_output_digest, flip_revealed_evidence_digest, load_stored_review_receipt,
    register_flip_execution_authority, render_flip_comparison_prompt, render_flip_inference_prompt,
    run_review_valve_at, validate_stored_flip_against_bundle,
};
use worksgood::completion_review_model::{build_flip_blind_input, build_flip_comparison_input};
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
                flip_proof: None,
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
                flip_proof: None,
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

fn fixture_flip_proof(
    store: &CompletionArtifactStore,
    bundle: &ResolvedReviewBundle,
    binding: &CompletionReviewBinding,
    route: &str,
    verdict: SemanticVerdict,
    findings: &[ReviewFinding],
) -> FlipProof {
    let hypothesis_value = FlipLatentHypothesis {
        goal: "fixture reconstructed goal".into(),
        constraints: Vec::new(),
        invariants: Vec::new(),
        failure_modes: Vec::new(),
    };
    let hypothesis_bytes = canonical_json(&serde_json::to_value(&hypothesis_value).unwrap());
    let hypothesis = store
        .put_bytes(&hypothesis_bytes, FLIP_HYPOTHESIS_MEDIA_TYPE)
        .unwrap();
    let blind_input = build_flip_blind_input(bundle);
    let blind_bytes = canonical_json(&serde_json::to_value(&blind_input).unwrap());
    let blind_object = store
        .put_bytes(&blind_bytes, FLIP_INPUT_MEDIA_TYPE)
        .unwrap();
    let blind_prompt = render_flip_inference_prompt(&blind_input);
    let blind_prompt_object = store
        .put_bytes(blind_prompt.as_bytes(), FLIP_PROMPT_MEDIA_TYPE)
        .unwrap();
    let inference_raw = store
        .put_bytes(&hypothesis_bytes, FLIP_RAW_OUTPUT_MEDIA_TYPE)
        .unwrap();
    let candidate_evidence =
        flip_candidate_evidence_digest(&blind_input.outputs, &blind_input.inspected_output_digests);
    let route_snapshot = || {
        FlipRouteSnapshot::new(
            route.into(),
            "pi".into(),
            "fixture".into(),
            Some("high".into()),
        )
    };
    let inference = FlipPhaseExecution {
        record_version: FLIP_PHASE_RECORD_VERSION,
        execution_id: "inference-call".into(),
        phase: FlipPhase::Inference,
        binding: binding.clone(),
        candidate_digest: bundle.manifest_digest.clone(),
        route: route_snapshot(),
        input_schema: FLIP_BLIND_INPUT_SCHEMA.into(),
        input_digest: blind_object.content_digest.clone(),
        input: blind_object,
        prompt_digest: blind_prompt_object.content_digest.clone(),
        prompt: blind_prompt_object,
        raw_output_digest: inference_raw.content_digest.clone(),
        raw_output: inference_raw,
        output_digest: hypothesis.content_digest.clone(),
        candidate_evidence_digest: candidate_evidence.clone(),
        revealed_intent_digest: None,
        revealed_evidence_digest: None,
        predecessor_record_digest: None,
        started_at: "2026-08-05T12:00:00Z".into(),
        finished_at: "2026-08-05T12:00:01Z".into(),
        executor: "pi".into(),
        outcome: FlipPhaseOutcome {
            success: true,
            usage: None,
            error: None,
        },
        record_digest: ContentDigest::of_bytes(b"pending"),
    }
    .seal();
    register_flip_execution_authority(store, &inference).unwrap();

    let comparison_input =
        build_flip_comparison_input(bundle, hypothesis.content_digest.clone(), hypothesis_value);
    let comparison_bytes = canonical_json(&serde_json::to_value(&comparison_input).unwrap());
    let comparison_object = store
        .put_bytes(&comparison_bytes, FLIP_INPUT_MEDIA_TYPE)
        .unwrap();
    let comparison_prompt = render_flip_comparison_prompt(&comparison_input);
    let comparison_prompt_object = store
        .put_bytes(comparison_prompt.as_bytes(), FLIP_PROMPT_MEDIA_TYPE)
        .unwrap();
    let verdict_value = match verdict {
        SemanticVerdict::Pass => ReviewVerdict::Pass,
        SemanticVerdict::Reject => ReviewVerdict::Reject,
    };
    let comparison_raw_bytes = canonical_json(&serde_json::json!({
        "verdict": match verdict { SemanticVerdict::Pass => "pass", SemanticVerdict::Reject => "reject" },
        "findings": findings,
    }));
    let comparison_raw = store
        .put_bytes(&comparison_raw_bytes, FLIP_RAW_OUTPUT_MEDIA_TYPE)
        .unwrap();
    let findings_digest =
        ContentDigest::of_bytes(&canonical_json(&serde_json::to_value(findings).unwrap()));
    let comparison = FlipPhaseExecution {
        record_version: FLIP_PHASE_RECORD_VERSION,
        execution_id: "comparison-call".into(),
        phase: FlipPhase::Comparison,
        binding: binding.clone(),
        candidate_digest: bundle.manifest_digest.clone(),
        route: route_snapshot(),
        input_schema: FLIP_COMPARISON_INPUT_SCHEMA.into(),
        input_digest: comparison_object.content_digest.clone(),
        input: comparison_object,
        prompt_digest: comparison_prompt_object.content_digest.clone(),
        prompt: comparison_prompt_object,
        raw_output_digest: comparison_raw.content_digest.clone(),
        raw_output: comparison_raw,
        output_digest: flip_comparison_output_digest(verdict_value, &findings_digest),
        candidate_evidence_digest: candidate_evidence,
        revealed_intent_digest: Some(bundle.requirements_digest.clone()),
        revealed_evidence_digest: Some(flip_revealed_evidence_digest(&comparison_input)),
        predecessor_record_digest: Some(inference.record_digest.clone()),
        started_at: "2026-08-05T12:00:02Z".into(),
        finished_at: "2026-08-05T12:00:03Z".into(),
        executor: "pi".into(),
        outcome: FlipPhaseOutcome {
            success: true,
            usage: None,
            error: None,
        },
        record_digest: ContentDigest::of_bytes(b"pending"),
    }
    .seal();
    register_flip_execution_authority(store, &comparison).unwrap();
    FlipProof {
        protocol: FLIP_PROTOCOL.into(),
        latent_hypothesis: hypothesis,
        inference,
        comparison,
        chain_digest: ContentDigest::of_bytes(b"pending"),
    }
    .seal()
}

struct MalformedFlipReviewer {
    calls: Vec<ReviewerKind>,
}

struct CorruptingFlipReviewer {
    calls: Vec<ReviewerKind>,
}

impl ManifestReviewer for MalformedFlipReviewer {
    fn route(&self) -> &str {
        "pi:malformed"
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &ResolvedReviewBundle,
        binding: Option<&CompletionReviewBinding>,
        artifact_store: &CompletionArtifactStore,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        self.calls.push(kind);
        let mut proof = fixture_flip_proof(
            artifact_store,
            bundle,
            binding.expect("malformed FLIP fixture binding"),
            self.route(),
            SemanticVerdict::Pass,
            &[],
        );
        proof.protocol = "self-asserted-protocol".into();
        proof = proof.seal();
        Ok(SemanticReview {
            verdict: SemanticVerdict::Pass,
            findings: Vec::new(),
            flip_proof: Some(proof),
        })
    }
}

impl ManifestReviewer for CorruptingFlipReviewer {
    fn route(&self) -> &str {
        "pi:corrupting"
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &ResolvedReviewBundle,
        binding: Option<&CompletionReviewBinding>,
        artifact_store: &CompletionArtifactStore,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        self.calls.push(kind);
        let proof = fixture_flip_proof(
            artifact_store,
            bundle,
            binding.expect("corrupting FLIP fixture binding"),
            self.route(),
            SemanticVerdict::Pass,
            &[],
        );
        let name = proof
            .inference
            .input
            .content_digest
            .as_str()
            .strip_prefix("b3:")
            .unwrap();
        std::fs::write(artifact_store.root().join("objects").join(name), b"{}").unwrap();
        Ok(SemanticReview {
            verdict: SemanticVerdict::Pass,
            findings: Vec::new(),
            flip_proof: Some(proof),
        })
    }
}

impl ManifestReviewer for FakeReviewer {
    fn route(&self) -> &str {
        &self.route
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &ResolvedReviewBundle,
        binding: Option<&worksgood::completion_review::CompletionReviewBinding>,
        artifact_store: &CompletionArtifactStore,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        self.calls.push(kind);
        self.result.clone().map(|mut review| {
            if kind == ReviewerKind::Flip {
                review.flip_proof = Some(fixture_flip_proof(
                    artifact_store,
                    bundle,
                    binding.expect("FLIP fixture binding"),
                    &self.route,
                    review.verdict,
                    &review.findings,
                ));
            }
            review
        })
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
fn genuine_flip_proof_rejects_every_broken_execution_binding() {
    let fixture = fixture();
    let mut flip = FakeReviewer::pass("pi:reviewer");
    let mut eval = FakeReviewer::pass("pi:evaluator");
    let outcome = run_review_valve_at(
        &fixture.store,
        &fixture.manifest_digest,
        &fixture.requirements_digest,
        Ok(fixture.bundle.clone()),
        &mut flip,
        &mut eval,
        NOW,
    )
    .unwrap();
    let valid_stored = outcome.flip;
    validate_stored_flip_against_bundle(&fixture.store, &valid_stored, &fixture.bundle).unwrap();
    let mut substituted_bundle = fixture.bundle.clone();
    substituted_bundle.outputs.clear();
    assert!(
        validate_stored_flip_against_bundle(&fixture.store, &valid_stored, &substituted_bundle,)
            .is_err(),
        "internally consistent proof must not substitute candidate evidence"
    );
    let valid = valid_stored.receipt.clone();
    assert!(valid.has_genuine_flip_proof(&fixture.store));

    let legacy = serde_json::json!({
        "protocol": "prompt-reconstruction-two-phase-v1",
        "latent_hypothesis": valid.flip_proof.as_ref().unwrap().latent_hypothesis,
        "inference_route": "pi:reviewer",
        "comparison_route": "pi:reviewer"
    });
    assert!(
        serde_json::from_value::<FlipProof>(legacy).is_err(),
        "protocol/routes/CID alone must not deserialize as proof"
    );

    let mut broken = valid.clone();
    broken.flip_proof = None;
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "missing chain"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    std::mem::swap(&mut proof.inference, &mut proof.comparison);
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "swapped phases"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.inference.route.exact_route = "pi:forged".into();
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "corrupted route snapshot"
    );

    // Recompute every public content hash coherently. Structural checks alone
    // now pass, but immutable-load authority must still reject the record
    // because no WG exact-call capture marker names the forged executions.
    let mut coherently_forged = valid.clone();
    let proof = coherently_forged.flip_proof.as_mut().unwrap();
    proof.inference.route = FlipRouteSnapshot::new(
        "pi:coherent-forgery".into(),
        "pi".into(),
        "forged".into(),
        Some("high".into()),
    );
    proof.inference = proof.inference.clone().seal();
    proof.comparison.route = proof.inference.route.clone();
    proof.comparison.predecessor_record_digest = Some(proof.inference.record_digest.clone());
    proof.comparison = proof.comparison.clone().seal();
    *proof = proof.clone().seal();
    coherently_forged.model_route = Some("pi:coherent-forgery".into());
    assert!(
        !coherently_forged.has_genuine_flip_proof(&fixture.store),
        "coherent public resealing is not WG-owned execution authority"
    );
    let forged_bytes = canonical_json(&serde_json::to_value(&coherently_forged).unwrap());
    let forged_object = fixture
        .store
        .put_bytes(
            &forged_bytes,
            "application/vnd.worksgood.review-receipt+json",
        )
        .unwrap();
    assert!(
        load_stored_review_receipt(&fixture.store, &forged_object).is_err(),
        "coherently resealed fields without exact-call authority must fail closed"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.inference.binding.generation += 1;
    proof.inference = proof.inference.clone().seal();
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "stale generation"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.comparison.candidate_digest = ContentDigest::of_bytes(b"other candidate");
    proof.comparison = proof.comparison.clone().seal();
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "cross-candidate comparison"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.comparison.started_at = "2026-08-05T11:59:00Z".into();
    proof.comparison = proof.comparison.clone().seal();
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "reordered chronology"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.comparison.execution_id = proof.inference.execution_id.clone();
    proof.comparison = proof.comparison.clone().seal();
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "same-call replay"
    );

    let mut broken = valid.clone();
    let proof = broken.flip_proof.as_mut().unwrap();
    proof.latent_hypothesis.content_digest = ContentDigest::of_bytes(b"swapped hypothesis");
    proof.latent_hypothesis.immutable_locator = ImmutableLocator::CompletionObject {
        digest: proof.latent_hypothesis.content_digest.clone(),
    };
    *proof = proof.clone().seal();
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "swapped phase-I output"
    );

    let mut broken = valid.clone();
    broken.findings_digest = ContentDigest::of_bytes(b"mutated comparison output");
    assert!(
        !broken.has_genuine_flip_proof(&fixture.store),
        "mutated decision evidence"
    );
}

#[test]
fn malformed_flip_proof_is_incomplete_and_skips_eval_even_for_advisory_callers() {
    let fixture = fixture();
    let mut flip = MalformedFlipReviewer { calls: Vec::new() };
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
    assert_eq!(outcome.status, ReviewValveStatus::IncompleteEvidence);
    assert_eq!(
        outcome.flip.receipt.verdict,
        ReviewVerdict::IncompleteEvidence
    );
    assert!(outcome.eval.is_none());
    assert!(eval.calls.is_empty());
}

#[test]
fn freshly_persisted_flip_is_fully_reloaded_before_eval() {
    let fixture = fixture();
    let mut flip = CorruptingFlipReviewer { calls: Vec::new() };
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

    assert!(matches!(
        error,
        ReviewValveError::Store(_) | ReviewValveError::InvalidReceipt(_)
    ));
    assert_eq!(flip.calls, vec![ReviewerKind::Flip]);
    assert!(eval.calls.is_empty(), "Eval must wait for immutable reload");
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
