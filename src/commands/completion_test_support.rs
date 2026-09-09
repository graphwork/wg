use worksgood::completion_manifest::{
    CompletionArtifactStore, ContentDigest, ResolvedReviewBundle,
};
use worksgood::completion_review::{
    CompletionReviewBinding, FLIP_BLIND_INPUT_SCHEMA, FLIP_COMPARISON_INPUT_SCHEMA,
    FLIP_HYPOTHESIS_MEDIA_TYPE, FLIP_INPUT_MEDIA_TYPE, FLIP_PHASE_RECORD_VERSION,
    FLIP_PROMPT_MEDIA_TYPE, FLIP_PROTOCOL, FLIP_RAW_OUTPUT_MEDIA_TYPE, FlipLatentHypothesis,
    FlipPhase, FlipPhaseExecution, FlipPhaseOutcome, FlipProof, FlipRouteSnapshot, ReviewFinding,
    SemanticVerdict, flip_candidate_evidence_digest, flip_comparison_output_digest,
    flip_revealed_evidence_digest, register_flip_execution_authority,
    render_flip_comparison_prompt, render_flip_inference_prompt,
};
use worksgood::completion_review_model::{build_flip_blind_input, build_flip_comparison_input};
use worksgood::identity::canonical_json;
use worksgood::simple_land::ReviewVerdict;

/// Test-only stand-in for the exact-call boundary. It writes the same immutable
/// raw/input/prompt objects and WG-owned create-once capture markers as the real
/// `ExactModelReviewer`; callers still exercise normal receipt reload checks.
pub fn test_flip_proof(
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
        execution_id: format!("fixture-inference:{}", uuid::Uuid::now_v7()),
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
    let raw_bytes = canonical_json(&serde_json::json!({
        "verdict": match verdict {
            SemanticVerdict::Pass => "pass",
            SemanticVerdict::Reject => "reject",
        },
        "findings": findings,
    }));
    let comparison_raw = store
        .put_bytes(&raw_bytes, FLIP_RAW_OUTPUT_MEDIA_TYPE)
        .unwrap();
    let findings_digest = ContentDigest::of_bytes(&canonical_json(
        &serde_json::to_value(findings).expect("findings serialize"),
    ));
    let comparison = FlipPhaseExecution {
        record_version: FLIP_PHASE_RECORD_VERSION,
        execution_id: format!("fixture-comparison:{}", uuid::Uuid::now_v7()),
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
