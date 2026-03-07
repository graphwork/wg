//! Smoke: safety operations — retract, cascade-stop, reset, hold integration test.
//!
//! Validates the full safety operations suite works end-to-end via the `wg` CLI
//! binary in an isolated temp directory. Proves the spark-v2 incident is a
//! one-command fix.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::Status;
use workgraph::parser::load_graph;

// ---------------------------------------------------------------------------
// Helpers
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

fn graph_path(wg_dir: &Path) -> PathBuf {
    wg_dir.join("graph.jsonl")
}

/// Create a standard parent → child1, child2 hierarchy for testing.
/// Returns the wg_dir path.
fn setup_hierarchy(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    // Parent task
    wg_ok(
        &wg_dir,
        &["add", "Parent task", "--id", "parent", "--immediate"],
    );

    // Child tasks depending on parent
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Child one",
            "--id",
            "child-1",
            "--after",
            "parent",
            "--immediate",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Child two",
            "--id",
            "child-2",
            "--after",
            "parent",
            "--immediate",
        ],
    );

    // Grandchild depending on child-1
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Grandchild",
            "--id",
            "grandchild",
            "--after",
            "child-1",
            "--immediate",
        ],
    );

    wg_dir
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test cascade-stop: abandons target and all transitive dependents.
#[test]
fn test_smoke_safety_cascade_stop() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Cascade-stop from parent should abandon entire subtree
    let output = wg_ok(&wg_dir, &["cascade-stop", "parent"]);
    assert!(
        output.contains("abandoned") || output.contains("affected"),
        "cascade-stop should report affected tasks, got: {}",
        output
    );

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Abandoned,
            "'{}' should be abandoned after cascade-stop",
            id
        );
    }
}

/// Test cascade-stop from a mid-level node only affects downstream.
#[test]
fn test_smoke_safety_cascade_stop_partial() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Cascade-stop from child-1 should only affect child-1 and grandchild
    wg_ok(&wg_dir, &["cascade-stop", "child-1"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(graph.get_task("parent").unwrap().status, Status::Open);
    assert_eq!(graph.get_task("child-2").unwrap().status, Status::Open);
    assert_eq!(
        graph.get_task("child-1").unwrap().status,
        Status::Abandoned
    );
    assert_eq!(
        graph.get_task("grandchild").unwrap().status,
        Status::Abandoned
    );
}

/// Test cascade-stop --hold pauses downstream tasks instead of abandoning.
#[test]
fn test_smoke_safety_cascade_stop_hold() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    wg_ok(&wg_dir, &["cascade-stop", "--hold", "parent"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        let task = graph.get_task(id).unwrap();
        assert!(
            task.paused,
            "'{}' should be paused after cascade-stop --hold",
            id
        );
        assert_eq!(
            task.status,
            Status::Open,
            "'{}' should still be Open (not abandoned) after --hold",
            id
        );
    }
}

/// Test hold/unhold: atomic subtree pause and resume.
#[test]
fn test_smoke_safety_hold_unhold() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Hold parent — entire subtree should be paused
    let output = wg_ok(&wg_dir, &["hold", "parent"]);
    assert!(output.contains("held") || output.contains("paused"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert!(
            graph.get_task(id).unwrap().paused,
            "'{}' should be paused after hold",
            id
        );
    }

    // Unhold parent — exactly those tasks should resume
    let output = wg_ok(&wg_dir, &["unhold", "parent"]);
    assert!(output.contains("resumed"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert!(
            !graph.get_task(id).unwrap().paused,
            "'{}' should be unpaused after unhold",
            id
        );
    }
}

/// Test hold/unhold is reversible — pre-paused tasks stay paused.
#[test]
fn test_smoke_safety_hold_unhold_preserves_prior_pause() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Pre-pause child-2 manually
    wg_ok(&wg_dir, &["pause", "child-2"]);
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(graph.get_task("child-2").unwrap().paused);

    // Hold parent
    wg_ok(&wg_dir, &["hold", "parent"]);

    // All should be paused
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert!(graph.get_task(id).unwrap().paused);
    }

    // Unhold parent — child-2 should STILL be paused (was pre-paused)
    wg_ok(&wg_dir, &["unhold", "parent"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(!graph.get_task("parent").unwrap().paused);
    assert!(!graph.get_task("child-1").unwrap().paused);
    assert!(
        graph.get_task("child-2").unwrap().paused,
        "child-2 was pre-paused and should remain paused after unhold"
    );
    assert!(!graph.get_task("grandchild").unwrap().paused);
}

/// Test hold from a mid-level node only affects downstream.
#[test]
fn test_smoke_safety_hold_partial() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Hold child-1 — only child-1 and grandchild affected
    wg_ok(&wg_dir, &["hold", "child-1"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(!graph.get_task("parent").unwrap().paused);
    assert!(graph.get_task("child-1").unwrap().paused);
    assert!(!graph.get_task("child-2").unwrap().paused);
    assert!(graph.get_task("grandchild").unwrap().paused);
}

/// Test retract: traces provenance lineage and abandons created tasks.
#[test]
fn test_smoke_safety_retract() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    // Create and claim a parent task
    wg_ok(
        &wg_dir,
        &["add", "Root job", "--id", "root-job", "--immediate"],
    );
    // Claim as a specific agent so provenance links the agent to this task
    let agent_id = "test-agent-1";
    wg_ok(&wg_dir, &["claim", "root-job", "--actor", agent_id]);
    let add_with_agent = |id: &str, title: &str| {
        Command::new(wg_binary())
            .arg("--dir")
            .arg(&wg_dir)
            .args(&["add", title, "--id", id, "--after", "root-job", "--immediate"])
            .env("WG_AGENT_ID", agent_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };

    let out = add_with_agent("spawned-1", "Spawned task 1");
    assert!(out.status.success(), "add spawned-1 failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let out = add_with_agent("spawned-2", "Spawned task 2");
    assert!(out.status.success(), "add spawned-2 failed: {:?}", String::from_utf8_lossy(&out.stderr));

    // Verify tasks exist
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(graph.get_task("spawned-1").is_some());
    assert!(graph.get_task("spawned-2").is_some());

    // Retract root-job — should abandon spawned tasks and reset root-job to open
    let output = wg_ok(&wg_dir, &["retract", "root-job", "--no-kill"]);
    assert!(
        output.contains("retracted") || output.contains("Retracted"),
        "retract output should mention retracted tasks, got: {}",
        output
    );

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(
        graph.get_task("root-job").unwrap().status,
        Status::Open,
        "root-job should be reset to Open after retract"
    );

    // Check spawned tasks are abandoned
    for id in &["spawned-1", "spawned-2"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Abandoned,
            "'{}' should be abandoned after retract",
            id
        );
    }
}

/// Test retract --dry-run shows plan without modifying.
#[test]
fn test_smoke_safety_retract_dry_run() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    wg_ok(
        &wg_dir,
        &["add", "My task", "--id", "my-task", "--immediate"],
    );
    wg_ok(&wg_dir, &["claim", "my-task"]);

    let output = wg_ok(&wg_dir, &["retract", "my-task", "--dry-run", "--no-kill"]);
    assert!(
        output.contains("Dry run"),
        "retract --dry-run should say 'Dry run', got: {}",
        output
    );

    // Task should still be InProgress (unchanged)
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(
        graph.get_task("my-task").unwrap().status,
        Status::InProgress,
        "task should be unchanged after dry-run"
    );
}

/// Test reset: clean-slate reset of a task.
#[test]
fn test_smoke_safety_reset_basic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    wg_ok(
        &wg_dir,
        &["add", "Reset me", "--id", "reset-me", "--immediate"],
    );
    wg_ok(&wg_dir, &["claim", "reset-me"]);
    wg_ok(&wg_dir, &["done", "reset-me"]);

    // Task is now Done
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(graph.get_task("reset-me").unwrap().status, Status::Done);

    // Reset it
    let output = wg_ok(&wg_dir, &["reset", "reset-me"]);
    assert!(output.contains("Reset") || output.contains("open"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let task = graph.get_task("reset-me").unwrap();
    assert_eq!(task.status, Status::Open);
    assert_eq!(task.assigned, None);
    assert_eq!(task.completed_at, None);
}

/// Test reset --downstream: resets target and all transitive dependents.
#[test]
fn test_smoke_safety_reset_downstream() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    // Create a chain: a → b → c, complete all
    wg_ok(&wg_dir, &["add", "Task A", "--id", "a", "--immediate"]);
    wg_ok(
        &wg_dir,
        &["add", "Task B", "--id", "b", "--after", "a", "--immediate"],
    );
    wg_ok(
        &wg_dir,
        &["add", "Task C", "--id", "c", "--after", "b", "--immediate"],
    );

    wg_ok(&wg_dir, &["claim", "a"]);
    wg_ok(&wg_dir, &["done", "a"]);
    wg_ok(&wg_dir, &["claim", "b"]);
    wg_ok(&wg_dir, &["done", "b"]);
    wg_ok(&wg_dir, &["claim", "c"]);
    wg_ok(&wg_dir, &["done", "c"]);

    // All done
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["a", "b", "c"] {
        assert_eq!(graph.get_task(id).unwrap().status, Status::Done);
    }

    // Reset a --downstream: all tasks should revert to Open
    wg_ok(&wg_dir, &["reset", "a", "--downstream"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["a", "b", "c"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Open,
            "'{}' should be Open after reset --downstream",
            id
        );
    }
}

/// Test reset --downstream --retract: full cleanup in one command.
/// This is the "spark-v2 one-command fix".
///
/// The --retract flag abandons tasks created by agents that are NOT already
/// in the downstream reset set. Tasks that are both agent-created AND downstream
/// get reset (not abandoned) since --downstream already covers them.
#[test]
fn test_smoke_safety_reset_downstream_retract() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    // Create parent, claim it as a specific agent
    wg_ok(
        &wg_dir,
        &["add", "Parent", "--id", "parent", "--immediate"],
    );
    let agent_id = "cleanup-agent";
    wg_ok(&wg_dir, &["claim", "parent", "--actor", agent_id]);

    // Agent creates "side-effect" tasks NOT downstream of parent (no --after).
    // These simulate agent side-effects that only retract can clean up.
    let add_as_agent = |id: &str, title: &str, extra_args: &[&str]| {
        let mut cmd = Command::new(wg_binary());
        cmd.arg("--dir")
            .arg(&wg_dir)
            .args(&["add", title, "--id", id])
            .args(extra_args)
            .arg("--immediate")
            .env("WG_AGENT_ID", agent_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "add {} failed: {}",
            id,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    add_as_agent("side-effect-1", "Side effect 1", &[]);
    add_as_agent("side-effect-2", "Side effect 2", &[]);

    // Also add a downstream task (user-created, depends on parent)
    wg_ok(
        &wg_dir,
        &[
            "add",
            "User downstream",
            "--id",
            "user-downstream",
            "--after",
            "parent",
            "--immediate",
        ],
    );

    // Complete parent
    wg_ok(&wg_dir, &["done", "parent"]);

    // Reset with --downstream --retract
    let output = wg_ok(
        &wg_dir,
        &["reset", "parent", "--downstream", "--retract"],
    );
    assert!(
        output.contains("Reset") || output.contains("open") || output.contains("retract"),
        "output should mention reset/retract, got: {}",
        output
    );

    let graph = load_graph(graph_path(&wg_dir)).unwrap();

    // Parent should be reset to Open
    assert_eq!(
        graph.get_task("parent").unwrap().status,
        Status::Open,
        "parent should be reset to Open"
    );

    // User downstream should be reset to Open (it's a dependent)
    assert_eq!(
        graph.get_task("user-downstream").unwrap().status,
        Status::Open,
        "user-downstream should be reset to Open"
    );

    // Side-effect tasks (not downstream) should be abandoned via retract
    for id in &["side-effect-1", "side-effect-2"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Abandoned,
            "'{}' should be abandoned via retract",
            id
        );
    }
}

/// Test cascade-stop skips terminal (done/abandoned) tasks.
#[test]
fn test_smoke_safety_cascade_stop_skips_terminal() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    wg_ok(&wg_dir, &["add", "Root", "--id", "root", "--immediate"]);
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Already done",
            "--id",
            "already-done",
            "--after",
            "root",
            "--immediate",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Still open",
            "--id",
            "still-open",
            "--after",
            "root",
            "--immediate",
        ],
    );

    // Complete "already-done"
    // Need to complete root first since already-done depends on it
    wg_ok(&wg_dir, &["claim", "root"]);
    wg_ok(&wg_dir, &["done", "root"]);
    wg_ok(&wg_dir, &["claim", "already-done"]);
    wg_ok(&wg_dir, &["done", "already-done"]);

    // Re-open root so we can cascade-stop from it
    wg_ok(&wg_dir, &["reset", "root"]);

    // Cascade-stop from root
    wg_ok(&wg_dir, &["cascade-stop", "root"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(
        graph.get_task("root").unwrap().status,
        Status::Abandoned,
        "root should be abandoned"
    );
    assert_eq!(
        graph.get_task("already-done").unwrap().status,
        Status::Done,
        "already-done should remain Done (terminal, skipped)"
    );
    assert_eq!(
        graph.get_task("still-open").unwrap().status,
        Status::Abandoned,
        "still-open should be abandoned"
    );
}

/// Test hold skips terminal tasks.
#[test]
fn test_smoke_safety_hold_skips_terminal() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    wg_ok(&wg_dir, &["add", "Root", "--id", "root", "--immediate"]);
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Completed child",
            "--id",
            "done-child",
            "--after",
            "root",
            "--immediate",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Open child",
            "--id",
            "open-child",
            "--after",
            "root",
            "--immediate",
        ],
    );

    // Complete root and done-child
    wg_ok(&wg_dir, &["claim", "root"]);
    wg_ok(&wg_dir, &["done", "root"]);
    wg_ok(&wg_dir, &["claim", "done-child"]);
    wg_ok(&wg_dir, &["done", "done-child"]);

    // Reset root so we can hold it
    wg_ok(&wg_dir, &["reset", "root"]);

    wg_ok(&wg_dir, &["hold", "root"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(graph.get_task("root").unwrap().paused);
    assert!(
        !graph.get_task("done-child").unwrap().paused,
        "done-child should not be paused (terminal)"
    );
    assert!(
        graph.get_task("open-child").unwrap().paused,
        "open-child should be paused"
    );
}

/// Combined scenario: hold → verify paused → unhold → cascade-stop.
/// Simulates the full safety workflow an operator would use.
#[test]
fn test_smoke_safety_full_workflow() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // Step 1: Hold the entire subtree (operator pauses to investigate)
    wg_ok(&wg_dir, &["hold", "parent"]);
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert!(graph.get_task(id).unwrap().paused);
    }

    // Step 2: Operator decides it's fine, unhold
    wg_ok(&wg_dir, &["unhold", "parent"]);
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert!(!graph.get_task(id).unwrap().paused);
    }

    // Step 3: Something goes wrong — cascade-stop the whole thing
    wg_ok(&wg_dir, &["cascade-stop", "parent"]);
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert_eq!(graph.get_task(id).unwrap().status, Status::Abandoned);
    }

    // Step 4: Reset everything for another attempt
    wg_ok(&wg_dir, &["reset", "parent", "--downstream"]);
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["parent", "child-1", "child-2", "grandchild"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Open,
            "'{}' should be Open after reset --downstream",
            id
        );
    }
}

/// Test dry-run modes don't modify state.
#[test]
fn test_smoke_safety_dry_runs() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_hierarchy(&tmp);

    // cascade-stop --dry-run
    let output = wg_ok(&wg_dir, &["cascade-stop", "--dry-run", "parent"]);
    assert!(output.contains("Dry run"));
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(graph.get_task("parent").unwrap().status, Status::Open);

    // hold --dry-run
    let output = wg_ok(&wg_dir, &["hold", "--dry-run", "parent"]);
    assert!(output.contains("Dry run"));
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert!(!graph.get_task("parent").unwrap().paused);

    // reset --dry-run
    let output = wg_ok(&wg_dir, &["reset", "--dry-run", "parent"]);
    assert!(output.contains("Dry run"));
    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    assert_eq!(graph.get_task("parent").unwrap().status, Status::Open);
}

/// Test cascade-stop on diamond-shaped graph (join point).
#[test]
fn test_smoke_safety_cascade_stop_diamond() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);

    // Diamond: root → [left, right] → join
    wg_ok(&wg_dir, &["add", "Root", "--id", "root", "--immediate"]);
    wg_ok(
        &wg_dir,
        &[
            "add", "Left", "--id", "left", "--after", "root", "--immediate",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "add", "Right", "--id", "right", "--after", "root", "--immediate",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Join",
            "--id",
            "join",
            "--after",
            "left,right",
            "--immediate",
        ],
    );

    wg_ok(&wg_dir, &["cascade-stop", "root"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    for id in &["root", "left", "right", "join"] {
        assert_eq!(
            graph.get_task(id).unwrap().status,
            Status::Abandoned,
            "'{}' should be abandoned in diamond cascade",
            id
        );
    }
}
