use tempfile::tempdir;
use worksgood::pi_watchdog::*;

fn fixture(now: i64) -> PiWatchdog {
    let dir = tempdir().unwrap().keep();
    let source = SourceTuple {
        task_id: "task-a".into(),
        generation: 2,
        attempt_id: "attempt-2-7".into(),
        attempt_fence: 11,
        worktree_lease_epoch: 11,
        worktree_path: dir.join("worktree"),
    };
    std::fs::create_dir_all(&source.worktree_path).unwrap();
    let route = RouteSnapshot {
        handler: "pi".into(),
        provider: "fake".into(),
        model: "slow-free".into(),
        reasoning: Some("high".into()),
        endpoint_redacted: "fake://local".into(),
        endpoint_hmac: "endpoint-hmac".into(),
        qos: QosClass::Free,
        pi_binary_digest: "pi-bin".into(),
        plugin_digest: "plugin".into(),
    };
    let session = SessionProof {
        session_id: "session-1".into(),
        branch_leaf: "leaf-1".into(),
        session_dir: dir.join("sessions"),
        session_file: dir.join("sessions/session-1.jsonl"),
        header_digest: "header".into(),
        append_prefix_digest: "prefix".into(),
        append_prefix_len: 1,
    };
    std::fs::create_dir_all(&session.session_dir).unwrap();
    std::fs::write(
        &session.session_file,
        "{\"type\":\"session\",\"id\":\"session-1\"}\n",
    )
    .unwrap();
    let process = ProcessIdentity {
        pid: 123,
        pgid: 123,
        start_ticks: 456,
        boot_id: "boot".into(),
        nonce: "nonce".into(),
    };
    PiWatchdog::new(
        source,
        route,
        session,
        process,
        TestPolicy::ordered().into(),
        now,
    )
    .unwrap()
}

#[test]
fn soft_and_hard_are_independent() {
    let mut w = fixture(0);
    w.observe(
        Observation::ProviderRequestStarted {
            call_id: "p1".into(),
        },
        0,
    )
    .unwrap();
    assert_eq!(w.tick(299).unwrap(), vec![]);
    assert_eq!(w.state().classification, Classification::Active);
    assert_eq!(w.tick(300).unwrap(), vec![ActionKind::ReadOnlyProbe]);
    assert_eq!(w.state().classification, Classification::Suspect);
    assert!(w.tick(480).unwrap().is_empty());
    assert_eq!(w.state().classification, Classification::Suspect);
    assert!(w.tick(899).unwrap().is_empty());
    assert_eq!(w.tick(900).unwrap(), vec![ActionKind::StartHardGrace]);
    assert_eq!(w.state().classification, Classification::HardResumeEligible);
    assert!(w.tick(959).unwrap().is_empty());
    w.observe(
        Observation::ProbeObserved {
            progress_seq: 1,
            session_leaf: "leaf-1".into(),
            alive: true,
        },
        960,
    )
    .unwrap();
    let actions = w.tick(960).unwrap();
    assert_eq!(
        actions,
        vec![
            ActionKind::ReserveContinuation,
            ActionKind::FenceExactProcess
        ]
    );
}

#[test]
fn meaningful_progress_only_resets_clock_and_long_runtime_is_safe() {
    let mut w = fixture(0);
    w.observe(
        Observation::ProviderRequestStarted {
            call_id: "p1".into(),
        },
        0,
    )
    .unwrap();
    for (at, observation) in [
        (250, Observation::ThinkingDelta),
        (500, Observation::TokenDelta { tokens: 1 }),
        (
            750,
            Observation::SessionAdvanced {
                leaf: "leaf-2".into(),
                prefix_digest: "p2".into(),
            },
        ),
        (
            1000,
            Observation::WorktreeProgress {
                manifest_digest: "m2".into(),
            },
        ),
        (
            1200,
            Observation::ToolProgress {
                tool_call_id: "t1".into(),
                progress: 1,
            },
        ),
    ] {
        w.observe(observation, at).unwrap();
        assert_eq!(w.state().classification, Classification::Active);
    }
    for observation in [
        Observation::Heartbeat,
        Observation::StatusPolled,
        Observation::OrdinaryMessage,
        Observation::ProbeTraffic,
    ] {
        w.observe(observation, 1250).unwrap();
    }
    assert_eq!(w.state().last_meaningful_at, 1200);
    assert!(w.tick(1499).unwrap().is_empty());
}

#[test]
fn settled_and_every_exit_need_finalization_not_terminal() {
    for observation in [
        Observation::AgentSettled,
        Observation::ProcessExited {
            status: ExitStatus::Code(0),
            reaped: true,
        },
        Observation::ProcessExited {
            status: ExitStatus::Code(9),
            reaped: true,
        },
        Observation::ProcessExited {
            status: ExitStatus::Signal(15),
            reaped: true,
        },
        Observation::PipeEof { reaped: true },
    ] {
        let mut w = fixture(0);
        let actions = w.observe(observation, 2).unwrap();
        assert_eq!(w.state().classification, Classification::NeedsFinalization);
        assert!(!w.state().terminal);
        assert_eq!(
            actions,
            vec![
                ActionKind::ReserveContinuation,
                ActionKind::LaunchSameSession,
                ActionKind::AppendCompletionPrompt
            ]
        );
        assert!(
            w.observe(Observation::ReplayPendingActions, 3)
                .unwrap()
                .is_empty()
        );
        assert_eq!(w.state().prompt_count, 1);
    }
}

#[test]
fn unsafe_effect_unknown_phase_wait_and_long_tool_never_auto_kill() {
    let mut unknown = fixture(0);
    unknown.observe(Observation::PhaseUnknown, 0).unwrap();
    assert_eq!(unknown.tick(300).unwrap(), vec![ActionKind::ReadOnlyProbe]);
    assert!(unknown.tick(10_000).unwrap().is_empty());
    assert_eq!(
        unknown.state().classification,
        Classification::StalledOperatorRequired
    );

    let mut unsafe_exit = fixture(0);
    unsafe_exit
        .observe(
            Observation::ToolIntent {
                contract: ToolContract::non_idempotent("danger"),
            },
            0,
        )
        .unwrap();
    assert!(
        unsafe_exit
            .observe(
                Observation::ProcessExited {
                    status: ExitStatus::Code(1),
                    reaped: true
                },
                1
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        unsafe_exit.state().classification,
        Classification::StalledOperatorRequired
    );

    let mut waiting = fixture(0);
    waiting
        .observe(
            Observation::WaitAccepted {
                correlation: "answer-1".into(),
            },
            0,
        )
        .unwrap();
    assert!(waiting.tick(100_000).unwrap().is_empty());
    assert_eq!(waiting.state().classification, Classification::WaitingUser);

    let mut tool = fixture(0);
    tool.observe(
        Observation::ToolIntent {
            contract: ToolContract::read_only("scan", 20_000),
        },
        0,
    )
    .unwrap();
    assert!(tool.tick(10_000).unwrap().is_empty());
    assert_eq!(tool.state().classification, Classification::LongTool);
}

#[test]
fn first_terminal_wins_and_old_epoch_is_late_evidence() {
    let mut w = fixture(0);
    let receipt =
        TerminalIntentReceipt::new(&w, 1, "call-done", TerminalDisposition::SuccessIntent);
    assert_eq!(
        w.observe(Observation::TerminalIntent(receipt.clone()), 10)
            .unwrap(),
        vec![ActionKind::QuiesceForFinalization]
    );
    assert!(w.state().terminal);
    assert!(w.observe(Observation::AgentSettled, 11).unwrap().is_empty());
    assert!(
        w.observe(Observation::TerminalIntent(receipt), 12)
            .unwrap()
            .is_empty()
    );
    let fail = TerminalIntentReceipt::new(&w, 1, "call-fail", TerminalDisposition::Failure);
    assert_eq!(
        w.observe(Observation::TerminalIntent(fail), 13)
            .unwrap_err()
            .code,
        "attempt_already_terminal"
    );
}

#[test]
fn proof_mismatch_budget_and_pid_reuse_hold() {
    for guard in [
        GuardFailure::Session,
        GuardFailure::Route,
        GuardFailure::Worktree,
        GuardFailure::PidIdentity,
        GuardFailure::Containment,
    ] {
        let mut w = fixture(0);
        w.observe(
            Observation::ProviderRequestStarted {
                call_id: "p".into(),
            },
            0,
        )
        .unwrap();
        w.tick(300).unwrap();
        w.tick(900).unwrap();
        w.observe(Observation::GuardFailure(guard), 960).unwrap();
        assert!(w.tick(960).unwrap().is_empty());
        assert_eq!(
            w.state().classification,
            Classification::StalledOperatorRequired
        );
        assert_eq!(w.state().process_epoch, 1);
    }
    let mut budget = fixture(0);
    budget.state_mut_for_test().epochs_used = 3;
    budget
        .observe(
            Observation::ProviderRequestStarted {
                call_id: "p".into(),
            },
            0,
        )
        .unwrap();
    budget.tick(300).unwrap();
    budget.tick(900).unwrap();
    budget
        .observe(
            Observation::ProbeObserved {
                progress_seq: 1,
                session_leaf: "leaf-1".into(),
                alive: true,
            },
            960,
        )
        .unwrap();
    assert!(budget.tick(960).unwrap().is_empty());
    assert_eq!(
        budget.state().reason_code.as_deref(),
        Some("continuation_epoch_budget_exhausted")
    );
}

#[test]
fn restart_replay_is_idempotent_and_budget_never_replenishes() {
    let mut w = fixture(0);
    w.observe(Observation::AgentSettled, 1).unwrap();
    let path = w.state_path().to_path_buf();
    let before = (
        w.state().prompt_count,
        w.state().epochs_used,
        w.state().elapsed_reserved_secs,
    );
    drop(w);
    let mut reopened = PiWatchdog::open(&path).unwrap();
    assert!(
        reopened
            .observe(Observation::ReplayPendingActions, 2)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        (
            reopened.state().prompt_count,
            reopened.state().epochs_used,
            reopened.state().elapsed_reserved_secs
        ),
        before
    );
    assert_eq!(reopened.state().process_epoch, 2);
    assert_eq!(reopened.state().session.session_id, "session-1");
    assert_eq!(reopened.state().source.attempt_id, "attempt-2-7");
}

#[test]
fn prompt_marker_uncertainty_holds_without_duplicate() {
    let mut w = fixture(0);
    w.inject_crash_barrier(CrashBarrier::AfterPromptIntent)
        .unwrap();
    assert!(w.observe(Observation::AgentSettled, 1).is_err());
    let path = w.state_path().to_owned();
    let mut reopened = PiWatchdog::open(&path).unwrap();
    reopened
        .observe(Observation::PromptMarkerUncertain, 2)
        .unwrap();
    assert_eq!(
        reopened.state().classification,
        Classification::StalledOperatorRequired
    );
    assert_eq!(reopened.state().prompt_count, 0);
}

#[test]
fn manual_grant_is_finite_and_charged_once() {
    let mut w = fixture(0);
    w.state_mut_for_test().epochs_used = 3;
    w.manual_resume(
        ManualGrant {
            action_id: "grant-1".into(),
            reason: "operator inspected".into(),
            epochs: 1,
            elapsed_secs: 600,
            effect_ack: None,
        },
        5,
    )
    .unwrap();
    assert_eq!(w.state().manual_epochs_granted, 1);
    assert_eq!(w.state().manual_elapsed_granted_secs, 600);
    w.manual_resume(
        ManualGrant {
            action_id: "grant-1".into(),
            reason: "operator inspected".into(),
            epochs: 1,
            elapsed_secs: 600,
            effect_ack: None,
        },
        6,
    )
    .unwrap();
    assert_eq!(w.state().manual_epochs_granted, 1);
}

#[test]
fn done_proof_is_layered_and_epoch_bound() {
    let mut w = fixture(0);
    let terminal = TerminalIntentReceipt::new(&w, 1, "done", TerminalDisposition::SuccessIntent);
    w.observe(Observation::TerminalIntent(terminal.clone()), 1)
        .unwrap();
    let mut proof = DoneProofV1::default();
    proof.terminal = Some(terminal);
    assert!(!proof.is_complete_for(&w));
    proof.quiescence = Some(w.quiescence_receipt("manifest", 2));
    proof.candidate_checkpoint = Some("candidate".into());
    proof.validation = Some("validation".into());
    proof.evaluation = Some("evaluation".into());
    assert!(!proof.is_complete_for(&w));
    proof.finalization_event = Some("finalize-event".into());
    assert!(proof.is_complete_for(&w));
}

#[test]
fn continuation_does_not_touch_other_domains() {
    let mut w = fixture(0);
    let before = w.state().domain_counters.clone();
    w.observe(Observation::AgentSettled, 1).unwrap();
    assert_eq!(w.state().domain_counters, before);
    assert_eq!(w.state().source.generation, 2);
    assert_eq!(w.state().source.attempt_fence, 11);
    assert_eq!(w.state().source.worktree_lease_epoch, 11);
}

#[test]
fn lifecycle_kernel_alone_defers_authorized_pi_exit_and_orders_terminal_cas() {
    use worksgood::graph::{Status, Task};
    use worksgood::lifecycle::*;
    let mut task = Task {
        id: "kernel-pi".into(),
        title: "kernel-pi".into(),
        status: Status::Open,
        ..Task::default()
    };
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::AttemptReserved {
                owner_id: Some("worker".into()),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "dispatcher".into(),
            },
            "spawn",
            "reserve",
        ),
    )
    .unwrap();
    let expected = FenceExpectation::current(&task);
    let attempt = task.lifecycle.current_attempt.clone().unwrap();
    let auth = PiContinuationAuthorization {
        authorization_id: "auth-1".into(),
        task_id: task.id.clone(),
        generation: 0,
        attempt_id: attempt.id.clone(),
        attempt_fence: attempt.fence,
        worktree_lease_epoch: attempt.fence,
        session_proof_digest: "session".into(),
        route_snapshot_digest: "route".into(),
        state: PiAuthorizationState::Active,
        max_replacement_epochs: 3,
        max_reserved_elapsed_secs: 1800,
        epochs_used: 0,
        elapsed_reserved_secs: 0,
        issued_by_policy: "static-v1".into(),
    };
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiContinuationAuthorized {
                authorization: auth,
                initial_process_epoch: 1,
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "dispatcher".into(),
            },
            "pi_authorized",
            "auth",
        )
        .expecting(expected),
    )
    .unwrap();
    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiProcessEpochExited {
                process_epoch: 1,
                exact_reap_proof: true,
                effect_safe: true,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "needs_finalization_exit",
            "exit-1",
        )
        .expecting(expected),
    )
    .unwrap();
    assert_eq!(
        task.status,
        Status::InProgress,
        "authorized Pi exit must remain pre-terminal"
    );

    let source = SourceTuple {
        task_id: task.id.clone(),
        generation: 0,
        attempt_id: attempt.id.clone(),
        attempt_fence: attempt.fence,
        worktree_lease_epoch: attempt.fence,
        worktree_path: "/tmp/w".into(),
    };
    let receipt = TerminalIntentReceipt {
        task_id: source.task_id,
        generation: source.generation,
        attempt_id: source.attempt_id,
        attempt_fence: source.attempt_fence,
        process_epoch: 1,
        tool_call_id: "done-call".into(),
        disposition: TerminalDisposition::SuccessIntent,
        idempotency_key: "done-call".into(),
    };
    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiTerminalIntent { receipt },
            LifecycleActor {
                kind: ActorKind::Worker,
                id: "worker".into(),
            },
            "success_intent",
            "terminal",
        )
        .expecting(expected),
    )
    .unwrap();
    assert_eq!(
        task.status,
        Status::InProgress,
        "success intent is not Done"
    );
    let expected = FenceExpectation::current(&task);
    let error = apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiContinuationEpochReserved {
                expected_process_epoch: 1,
                next_process_epoch: 2,
                elapsed_charge_secs: 600,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "hard_resume",
            "reserve-epoch",
        )
        .expecting(expected),
    )
    .unwrap_err();
    assert_eq!(error.code, "attempt_already_terminal");
}
