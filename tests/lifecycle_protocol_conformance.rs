use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use worksgood::finalization::FinalizationPhase;
use worksgood::lifecycle_protocol::{
    Candidate, Capability, Decision, Event, LIFECYCLE_WIRE_VERSION, PendingAction, RejectReason,
    State, SuccessfulDisposition, TaskPhase, TraceFixture, reduce, replay,
};
use worksgood::service::convergence::EXITED_WORKER_FINISH_REDUCER_VERSION;
use worksgood::service::{
    FinishConvergenceSnapshot, WrapperChildCapability as RuntimeCapability,
    reduce_exited_worker_finish,
};

#[derive(Debug, Deserialize)]
struct RuntimeFixture {
    reducer_version: u32,
    capability: RuntimeCapability,
    cases: Vec<RuntimeCase>,
}

#[derive(Debug, Deserialize)]
struct RuntimeCase {
    name: String,
    #[serde(default)]
    presented_capability: Option<RuntimeCapability>,
    owner_proven_dead: bool,
    completion_receipted: bool,
    transaction_phase: Option<FinalizationPhase>,
    now_unix: i64,
    expected: serde_json::Value,
}

fn cap() -> Capability {
    Capability {
        task_id: "fix-candidate-wg-control-plane-destruction".into(),
        generation: 0,
        attempt_id: "attempt-0-1".into(),
        fence: 1,
        wrapper_epoch: 1,
        child_epoch: 1,
        wrapper_identity_digest: "b3:wrapper-attempt-0-1".into(),
        child_identity_digest: "b3:native-pi-attempt-0-1".into(),
        owned_child: true,
    }
}

fn stale_cap() -> Capability {
    Capability {
        task_id: "fix-candidate-wg-control-plane-destruction".into(),
        generation: 0,
        attempt_id: "attempt-0-0".into(),
        fence: 0,
        wrapper_epoch: 9,
        child_epoch: 9,
        wrapper_identity_digest: "b3:unrelated-wrapper".into(),
        child_identity_digest: "b3:unrelated-child".into(),
        owned_child: true,
    }
}

fn candidate() -> Candidate {
    Candidate {
        id: "candidate:b3:wip-exact".into(),
        base_cas: "base:b3:main-0".into(),
        protected_free: true,
    }
}

fn validate() -> Event {
    Event::CandidateValidated {
        caller: cap(),
        candidate: candidate(),
    }
}

fn settle() -> Event {
    Event::ChildSettled {
        caller: cap(),
        candidate: candidate(),
        deadline: 100,
    }
}

fn begin(disposition: SuccessfulDisposition) -> Event {
    Event::BeginFinish {
        caller: cap(),
        disposition,
    }
}

fn handoff(disposition: SuccessfulDisposition) -> Event {
    Event::WrapperHandoff {
        caller: cap(),
        disposition,
        deadline: 101,
    }
}

fn promote(current_base_cas: &str) -> Event {
    Event::Promote {
        caller: cap(),
        candidate_id: candidate().id,
        base_cas: candidate().base_cas,
        current_base_cas: current_base_cas.into(),
    }
}

fn cleanup() -> Event {
    Event::CommitCleanup { caller: cap() }
}

fn fixture(name: &str, events: Vec<Event>) -> TraceFixture {
    let initial = State::initial(cap());
    let (final_state, decisions) = replay(&initial, &events);
    TraceFixture {
        wire_version: LIFECYCLE_WIRE_VERSION,
        name: name.into(),
        initial,
        events,
        expected_decisions: decisions,
        expected_final: final_state.normalized(),
    }
}

fn fixtures() -> Vec<TraceFixture> {
    let mut fixtures = Vec::new();
    for (name, disposition) in [
        ("happy_land", SuccessfulDisposition::Land),
        ("happy_deliver", SuccessfulDisposition::Deliver),
        ("happy_report", SuccessfulDisposition::Report),
    ] {
        fixtures.push(fixture(
            name,
            vec![
                validate(),
                settle(),
                handoff(disposition),
                promote("base:b3:main-0"),
                cleanup(),
            ],
        ));
    }

    // Exact production topology: wrapper owns native child; the child settles
    // and is observed dead; no tx exists; the exact wrapper handoff is valid.
    fixtures.push(fixture(
        "incident_attempt_0_1_wrapper_handoff",
        vec![
            validate(),
            settle(),
            handoff(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            cleanup(),
        ],
    ));

    fixtures.push(fixture(
        "stale_unrelated_caller",
        vec![Event::WrapperHandoff {
            caller: stale_cap(),
            disposition: SuccessfulDisposition::Land,
            deadline: 101,
        }],
    ));
    fixtures.push(fixture(
        "owner_death_same_session_continuation",
        vec![
            Event::OwnerProvenDead {
                caller: cap(),
                truthful: true,
                deadline: 101,
            },
            Event::ResumeSame {
                caller: cap(),
                new_wrapper_epoch: 2,
                new_child_epoch: 2,
            },
        ],
    ));
    fixtures.push(fixture(
        "lost_finish_response",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            cleanup(),
            cleanup(),
        ],
    ));
    fixtures.push(fixture(
        "cas_target_movement",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            promote("base:b3:moved"),
        ],
    ));
    fixtures.push(fixture(
        "crash_before_tx",
        vec![
            validate(),
            settle(),
            Event::Crash,
            begin(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            cleanup(),
        ],
    ));
    fixtures.push(fixture(
        "crash_after_tx",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            Event::Crash,
            promote("base:b3:main-0"),
            cleanup(),
        ],
    ));
    fixtures.push(fixture(
        "crash_after_promotion",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            Event::Crash,
            cleanup(),
        ],
    ));
    fixtures.push(fixture(
        "crash_after_cleanup",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            cleanup(),
            Event::Crash,
        ],
    ));
    fixtures.push(fixture(
        "double_promotion_replay",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Land),
            promote("base:b3:main-0"),
            promote("base:b3:main-0"),
            cleanup(),
        ],
    ));
    fixtures.push(fixture(
        "message_cannot_resurrect",
        vec![
            validate(),
            settle(),
            begin(SuccessfulDisposition::Report),
            promote("base:b3:main-0"),
            cleanup(),
            Event::Message {
                body: "late worker says running".into(),
            },
            Event::CandidateValidated {
                caller: cap(),
                candidate: candidate(),
            },
        ],
    ));
    fixtures.push(fixture(
        "contention_breaker_neutral",
        vec![Event::OwnershipContention {
            caller: stale_cap(),
        }],
    ));
    fixtures
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/fixtures/v1")
}

fn runtime_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("formal/fixtures/runtime/v1/exited_worker_finish.json")
}

#[test]
fn lifecycle_golden_traces_match_reference_reducer() {
    let expected = fixtures();
    let dir = fixture_dir();

    if std::env::var_os("UPDATE_LIFECYCLE_GOLDENS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        for fixture in &expected {
            let path = dir.join(format!("{}.json", fixture.name));
            fs::write(
                path,
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
    assert_eq!(paths.len(), expected.len(), "fixture set drift");

    for path in paths {
        let fixture: TraceFixture = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(fixture.wire_version, LIFECYCLE_WIRE_VERSION, "{path:?}");
        let (actual, decisions) = replay(&fixture.initial, &fixture.events);
        assert_eq!(decisions, fixture.expected_decisions, "{path:?}");
        assert_eq!(actual.normalized(), fixture.expected_final, "{path:?}");
        assert!(expected.contains(&fixture), "unexpected golden {path:?}");
    }
}

#[test]
fn production_exited_worker_reducer_matches_runtime_golden() {
    let path = runtime_fixture_path();
    let fixture: RuntimeFixture = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        fixture.reducer_version,
        EXITED_WORKER_FINISH_REDUCER_VERSION
    );

    // The formal incident and production reducer capability are byte-identical,
    // including the final `fence` spelling and both process identity digests.
    assert_eq!(
        serde_json::to_value(&fixture.capability).unwrap(),
        serde_json::to_value(cap()).unwrap()
    );

    for case in fixture.cases {
        let actual = reduce_exited_worker_finish(&FinishConvergenceSnapshot {
            presented_capability: case
                .presented_capability
                .unwrap_or_else(|| fixture.capability.clone()),
            authoritative_capability: fixture.capability.clone(),
            owner_proven_dead: case.owner_proven_dead,
            completion_receipted: case.completion_receipted,
            transaction_phase: case.transaction_phase,
            now_unix: case.now_unix,
        });
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            case.expected,
            "runtime convergence case {}",
            case.name
        );
    }
}

#[test]
fn invalid_rules_are_rejected_and_rejections_are_inert() {
    let initial = State::initial(cap());
    let (stale, decision) = reduce(
        &initial,
        &Event::CandidateValidated {
            caller: stale_cap(),
            candidate: candidate(),
        },
    );
    assert_eq!(stale, initial);
    assert!(matches!(decision, Decision::Rejected(_)));

    let mut unsafe_candidate = candidate();
    unsafe_candidate.protected_free = false;
    let (unsafe_projection, decision) = reduce(
        &initial,
        &Event::CandidateValidated {
            caller: cap(),
            candidate: unsafe_candidate,
        },
    );
    assert_eq!(unsafe_projection, initial);
    assert_eq!(
        decision,
        Decision::Rejected(RejectReason::CandidateNotProtected)
    );
}

#[test]
fn topology_replay_and_terminal_mutations_are_exactly_inert() {
    let initial = State::initial(cap());
    let (stale, decision) = reduce(
        &initial,
        &Event::WrapperHandoff {
            caller: stale_cap(),
            disposition: SuccessfulDisposition::Land,
            deadline: 101,
        },
    );
    assert_eq!(stale, initial);
    assert_eq!(decision, Decision::Rejected(RejectReason::StaleCapability));

    let mut zero_epoch = cap();
    zero_epoch.wrapper_epoch = 0;
    let mut malformed = initial.clone();
    malformed.owner = Some(zero_epoch.clone());
    malformed.worktree_lease = Some(zero_epoch.clone());
    malformed.session_lease = Some(zero_epoch.clone());
    let (malformed_after, decision) = reduce(
        &malformed,
        &Event::CandidateValidated {
            caller: zero_epoch.clone(),
            candidate: candidate(),
        },
    );
    let (malformed_after, handoff_decision) = reduce(
        &malformed_after,
        &Event::WrapperHandoff {
            caller: zero_epoch,
            disposition: SuccessfulDisposition::Land,
            deadline: 101,
        },
    );
    assert_eq!(decision, Decision::Applied);
    assert_eq!(malformed_after.accepted_candidate, Some(candidate()));
    assert!(malformed_after.finish_tx.is_none());
    assert_eq!(
        handoff_decision,
        Decision::Rejected(RejectReason::InvalidTopology)
    );

    let events = vec![
        validate(),
        settle(),
        begin(SuccessfulDisposition::Land),
        promote("base:b3:main-0"),
    ];
    let (promoted, _) = replay(&initial, &events);
    let (replayed, decision) = reduce(&promoted, &promote("base:b3:main-0"));
    assert_eq!(replayed, promoted);
    assert_eq!(decision, Decision::Noop);
    assert_eq!(replayed.promotion_count, 1);

    let (done, _) = reduce(&replayed, &cleanup());
    assert_eq!(done.phase, TaskPhase::Done);
    assert!(done.finish_lease.is_none());
    for late in [
        Event::Message {
            body: "late resurrection".into(),
        },
        validate(),
        Event::Fail { caller: cap() },
    ] {
        let (after, decision) = reduce(&done, &late);
        assert_eq!(after, done);
        assert_eq!(decision, Decision::Noop);
    }
}

#[test]
fn convergence_cuts_preserve_identity_and_decrease_rank() {
    let initial = State::initial(cap());
    let (settled, _) = replay(&initial, &[validate(), settle()]);
    assert_eq!(settled.pending_action, Some(PendingAction::BeginFinish));
    assert_eq!(settled.recovery_rank(), 3);

    let (tx, decision) = reduce(&settled, &begin(SuccessfulDisposition::Land));
    assert_eq!(decision, Decision::Applied);
    assert_eq!(tx.recovery_rank(), 2);
    assert_eq!(tx.finish_lease, Some(cap()));

    let (promoted, decision) = reduce(&tx, &promote("base:b3:main-0"));
    assert_eq!(decision, Decision::Applied);
    assert_eq!(promoted.recovery_rank(), 1);

    let (done, decision) = reduce(&promoted, &cleanup());
    assert_eq!(decision, Decision::Applied);
    assert_eq!(done.recovery_rank(), 0);
    assert!(done.dependency_satisfied());
    assert_eq!(done.promotion_count, 1);

    let (dead, _) = reduce(
        &initial,
        &Event::OwnerProvenDead {
            caller: cap(),
            truthful: true,
            deadline: 101,
        },
    );
    let (continued, decision) = reduce(
        &dead,
        &Event::ResumeSame {
            caller: cap(),
            new_wrapper_epoch: 2,
            new_child_epoch: 2,
        },
    );
    assert_eq!(decision, Decision::Applied);
    let next = continued.owner.as_ref().expect("continued owner");
    assert_eq!(next.task_id, cap().task_id);
    assert_eq!(next.attempt_id, cap().attempt_id);
    assert_eq!(next.generation, cap().generation);
    assert_eq!(next.fence, cap().fence);
    assert_eq!(continued.worktree_lease.as_ref(), Some(next));
    assert_eq!(continued.session_lease.as_ref(), Some(next));
    assert_eq!(continued.breaker_charges, 0);
}
