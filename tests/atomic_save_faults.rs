//! Adversarial conformance tests for the atomic GraphSave/WorkSave protocol.
//!
//! These tests intentionally combine the pure reducer with real filesystem and
//! Git adapters. The shell smokes exercise the candidate `wg` binary around the
//! same incident cuts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use worksgood::completion_evidence::{AttemptSaveKey, EvidenceBinding, content_cid};
use worksgood::finalization::QuiescenceProof;
use worksgood::graph::{Node, Status, Task, WorkGraph};
use worksgood::query::{DependencyDisposition, dependency_disposition, ready_tasks};
use worksgood::save_transaction::{
    SaveFact, SavePhase, SaveTransactionKernel, SaveTransactionState, SaveTransitionRequest,
};
use worksgood::service::convergence::{
    FinishConvergenceAction, FinishConvergenceSnapshot, SaveReplayAction, WrapperChildCapability,
    reduce_exited_worker_finish, reduce_save_transaction_replay,
};
use worksgood::work_save::{WorkSaveCaptureRequest, WorkSaveStore, capture_work_save};
use worksgood::worktree_observer::{
    CandidatePathPolicy, ObserverConfig, ObserverIdentity, WorktreeObserver,
};

#[path = "../src/commands/completion_repair.rs"]
mod completion_repair;

fn source(task: &str) -> AttemptSaveKey {
    AttemptSaveKey {
        graph_id: "graph:atomic-save-faults".into(),
        task_id: task.into(),
        generation: 3,
        attempt_id: "attempt-3-7".into(),
        attempt_fence: 11,
        worktree_lease_epoch: 11,
        process_epoch: 2,
        wrapper_epoch: 1,
        route_snapshot_cid: "route:sha256:fixture".into(),
        session_proof_digest: "session:sha256:fixture".into(),
        worktree_identity_digest: "root:sha256:fixture".into(),
    }
}

fn transition(
    state: &SaveTransactionState,
    phase: SavePhase,
    key: &str,
    fact: SaveFact,
) -> SaveTransitionRequest {
    SaveTransitionRequest {
        source: state.source.clone(),
        expected_revision: state.revision,
        expected_phase: state.phase,
        next_phase: phase,
        idempotency_key: key.into(),
        action_key: format!("action:{key}"),
        fact,
    }
}

fn evidence(cid: &str, binding: Option<EvidenceBinding>) -> SaveFact {
    SaveFact::Evidence {
        cid: cid.into(),
        binding,
    }
}

fn task(id: &str, status: Status) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        status,
        ..Task::default()
    }
}

#[test]
fn false_done_dependency_dispatch() {
    let mut graph = WorkGraph::new();
    let false_done = task("false-done", Status::Done);
    assert!(false_done.graph_save_completion_disposition().is_none());
    graph.add_node(Node::Task(false_done));
    let mut dependent = task("dependent", Status::Open);
    dependent.after.push("false-done".into());
    graph.add_node(Node::Task(dependent));

    // The migration adapter must convert a naked legacy Done into a
    // non-satisfying reconciliation hold before dispatch is evaluated.
    let report = completion_repair::classify_legacy_completions(
        &graph,
        &[],
        br#"{"status":"done-without-bundle"}"#,
        None,
    )
    .unwrap();
    assert_eq!(report.quarantined_count(), 1);
    assert_eq!(report.records[0].blocked_downstream, vec!["dependent"]);
    completion_repair::apply_quarantine_plan(&mut graph, &report).unwrap();

    assert_eq!(
        graph.get_task("false-done").unwrap().status,
        Status::Incomplete
    );
    assert!(matches!(
        dependency_disposition("false-done", "dependent", &graph, None),
        DependencyDisposition::Blocked { .. }
    ));
    assert!(
        ready_tasks(&graph)
            .iter()
            .all(|task| task.id != "dependent")
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

fn work_save_fixture() -> (TempDir, PathBuf, PathBuf, AttemptSaveKey) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Atomic Save Fault"]);
    git(&root, &["config", "user.email", "atomic-save@test.invalid"]);
    fs::write(root.join("tracked.txt"), "base\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-qm", "base"]);
    fs::create_dir(root.join(".wg")).unwrap();

    let mut source = source("broker-handoff");
    let observer_dir = root.join(".wg/observer");
    let observer = WorktreeObserver::attach_at(
        &root,
        &observer_dir,
        ObserverIdentity {
            task_id: source.task_id.clone(),
            generation: source.generation,
            attempt_id: source.attempt_id.clone(),
            attempt_fence: source.attempt_fence,
            worktree_id: "agent-7".into(),
            worktree_lease_epoch: source.worktree_lease_epoch,
            process_epoch: source.process_epoch,
            observer_epoch: 1,
        },
        CandidatePathPolicy::new(Vec::new(), vec!["target/**".into()]).unwrap(),
        ObserverConfig::default(),
        1,
    )
    .unwrap();
    source.worktree_identity_digest =
        content_cid(&observer.projection().source.root_identity).unwrap();
    (temp, root, observer_dir, source)
}

fn capture_request(
    root: &Path,
    observer_dir: &Path,
    source: AttemptSaveKey,
) -> WorkSaveCaptureRequest {
    WorkSaveCaptureRequest {
        source,
        worktree_root: root.to_path_buf(),
        project_root: root.to_path_buf(),
        observer_state_dir: observer_dir.to_path_buf(),
        completion_intent_cid: "intent:broker-handoff".into(),
        prepared_base_commit_oid: git(root, &["rev-parse", "HEAD"]),
        quiescence: QuiescenceProof {
            receipt_cid: "quiescence:broker-handoff".into(),
            process_identity_digest: "process:starttime:boot:nonce".into(),
            process_group_empty: true,
            nonce_pipe_eof: true,
            observed_manifest_digest: None,
        },
        producer_build_id: "atomic-save-fault-test".into(),
    }
}

#[test]
fn broker_handoff_requires_bound_worktree() {
    let (_temp, root, observer_dir, source) = work_save_fixture();
    fs::write(root.join("uncommitted-wip.txt"), "valuable brokered WIP\n").unwrap();
    let store = WorkSaveStore::open(&root.join(".wg")).unwrap();

    let mut wrong = source.clone();
    wrong.worktree_identity_digest = "root:registry-mismatch".into();
    let error = capture_work_save(&store, &capture_request(&root, &observer_dir, wrong))
        .unwrap_err()
        .to_string();
    assert!(error.contains("root identity"), "{error}");

    let captured =
        capture_work_save(&store, &capture_request(&root, &observer_dir, source)).unwrap();
    assert!(!captured.receipt.clean);
    assert_eq!(
        git(
            &root,
            &[
                "show",
                &format!("{}:uncommitted-wip.txt", captured.receipt.rescue_commit_oid),
            ],
        ),
        "valuable brokered WIP"
    );
}

#[test]
fn crash_after_each_durable_boundary() {
    let source = source("crash-replay");
    let binding = EvidenceBinding {
        source: source.clone(),
        candidate_id: "candidate:one".into(),
        base_commit_oid: "base:one".into(),
    };
    let phases = [
        SavePhase::Prepared,
        SavePhase::Quiescing,
        SavePhase::WorkSaved,
        SavePhase::CandidateSealed,
        SavePhase::Validated,
        SavePhase::Accepted,
        SavePhase::DispositionRecorded,
        SavePhase::EffectPrepared,
        SavePhase::EffectCommitted,
        SavePhase::CleanupPrepared,
        SavePhase::CleanupCommitted,
    ];
    let mut state = SaveTransactionState::new(source).unwrap();
    let temp = tempfile::tempdir().unwrap();
    for (index, phase) in phases.into_iter().enumerate() {
        let fact = evidence(
            &format!("cid:{index}:{phase:?}"),
            (phase >= SavePhase::WorkSaved).then(|| binding.clone()),
        );
        let request = transition(&state, phase, &format!("phase-{index}"), fact);
        let plan = SaveTransactionKernel::transition(&state, request.clone()).unwrap();
        assert!(!plan.duplicate);
        let path = temp.path().join("head.json");
        worksgood::atomic_file::write_atomic(
            &path,
            &serde_json::to_vec_pretty(&plan.state).unwrap(),
        )
        .unwrap();
        let replayed: SaveTransactionState =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let duplicate = SaveTransactionKernel::transition(&replayed, request).unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.state, replayed);
        assert!(duplicate.state.recovery_rank() < state.recovery_rank());
        state = duplicate.state;
    }
}

fn capability() -> WrapperChildCapability {
    WrapperChildCapability {
        task_id: "dead-worker".into(),
        generation: 2,
        attempt_id: "attempt-2-4".into(),
        fence: 9,
        wrapper_epoch: 1,
        child_epoch: 1,
        wrapper_identity_digest: "wrapper:pid-start-boot".into(),
        child_identity_digest: "child:pid-start-boot".into(),
        owned_child: true,
    }
}

#[test]
fn dead_worker_without_intent_converges_nonrunning() {
    let cap = capability();
    let decision = reduce_exited_worker_finish(&FinishConvergenceSnapshot {
        presented_capability: cap.clone(),
        authoritative_capability: cap.clone(),
        owner_proven_dead: true,
        completion_receipted: false,
        transaction_phase: None,
        now_unix: 100,
    });
    assert_eq!(
        decision.pending_action,
        FinishConvergenceAction::ResumeSameSession
    );
    assert_eq!(decision.deadline_unix, Some(105));

    let mut state = SaveTransactionState::new(source("dead-worker")).unwrap();
    state = SaveTransactionKernel::transition(
        &state,
        transition(
            &state,
            SavePhase::NeedsReconciliation,
            "dead-no-proof",
            SaveFact::Hold {
                reason: "dead owner has no terminal intent or continuation proof".into(),
            },
        ),
    )
    .unwrap()
    .state;
    let replay = reduce_save_transaction_replay(&state, true, 105);
    assert_eq!(replay.action, SaveReplayAction::Hold);
    assert!(replay.deadline_unix.is_none());
}

#[test]
fn target_movement_holds_candidate() {
    let source = source("target-moved");
    let binding = EvidenceBinding {
        source: source.clone(),
        candidate_id: "candidate:retained".into(),
        base_commit_oid: "base:expected".into(),
    };
    let mut state = SaveTransactionState::new(source).unwrap();
    for (index, phase) in [
        SavePhase::Prepared,
        SavePhase::Quiescing,
        SavePhase::WorkSaved,
        SavePhase::CandidateSealed,
        SavePhase::Validated,
        SavePhase::Accepted,
        SavePhase::DispositionRecorded,
        SavePhase::EffectPrepared,
    ]
    .into_iter()
    .enumerate()
    {
        state = SaveTransactionKernel::transition(
            &state,
            transition(
                &state,
                phase,
                &format!("target-{index}"),
                evidence(
                    &format!("cid:target-{index}"),
                    (phase >= SavePhase::WorkSaved).then(|| binding.clone()),
                ),
            ),
        )
        .unwrap()
        .state;
    }
    let held = SaveTransactionKernel::transition(
        &state,
        transition(
            &state,
            SavePhase::NeedsRepair,
            "target-cas-moved",
            SaveFact::Hold {
                reason: "target moved before exact old-to-new CAS; candidate retained".into(),
            },
        ),
    )
    .unwrap()
    .state;
    assert_eq!(held.phase, SavePhase::NeedsRepair);
    assert_eq!(held.binding, Some(binding));
}

#[test]
fn reset_retry_saves_before_generation() {
    let source = source("reset-retry");
    let binding = EvidenceBinding {
        source: source.clone(),
        candidate_id: "candidate:reset-wip".into(),
        base_commit_oid: "base:reset".into(),
    };
    let mut state = SaveTransactionState::new(source.clone()).unwrap();
    for (index, phase) in [
        SavePhase::Prepared,
        SavePhase::Quiescing,
        SavePhase::WorkSaved,
    ]
    .into_iter()
    .enumerate()
    {
        state = SaveTransactionKernel::transition(
            &state,
            transition(
                &state,
                phase,
                &format!("reset-{index}"),
                evidence(
                    &format!("cid:reset-{index}"),
                    (phase == SavePhase::WorkSaved).then(|| binding.clone()),
                ),
            ),
        )
        .unwrap()
        .state;
    }
    state = SaveTransactionKernel::transition(
        &state,
        transition(
            &state,
            SavePhase::AbortedPreserved,
            "reset-abort-preserved",
            evidence("cid:aborted-preserved", Some(binding)),
        ),
    )
    .unwrap()
    .state;
    assert_eq!(state.phase, SavePhase::AbortedPreserved);
    let mut next = source;
    next.generation += 1;
    next.attempt_id = "attempt-4-1".into();
    next.attempt_fence += 1;
    assert_ne!(next.transaction_id().unwrap(), state.transaction_id);

    let stale = transition(
        &state,
        SavePhase::NeedsReconciliation,
        "stale-old-actor",
        SaveFact::Hold {
            reason: "stale actor".into(),
        },
    );
    assert_eq!(
        SaveTransactionKernel::transition(&state, stale)
            .unwrap_err()
            .code,
        "transaction-terminal"
    );
}

#[test]
fn lost_done_response_replays_graphsave_intent() {
    let state = SaveTransactionState::new(source("lost-response")).unwrap();
    let request = transition(
        &state,
        SavePhase::Prepared,
        "stable-client-request",
        evidence("cid:intent", None),
    );
    let committed = SaveTransactionKernel::transition(&state, request.clone()).unwrap();
    let replay = SaveTransactionKernel::transition(&committed.state, request).unwrap();
    assert!(replay.duplicate);
    assert_eq!(replay.state, committed.state);

    let conflict = transition(
        &state,
        SavePhase::Prepared,
        "stable-client-request",
        evidence("cid:different-payload", None),
    );
    assert_eq!(
        SaveTransactionKernel::transition(&committed.state, conflict)
            .unwrap_err()
            .code,
        "idempotency-conflict"
    );
}

#[test]
fn legacy_done_without_evidence_is_quarantined() {
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task("legacy", Status::Done)));
    let original = serde_json::to_vec(graph.get_task("legacy").unwrap()).unwrap();
    let report =
        completion_repair::classify_legacy_completions(&graph, &[], &original, None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    completion_repair::persist_migration_evidence(temp.path(), &original, None, &report).unwrap();
    completion_repair::apply_quarantine_plan(&mut graph, &report).unwrap();
    assert_eq!(graph.get_task("legacy").unwrap().status, Status::Incomplete);
    assert!(
        completion_repair::legacy_store_dir(temp.path())
            .join("snapshots")
            .is_dir()
    );

    let after = serde_json::to_vec(graph.get_task("legacy").unwrap()).unwrap();
    let second = completion_repair::classify_legacy_completions(&graph, &[], &after, None).unwrap();
    assert!(second.is_noop());
    assert!(
        original
            .windows(b"\"done\"".len())
            .any(|w| w == b"\"done\"")
    );
}

#[test]
fn stale_capability_and_binary_skew_are_inert() {
    let state = SaveTransactionState::new(source("skew")).unwrap();
    let mut stale_source = state.source.clone();
    stale_source.attempt_fence += 1;
    let mut request = transition(
        &state,
        SavePhase::Prepared,
        "stale-capability",
        evidence("cid:stale", None),
    );
    request.source = stale_source;
    assert_eq!(
        SaveTransactionKernel::transition(&state, request)
            .unwrap_err()
            .code,
        "stale-source"
    );

    let mut unsupported = state.clone();
    unsupported.schema_version += 1;
    let request = transition(
        &unsupported,
        SavePhase::Prepared,
        "old-daemon-new-client",
        evidence("cid:skew", None),
    );
    assert_eq!(
        SaveTransactionKernel::transition(&unsupported, request)
            .unwrap_err()
            .code,
        "unsupported-protocol"
    );
    assert_eq!(state.phase, SavePhase::Absent);
}

#[test]
fn delete_before_cleanup_receipt_reconstructs_exactly() {
    let source = source("cleanup-loss");
    let binding = EvidenceBinding {
        source: source.clone(),
        candidate_id: "candidate:cleanup".into(),
        base_commit_oid: "base:cleanup".into(),
    };
    let mut state = SaveTransactionState::new(source).unwrap();
    for (index, phase) in [
        SavePhase::Prepared,
        SavePhase::Quiescing,
        SavePhase::WorkSaved,
        SavePhase::CandidateSealed,
        SavePhase::Validated,
        SavePhase::Accepted,
        SavePhase::DispositionRecorded,
        SavePhase::EffectPrepared,
        SavePhase::EffectCommitted,
        SavePhase::CleanupPrepared,
    ]
    .into_iter()
    .enumerate()
    {
        state = SaveTransactionKernel::transition(
            &state,
            transition(
                &state,
                phase,
                &format!("cleanup-{index}"),
                evidence(
                    &format!("cid:cleanup-{index}"),
                    (phase >= SavePhase::WorkSaved).then(|| binding.clone()),
                ),
            ),
        )
        .unwrap()
        .state;
    }
    assert_eq!(
        reduce_save_transaction_replay(&state, true, 50).action,
        SaveReplayAction::ReplayCleanup
    );
    let committed = SaveTransactionKernel::transition(
        &state,
        transition(
            &state,
            SavePhase::CleanupCommitted,
            "matching-root-tombstone",
            evidence("cid:cleanup-receipt", Some(binding)),
        ),
    )
    .unwrap()
    .state;
    assert_eq!(
        reduce_save_transaction_replay(&committed, true, 55).action,
        SaveReplayAction::ReplayGraphSave
    );
}
