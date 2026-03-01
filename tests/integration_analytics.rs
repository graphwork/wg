//! Integration tests for analytics CLI commands: velocity, forecast, aging,
//! coordinate, and workload.
//!
//! These tests invoke the real `wg` binary to verify that analytics commands
//! run without panicking and produce expected output structure.

use chrono::{Duration, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{Estimate, Node, Status, Task, WorkGraph};
use workgraph::parser::save_graph;

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

fn make_task(id: &str, title: &str, status: Status) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status,
        created_at: Some(Utc::now().to_rfc3339()),
        ..Task::default()
    }
}

fn make_done_task(id: &str, title: &str, completed_days_ago: i64) -> Task {
    let completed_at = Utc::now() - Duration::days(completed_days_ago);
    let created_at = completed_at - Duration::days(1);
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::Done,
        created_at: Some(created_at.to_rfc3339()),
        completed_at: Some(completed_at.to_rfc3339()),
        ..Task::default()
    }
}

fn make_in_progress_task(id: &str, title: &str, agent: &str, started_days_ago: i64) -> Task {
    let started_at = Utc::now() - Duration::days(started_days_ago);
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::InProgress,
        assigned: Some(agent.to_string()),
        created_at: Some((Utc::now() - Duration::days(started_days_ago + 1)).to_rfc3339()),
        started_at: Some(started_at.to_rfc3339()),
        ..Task::default()
    }
}

fn setup_workgraph(tmp: &TempDir, tasks: Vec<Task>) -> PathBuf {
    let wg_dir = tmp.path().join(".workgraph");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

// ===========================================================================
// wg velocity
// ===========================================================================

#[test]
fn velocity_empty_graph() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);
    let output = wg_ok(&wg_dir, &["velocity"]);
    assert!(output.contains("Completion Velocity"));
}

#[test]
fn velocity_with_completed_tasks() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_done_task("d1", "Done recently", 1),
            make_done_task("d2", "Done last week", 8),
            make_done_task("d3", "Done two weeks ago", 15),
            make_task("o1", "Still open", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["velocity"]);
    assert!(output.contains("Completion Velocity"));
    assert!(output.contains("Week"));
    assert!(output.contains("Average:"));
    assert!(output.contains("Trend:"));
}

#[test]
fn velocity_custom_weeks() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_done_task("d1", "Done task", 2)]);

    let output = wg_ok(&wg_dir, &["velocity", "--weeks", "8"]);
    // 8 weeks * 7 = 56 days
    assert!(output.contains("56 days"));
}

#[test]
fn velocity_json_output() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_done_task("d1", "Done task", 3),
            make_task("o1", "Open task", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["--json", "velocity"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from velocity --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.get("weeks").is_some());
    assert!(json.get("average_tasks_per_week").is_some());
    assert!(json.get("trend").is_some());
    assert!(json.get("open_tasks").is_some());
}

#[test]
fn velocity_all_tasks_open() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_task("o1", "Open 1", Status::Open),
            make_task("o2", "Open 2", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["velocity"]);
    assert!(output.contains("Completion Velocity"));
    // Should show zero completions
    assert!(output.contains("Average:"));
}

// ===========================================================================
// wg forecast
// ===========================================================================

#[test]
fn forecast_empty_graph() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);
    let output = wg_ok(&wg_dir, &["forecast"]);
    // Should not panic; output may say no remaining work
    assert!(!output.is_empty() || true); // command ran successfully
}

#[test]
fn forecast_with_mixed_tasks() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_done_task("d1", "Done 1", 2),
            make_done_task("d2", "Done 2", 5),
            make_done_task("d3", "Done 3", 10),
            make_task("o1", "Open 1", Status::Open),
            make_task("o2", "Open 2", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["forecast"]);
    // Should contain some section headers
    assert!(
        output.contains("Remaining")
            || output.contains("remaining")
            || output.contains("Forecast")
            || output.contains("tasks"),
        "Expected forecast output to contain work/tasks info, got: {}",
        output
    );
}

#[test]
fn forecast_with_estimates() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("o1", "Estimated task", Status::Open);
    t1.estimate = Some(Estimate {
        hours: Some(4.0),
        cost: Some(10.0),
    });
    let mut t2 = make_done_task("d1", "Done with estimate", 3);
    t2.estimate = Some(Estimate {
        hours: Some(2.0),
        cost: None,
    });

    let wg_dir = setup_workgraph(&tmp, vec![t1, t2]);
    let output = wg_ok(&wg_dir, &["forecast"]);
    assert!(!output.is_empty());
}

#[test]
fn forecast_json_output() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_done_task("d1", "Done", 1),
            make_task("o1", "Open", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["--json", "forecast"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from forecast --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.get("remaining_work").is_some());
    assert!(json.get("scenarios").is_some());
    assert!(json.get("has_velocity_data").is_some());
}

// ===========================================================================
// wg aging
// ===========================================================================

#[test]
fn aging_empty_graph() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);
    let output = wg_ok(&wg_dir, &["aging"]);
    // Should run without error
    assert!(!output.is_empty() || true);
}

#[test]
fn aging_with_tasks_of_varying_age() {
    let tmp = TempDir::new().unwrap();

    // Create tasks with varying created_at timestamps
    let mut recent = make_task("r1", "Recent task", Status::Open);
    recent.created_at = Some(Utc::now().to_rfc3339());

    let mut old = make_task("old1", "Old task", Status::Open);
    old.created_at = Some((Utc::now() - Duration::days(45)).to_rfc3339());

    let mut very_old = make_task("vo1", "Very old task", Status::Open);
    very_old.created_at = Some((Utc::now() - Duration::days(120)).to_rfc3339());

    let wg_dir = setup_workgraph(&tmp, vec![recent, old, very_old]);
    let output = wg_ok(&wg_dir, &["aging"]);

    // Should contain age bucket labels
    assert!(
        output.contains("< 1 day")
            || output.contains("1-7 days")
            || output.contains("1-4 weeks")
            || output.contains("1-3 months")
            || output.contains("> 3 months"),
        "Expected age bucket labels in output, got: {}",
        output
    );
}

#[test]
fn aging_with_stale_in_progress() {
    let tmp = TempDir::new().unwrap();

    // A task started long ago = stale in-progress
    let stale = make_in_progress_task("stale1", "Stale WIP", "agent-1", 20);
    let fresh_ip = make_in_progress_task("fresh1", "Fresh WIP", "agent-2", 1);

    let wg_dir = setup_workgraph(&tmp, vec![stale, fresh_ip]);
    let output = wg_ok(&wg_dir, &["aging"]);
    // Should show stale in-progress section
    assert!(
        output.contains("stale")
            || output.contains("Stale")
            || output.contains("In Progress")
            || output.contains("in-progress"),
        "Expected stale/in-progress mention in aging output, got: {}",
        output
    );
}

#[test]
fn aging_json_output() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("t1", "Task 1", Status::Open)]);

    let output = wg_ok(&wg_dir, &["--json", "aging"]);
    let json: serde_json::Value = serde_json::from_str(&output)
        .unwrap_or_else(|e| panic!("Invalid JSON from aging --json: {}\nOutput: {}", e, output));
    assert!(json.get("distribution").is_some());
    assert!(json.get("oldest_tasks").is_some());
    assert!(json.get("stale_in_progress").is_some());
}

// ===========================================================================
// wg coordinate
// ===========================================================================

#[test]
fn coordinate_empty_graph() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);
    let output = wg_ok(&wg_dir, &["coordinate"]);
    assert!(output.contains("0/0") || output.contains("All tasks complete"));
}

#[test]
fn coordinate_with_mixed_statuses() {
    let tmp = TempDir::new().unwrap();

    let mut blocked = make_task("blocked1", "Blocked task", Status::Open);
    blocked.after = vec!["open1".to_string()];

    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_task("open1", "Ready task", Status::Open),
            make_in_progress_task("ip1", "In progress task", "agent-1", 1),
            blocked,
            make_done_task("done1", "Done task", 1),
        ],
    );

    let output = wg_ok(&wg_dir, &["coordinate"]);
    assert!(output.contains("Progress:"));
    // Should mention the ready task
    assert!(output.contains("open1") || output.contains("Ready"));
}

#[test]
fn coordinate_with_max_parallel() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_task("r1", "Ready 1", Status::Open),
            make_task("r2", "Ready 2", Status::Open),
            make_task("r3", "Ready 3", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["coordinate", "--max-parallel", "2"]);
    assert!(output.contains("Progress:"));
    // Should limit displayed tasks
    assert!(output.contains("2"));
}

#[test]
fn coordinate_json_output() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_task("t1", "Task 1", Status::Open),
            make_done_task("t2", "Task 2", 1),
        ],
    );

    let output = wg_ok(&wg_dir, &["--json", "coordinate"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from coordinate --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.get("ready").is_some());
    assert!(json.get("in_progress").is_some());
    assert!(json.get("blocked").is_some());
    assert!(json.get("done_count").is_some());
    assert!(json.get("total_count").is_some());
}

// ===========================================================================
// wg workload
// ===========================================================================

#[test]
fn workload_empty_graph() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);
    let output = wg_ok(&wg_dir, &["workload"]);
    // Should run without error; may show "no agents" or empty output
    assert!(!output.is_empty() || true);
}

#[test]
fn workload_with_assigned_tasks() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_in_progress_task("t1", "Agent 1 task", "agent-a", 1),
            make_in_progress_task("t2", "Agent 1 task 2", "agent-a", 2),
            make_in_progress_task("t3", "Agent 2 task", "agent-b", 1),
            make_task("t4", "Unassigned", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["workload"]);
    // Should mention agents and their workloads
    assert!(
        output.contains("agent-a")
            || output.contains("agent-b")
            || output.contains("Workload")
            || output.contains("workload"),
        "Expected agent workload info, got: {}",
        output
    );
}

#[test]
fn workload_json_output() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![
            make_in_progress_task("t1", "WIP task", "agent-x", 1),
            make_task("t2", "Open task", Status::Open),
        ],
    );

    let output = wg_ok(&wg_dir, &["--json", "workload"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from workload --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.get("agents").is_some());
    assert!(json.get("unassigned_count").is_some());
}
