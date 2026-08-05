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
    retired(
        &["evaluate", "run", "task"],
        "legacy evaluation mutation is retired",
    );
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
