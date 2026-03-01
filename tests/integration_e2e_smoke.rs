//! End-to-end smoke test: init through full agent lifecycle.
//!
//! Exercises the complete happy-path workflow via the real `wg` binary in an
//! isolated temp directory. Catches regressions that per-command unit tests miss.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::Status;
use workgraph::parser::load_graph;

// ---------------------------------------------------------------------------
// Helpers (same pattern as other integration tests)
// ---------------------------------------------------------------------------

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(
        path.exists(),
        "wg binary not found at {:?}. Run `cargo build` first.",
        path
    );
    path
}

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn wg_ok(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd(wg_dir, args);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "wg {:?} failed.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    stdout
}

// ---------------------------------------------------------------------------
// The smoke test
// ---------------------------------------------------------------------------

/// Full lifecycle: init → agency init → add → show → viz → status → list →
/// edit → claim → log → artifact → done → check → analyze.
///
/// `evaluate run` is skipped because it requires an LLM call, but we verify
/// `evaluate show` handles a task with no evaluations gracefully.
#[test]
fn smoke_test_full_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    // ── 1. wg init ──────────────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["init"]);
    assert!(
        output.contains("Initialized workgraph"),
        "init should confirm initialization, got: {}",
        output
    );
    assert!(wg_dir.exists(), ".workgraph directory should exist");
    assert!(
        wg_dir.join("graph.jsonl").exists(),
        "graph.jsonl should be created"
    );

    // ── 2. wg agency init ───────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["agency", "init"]);
    assert!(
        wg_dir.join("agency").exists(),
        "agency directory should exist after agency init"
    );
    // agency init should create roles/tradeoffs/agents
    let agency_dir = wg_dir.join("agency");
    assert!(
        agency_dir.join("cache/roles").exists() || agency_dir.join("cache").exists(),
        "agency roles cache should be created, got output: {}",
        output
    );

    // ── 3. wg add 'Test task' --context-scope task ──────────────────────
    let output = wg_ok(&wg_dir, &["add", "Test task", "--context-scope", "task"]);
    assert!(
        output.contains("test-task") || output.contains("Test task"),
        "add should echo the task, got: {}",
        output
    );

    // Verify the task exists in the graph
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert_eq!(task.title, "Test task");
    assert_eq!(task.status, Status::Open);

    // ── 4. wg show test-task ────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["show", "test-task"]);
    assert!(
        output.contains("test-task"),
        "show should display the task ID, got: {}",
        output
    );
    assert!(
        output.contains("Test task"),
        "show should display the title, got: {}",
        output
    );

    // ── 5. wg viz ───────────────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["viz", "--no-tui"]);
    assert!(
        output.contains("test-task") || output.contains("Test task"),
        "viz should render the task, got: {}",
        output
    );

    // ── 6. wg status ────────────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["status"]);
    assert!(!output.is_empty(), "status should produce output");

    // ── 7. wg list ──────────────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["list"]);
    assert!(
        output.contains("test-task"),
        "list should include our task, got: {}",
        output
    );

    // ── 8. wg edit test-task --add-tag smoke ────────────────────────────
    wg_ok(&wg_dir, &["edit", "test-task", "--add-tag", "smoke"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert!(
        task.tags.contains(&"smoke".to_string()),
        "edit --add-tag should add the tag, got tags: {:?}",
        task.tags
    );

    // ── 9. wg claim test-task ───────────────────────────────────────────
    wg_ok(&wg_dir, &["claim", "test-task"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert_eq!(
        task.status,
        Status::InProgress,
        "claim should set status to in-progress"
    );

    // ── 10. wg log test-task 'Working on it' ────────────────────────────
    wg_ok(&wg_dir, &["log", "test-task", "Working on it"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert!(
        task.log.iter().any(|e| e.message.contains("Working on it")),
        "log entry should be recorded, got log: {:?}",
        task.log
    );

    // ── 11. wg artifact test-task /tmp/test.txt ─────────────────────────
    // Create the artifact file first so it's a valid path
    let artifact_path = tmp.path().join("test.txt");
    fs::write(&artifact_path, "smoke test artifact").unwrap();
    let artifact_str = artifact_path.to_str().unwrap();

    wg_ok(&wg_dir, &["artifact", "test-task", artifact_str]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert!(
        task.artifacts.contains(&artifact_str.to_string()),
        "artifact should be registered, got artifacts: {:?}",
        task.artifacts
    );

    // ── 12. wg done test-task ───────────────────────────────────────────
    wg_ok(&wg_dir, &["done", "test-task"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("test-task").unwrap();
    assert_eq!(task.status, Status::Done, "done should set status to done");
    assert!(
        task.completed_at.is_some(),
        "done should set completed_at timestamp"
    );

    // ── 13. wg check ────────────────────────────────────────────────────
    let output = wg_cmd(&wg_dir, &["check"]);
    assert!(
        output.status.success(),
        "check should pass on a clean graph.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // ── 14. wg evaluate show test-task ──────────────────────────────────
    // We skip `evaluate run` (requires LLM). Instead, verify that
    // `evaluate show` handles a task with no evaluations gracefully.
    let output = wg_cmd(&wg_dir, &["evaluate", "show", "test-task"]);
    // It may succeed with empty output or fail with "no evaluations" — both OK
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success()
            || combined.contains("No evaluation")
            || combined.contains("no evaluation"),
        "evaluate show should either succeed or report no evaluations, got: {}",
        combined
    );

    // ── 15. wg analyze ──────────────────────────────────────────────────
    let output = wg_ok(&wg_dir, &["analyze"]);
    assert!(!output.is_empty(), "analyze should produce a health report");
}

/// Verify that a dependency chain (add with --after) works through the
/// full lifecycle: parent done unblocks child.
#[test]
fn smoke_test_dependency_chain() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    wg_ok(&wg_dir, &["init"]);

    // Create parent task
    wg_ok(&wg_dir, &["add", "Parent task", "--id", "parent"]);

    // Create child task that depends on parent
    wg_ok(
        &wg_dir,
        &["add", "Child task", "--id", "child", "--after", "parent"],
    );

    // Child should not be in ready list
    let output = wg_ok(&wg_dir, &["ready"]);
    assert!(
        !output.contains("Child task"),
        "child should not be ready while parent is open, got: {}",
        output
    );

    // Complete parent
    wg_ok(&wg_dir, &["claim", "parent"]);
    wg_ok(&wg_dir, &["done", "parent"]);

    // Child should now be ready
    let output = wg_ok(&wg_dir, &["ready"]);
    assert!(
        output.contains("child"),
        "child should be ready after parent is done, got: {}",
        output
    );

    // Complete the child
    wg_ok(&wg_dir, &["claim", "child"]);
    wg_ok(&wg_dir, &["done", "child"]);

    // Check should pass
    let output = wg_cmd(&wg_dir, &["check"]);
    assert!(
        output.status.success(),
        "check should pass on completed graph"
    );

    // Status should show both done
    let output = wg_ok(&wg_dir, &["status"]);
    assert!(!output.is_empty());
}

/// Verify fail → retry → complete lifecycle.
#[test]
fn smoke_test_fail_retry_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    wg_ok(&wg_dir, &["init"]);
    wg_ok(&wg_dir, &["add", "Flaky task", "--id", "flaky"]);

    // Claim and fail
    wg_ok(&wg_dir, &["claim", "flaky"]);
    wg_ok(&wg_dir, &["fail", "flaky", "--reason", "transient error"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(graph.get_task("flaky").unwrap().status, Status::Failed);

    // Retry
    wg_ok(&wg_dir, &["retry", "flaky"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("flaky").unwrap();
    assert_eq!(task.status, Status::Open);
    assert_eq!(task.retry_count, 1);

    // Claim again and succeed
    wg_ok(&wg_dir, &["claim", "flaky"]);
    wg_ok(&wg_dir, &["done", "flaky"]);

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(graph.get_task("flaky").unwrap().status, Status::Done);
}
