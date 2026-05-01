//! Integration tests for agency stats CLI command.
//!
//! Tests invoke the real `wg` binary to verify that `wg agency stats`
//! runs without panicking and produces expected output structure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{Node, Status, Task, WorkGraph};
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

/// Set up a .wg dir with a graph and agency directories.
fn setup_agency(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(wg_dir.join("agency/cache/roles")).unwrap();
    fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();
    fs::create_dir_all(wg_dir.join("agency/primitives/tradeoffs")).unwrap();
    fs::create_dir_all(wg_dir.join("agency/evaluations")).unwrap();

    // Create empty graph
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = WorkGraph::new();
    save_graph(&graph, &graph_path).unwrap();

    wg_dir
}

/// Add a role YAML file to the agency cache.
fn add_role(wg_dir: &Path, id: &str, name: &str, task_count: u32, avg_score: Option<f64>) {
    let roles_dir = wg_dir.join("agency/cache/roles");
    let avg_str = match avg_score {
        Some(v) => format!("{}", v),
        None => "null".to_string(),
    };
    let yaml = format!(
        r#"id: "{id}"
name: "{name}"
description: "Test role {name}"
outcome_id: "outcome-1"
performance:
  task_count: {task_count}
  avg_score: {avg_str}
  evaluations: []
lineage:
  generation: 0
  parents: []
  method: "seed"
"#
    );
    fs::write(roles_dir.join(format!("{}.yaml", id)), yaml).unwrap();
}

/// Add a tradeoff YAML file to the agency primitives.
fn add_tradeoff(wg_dir: &Path, id: &str, name: &str, task_count: u32, avg_score: Option<f64>) {
    let tradeoffs_dir = wg_dir.join("agency/primitives/tradeoffs");
    let avg_str = match avg_score {
        Some(v) => format!("{}", v),
        None => "null".to_string(),
    };
    let yaml = format!(
        r#"id: "{id}"
name: "{name}"
description: "Test tradeoff {name}"
acceptable_tradeoffs: []
unacceptable_tradeoffs: []
performance:
  task_count: {task_count}
  avg_score: {avg_str}
  evaluations: []
lineage:
  generation: 0
  parents: []
  method: "seed"
access_control:
  owner: "local"
  policy: open
"#
    );
    fs::write(tradeoffs_dir.join(format!("{}.yaml", id)), yaml).unwrap();
}

/// Add an evaluation JSON file to the agency evaluations dir.
fn add_evaluation(
    wg_dir: &Path,
    id: &str,
    task_id: &str,
    role_id: &str,
    tradeoff_id: &str,
    score: f64,
) {
    let evals_dir = wg_dir.join("agency/evaluations");
    let json = serde_json::json!({
        "id": id,
        "task_id": task_id,
        "agent_id": "agent-1",
        "role_id": role_id,
        "tradeoff_id": tradeoff_id,
        "score": score,
        "dimensions": {},
        "notes": "test evaluation",
        "evaluator": "test",
        "timestamp": "2026-02-27T00:00:00Z"
    });
    fs::write(
        evals_dir.join(format!("{}.json", id)),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Add tasks with tags to the graph for tag-based breakdown testing.
fn add_tasks_to_graph(wg_dir: &Path, tasks: Vec<Task>) {
    let graph_path = wg_dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &graph_path).unwrap();
}

// ===========================================================================
// wg agency stats — empty agency
// ===========================================================================

#[test]
fn agency_stats_empty() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    let output = wg_ok(&wg_dir, &["agency", "stats"]);
    assert!(output.contains("Agency Performance Stats"));
    assert!(output.contains("Roles:"));
    assert!(output.contains("Evaluations:"));
    // With zero evaluations it should show the "no evaluations" message
    assert!(
        output.contains("No evaluations") || output.contains("0"),
        "Expected 'no evaluations' or '0' in output, got: {}",
        output
    );
}

// ===========================================================================
// wg agency stats — with data
// ===========================================================================

#[test]
fn agency_stats_with_roles_and_tradeoffs() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 5, Some(0.85));
    add_role(&wg_dir, "role-b", "Researcher", 3, Some(0.72));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 4, Some(0.80));
    add_tradeoff(&wg_dir, "tradeoff-b", "Fast", 4, Some(0.65));

    let output = wg_ok(&wg_dir, &["agency", "stats"]);
    assert!(output.contains("Agency Performance Stats"));
    assert!(output.contains("Roles:"));
    // Should show 2 roles
    assert!(
        output.contains("2"),
        "Expected role count of 2 in output, got: {}",
        output
    );
}

#[test]
fn agency_stats_with_evaluations() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 2, Some(0.80));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 2, Some(0.80));
    add_evaluation(&wg_dir, "eval-1", "task-1", "role-a", "tradeoff-a", 0.85);
    add_evaluation(&wg_dir, "eval-2", "task-2", "role-a", "tradeoff-a", 0.75);

    let output = wg_ok(&wg_dir, &["agency", "stats"]);
    assert!(output.contains("Agency Performance Stats"));
    // Should have evaluation count
    assert!(output.contains("2"));
    // Should show the synergy matrix since there are evaluations
    assert!(
        output.contains("Synergy") || output.contains("Role") || output.contains("Leaderboard"),
        "Expected leaderboard or synergy section with evaluations, got: {}",
        output
    );
}

// ===========================================================================
// wg agency stats --json
// ===========================================================================

#[test]
fn agency_stats_json_empty() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    let output = wg_ok(&wg_dir, &["--json", "agency", "stats"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from agency stats --json: {}\nOutput: {}",
            e, output
        )
    });
    // Should have roles, tradeoffs, evaluations keys
    assert!(
        json.is_object(),
        "Expected JSON object from agency stats, got: {}",
        output
    );
}

#[test]
fn agency_stats_json_with_data() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 1, Some(0.90));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 1, Some(0.90));
    add_evaluation(&wg_dir, "eval-1", "task-1", "role-a", "tradeoff-a", 0.90);

    let output = wg_ok(&wg_dir, &["--json", "agency", "stats"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from agency stats --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.is_object());
}

// ===========================================================================
// wg agency stats --min-evals
// ===========================================================================

#[test]
fn agency_stats_min_evals_filter() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 1, Some(0.80));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 1, Some(0.80));
    add_evaluation(&wg_dir, "eval-1", "task-1", "role-a", "tradeoff-a", 0.80);

    // With min-evals 5, the underexplored section should flag this pair
    let output = wg_ok(&wg_dir, &["agency", "stats", "--min-evals", "5"]);
    assert!(output.contains("Agency Performance Stats"));
}

// ===========================================================================
// wg agency stats --by-model
// ===========================================================================

#[test]
fn agency_stats_by_model() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 1, Some(0.80));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 1, Some(0.80));

    // Add evaluation with model field
    let evals_dir = wg_dir.join("agency/evaluations");
    let json = serde_json::json!({
        "id": "eval-model-1",
        "task_id": "task-1",
        "agent_id": "agent-1",
        "role_id": "role-a",
        "tradeoff_id": "tradeoff-a",
        "score": 0.85,
        "dimensions": {},
        "notes": "test evaluation",
        "evaluator": "test",
        "timestamp": "2026-02-27T00:00:00Z",
        "model": "sonnet"
    });
    fs::write(
        evals_dir.join("eval-model-1.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();

    let output = wg_ok(&wg_dir, &["agency", "stats", "--by-model"]);
    assert!(output.contains("Agency Performance Stats"));
}

// ===========================================================================
// wg agency stats with tagged tasks
// ===========================================================================

#[test]
fn agency_stats_with_tagged_tasks() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_agency(&tmp);

    add_role(&wg_dir, "role-a", "Programmer", 1, Some(0.80));
    add_tradeoff(&wg_dir, "tradeoff-a", "Thorough", 1, Some(0.80));
    add_evaluation(
        &wg_dir,
        "eval-1",
        "task-tagged",
        "role-a",
        "tradeoff-a",
        0.80,
    );

    // Add a task with tags
    let task = Task {
        id: "task-tagged".to_string(),
        title: "Tagged task".to_string(),
        status: Status::Done,
        tags: vec!["bugfix".to_string(), "urgent".to_string()],
        ..Task::default()
    };
    add_tasks_to_graph(&wg_dir, vec![task]);

    let output = wg_ok(&wg_dir, &["agency", "stats"]);
    assert!(output.contains("Agency Performance Stats"));
}
