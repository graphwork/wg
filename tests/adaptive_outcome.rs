mod common;

use common::adaptive::*;
use worksgood::adaptive_agency::*;

fn episode(store: &AdaptiveStore) -> LearningEpisodeV1 {
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
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
    projector
        .project(terminal_input(binding, "terminal-1"), &seal)
        .unwrap()
}

#[test]
fn anti_self_scoring() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let episode = episode(&store);
    let projector = store.learning_projector();

    let independent = projector
        .record_assessment(assessment_input(&episode.episode_id))
        .unwrap();
    assert_eq!(
        independent.independence,
        AssessmentIndependenceV1::Independent
    );

    for principal in ["source", "assigner", "evolver", "flip", "eval"] {
        let mut input = assessment_input(&episode.episode_id);
        input.scorer_principal = principal.into();
        input.evidence_digest = format!("evidence-{principal}");
        let assessment = projector.record_assessment(input).unwrap();
        assert!(matches!(
            assessment.independence,
            AssessmentIndependenceV1::NonIndependent { .. }
        ));
    }
    let mut same_cohort = assessment_input(&episode.episode_id);
    same_cohort.evidence_digest = "same-cohort".into();
    same_cohort.scorer_route_cohort = same_cohort.source_route_cohort.clone();
    assert!(matches!(
        projector
            .record_assessment(same_cohort)
            .unwrap()
            .independence,
        AssessmentIndependenceV1::NonIndependent { .. }
    ));

    let projection = projector.performance_projection().unwrap();
    assert_eq!(projection.task_count, 1);
    assert_eq!(projection.scored_episode_count, 1);
    assert_eq!(projection.avg_score, Some(0.8));
}
