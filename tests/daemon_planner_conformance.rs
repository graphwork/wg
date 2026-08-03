use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use worksgood::service::{
    DecisionTrace, PlannedEffect, PlannerActionKind, PlannerFailedPrerequisiteClass,
    PlannerIncidentCode, PlannerRuleset, PlannerViolationCode, PlannerWaitKind, ReplayReport,
    replay_bytes, replay_daemon,
};

#[derive(Debug, Deserialize)]
struct IncidentFixture {
    name: String,
    expected_historical_violation: PlannerViolationCode,
    expected_corrected_action: Option<PlannerActionKind>,
    expected_corrected_wait: Option<PlannerWaitKind>,
    trace: DecisionTrace,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("formal")
        .join("fixtures")
        .join("daemon")
        .join("v1")
}

fn load_fixtures() -> Vec<IncidentFixture> {
    let mut paths = fs::read_dir(fixtures_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

#[derive(Debug, Deserialize)]
struct FailedPrerequisiteFixture {
    name: String,
    expected_action: Option<PlannerActionKind>,
    expected_wait: Option<PlannerWaitKind>,
    expected_class: PlannerFailedPrerequisiteClass,
    trace: DecisionTrace,
}

fn failed_prerequisite_fixtures() -> Vec<FailedPrerequisiteFixture> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/fixtures/daemon/v2");
    let mut paths = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

#[test]
fn every_seeded_incident_violates_historical_and_converges_corrected() {
    let fixtures = load_fixtures();
    assert_eq!(fixtures.len(), 9);
    for mut fixture in fixtures {
        assert_eq!(fixture.trace.ruleset, PlannerRuleset::Historical);
        let historical = replay_daemon(&fixture.trace).unwrap();
        assert!(
            historical.steps[0]
                .violations
                .contains(&fixture.expected_historical_violation),
            "{} did not expose its named historical violation",
            fixture.name
        );

        fixture.trace.ruleset = PlannerRuleset::Corrected;
        let corrected = replay_daemon(&fixture.trace).unwrap();
        assert!(
            corrected.steps[0].violations.is_empty(),
            "{} did not converge under corrected rules: {:?}",
            fixture.name,
            corrected.steps[0].violations
        );
        let task = corrected.final_state.tasks.values().next().unwrap();
        let forward_count = usize::from(task.runnable.is_some())
            + usize::from(matches!(
                task.owner,
                worksgood::service::PlannerOwnerEvidence::AuthenticatedLive { .. }
            ))
            + usize::from(task.external_wait.is_some())
            + usize::from(task.scheduled.is_some());
        assert_eq!(forward_count, 1, "{} lacks one forward class", fixture.name);
        assert!(
            corrected.final_state.repaired_incidents.contains(
                &fixture
                    .trace
                    .observations
                    .iter()
                    .find_map(|entry| match &entry.observation {
                        worksgood::service::Observation::Task(task) => {
                            task.incidents.iter().next().copied()
                        }
                        _ => None,
                    })
                    .unwrap()
            )
        );
        match fixture.expected_corrected_action {
            Some(action) => assert!(
                corrected.steps[0]
                    .effects
                    .iter()
                    .any(|effect| effect.action == action),
                "{} did not emit {:?}",
                fixture.name,
                action
            ),
            None => assert!(corrected.steps[0].effects.is_empty()),
        }
        if let Some(wait) = fixture.expected_corrected_wait {
            assert_eq!(task.external_wait.as_ref().unwrap().kind, wait);
        }
    }
}

#[test]
fn repeated_offline_replay_is_byte_identical_and_has_no_filesystem_effects() {
    for fixture in load_fixtures() {
        let before = fs::read_dir(fixtures_dir()).unwrap().count();
        let first = replay_bytes(&fixture.trace).unwrap();
        let second = replay_bytes(&fixture.trace).unwrap();
        assert_eq!(first, second, "{} replay bytes drifted", fixture.name);
        assert_eq!(before, fs::read_dir(fixtures_dir()).unwrap().count());
        let report: ReplayReport = serde_json::from_slice(&first).unwrap();
        assert_eq!(report.steps.len(), 1);
    }
}

#[test]
fn crash_boundaries_and_reordered_duplicate_acks_are_logically_exactly_once() {
    let mut fixture = load_fixtures()
        .into_iter()
        .find(|fixture| fixture.name == "target_moved_during_finish")
        .unwrap();
    fixture.trace.ruleset = PlannerRuleset::Corrected;
    let issued = replay_daemon(&fixture.trace).unwrap();
    let effect = issued.steps[0].effects[0].clone();

    let mut trace = fixture.trace.clone();
    trace
        .observations
        .push(worksgood::service::ObservationEnvelope {
            sequence: 2,
            logical_time: 101,
            observation: worksgood::service::Observation::Crash,
        });
    let mut repeated = trace.observations[0].clone();
    repeated.sequence = 3;
    repeated.logical_time = 102;
    trace.observations.push(repeated);
    for sequence in [4, 5] {
        trace
            .observations
            .push(worksgood::service::ObservationEnvelope {
                sequence,
                logical_time: 102 + sequence,
                observation: worksgood::service::Observation::EffectAcknowledged {
                    effect_id: effect.effect_id.clone(),
                    outcome: worksgood::service::PlannerAckOutcome::Succeeded,
                },
            });
    }
    let report = replay_daemon(&trace).unwrap();
    assert_eq!(
        report
            .steps
            .iter()
            .flat_map(|step| &step.effects)
            .filter(|candidate| candidate.effect_id == effect.effect_id)
            .count(),
        1
    );
    assert_eq!(report.final_state.effects.len(), 1);
}

#[test]
fn two_tasks_two_attempts_stale_and_current_effects_never_alias() {
    let fixture = load_fixtures()
        .into_iter()
        .find(|fixture| fixture.name == "target_moved_during_finish")
        .unwrap();
    let first_observation = fixture.trace.observations[0].clone();
    let mut second_observation = first_observation.clone();
    second_observation.sequence = 2;
    if let worksgood::service::Observation::Task(task) = &mut second_observation.observation {
        task.key.task_id = worksgood::service::PlannerOpaqueId::new("task-current").unwrap();
        task.key.attempt_id = worksgood::service::PlannerOpaqueId::new("attempt-current").unwrap();
        task.key.fence += 1;
    }
    let mut trace = fixture.trace;
    trace.ruleset = PlannerRuleset::Corrected;
    trace.observations = vec![first_observation, second_observation];
    let report = replay_daemon(&trace).unwrap();
    let effects = report
        .steps
        .iter()
        .flat_map(|step| step.effects.iter())
        .collect::<Vec<&PlannedEffect>>();
    assert_eq!(effects.len(), 2);
    assert_ne!(effects[0].effect_id, effects[1].effect_id);
    assert_ne!(effects[0].task, effects[1].task);
}

#[test]
fn failed_prerequisite_replays_are_byte_identical_and_forward_exhaustive() {
    let fixtures = failed_prerequisite_fixtures();
    assert_eq!(fixtures.len(), 4);
    for fixture in fixtures {
        let first = replay_bytes(&fixture.trace).unwrap();
        let second = replay_bytes(&fixture.trace).unwrap();
        assert_eq!(first, second, "{} replay bytes drifted", fixture.name);
        let report: ReplayReport = serde_json::from_slice(&first).unwrap();
        assert!(report.steps[0].violations.is_empty(), "{}", fixture.name);
        let task = report.final_state.tasks.values().next().unwrap();
        let failed = task.failed_prerequisite.as_ref().unwrap();
        assert_eq!(failed.class, fixture.expected_class);
        assert_eq!(
            report.steps[0].effects.first().map(|effect| effect.action),
            fixture.expected_action,
            "{} action drifted",
            fixture.name
        );
        assert_eq!(
            task.external_wait.as_ref().map(|wait| wait.kind),
            fixture.expected_wait,
            "{} wait drifted",
            fixture.name
        );
        assert_eq!(
            report.steps[0]
                .effects
                .first()
                .and_then(|effect| effect.prerequisite.as_ref()),
            fixture.expected_action.as_ref().map(|_| &failed.source),
            "{} lost exact prerequisite binding",
            fixture.name
        );
    }
}

#[test]
fn nonsemantic_budget_property_emits_exactly_one_retry_or_reconciliation_while_semantic_never_retries()
 {
    for mut fixture in failed_prerequisite_fixtures() {
        for automatic_retries in 0..=3 {
            for max_automatic_retries in 0..=3 {
                let task = fixture
                    .trace
                    .observations
                    .iter_mut()
                    .find_map(|entry| match &mut entry.observation {
                        worksgood::service::Observation::Task(task) => Some(task),
                        _ => None,
                    })
                    .unwrap();
                let class = {
                    let failed = task.failed_prerequisite.as_mut().unwrap();
                    failed.automatic_retries = automatic_retries;
                    failed.max_automatic_retries = max_automatic_retries;
                    failed.class
                };
                let report = replay_daemon(&fixture.trace).unwrap();
                let effects = &report.steps[0].effects;
                if class == PlannerFailedPrerequisiteClass::SemanticValidationRejected {
                    assert!(effects.is_empty(), "semantic rejection retried");
                    assert!(
                        report
                            .final_state
                            .tasks
                            .values()
                            .next()
                            .unwrap()
                            .external_wait
                            .is_some()
                    );
                } else {
                    assert_eq!(effects.len(), 1);
                    let expected = if automatic_retries < max_automatic_retries {
                        match class {
                            PlannerFailedPrerequisiteClass::ProviderUnavailableAfterDurableCandidate => PlannerActionKind::ReplanFinish,
                            PlannerFailedPrerequisiteClass::SourceExecutionNoProgress
                            | PlannerFailedPrerequisiteClass::SourceExecutionWithProgress
                            | PlannerFailedPrerequisiteClass::OrphanBeforeSpawn => PlannerActionKind::RetryFailedPrerequisite,
                            PlannerFailedPrerequisiteClass::SemanticValidationRejected => unreachable!(),
                        }
                    } else {
                        PlannerActionKind::RecordNeedsReconciliation
                    };
                    assert_eq!(effects[0].action, expected);
                }
            }
        }
    }
}

#[test]
fn fixture_codes_cover_all_required_incidents() {
    let observed = load_fixtures()
        .into_iter()
        .flat_map(|fixture| {
            fixture
                .trace
                .observations
                .into_iter()
                .flat_map(|entry| match entry.observation {
                    worksgood::service::Observation::Task(task) => task.incidents,
                    _ => Default::default(),
                })
        })
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        PlannerIncidentCode::ExitedWrapperRejectedStale,
        PlannerIncidentCode::ReopenBeforeOwnerRelease,
        PlannerIncidentCode::ParkResumeOverlap,
        PlannerIncidentCode::ObsoleteDaemonChatCreationLostResponse,
        PlannerIncidentCode::TargetMovedDuringFinish,
        PlannerIncidentCode::SurpriseArchivalBacklog,
        PlannerIncidentCode::ControlPlaneCandidateReplacement,
        PlannerIncidentCode::DeadPiOwnerRetainingLeases,
        PlannerIncidentCode::AbandonedDependencySatisfiedReadiness,
    ]
    .into_iter()
    .collect();
    assert_eq!(observed, expected);
}
