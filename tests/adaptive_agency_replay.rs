mod common;

use common::adaptive::*;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

#[test]
fn crash_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    let selected = store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    assert_eq!(
        selected,
        store
            .selection_sink()
            .select(binding.clone(), "2026-09-03T00:00:09Z")
            .unwrap()
    );

    // Crash after durable start, before receipt: recovery settles unknown and
    // retry obtains a new ordinal without resetting route/run identity.
    let crashed = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    assert_eq!(
        store
            .review_sink()
            .settle_expired("2026-09-03T00:00:02Z")
            .unwrap()
            .len(),
        1
    );
    assert!(
        store.reader().review_attempts().unwrap()[0]
            .outcome
            .as_ref()
            .is_some_and(|outcome| *outcome
                == ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::InterruptedUnknown))
    );
    let retry = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:03Z",
        "2026-09-03T00:00:04Z",
    );
    assert_eq!(retry.review_run_id, crashed.review_run_id);
    assert_eq!(retry.ordinal, 2);
    let finish_id = finish(
        &store,
        &retry,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
        "receipt-pass",
        Some(usage(Some(0.01))),
    );
    assert_eq!(
        finish_id,
        finish(
            &store,
            &retry,
            ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
            "receipt-pass",
            Some(usage(Some(0.01))),
        ),
        "identical durable receipt replay is a no-op"
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
    assert_eq!(
        seal,
        projector
            .seal_trajectory(
                "graph-test",
                "task-a",
                2,
                "terminal-1",
                "2026-09-03T00:00:10Z",
            )
            .unwrap()
    );
    let episode = projector
        .project(terminal_input(binding.clone(), "terminal-1"), &seal)
        .unwrap();
    assert_eq!(
        episode.episode_id,
        projector
            .project(terminal_input(binding, "terminal-1"), &seal)
            .unwrap()
            .episode_id
    );
}
