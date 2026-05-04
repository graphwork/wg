//! Integration tests for the failed-dependency triage protocol.
//!
//! Tests:
//! - Triage detection: task with failed dep appears in ready list (terminal dep)
//! - Reset mechanism: wg requeue transitions InProgress → Open, increments triage_count
//! - Loop guard: requeue fails after max_triage_attempts
//! - End-to-end triage flow: task A fails → task B enters triage → fix task → A retried → B dispatches
//! - triage_count resets on cycle reactivation

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use workgraph::graph::{LogEntry, Node, Status, Task, WorkGraph};
use workgraph::parser::{load_graph, save_graph};
use workgraph::query::ready_tasks;

fn make_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        ..Task::default()
    }
}

fn graph_path(dir: &Path) -> std::path::PathBuf {
    dir.join("graph.jsonl")
}

fn setup_workgraph(dir: &Path, tasks: Vec<Task>) -> std::path::PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = graph_path(dir);
    let mut graph = WorkGraph::new();
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &path).unwrap();
    path
}

fn wg_requeue(wg_dir: &Path, task_id: &str, reason: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wg"))
        .args([
            "--dir",
            wg_dir.to_str().unwrap(),
            "requeue",
            task_id,
            "--reason",
            reason,
        ])
        .output()
        .expect("Failed to execute wg requeue")
}

// ---------------------------------------------------------------------------
// Test: Task with failed dep remains blocked until triage resolves it
// ---------------------------------------------------------------------------

#[test]
fn test_task_with_failed_dep_is_not_ready() {
    let mut graph = WorkGraph::new();

    let mut task_a = make_task("task-a", "Build parser");
    task_a.status = Status::Failed;
    task_a.failure_reason = Some("test assertion error".to_string());
    graph.add_node(Node::Task(task_a));

    let mut task_b = make_task("task-b", "Use parser output");
    task_b.after = vec!["task-a".to_string()];
    graph.add_node(Node::Task(task_b));

    let ready = ready_tasks(&graph);
    assert!(
        ready.iter().all(|t| t.id != "task-b"),
        "task-b should remain blocked while task-a is Failed"
    );
}

// ---------------------------------------------------------------------------
// Test: wg requeue transitions InProgress → Open, increments triage_count
// ---------------------------------------------------------------------------

#[test]
fn test_requeue_via_cli() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();

    let mut task = make_task("task-b", "Use parser output");
    task.status = Status::InProgress;
    task.assigned = Some("agent-42".to_string());
    task.started_at = Some("2026-01-01T00:00:00Z".to_string());
    task.session_id = Some("sess-1".to_string());
    task.agent = Some("agent-hash-abc".to_string());
    setup_workgraph(wg_dir, vec![task]);

    let output = wg_requeue(wg_dir, "task-b", "Triage: fix for task-a");
    assert!(
        output.status.success(),
        "wg requeue should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let graph = load_graph(&graph_path(wg_dir)).unwrap();
    let task = graph.get_task("task-b").unwrap();
    assert_eq!(task.status, Status::Open);
    assert_eq!(task.triage_count, 1);
    assert_eq!(task.assigned, None);
    assert_eq!(task.started_at, None);
    assert_eq!(task.session_id, None);
    // Agent identity should be preserved
    assert_eq!(task.agent, Some("agent-hash-abc".to_string()));
}

// ---------------------------------------------------------------------------
// Test: wg requeue fails when triage budget is exhausted
// ---------------------------------------------------------------------------

#[test]
fn test_requeue_budget_exhaustion_via_cli() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();

    let mut task = make_task("task-c", "Downstream task");
    task.status = Status::InProgress;
    task.triage_count = 3; // Default max is 3
    setup_workgraph(wg_dir, vec![task]);

    let output = wg_requeue(wg_dir, "task-c", "Another triage attempt");
    assert!(
        !output.status.success(),
        "wg requeue should fail when budget exhausted"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Triage budget exhausted"),
        "Error should mention budget, got: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test: wg requeue on non-InProgress task errors
// ---------------------------------------------------------------------------

#[test]
fn test_requeue_open_task_errors_via_cli() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();

    setup_workgraph(wg_dir, vec![make_task("task-d", "Open task")]);

    let output = wg_requeue(wg_dir, "task-d", "reason");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not in-progress"));
}

// ---------------------------------------------------------------------------
// Test: End-to-end triage flow
// ---------------------------------------------------------------------------

#[test]
fn test_triage_end_to_end_flow() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();

    // Step 1: A fails
    let mut task_a = make_task("task-a", "Implement config parser");
    task_a.status = Status::Failed;
    task_a.failure_reason = Some("test_parse_config assertion error".to_string());
    task_a.log = vec![LogEntry {
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        actor: Some("agent-1".to_string()),
        user: None,
        message: "Parser fails on nested keys".to_string(),
    }];

    // Step 2: B is dispatched (deps terminal)
    let mut task_b = make_task("task-b", "Use parser output");
    task_b.status = Status::InProgress;
    task_b.assigned = Some("agent-2".to_string());
    task_b.after = vec!["task-a".to_string()];

    let path = setup_workgraph(wg_dir, vec![task_a, task_b]);

    // Step 3: Agent creates fix task --before task-a
    {
        let mut graph = load_graph(&path).unwrap();
        let mut fix_task = make_task("fix-parser", "Fix: config parser nested keys");
        fix_task.status = Status::Open;
        fix_task.before = vec!["task-a".to_string()];
        graph.add_node(Node::Task(fix_task));

        // Wire bidirectional: task-a now blocked by fix
        if let Some(a) = graph.get_task_mut("task-a") {
            a.after.push("fix-parser".to_string());
        }
        save_graph(&graph, &path).unwrap();
    }

    // Step 4: Agent retries task-a (simulated)
    {
        let mut graph = load_graph(&path).unwrap();
        if let Some(a) = graph.get_task_mut("task-a") {
            a.status = Status::Open;
            a.failure_reason = None;
        }
        save_graph(&graph, &path).unwrap();
    }

    // Step 5: Agent requeues task-b via CLI
    let output = wg_requeue(wg_dir, "task-b", "Created fix for failed dep task-a");
    assert!(
        output.status.success(),
        "Requeue should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify graph state
    let graph = load_graph(&path).unwrap();

    // task-b is open, triaged
    let b = graph.get_task("task-b").unwrap();
    assert_eq!(b.status, Status::Open);
    assert_eq!(b.triage_count, 1);

    // Fix task is ready
    let ready = ready_tasks(&graph);
    assert!(
        ready.iter().any(|t| t.id == "fix-parser"),
        "Fix task should be ready"
    );
    // task-a blocked by fix
    assert!(
        !ready.iter().any(|t| t.id == "task-a"),
        "task-a should be blocked by fix task"
    );
    // task-b blocked by task-a (not terminal)
    assert!(
        !ready.iter().any(|t| t.id == "task-b"),
        "task-b should be blocked by task-a"
    );

    // Step 6: Fix completes → task-a ready
    {
        let mut graph = load_graph(&path).unwrap();
        if let Some(f) = graph.get_task_mut("fix-parser") {
            f.status = Status::Done;
        }
        save_graph(&graph, &path).unwrap();
    }
    let graph = load_graph(&path).unwrap();
    let ready = ready_tasks(&graph);
    assert!(
        ready.iter().any(|t| t.id == "task-a"),
        "task-a should be ready after fix completes"
    );

    // Step 7: task-a succeeds → task-b ready
    {
        let mut graph = load_graph(&path).unwrap();
        if let Some(a) = graph.get_task_mut("task-a") {
            a.status = Status::Done;
        }
        save_graph(&graph, &path).unwrap();
    }
    let graph = load_graph(&path).unwrap();
    let ready = ready_tasks(&graph);
    assert!(
        ready.iter().any(|t| t.id == "task-b"),
        "task-b should be ready now"
    );
    assert_eq!(graph.get_task("task-b").unwrap().triage_count, 1);
}

// ---------------------------------------------------------------------------
// Test: Loop prevention fires after max iterations
// ---------------------------------------------------------------------------

#[test]
fn test_loop_prevention_fires_after_max_attempts() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();

    let mut task = make_task("loop-task", "Task with triage loop");
    task.status = Status::InProgress;
    task.triage_count = 0;
    setup_workgraph(wg_dir, vec![task]);

    // Triage rounds 1-3 succeed
    for i in 1..=3 {
        let output = wg_requeue(wg_dir, "loop-task", &format!("triage round {}", i));
        assert!(output.status.success(), "Round {} should succeed", i);

        // Re-dispatch (set back to InProgress)
        let path = graph_path(wg_dir);
        let mut graph = load_graph(&path).unwrap();
        if let Some(t) = graph.get_task_mut("loop-task") {
            t.status = Status::InProgress;
            t.assigned = Some(format!("agent-{}", i));
        }
        save_graph(&graph, &path).unwrap();
    }

    // Round 4 should fail
    let output = wg_requeue(wg_dir, "loop-task", "triage round 4");
    assert!(
        !output.status.success(),
        "Round 4 should fail (budget exhausted)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Triage budget exhausted"));

    // Verify final triage_count
    let graph = load_graph(&graph_path(wg_dir)).unwrap();
    assert_eq!(graph.get_task("loop-task").unwrap().triage_count, 3);
}

// ---------------------------------------------------------------------------
// Test: triage_count resets on cycle reactivation
// ---------------------------------------------------------------------------

#[test]
fn test_triage_count_resets_on_cycle_reactivation() {
    let dir = tempdir().unwrap();
    let wg_dir = dir.path();
    let path = graph_path(wg_dir);

    // 2-node cycle: task-c ↔ task-d (mutual after edges)
    let mut task_c = make_task("task-c", "Cycle header");
    task_c.status = Status::Done;
    task_c.triage_count = 2;
    task_c.after = vec!["task-d".to_string()]; // back-edge: c depends on d
    task_c.cycle_config = Some(workgraph::graph::CycleConfig {
        max_iterations: 3,
        guard: None,
        delay: None,
        no_converge: false,
        restart_on_failure: true,
        max_failure_restarts: None,
    });

    let mut task_d = make_task("task-d", "Cycle member");
    task_d.status = Status::Done;
    task_d.triage_count = 1;
    task_d.after = vec!["task-c".to_string()]; // forward edge: d depends on c

    fs::create_dir_all(wg_dir).unwrap();
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task_c));
    graph.add_node(Node::Task(task_d));
    save_graph(&graph, &path).unwrap();

    // Evaluate cycles
    let mut graph = load_graph(&path).unwrap();
    let cycle_analysis = graph.compute_cycle_analysis();
    workgraph::graph::evaluate_all_cycle_iterations(&mut graph, &cycle_analysis);

    // triage_count should be reset to 0
    assert_eq!(graph.get_task("task-c").unwrap().triage_count, 0);
    assert_eq!(graph.get_task("task-d").unwrap().triage_count, 0);
    // loop_iteration should be incremented
    assert_eq!(graph.get_task("task-c").unwrap().loop_iteration, 1);
}
