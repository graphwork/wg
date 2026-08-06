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

fn verified_compaction(w: &PiWatchdog, entry: &str, parent: &str) -> VerifiedCompactionOccurrence {
    VerifiedCompactionOccurrence {
        graph_id: "wggraph:v1:test".into(),
        compaction_entry_id: entry.into(),
        compaction_parent_id: parent.into(),
        compaction_entry_digest: format!("digest-{entry}"),
        session_id: w.state().session.session_id.clone(),
        session_file_digest: "session-file-digest".into(),
        session_leaf_id: entry.into(),
        native_compaction_event_seq: w.state().compaction_kicks.len() as u64 + 1,
        process_pid: w.state().process.pid,
        process_epoch: w.state().process_epoch,
        process_identity_digest: w.state().process.digest(),
        provider: w.state().route.provider.clone(),
        model: w.state().route.model.clone(),
        reasoning: w.state().route.reasoning.clone(),
        route_snapshot_digest: w.state().route.digest(),
        plugin_compat: w.state().route.plugin_digest.clone(),
        reason: "threshold".into(),
        will_retry: false,
        quiescent: true,
        host_idle: false,
        queue_empty: true,
        tool_clear: true,
    }
}

#[test]
fn threshold_compaction_gap_authorizes_permits_and_acks_exactly_once() {
    let mut w = fixture(0);
    let input = verified_compaction(&w, "compact-1", "assistant-1");
    let action = w.authorize_compaction_kick(input.clone(), 1).unwrap();
    assert_eq!(action.state, PiCompactionKickState::Authorized);
    assert_eq!(w.state().continuation_epoch, 0);

    let duplicate = w.authorize_compaction_kick(input, 2).unwrap();
    assert_eq!(duplicate.action_id, action.action_id);
    assert_eq!(w.state().compaction_kicks.len(), 1);

    let mut reused_native_event = verified_compaction(&w, "compact-forged", "assistant-2");
    reused_native_event.native_compaction_event_seq = action.native_compaction_event_seq;
    assert_eq!(
        w.authorize_compaction_kick(reused_native_event, 2)
            .unwrap_err()
            .code,
        "compaction_native_event_reused"
    );

    let permit = w
        .permit_compaction_kick(&action.action_id, 1, true, 3)
        .unwrap();
    assert!(permit.fresh_delivery_grant);
    assert_eq!(permit.prompt.as_deref(), Some(COMPACTION_KICK_PROMPT));
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);

    let replay = w
        .permit_compaction_kick(&action.action_id, 1, true, 4)
        .unwrap();
    assert!(!replay.fresh_delivery_grant);
    assert!(replay.prompt.is_none());
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);

    let ack = w
        .acknowledge_compaction_kick(&action.action_id, false, 5)
        .unwrap();
    assert_eq!(ack.state, PiCompactionKickState::Acknowledged);
    w.ingest_native_value(&serde_json::json!({"type":"agent_start"}), 6)
        .unwrap();
    assert_eq!(
        w.compaction_kick(&action.action_id).unwrap().state,
        PiCompactionKickState::Running
    );
    // Settlement closes diagnostics but must not manufacture the legacy
    // session-file prompt or reserve another epoch.
    assert!(
        w.ingest_native_value(&serde_json::json!({"type":"agent_settled"}), 7)
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().prompt_count, 0);
    assert_eq!(
        w.compaction_kick(&action.action_id).unwrap().state,
        PiCompactionKickState::SettledAfterKick
    );
}

#[test]
fn accepted_terminal_after_ack_refreshes_and_acknowledges_abort_exactly_once() {
    let mut w = fixture(0);
    let action = w
        .authorize_compaction_kick(
            verified_compaction(&w, "compact-terminal", "assistant-1"),
            1,
        )
        .unwrap();
    w.permit_compaction_kick(&action.action_id, 1, true, 2)
        .unwrap();
    w.acknowledge_compaction_kick(&action.action_id, false, 3)
        .unwrap();

    // The initial ack predated an accepted lifecycle terminal. The bounded
    // cancellation subscription refreshes that exact action rather than
    // sending or charging again, then records an idempotent abort receipt.
    let refreshed = w
        .acknowledge_compaction_kick(&action.action_id, true, 4)
        .unwrap();
    assert!(refreshed.abort);
    assert_eq!(refreshed.state, PiCompactionKickState::TerminalObserved);
    let aborted = w.abort_ack_compaction_kick(&action.action_id, 5).unwrap();
    assert_eq!(
        aborted.state,
        PiCompactionKickState::TerminalAbortAcknowledged
    );
    let replay = w.abort_ack_compaction_kick(&action.action_id, 6).unwrap();
    assert_eq!(
        replay.state,
        PiCompactionKickState::TerminalAbortAcknowledged
    );
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);
    assert_eq!(w.state().compaction_kicks.len(), 1);
}

#[test]
fn broker_settlement_before_stream_observer_does_not_double_charge() {
    let mut w = fixture(0);
    let action = w
        .authorize_compaction_kick(verified_compaction(&w, "compact-race", "assistant-1"), 1)
        .unwrap();
    w.permit_compaction_kick(&action.action_id, 1, true, 2)
        .unwrap();
    w.acknowledge_compaction_kick(&action.action_id, false, 3)
        .unwrap();
    w.mark_compaction_kick_running(4).unwrap();

    // Pi awaits extension event handlers before the independent file follower
    // necessarily sees the same agent_settled record.
    w.settle_compaction_kick(&action.action_id, 5).unwrap();
    assert!(
        w.ingest_native_value(&serde_json::json!({"type":"agent_settled"}), 6)
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);
    assert_eq!(w.state().prompt_count, 0);
    assert_eq!(w.state().phase, Phase::Settled);
}

#[test]
fn distinct_threshold_compactions_share_finite_epoch_budget_without_attempt_cap() {
    let mut w = fixture(0);
    let first = w
        .authorize_compaction_kick(verified_compaction(&w, "compact-1", "a1"), 1)
        .unwrap();
    w.permit_compaction_kick(&first.action_id, 1, true, 2)
        .unwrap();
    w.acknowledge_compaction_kick(&first.action_id, false, 3)
        .unwrap();

    let second = w
        .authorize_compaction_kick(verified_compaction(&w, "compact-2", "a2"), 4)
        .unwrap();
    assert_ne!(first.occurrence_id, second.occurrence_id);
    assert_ne!(first.action_id, second.action_id);
    let permit = w
        .permit_compaction_kick(&second.action_id, 2, true, 5)
        .unwrap();
    assert!(permit.fresh_delivery_grant);
    assert_eq!(w.state().continuation_epoch, 2);
    assert_eq!(w.state().compaction_kicks.len(), 2);
}

#[test]
fn compaction_kick_must_not_trigger_for_manual_overflow_queue_idle_tool_or_mismatch() {
    let base = fixture(0);
    let mut cases = Vec::new();
    let mut manual = verified_compaction(&base, "c1", "a1");
    manual.reason = "manual".into();
    cases.push(manual);
    let mut overflow = verified_compaction(&base, "c2", "a2");
    overflow.reason = "overflow".into();
    overflow.will_retry = true;
    cases.push(overflow);
    let mut queued = verified_compaction(&base, "c3", "a3");
    queued.queue_empty = false;
    cases.push(queued);
    let mut idle = verified_compaction(&base, "c4", "a4");
    idle.host_idle = true;
    cases.push(idle);
    let mut tool = verified_compaction(&base, "c5", "a5");
    tool.tool_clear = false;
    cases.push(tool);
    let mut route = verified_compaction(&base, "c6", "a6");
    route.model = "different".into();
    cases.push(route);
    let mut process = verified_compaction(&base, "c7", "a7");
    process.process_epoch = 9;
    cases.push(process);

    for input in cases {
        let mut w = fixture(0);
        assert!(w.authorize_compaction_kick(input, 1).is_err());
        assert!(w.state().compaction_kicks.is_empty());
        assert_eq!(w.state().continuation_epoch, 0);
    }
}

#[test]
fn accepted_done_fail_and_wait_receipts_suppress_compaction_authorization() {
    for (index, disposition) in [
        TerminalDisposition::SuccessIntent,
        TerminalDisposition::Failure,
        TerminalDisposition::Park,
    ]
    .into_iter()
    .enumerate()
    {
        let mut w = fixture(0);
        let receipt = TerminalIntentReceipt::new(&w, 1, format!("terminal-{index}"), disposition);
        w.observe(Observation::TerminalIntent(receipt), 1).unwrap();
        let error = w
            .authorize_compaction_kick(
                verified_compaction(&w, &format!("terminal-entry-{index}"), "assistant"),
                2,
            )
            .unwrap_err();
        assert_eq!(error.code, "attempt_already_terminal");
        assert!(w.state().compaction_kicks.is_empty());
        assert_eq!(w.state().continuation_epoch, 0);
    }
}

#[test]
fn shared_epoch_and_elapsed_exhaustion_hold_without_a_kick_record() {
    let mut epochs = fixture(0);
    let epoch_limit = epochs.policy().max_continuation_epochs;
    epochs.state_mut_for_test().epochs_used = epoch_limit;
    let error = epochs
        .authorize_compaction_kick(verified_compaction(&epochs, "epoch-limit", "assistant"), 1)
        .unwrap_err();
    assert_eq!(error.code, "continuation_budget_exhausted");
    assert_eq!(
        epochs.state().classification,
        Classification::StalledOperatorRequired
    );
    assert!(epochs.state().compaction_kicks.is_empty());

    let mut elapsed = fixture(0);
    let elapsed_limit = elapsed.policy().max_continuation_elapsed_secs;
    elapsed.state_mut_for_test().elapsed_reserved_secs = elapsed_limit;
    let error = elapsed
        .authorize_compaction_kick(
            verified_compaction(&elapsed, "elapsed-limit", "assistant"),
            1,
        )
        .unwrap_err();
    assert_eq!(error.code, "continuation_budget_exhausted");
    assert_eq!(
        elapsed.state().classification,
        Classification::StalledOperatorRequired
    );
    assert!(elapsed.state().compaction_kicks.is_empty());
}

#[test]
fn compaction_crash_boundaries_reopen_without_duplicate_delivery_or_charge() {
    let mut w = fixture(0);
    let occurrence = verified_compaction(&w, "crash-entry", "assistant");

    // Crash/restart after durable authorization: replay finds the same action,
    // and the shared continuation budget is still untouched.
    let authorized = w.authorize_compaction_kick(occurrence.clone(), 1).unwrap();
    let path = w.state_path().to_path_buf();
    drop(w);
    let mut w = PiWatchdog::open(&path).unwrap();
    let replayed = w.authorize_compaction_kick(occurrence, 2).unwrap();
    assert_eq!(replayed.action_id, authorized.action_id);
    assert_eq!(w.state().continuation_epoch, 0);

    // Crash/restart after permit persistence/reply: the one durable grant is
    // never fresh again. This also models a crash after the native send but
    // before message_start acknowledgement; restart must not resend.
    let first = w
        .permit_compaction_kick(&authorized.action_id, 1, true, 3)
        .unwrap();
    assert!(first.fresh_delivery_grant);
    drop(w);
    let mut w = PiWatchdog::open(&path).unwrap();
    for now in [4, 5] {
        let replay = w
            .permit_compaction_kick(&authorized.action_id, 1, true, now)
            .unwrap();
        assert!(!replay.fresh_delivery_grant);
        assert!(replay.prompt.is_none());
    }
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);

    // Crash/restart during acknowledgement is idempotent and preserves the
    // selected action without creating another delivery opportunity.
    w.acknowledge_compaction_kick(&authorized.action_id, false, 6)
        .unwrap();
    drop(w);
    let mut w = PiWatchdog::open(&path).unwrap();
    let ack = w
        .acknowledge_compaction_kick(&authorized.action_id, false, 7)
        .unwrap();
    assert_eq!(ack.state, PiCompactionKickState::Acknowledged);
    assert_eq!(w.state().epochs_used, 1);

    // Settlement and wrapper-exit recovery preserve the same one action and
    // one charge. Neither boundary derives a replacement kick.
    w.settle_compaction_kick(&authorized.action_id, 8).unwrap();
    drop(w);
    let mut w = PiWatchdog::open(&path).unwrap();
    w.settle_compaction_kick(&authorized.action_id, 9).unwrap();
    w.observe(
        Observation::ProcessExited {
            status: ExitStatus::Code(0),
            reaped: true,
        },
        10,
    )
    .unwrap();
    assert_eq!(w.state().compaction_kicks.len(), 1);
    assert_eq!(
        w.compaction_kick(&authorized.action_id).unwrap().state,
        PiCompactionKickState::SettledAfterKick
    );
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().epochs_used, 1);
    assert!(
        !w.permit_compaction_kick(&authorized.action_id, 1, true, 11)
            .unwrap()
            .fresh_delivery_grant
    );
}

#[test]
fn process_exit_marks_prepermit_cancelled_and_unacked_send_uncertain() {
    let mut authorized = fixture(0);
    let action = authorized
        .authorize_compaction_kick(
            verified_compaction(&authorized, "exit-authorized", "assistant"),
            1,
        )
        .unwrap();
    authorized
        .observe(
            Observation::ProcessExited {
                status: ExitStatus::Signal(9),
                reaped: true,
            },
            2,
        )
        .unwrap();
    assert_eq!(
        authorized.compaction_kick(&action.action_id).unwrap().state,
        PiCompactionKickState::CancelledProcessExit
    );

    let mut permitted = fixture(0);
    let action = permitted
        .authorize_compaction_kick(
            verified_compaction(&permitted, "exit-permitted", "assistant"),
            1,
        )
        .unwrap();
    permitted
        .permit_compaction_kick(&action.action_id, 1, true, 2)
        .unwrap();
    permitted
        .observe(
            Observation::ProcessExited {
                status: ExitStatus::Code(1),
                reaped: true,
            },
            3,
        )
        .unwrap();
    assert_eq!(
        permitted.compaction_kick(&action.action_id).unwrap().state,
        PiCompactionKickState::Uncertain
    );
    assert!(
        !permitted
            .permit_compaction_kick(&action.action_id, 1, true, 4)
            .unwrap()
            .fresh_delivery_grant
    );
}

#[test]
fn failed_and_aborted_compaction_events_are_diagnostic_only() {
    for event in [
        serde_json::json!({
            "type":"compaction_end",
            "reason":"threshold",
            "aborted":false,
            "willRetry":false,
            "errorMessage":"secret provider failure"
        }),
        serde_json::json!({
            "type":"compaction_end",
            "reason":"threshold",
            "aborted":true,
            "willRetry":false,
            "result":null
        }),
    ] {
        let mut w = fixture(0);
        assert!(w.ingest_native_value(&event, 1).unwrap().is_empty());
        assert_eq!(w.state().native_activity.compaction_succeeded, Some(false));
        assert!(w.state().compaction_kicks.is_empty());
        assert_eq!(w.state().continuation_epoch, 0);
        assert!(
            !serde_json::to_string(w.state())
                .unwrap()
                .contains("secret provider failure")
        );
    }
}

#[test]
fn native_live_projection_is_numeric_deduplicated_and_text_free() {
    let mut w = fixture(0);
    let reasoning_canary = "RAW_REASONING_CANARY_7f3b";
    w.ingest_native_value(
        &serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":reasoning_canary,"thinkingTokens":7}}),
        1,
    )
    .unwrap();
    w.ingest_native_value(
        &serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hostile output","outputTokens":5}}),
        2,
    )
    .unwrap();
    w.ingest_native_value(
        &serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"more hostile output","outputTokens":11}}),
        4,
    )
    .unwrap();
    let turn = serde_json::json!({"type":"turn_end","turnId":"turn-1","message":{"usage":{"input":10,"output":11,"cacheRead":3,"cacheWrite":2,"totalTokens":26,"cost":{"total":0.25}}}});
    w.ingest_native_value(&turn, 5).unwrap();
    w.ingest_native_value(&turn, 6).unwrap();
    let native = &w.state().native_activity;
    assert_eq!(native.thinking_tokens, Some(7));
    assert_eq!(native.output_tokens, Some(11));
    assert_eq!(native.output_samples.len(), 2);
    assert_eq!(native.usage_total, Some(26));
    assert_eq!(native.usage_receipt_count, 1);
    let serialized = serde_json::to_string(w.state()).unwrap();
    assert!(!serialized.contains(reasoning_canary));
    assert!(!serialized.contains("hostile output"));
}

#[test]
fn native_stream_cursor_replay_is_monotonic_and_cost_is_not_doubled() {
    let mut w = fixture(0);
    let stream = "/bounded/capture/raw_stream.jsonl";
    let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"SECRET","thinkingTokens":7}}"#;
    w.ingest_native_line(line, stream, line.len() as u64 + 1, 1)
        .unwrap();
    w.ingest_native_line(line, stream, line.len() as u64 + 1, 2)
        .unwrap();
    let usage = r#"{"type":"turn_end","turnId":"turn-a","message":{"usage":{"input":10,"output":2,"totalTokens":12,"cost":{"total":0.25}}}}"#;
    let usage_end = line.len() as u64 + usage.len() as u64 + 2;
    w.ingest_native_line(usage, stream, usage_end, 3).unwrap();
    let path = w.state_path().to_path_buf();
    drop(w);
    let mut reopened = PiWatchdog::open(&path).unwrap();
    reopened
        .ingest_native_line(line, stream, line.len() as u64 + 1, 4)
        .unwrap();
    reopened
        .ingest_native_line(usage, stream, usage_end, 4)
        .unwrap();
    assert_eq!(reopened.state().native_activity.thinking_activity_seq, 1);
    assert_eq!(reopened.state().native_activity.usage_receipt_count, 1);
    assert_eq!(reopened.state().native_activity.usage_total, Some(12));
    assert_eq!(
        reopened.state().native_activity.usage_cost.as_deref(),
        Some("0.250000")
    );
    assert!(
        !serde_json::to_string(reopened.state())
            .unwrap()
            .contains("SECRET")
    );
}

#[test]
fn bootstrap_and_substantive_journal_reconcile_without_deleting_evidence() {
    let mut w = fixture(0);
    let session = w.state().session.clone();
    let substantive = session
        .session_dir
        .join("2026-01-01T00-00-00Z_session-1.jsonl");
    std::fs::write(
        &substantive,
        "{\"type\":\"session\",\"version\":3,\"id\":\"session-1\"}\n{\"type\":\"message\",\"id\":\"m1\"}\n",
    )
    .unwrap();
    assert!(w.reconcile_session_journal(2).unwrap());
    assert_eq!(w.state().session.session_file, substantive);
    assert!(session.session_file.exists());
    let other = session
        .session_dir
        .join("2026-01-02T00-00-00Z_session-1.jsonl");
    std::fs::write(
        &other,
        "{\"type\":\"session\",\"version\":3,\"id\":\"session-1\"}\n{\"type\":\"message\",\"id\":\"m2\"}\n",
    )
    .unwrap();
    let error = w.reconcile_session_journal(3).unwrap_err();
    assert_eq!(error.code, "ambiguous_substantive_session_journals");
    assert!(session.session_file.exists() && substantive.exists() && other.exists());
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
fn settled_persists_wrapper_bound_completion_handoff_across_restart() {
    let mut w = fixture(0);
    let wrapper = ProcessIdentity {
        pid: 122,
        pgid: 122,
        start_ticks: 400,
        boot_id: "boot".into(),
        nonce: "wrapper-nonce".into(),
    };
    w.bind_terminal_wrapper(wrapper.clone(), 1).unwrap();
    w.observe(Observation::AgentSettled, 2).unwrap();
    let handoff = w.state().completion_handoff.as_ref().unwrap();
    assert_eq!(handoff.source, w.state().source);
    assert_eq!(handoff.process_epoch, 1);
    assert_eq!(handoff.process_identity_digest, w.state().process.digest());
    assert_eq!(
        handoff.terminal_wrapper_identity_digest.as_deref(),
        Some(wrapper.digest().as_str())
    );
    assert_eq!(handoff.session_id, "session-1");
    assert_eq!(handoff.session_head, "leaf-1");

    let reopened = PiWatchdog::open(w.state_path()).unwrap();
    assert_eq!(reopened.state().terminal_wrapper, Some(wrapper));
    assert_eq!(
        reopened
            .state()
            .completion_handoff
            .as_ref()
            .map(|receipt| receipt.observed_at),
        Some(2)
    );
    assert!(!reopened.state().terminal, "settlement is not success");
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
fn same_pid_continuation_keeps_process_epoch_across_restart_and_exit() {
    let mut w = fixture(0);
    let original_process = w.state().process.clone();
    w.observe(Observation::AgentSettled, 1).unwrap();
    let path = w.state_path().to_path_buf();
    let before = (
        w.state().prompt_count,
        w.state().epochs_used,
        w.state().elapsed_reserved_secs,
    );
    assert_eq!(w.state().process_epoch, 1);
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().process, original_process);
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
    assert_eq!(reopened.state().process_epoch, 1);
    assert_eq!(reopened.state().continuation_epoch, 1);
    let terminal = TerminalIntentReceipt::new(
        &reopened,
        1,
        "same-process-done",
        TerminalDisposition::SuccessIntent,
    );
    reopened
        .observe(Observation::TerminalIntent(terminal), 3)
        .unwrap();
    assert!(reopened.state().terminal);
    assert_eq!(reopened.state().session.session_id, "session-1");
    assert_eq!(reopened.state().source.attempt_id, "attempt-2-7");
}

#[test]
fn legacy_same_process_split_repairs_once_without_touching_session_bytes() {
    let mut w = fixture(0);
    w.observe(Observation::AgentSettled, 1).unwrap();
    let session_before = std::fs::read(&w.state().session.session_file).unwrap();
    let identity = w.state().process.digest();
    w.state_mut_for_test().schema_version = 1;
    w.state_mut_for_test().process_epoch = 2;
    w.state_mut_for_test().native_activity.process_epoch = 2;
    let authority = w
        .attest_lifecycle_process_authority(1, &identity, 2)
        .unwrap();
    assert_eq!(authority.process_epoch, 1);
    assert_eq!(w.state().schema_version, 2);
    assert_eq!(w.state().native_activity.process_epoch, 1);
    assert_eq!(
        std::fs::read(&w.state().session.session_file).unwrap(),
        session_before
    );
    let path = w.state_path().to_path_buf();
    drop(w);
    let mut reopened = PiWatchdog::open(&path).unwrap();
    assert_eq!(
        reopened
            .attest_lifecycle_process_authority(1, &identity, 3)
            .unwrap(),
        authority
    );
}

#[test]
fn replacement_process_atomically_fences_old_receipts_and_survives_restart() {
    let mut w = fixture(0);
    let old_receipt =
        TerminalIntentReceipt::new(&w, 1, "old-done", TerminalDisposition::SuccessIntent);
    let old_identity = w.state().process.clone();
    let replacement = ProcessIdentity {
        pid: 124,
        pgid: 124,
        start_ticks: 789,
        boot_id: old_identity.boot_id.clone(),
        nonce: "replacement-nonce".into(),
    };
    let authority = w
        .replace_process_epoch(&old_identity, replacement.clone(), 1)
        .unwrap();
    assert_eq!(authority.process_epoch, 2);
    assert_eq!(authority.process_identity_digest, replacement.digest());
    assert_eq!(w.state().process, replacement);
    assert_eq!(
        w.observe(Observation::TerminalIntent(old_receipt), 2)
            .unwrap_err()
            .code,
        "stale_process_epoch"
    );

    let path = w.state_path().to_path_buf();
    drop(w);
    let mut reopened = PiWatchdog::open(&path).unwrap();
    assert_eq!(reopened.process_epoch_authority(), authority);
    let current = TerminalIntentReceipt::new(
        &reopened,
        authority.process_epoch,
        "new-done",
        TerminalDisposition::SuccessIntent,
    );
    reopened
        .observe(Observation::TerminalIntent(current.clone()), 3)
        .unwrap();
    assert!(
        reopened
            .observe(Observation::TerminalIntent(current), 4)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn restart_consumes_reserved_same_process_continuation_exactly_once() {
    let mut w = fixture(0);
    w.inject_crash_barrier(CrashBarrier::AfterContinuationReserved)
        .unwrap();
    assert!(w.observe(Observation::AgentSettled, 1).is_err());
    assert_eq!(w.state().process_epoch, 1);
    assert_eq!(w.state().continuation_epoch, 1);
    assert_eq!(w.state().prompt_count, 0);
    let path = w.state_path().to_owned();
    drop(w);

    let mut reopened = PiWatchdog::open(&path).unwrap();
    assert!(reopened.reconcile_pending_same_process_prompt(2).unwrap());
    assert_eq!(reopened.state().prompt_count, 1);
    assert_eq!(reopened.state().process_epoch, 1);
    drop(reopened);
    let mut replayed = PiWatchdog::open(&path).unwrap();
    assert!(!replayed.reconcile_pending_same_process_prompt(3).unwrap());
    assert_eq!(replayed.state().prompt_count, 1);
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
                initial_process_identity_digest: "process-1".into(),
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
            TransitionKind::PiContinuationEpochReserved {
                expected_process_epoch: 1,
                process_identity_digest: "process-1".into(),
                expected_continuation_epoch: 0,
                next_continuation_epoch: 1,
                elapsed_charge_secs: 600,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "same_process_continuation",
            "continuation-1",
        )
        .expecting(expected),
    )
    .unwrap();
    assert_eq!(task.lifecycle.pi_process_epoch, 1);
    assert_eq!(task.lifecycle.pi_continuation_epoch, 1);

    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiProcessEpochReplaced {
                expected_process_epoch: 1,
                expected_process_identity_digest: "process-1".into(),
                next_process_epoch: 2,
                next_process_identity_digest: "process-2".into(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "exact_process_replaced",
            "replace-2",
        )
        .expecting(expected),
    )
    .unwrap();
    assert_eq!(task.lifecycle.pi_process_epoch, 2);
    assert_eq!(task.lifecycle.pi_process_identity_digest, "process-2");

    let expected = FenceExpectation::current(&task);
    let stale_exit = apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiProcessEpochExited {
                process_epoch: 1,
                process_identity_digest: "process-1".into(),
                exact_reap_proof: true,
                effect_safe: true,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "old-pi-watchdog".into(),
            },
            "old_process_exit",
            "exit-1",
        )
        .expecting(expected),
    )
    .unwrap_err();
    assert_eq!(stale_exit.code, "stale_process_epoch");

    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiProcessEpochExited {
                process_epoch: 2,
                process_identity_digest: "process-2".into(),
                exact_reap_proof: true,
                effect_safe: true,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "needs_finalization_exit",
            "exit-2",
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
        process_epoch: 2,
        process_identity_digest: "process-2".into(),
        tool_call_id: "done-call".into(),
        disposition: TerminalDisposition::SuccessIntent,
        idempotency_key: "done-call".into(),
    };
    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiCompactionKickAcknowledged {
                action_id: "kick-a".into(),
                process_epoch: 2,
                process_identity_digest: "process-2".into(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-kick".into(),
            },
            "kick_ack",
            "kick-ack-a",
        )
        .expecting(expected),
    )
    .unwrap();
    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiKickEffectLeaseOpened {
                lease: PiKickEffectLease {
                    action_id: "kick-a".into(),
                    tool_call_id: "effect-1".into(),
                    process_epoch: 2,
                    process_identity_digest: "process-2".into(),
                },
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-kick".into(),
            },
            "effect_begin",
            "effect-begin-1",
        )
        .expecting(expected),
    )
    .unwrap();
    let expected = FenceExpectation::current(&task);
    let effect_race = apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiTerminalIntent {
                receipt: receipt.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Worker,
                id: "worker".into(),
            },
            "success_intent",
            "terminal",
        )
        .expecting(expected),
    )
    .unwrap_err();
    assert_eq!(effect_race.code, "effect_in_flight");
    assert!(task.lifecycle.pi_terminal_reservation.is_none());
    let expected = FenceExpectation::current(&task);
    apply_transition(
        &mut task,
        TransitionRequest::new(
            TransitionKind::PiKickEffectLeaseClosed {
                action_id: "kick-a".into(),
                tool_call_id: "effect-1".into(),
                process_epoch: 2,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-kick".into(),
            },
            "effect_end",
            "effect-end-1",
        )
        .expecting(expected),
    )
    .unwrap();
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
    assert!(task.lifecycle.pi_kick_active_actions.is_empty());
    assert!(task.lifecycle.pi_kick_revoked_actions.contains("kick-a"));
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
                expected_process_epoch: 2,
                process_identity_digest: "process-2".into(),
                expected_continuation_epoch: 1,
                next_continuation_epoch: 2,
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
