use std::collections::BTreeMap;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

pub fn source() -> SourceBindingV1 {
    SourceBindingV1 {
        graph_identity: "graph-test".into(),
        task_id: "task-a".into(),
        generation: 2,
        source_attempt_id: "attempt-2-1".into(),
        source_fence: 7,
        assignment_receipt_id: "asg-test".into(),
    }
}

pub fn candidate(sequence: u64, manifest: &str) -> CandidateBindingV1 {
    CandidateBindingV1 {
        source: source(),
        candidate_sequence: sequence,
        manifest_digest: manifest.into(),
        requirements_digest: "req-1".into(),
        source_revision: format!("rev-{sequence}"),
        dependency_revision_digest: "deps-1".into(),
        output_digests: vec![format!("out-{sequence}")],
        validation_evidence_digest: format!("validation-{sequence}"),
    }
}

pub fn policy() -> PolicySnapshot {
    PolicySnapshot {
        policy_id: "completion-v1".into(),
        policy_digest: "policy-digest".into(),
        strict: true,
        max_infrastructure_attempts: 2,
    }
}

pub fn route(generation: u32) -> RouteSnapshot {
    RouteSnapshot::exact(
        "pi:test:review-model",
        Some("high".into()),
        "test-adapter",
        "1",
        generation,
    )
    .unwrap()
}

pub fn start(
    store: &AdaptiveStore,
    binding: CandidateBindingV1,
    kind: ReviewerKind,
    route_generation: u32,
    started: &str,
    expires: &str,
) -> ReviewAttemptHandle {
    store
        .review_sink()
        .start(
            binding,
            kind,
            ReviewProduct::Completion,
            policy(),
            route(route_generation),
            "cap-readonly".into(),
            started.into(),
            expires.into(),
        )
        .unwrap()
}

pub fn finish(
    store: &AdaptiveStore,
    handle: &ReviewAttemptHandle,
    outcome: ReviewOutcomeV1,
    receipt: &str,
    usage: Option<UsageV1>,
) -> String {
    store
        .review_sink()
        .finish(
            handle,
            ReviewFinishInput {
                outcome,
                completed_at: "2026-09-03T00:00:02Z".into(),
                duration_ms: 1000,
                response_digest: Some(format!("response-{receipt}")),
                findings_digest: Some(format!("findings-{receipt}")),
                inspected_output_digests: vec!["out".into()],
                usage,
                stop_reason: Some("stop".into()),
                provider_reported_route: Some("pi:test:review-model".into()),
                receipt_digest: receipt.into(),
            },
        )
        .unwrap()
}

pub fn usage(cost: Option<f64>) -> UsageV1 {
    UsageV1 {
        input_tokens: 10,
        output_tokens: 2,
        cache_read_tokens: 3,
        cache_write_tokens: 1,
        total_tokens: 16,
        provider_cost: cost,
        currency: "USD".into(),
        source: "provider-reported".into(),
    }
}

pub fn terminal_input(binding: CandidateBindingV1, terminal: &str) -> TerminalEpisodeInputV1 {
    TerminalEpisodeInputV1 {
        graph_identity: binding.source.graph_identity.clone(),
        task_id: binding.source.task_id.clone(),
        generation: binding.source.generation,
        terminal_event_id: terminal.into(),
        terminal_disposition: TerminalDispositionV1::Done,
        source_attempt_id: Some(binding.source.source_attempt_id.clone()),
        source_fence: Some(binding.source.source_fence),
        assignment_provenance: AssignmentProvenanceV1::BoundReceipt(
            binding.source.assignment_receipt_id.clone(),
        ),
        terminal_provenance: TerminalProvenanceV1::CompletionReceipt("completion-1".into()),
        terminal_candidate_binding: Some(binding),
        source_quality_eligibility: SourceQualityEligibilityV1::Eligible,
        created_at: "2026-09-03T00:00:10Z".into(),
    }
}

pub fn assessment_input(episode_id: &str) -> OutcomeAssessmentInputV1 {
    OutcomeAssessmentInputV1 {
        episode_id: episode_id.into(),
        scorer_policy_id: "scorer-policy".into(),
        scorer_principal: "scorer".into(),
        route: RouteSnapshot::exact(
            "pi:test:scorer-model",
            Some("high".into()),
            "scorer",
            "1",
            0,
        )
        .unwrap(),
        evidence_digest: "terminal-evidence".into(),
        score: 0.8,
        dimensions: BTreeMap::new(),
        notes_digest: "notes".into(),
        usage: Some(usage(Some(0.02))),
        usage_state: UsageStateV1::Reported,
        source_principal: Some("source".into()),
        assigner_principal: Some("assigner".into()),
        evolver_principal: Some("evolver".into()),
        calibrated_reviewer_principals: vec!["flip".into(), "eval".into()],
        source_route_cohort: "source-cohort".into(),
        scorer_route_cohort: "scorer-cohort".into(),
        fresh_context: true,
        read_only_capabilities: true,
        created_at: "2026-09-03T00:01:00Z".into(),
    }
}
