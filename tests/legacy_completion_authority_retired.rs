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
    let retired_merge = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args(["merge-resolution", "retry", "task"])
        .output()
        .unwrap();
    let retired_merge_stderr = String::from_utf8_lossy(&retired_merge.stderr);
    assert!(
        retired_merge_stderr.contains("wg merge-resolution status <TASK>")
            && retired_merge_stderr.contains("wg resume <TASK> --only"),
        "retired merge-resolution error omitted supported recovery: {retired_merge_stderr}"
    );
    assert!(
        !retired_merge_stderr.contains("same worker"),
        "retired merge-resolution error requires a worker that may be gone: {retired_merge_stderr}"
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
fn completion_recovery_help_lists_only_supported_commands() {
    let merge_help = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args(["merge-resolution", "--help"])
        .output()
        .unwrap();
    assert!(merge_help.status.success());
    let merge_help = String::from_utf8_lossy(&merge_help.stdout);
    assert!(merge_help.contains("status") && merge_help.contains("inspect"));
    for retired in [
        "\n  run",
        "\n  retry",
        "\n  resume",
        "\n  change-route",
        "\n  decide",
        "\n  reject",
        "\n  refresh-target",
        "\n  repair-source",
        "\n  escalate-human",
        "\n  abort",
        "\n  rollback",
    ] {
        assert!(
            !merge_help.contains(retired),
            "help advertised retired mutation {retired:?}: {merge_help}"
        );
    }

    let resume_help = Command::new(env!("CARGO_BIN_EXE_wg"))
        .args(["resume", "--help"])
        .output()
        .unwrap();
    assert!(resume_help.status.success());
    let resume_help = String::from_utf8_lossy(&resume_help.stdout);
    assert!(resume_help.contains("wg merge-resolution status <TASK>"));
    assert!(resume_help.contains("wg resume <TASK> --only"));
    assert!(resume_help.contains("without the source worker"));
    assert!(!resume_help.contains("reset --hard"));
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
