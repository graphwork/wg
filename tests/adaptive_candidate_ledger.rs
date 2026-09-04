mod common;

use common::adaptive::*;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

#[test]
fn exact_binding_supersession() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let a = candidate(1, "manifest-a");
    let b = candidate(2, "manifest-b");
    store
        .selection_sink()
        .select(a.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let a_attempt = start(
        &store,
        a.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    finish(
        &store,
        &a_attempt,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Reject),
        "receipt-a",
        Some(usage(Some(0.01))),
    );
    store
        .selection_sink()
        .select(b.clone(), "2026-09-03T00:00:03Z")
        .unwrap();
    let b_attempt = start(
        &store,
        b.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:03Z",
        "2026-09-03T00:00:04Z",
    );
    finish(
        &store,
        &b_attempt,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
        "receipt-b",
        Some(usage(Some(0.01))),
    );

    let stale = store.completion_consumption_sink().consume(
        &a_attempt.review_attempt_id,
        &a,
        "receipt-a",
        "controller-v1",
        7,
        ConsumptionEffect::RejectedEvidence,
        "2026-09-03T00:00:05Z",
    );
    assert!(stale.is_err(), "superseded A must never be consumed");
    let consumed = store
        .completion_consumption_sink()
        .consume(
            &b_attempt.review_attempt_id,
            &b,
            "receipt-b",
            "controller-v1",
            7,
            ConsumptionEffect::AcceptedEvidence,
            "2026-09-03T00:00:05Z",
        )
        .unwrap();
    assert_eq!(
        consumed,
        store
            .completion_consumption_sink()
            .consume(
                &b_attempt.review_attempt_id,
                &b,
                "receipt-b",
                "controller-v1",
                7,
                ConsumptionEffect::AcceptedEvidence,
                "2026-09-03T00:00:06Z",
            )
            .unwrap(),
        "consumption replay must be exactly once"
    );

    let views = store.reader().review_attempts().unwrap();
    assert_eq!(views.len(), 2);
    assert!(!views[0].current_candidate);
    assert!(views[1].current_candidate && views[1].consumed);
    let mut changed = b.clone();
    changed.requirements_digest = "changed".into();
    assert!(
        store
            .completion_consumption_sink()
            .consume(
                &b_attempt.review_attempt_id,
                &changed,
                "receipt-b",
                "controller-v1",
                7,
                ConsumptionEffect::AcceptedEvidence,
                "2026-09-03T00:00:07Z",
            )
            .is_err()
    );
}

#[test]
fn semantic_and_infrastructure_partitions() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let reject = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    finish(
        &store,
        &reject,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Reject),
        "reject",
        None,
    );
    let timeout = start(
        &store,
        binding.clone(),
        ReviewerKind::Eval,
        0,
        "2026-09-03T00:00:03Z",
        "2026-09-03T00:00:04Z",
    );
    finish(
        &store,
        &timeout,
        ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::Timeout),
        "timeout",
        None,
    );
    let projector = store.learning_projector();
    let seal = projector
        .seal_trajectory(
            "graph-test",
            "task-a",
            2,
            "terminal-1",
            "2026-09-03T00:00:10Z",
        )
        .unwrap();
    let episode = projector
        .project(terminal_input(binding, "terminal-1"), &seal)
        .unwrap();
    assert_eq!(episode.semantic_trajectory.rejects, 1);
    assert_eq!(episode.infrastructure_summary.timeouts, 1);
    assert_eq!(projector.performance_projection().unwrap().task_count, 1);
}

#[test]
fn retry_and_reroute_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let first = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    finish(
        &store,
        &first,
        ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::Timeout),
        "timeout",
        None,
    );
    let retry = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:02Z",
        "2026-09-03T00:00:03Z",
    );
    assert_eq!(retry.review_run_id, first.review_run_id);
    assert_eq!(retry.ordinal, 2);
    let reroute = start(
        &store,
        binding,
        ReviewerKind::Flip,
        1,
        "2026-09-03T00:00:04Z",
        "2026-09-03T00:00:05Z",
    );
    assert_ne!(reroute.review_run_id, first.review_run_id);
    assert_eq!(reroute.ordinal, 1);
    let events = store.reader().events().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.event_id() == first.started_event_id)
    );
}
