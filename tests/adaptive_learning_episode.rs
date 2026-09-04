mod common;

use common::adaptive::*;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

#[test]
fn repeated_candidates_count_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    for n in 1..=3 {
        let binding = candidate(n, &format!("manifest-{n}"));
        store
            .selection_sink()
            .select(binding.clone(), format!("2026-09-03T00:00:0{n}Z"))
            .unwrap();
        let attempt = start(
            &store,
            binding.clone(),
            ReviewerKind::Flip,
            0,
            "2026-09-03T00:00:00Z",
            "2026-09-03T00:00:01Z",
        );
        finish(
            &store,
            &attempt,
            ReviewOutcomeV1::Semantic(if n == 3 {
                SemanticOutcome::Pass
            } else {
                SemanticOutcome::Reject
            }),
            &format!("receipt-{n}"),
            None,
        );
    }
    let final_candidate = candidate(3, "manifest-3");
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
    let first = projector
        .project(terminal_input(final_candidate.clone(), "terminal-1"), &seal)
        .unwrap();
    let replay = projector
        .project(terminal_input(final_candidate, "terminal-1"), &seal)
        .unwrap();
    assert_eq!(first.episode_id, replay.episode_id);
    assert_eq!(first.semantic_trajectory.candidate_count, 3);
    assert_eq!(first.semantic_trajectory.rejects, 2);
    let projection = projector.performance_projection().unwrap();
    assert_eq!(projection.task_count, 1);
    assert_eq!(projection.episode_ids, vec![first.episode_id]);
}

#[test]
fn terminal_eligibility_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let projector = store.learning_projector();

    let cases = [
        ("done", TerminalDispositionV1::Done, true),
        ("source-failure", TerminalDispositionV1::Failed, true),
        ("infra-failure", TerminalDispositionV1::Failed, false),
        ("cancelled", TerminalDispositionV1::Cancelled, false),
        ("operator", TerminalDispositionV1::Done, false),
    ];
    for (index, (name, disposition, eligible)) in cases.into_iter().enumerate() {
        let task = format!("task-{name}");
        let mut case_binding = binding.clone();
        case_binding.source.task_id = task.clone();
        case_binding.source.generation = index as u64;
        case_binding.candidate_sequence = 1;
        store
            .selection_sink()
            .select(case_binding.clone(), "2026-09-03T00:00:00Z")
            .unwrap();
        let terminal = format!("terminal-{name}");
        let seal = projector
            .seal_trajectory(
                "graph-test",
                &task,
                index as u64,
                &terminal,
                "2026-09-03T00:00:10Z",
            )
            .unwrap();
        let mut input = terminal_input(case_binding, &terminal);
        input.task_id = task;
        input.generation = index as u64;
        input.terminal_disposition = disposition;
        input.source_quality_eligibility = if eligible {
            SourceQualityEligibilityV1::Eligible
        } else {
            SourceQualityEligibilityV1::Ineligible {
                reason: name.into(),
            }
        };
        let episode = projector.project(input, &seal).unwrap();
        assert_eq!(
            episode.source_quality_eligibility == SourceQualityEligibilityV1::Eligible,
            eligible
        );
    }
    assert_eq!(projector.performance_projection().unwrap().task_count, 2);
}
