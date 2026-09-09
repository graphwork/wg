mod common;

use common::adaptive::{assessment_input, candidate, finish, start, terminal_input};
use std::collections::BTreeMap;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

fn composition(id: &str) -> CompositionSnapshotV1 {
    CompositionSnapshotV1 {
        agent_id: id.into(),
        role_id: format!("role-{id}"),
        tradeoff_id: format!("tradeoff-{id}"),
        component_ids: vec![format!("component-{id}")],
        outcome_id: format!("outcome-{id}"),
        composition_digest: format!("composition-{id}"),
    }
}

fn assignment_input(composition: Option<CompositionSnapshotV1>) -> AssignmentReceiptInputV1 {
    let decision = composition.as_ref().map_or_else(
        || AssignmentDecisionV1::Uncomposed {
            reason: "direct dispatch".into(),
        },
        |composition| AssignmentDecisionV1::Automatic {
            composition_digest: composition.composition_digest.clone(),
        },
    );
    AssignmentReceiptInputV1 {
        graph_identity: "graph-test".into(),
        task_id: "task-a".into(),
        generation: 2,
        attempt_id: "attempt-2-1".into(),
        attempt_fence: 7,
        admission_snapshot_digest: "admission-1".into(),
        context_partition: "actual_work".into(),
        decision,
        selector: AssignmentSelectorSnapshotV1 {
            kind: "deterministic-reward-ranking".into(),
            principal: "selector".into(),
            policy_digest: "selector-v1".into(),
            exact_route: None,
        },
        candidate_scores: BTreeMap::new(),
        selected_composition: composition,
        started_at: "2026-09-03T00:00:00Z".into(),
        completed_at: "2026-09-03T00:00:01Z".into(),
        failure: None,
    }
}

fn episode_for(store: &AdaptiveStore, receipt: &AssignmentReceiptV1) -> LearningEpisodeV1 {
    let mut binding = candidate(1, "manifest-a");
    binding.source.assignment_receipt_id = receipt.receipt_id.clone();
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:02Z")
        .unwrap();
    let projector = store.learning_projector();
    let seal = projector
        .seal_trajectory(
            "graph-test",
            "task-a",
            2,
            "terminal-1",
            "2026-09-03T00:00:03Z",
        )
        .unwrap();
    projector
        .project(terminal_input(binding, "terminal-1"), &seal)
        .unwrap()
}

#[test]
fn legacy_uncomposed_receipt_remains_readable_as_direct_dispatch() {
    let legacy = r#"{
        "schema":1,
        "receipt_id":"b3:legacy",
        "graph_identity":"graph-test",
        "task_id":"task-a",
        "generation":0,
        "attempt_id":"attempt-0-1",
        "attempt_fence":1,
        "decision":{"kind":"uncomposed","reason":"predates admission receipts"},
        "created_at":"unknown-legacy"
    }"#;
    let receipt: AssignmentReceiptV1 = serde_json::from_str(legacy).unwrap();
    assert!(receipt.admission_snapshot_digest.is_empty());
    assert!(receipt.context_partition.is_empty());
    assert_eq!(receipt.selector, AssignmentSelectorSnapshotV1::direct());
    assert!(receipt.started_at.is_empty() && receipt.completed_at.is_empty());
}

#[test]
fn assignment_receipt_is_attempt_bound_and_replay_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let input = assignment_input(Some(composition("a")));
    let first = store.record_attempt_assignment(input.clone()).unwrap();
    let replay = store.record_attempt_assignment(input).unwrap();
    assert_eq!(first, replay);
    assert_eq!(store.reader().assignment_receipts().unwrap().len(), 1);
    assert!(first.selection_id.is_some());
    assert_eq!(first.attempt_id, "attempt-2-1");
    assert_eq!(first.attempt_fence, 7);
}

#[test]
fn delayed_reward_and_evolver_input_ignore_reviewers_and_infrastructure() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let receipt = store
        .record_attempt_assignment(assignment_input(Some(composition("a"))))
        .unwrap();
    let episode = episode_for(&store, &receipt);

    // Reviewer self-opinion and infrastructure failures remain trajectory-only.
    let binding = episode.terminal_candidate_binding.clone().unwrap();
    let review = start(
        &store,
        binding,
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:04Z",
        "2026-09-03T00:00:05Z",
    );
    finish(
        &store,
        &review,
        ReviewOutcomeV1::Infrastructure(InfrastructureOutcome::Timeout),
        "infra-timeout",
        None,
    );
    let manifest = store
        .learning_projector()
        .project_assignment_rewards()
        .unwrap();
    assert!(manifest.assignment_reward_ids.is_empty());
    assert!(store.reader().assignment_rewards().unwrap().is_empty());
    assert!(store.reader().evolution_inputs().unwrap().is_empty());

    let mut self_score = assessment_input(&episode.episode_id);
    self_score.scorer_principal = "source".into();
    self_score.scorer_policy_id = "self-score-policy".into();
    store
        .learning_projector()
        .record_assessment(self_score)
        .unwrap();
    assert!(store.reader().assignment_rewards().unwrap().is_empty());
    assert!(store.reader().evolution_inputs().unwrap().is_empty());

    let assessment = store
        .learning_projector()
        .record_assessment(assessment_input(&episode.episode_id))
        .unwrap();
    let rewards = store.reader().assignment_rewards().unwrap();
    assert_eq!(rewards.len(), 1);
    assert_eq!(rewards[0].reward, assessment.score);
    assert_eq!(
        store
            .reader()
            .mean_reward_for_composition("composition-a", "actual_work")
            .unwrap(),
        Some(0.8)
    );
    let inputs = store.reader().evolution_inputs().unwrap();
    assert!(inputs.iter().any(|input| {
        input.episode_ids == vec![episode.episode_id.clone()]
            && input.assignment_reward_ids == vec![rewards[0].reward_id.clone()]
    }));

    // Exact replay does not duplicate reward or evolver episode consumption.
    store
        .learning_projector()
        .record_assessment(assessment_input(&episode.episode_id))
        .unwrap();
    assert_eq!(store.reader().assignment_rewards().unwrap().len(), 1);
}

#[test]
fn later_independent_outcome_supersedes_active_reward_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let receipt = store
        .record_attempt_assignment(assignment_input(Some(composition("a"))))
        .unwrap();
    let episode = episode_for(&store, &receipt);
    store
        .learning_projector()
        .record_assessment(assessment_input(&episode.episode_id))
        .unwrap();

    let mut corrected = assessment_input(&episode.episode_id);
    corrected.scorer_policy_id = "trusted-correction-v1".into();
    corrected.scorer_principal = "independent-corrector".into();
    corrected.evidence_digest = "corrected-terminal-evidence".into();
    corrected.score = 0.2;
    corrected.created_at = "2026-09-03T00:02:00Z".into();
    store
        .learning_projector()
        .record_assessment(corrected)
        .unwrap();

    let all_rewards = store.reader().assignment_rewards().unwrap();
    assert_eq!(all_rewards.len(), 2);
    let active = store.reader().active_assignment_rewards().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].reward, 0.2);
    assert!(active[0].supersedes.is_some());
    assert_eq!(
        store
            .reader()
            .mean_reward_for_composition("composition-a", "actual_work")
            .unwrap(),
        Some(0.2)
    );
}

#[test]
fn direct_uncomposed_attempt_never_fabricates_composition_reward() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let receipt = store
        .record_attempt_assignment(assignment_input(None))
        .unwrap();
    let episode = episode_for(&store, &receipt);
    store
        .learning_projector()
        .record_assessment(assessment_input(&episode.episode_id))
        .unwrap();
    assert!(matches!(
        receipt.decision,
        AssignmentDecisionV1::Uncomposed { .. }
    ));
    assert!(store.reader().assignment_rewards().unwrap().is_empty());
}
