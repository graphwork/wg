use std::process::Command;
use tempfile::tempdir;

fn retired(args: &[&str], expected: &str) {
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".wg")).unwrap();
    std::fs::write(root.path().join(".wg/graph.jsonl"), b"").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args(args)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "legacy mutation unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "expected {expected:?} in stderr, got: {stderr}"
    );
}

#[test]
fn legacy_completion_mutators_are_unreachable_from_the_cli() {
    retired(
        &["finalize", "begin", "task"],
        "legacy finalization mutation is retired",
    );
    retired(
        &[
            "candidate",
            "waive",
            "task",
            "--report",
            "report",
            "--reason",
            "reason",
        ],
        "legacy candidate mutation is retired",
    );
    retired(
        &["merge-resolution", "retry", "task"],
        "legacy merge-resolution mutation is retired",
    );
    // Scored evaluation is restored only as a task-centric observer. An
    // invalid/non-terminal source is refused at the eligibility boundary; it
    // does not regain any of the evaluator lifecycle mutation surface.
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".wg")).unwrap();
    std::fs::write(root.path().join(".wg/graph.jsonl"), b"").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args(["evaluate", "run", "task"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed scored-evaluation eligibility"),
        "{stderr}"
    );
    assert!(!stderr.contains("legacy evaluation mutation is retired"));

    retired(
        &["fail", "task", "--eval-reject"],
        "legacy evaluation rejection is retired",
    );
}

#[test]
fn attempt_capabilities_never_authorize_finish_handoff() {
    let operations = worksgood::worker_control::WorkerOperationKind::default_attempt_operations();
    assert!(!operations.contains(&worksgood::worker_control::WorkerOperationKind::FinishHandoff));
}

#[test]
fn daemon_wrapper_and_worker_cli_have_no_legacy_completion_authority_calls() {
    let daemon = concat!(
        include_str!("../src/commands/service/mod.rs"),
        include_str!("../src/commands/service/coordinator.rs"),
        include_str!("../src/commands/service/triage.rs"),
        include_str!("../src/commands/service/zero_output.rs"),
    );
    for retired in [
        "PlannerStore::open",
        "FinalizationStore::open",
        "settle_prepared_worker_done(",
        "converge_exited_worker_finishes(",
    ] {
        assert!(
            !daemon.contains(retired),
            "daemon regained retired authority call {retired}"
        );
    }

    let wrapper = include_str!("../src/commands/spawn/execution.rs");
    assert!(!wrapper.contains("finalize settle"));
    assert!(!wrapper.contains("wg finish"));
    assert!(!wrapper.contains("FinishHandoff"));

    let worker_cli = include_str!("../src/worker_cli.rs");
    assert!(!worker_cli.contains("WorkerOperation::FinishHandoff"));
    assert!(!worker_cli.contains("SaveTransaction"));

    let show = include_str!("../src/commands/show.rs");
    assert!(!show.contains("FinalizationStore::open"));
}
