use std::fs;
use std::path::{Path, PathBuf};

use worksgood::completion_evidence::{AttemptSaveKey, EvidenceBinding};
use worksgood::lifecycle_protocol::{
    SaveTraceDecisionV2, SaveTraceFixtureV2, SaveTraceStepV2, replay_save_trace_v2,
};
use worksgood::save_transaction::{
    SAVE_TRANSACTION_SCHEMA_VERSION, SaveFact, SavePhase, SaveTransactionState,
    SaveTransitionRequest,
};

fn source() -> AttemptSaveKey {
    AttemptSaveKey {
        graph_id: "graph:v2:golden".into(),
        task_id: "atomic-save-formal-rust-traces".into(),
        generation: 2,
        attempt_id: "attempt-2-1".into(),
        attempt_fence: 7,
        worktree_lease_epoch: 3,
        process_epoch: 1,
        wrapper_epoch: 1,
        route_snapshot_cid: "wgcid:v2:route".into(),
        session_proof_digest: "b3:session".into(),
        worktree_identity_digest: "b3:root".into(),
    }
}

fn binding() -> EvidenceBinding {
    EvidenceBinding {
        source: source(),
        candidate_id: "candidate:b3:exact".into(),
        base_commit_oid: "base:b3:main".into(),
    }
}

fn request(
    revision: u64,
    phase: SavePhase,
    next_phase: SavePhase,
    key: &str,
    fact: SaveFact,
) -> SaveTransitionRequest {
    SaveTransitionRequest {
        source: source(),
        expected_revision: revision,
        expected_phase: phase,
        next_phase,
        idempotency_key: key.into(),
        action_key: format!("action:{key}"),
        fact,
    }
}

fn evidence(phase: SavePhase, key: &str) -> SaveFact {
    SaveFact::Evidence {
        cid: format!("wgcid:v2:{key}"),
        binding: (phase >= SavePhase::WorkSaved && phase <= SavePhase::CleanupCommitted)
            .then(binding),
    }
}

fn fixture(name: &str, requests: Vec<SaveTransitionRequest>) -> SaveTraceFixtureV2 {
    let initial = SaveTransactionState::new(source()).unwrap();
    let mut value = SaveTraceFixtureV2 {
        schema_version: SAVE_TRANSACTION_SCHEMA_VERSION,
        name: name.into(),
        initial: initial.clone(),
        steps: requests
            .into_iter()
            .map(|request| SaveTraceStepV2 {
                request,
                expected: SaveTraceDecisionV2::Applied,
            })
            .collect(),
        expected_final: initial,
    };
    let (final_state, decisions) = replay_save_trace_v2(&value);
    for (step, decision) in value.steps.iter_mut().zip(decisions) {
        step.expected = decision;
    }
    value.expected_final = final_state;
    value
}

fn fixtures() -> Vec<SaveTraceFixtureV2> {
    use SavePhase::*;
    let normal = [
        Prepared,
        Quiescing,
        WorkSaved,
        CandidateSealed,
        Validated,
        Accepted,
        DispositionRecorded,
        EffectPrepared,
        EffectCommitted,
        CleanupPrepared,
        CleanupCommitted,
    ];
    let mut phase = Absent;
    let happy = normal
        .into_iter()
        .enumerate()
        .map(|(index, next)| {
            let result = request(
                index as u64,
                phase,
                next,
                &format!("step-{index}"),
                evidence(next, &format!("step-{index}")),
            );
            phase = next;
            result
        })
        .collect();

    let intent = request(0, Absent, Prepared, "intent", evidence(Prepared, "intent"));
    let mut conflicting = intent.clone();
    conflicting.fact = evidence(Prepared, "different-intent");
    let mut stale_source = request(0, Absent, Prepared, "stale", evidence(Prepared, "stale"));
    stale_source.source.attempt_fence += 1;

    vec![
        fixture("happy_cleanup_boundary_v2", happy),
        fixture(
            "exact_duplicate_and_conflict_v2",
            vec![intent.clone(), intent, conflicting],
        ),
        fixture("stale_source_v2", vec![stale_source]),
        fixture(
            "illegal_phase_skip_v2",
            vec![request(
                0,
                Absent,
                WorkSaved,
                "skip",
                evidence(WorkSaved, "skip"),
            )],
        ),
        fixture(
            "legacy_unproven_hold_v2",
            vec![request(
                0,
                Absent,
                NeedsReconciliation,
                "legacy",
                SaveFact::Hold {
                    reason: "legacy Done has no complete GraphSave".into(),
                },
            )],
        ),
    ]
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/fixtures/v2")
}

#[test]
fn committed_v2_save_traces_match_the_production_kernel() {
    let expected = fixtures();
    let dir = fixture_dir();
    if std::env::var_os("UPDATE_SAVE_TRANSACTION_GOLDENS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        for fixture in &expected {
            fs::write(
                dir.join(format!("{}.json", fixture.name)),
                format!("{}\n", serde_json::to_string_pretty(fixture).unwrap()),
            )
            .unwrap();
        }
    }

    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), expected.len(), "v2 fixture set drift");

    for path in paths {
        let fixture: SaveTraceFixtureV2 =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(fixture.schema_version, SAVE_TRANSACTION_SCHEMA_VERSION);
        let (actual, decisions) = replay_save_trace_v2(&fixture);
        assert_eq!(actual, fixture.expected_final, "{path:?}");
        assert_eq!(
            decisions,
            fixture
                .steps
                .iter()
                .map(|step| step.expected.clone())
                .collect::<Vec<_>>(),
            "{path:?}"
        );
        assert!(expected.contains(&fixture), "unexpected v2 golden {path:?}");
    }
}

#[test]
fn destructive_and_success_boundaries_fail_closed() {
    use SavePhase::*;
    let state = SaveTransactionState::new(source()).unwrap();
    for forbidden in [
        EffectPrepared,
        CleanupPrepared,
        CleanupCommitted,
        GraphSaved,
    ] {
        let trace = fixture(
            "forbidden",
            vec![request(
                0,
                Absent,
                forbidden,
                "forbidden",
                evidence(forbidden, "forbidden"),
            )],
        );
        assert_eq!(trace.expected_final, state);
        assert!(matches!(
            trace.steps[0].expected,
            SaveTraceDecisionV2::Rejected { .. }
        ));
    }
}
