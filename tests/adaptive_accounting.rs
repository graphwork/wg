mod common;

use common::adaptive::*;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

#[test]
fn deduplicated_lane_totals() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let flip = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    finish(
        &store,
        &flip,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
        "flip-receipt",
        Some(usage(Some(0.01))),
    );
    let eval = start(
        &store,
        binding,
        ReviewerKind::Eval,
        0,
        "2026-09-03T00:00:02Z",
        "2026-09-03T00:00:03Z",
    );
    let finish_id = finish(
        &store,
        &eval,
        ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::Timeout),
        "eval-receipt",
        None,
    );
    assert_eq!(
        finish_id,
        finish(
            &store,
            &eval,
            ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::Timeout),
            "eval-receipt",
            None,
        )
    );
    let accounting = store.reader().accounting().unwrap();
    assert_eq!(accounting.completion_flip.attempt_count, 1);
    assert_eq!(accounting.completion_flip.input_tokens, 10);
    assert_eq!(accounting.completion_flip.provider_cost, 0.01);
    assert_eq!(accounting.completion_eval.attempt_count, 1);
    assert_eq!(accounting.completion_eval.unknown_cost_attempts, 1);
    assert_eq!(accounting.all_agency_provider_cost, 0.01);
}
