//! Integration tests for phantom edge prevention.
//!
//! Tests cover:
//! - `wg add --after`: valid dep accepted, invalid dep rejected, paused defers, allow-phantom opts in
//! - `wg edit --add-after`: same validation
//! - `wg publish`: batch with cross-refs works, batch with dangling ref errors
//! - Retroactive backlink repair
//! - `phantom_blockers()` query
//! - `why-blocked` phantom labeling

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{Node, Task, WorkGraph};
use workgraph::parser::{load_graph, save_graph};

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
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
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

fn wg_fail(wg_dir: &Path, args: &[&str]) -> (String, String) {
    let output = wg_cmd(wg_dir, args);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "wg {:?} should have failed but succeeded.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    (stdout, stderr)
}

fn make_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        ..Task::default()
    }
}

fn setup_workgraph(tmp: &TempDir, tasks: Vec<Task>) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

fn graph_path(wg_dir: &Path) -> PathBuf {
    wg_dir.join("graph.jsonl")
}

// ===========================================================================
// wg add --after: valid dependency accepted
// ===========================================================================

#[test]
fn add_with_valid_after_succeeds() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("dep-task", "Dependency")]);

    let out = wg_ok(
        &wg_dir,
        &["add", "New Task", "--after", "dep-task", "--no-place"],
    );
    assert!(out.contains("Added task"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let new_task = graph
        .tasks()
        .find(|t| t.title == "New Task")
        .expect("new task should exist");
    assert!(new_task.after.contains(&"dep-task".to_string()));
}

// ===========================================================================
// wg add --after: invalid dependency rejected (strict default)
// ===========================================================================

#[test]
fn add_with_nonexistent_after_fails() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("existing", "Existing")]);

    let (stdout, stderr) = wg_fail(
        &wg_dir,
        &[
            "add",
            "New Task",
            "--after",
            "nonexistent-task-id",
            "--no-place",
        ],
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("does not exist"),
        "Error should mention dependency does not exist. Got: {}",
        combined
    );
}

// ===========================================================================
// wg add --after: fuzzy suggestion on rejection
// ===========================================================================

#[test]
fn add_with_typo_suggests_correction() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("build-artifacts", "Build")]);

    let (stdout, stderr) = wg_fail(
        &wg_dir,
        &["add", "Deploy", "--after", "bild-artifacts", "--no-place"],
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("build-artifacts"),
        "Error should suggest 'build-artifacts'. Got: {}",
        combined
    );
}

// ===========================================================================
// wg add --after --paused: deferred validation (warning only)
// ===========================================================================

#[test]
fn add_paused_with_nonexistent_after_succeeds() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    // Adding with --paused should succeed even with phantom deps
    let out = wg_ok(
        &wg_dir,
        &["add", "Deferred Task", "--after", "future-task", "--paused"],
    );
    assert!(out.contains("Added task"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let task = graph
        .tasks()
        .find(|t| t.title == "Deferred Task")
        .expect("deferred task should exist");
    assert!(task.after.contains(&"future-task".to_string()));
    assert!(task.paused);
}

// ===========================================================================
// wg add --after --allow-phantom: explicit opt-in
// ===========================================================================

#[test]
fn add_allow_phantom_with_nonexistent_after_succeeds() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    let out = wg_ok(
        &wg_dir,
        &[
            "add",
            "Phantom Task",
            "--after",
            "ghost-dep",
            "--allow-phantom",
            "--no-place",
        ],
    );
    assert!(out.contains("Added task"));

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let task = graph
        .tasks()
        .find(|t| t.title == "Phantom Task")
        .expect("phantom task should exist");
    assert!(task.after.contains(&"ghost-dep".to_string()));
}

// ===========================================================================
// wg edit --add-after: valid dependency accepted
// ===========================================================================

#[test]
fn edit_add_after_valid_dep_succeeds() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![make_task("task-a", "Task A"), make_task("task-b", "Task B")],
    );

    wg_ok(&wg_dir, &["edit", "task-a", "--add-after", "task-b"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let a = graph.get_task("task-a").unwrap();
    assert!(a.after.contains(&"task-b".to_string()));
}

// ===========================================================================
// wg edit --add-after: invalid dependency rejected
// ===========================================================================

#[test]
fn edit_add_after_nonexistent_dep_fails() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("task-a", "Task A")]);

    let (stdout, stderr) = wg_fail(&wg_dir, &["edit", "task-a", "--add-after", "nonexistent"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("does not exist"),
        "Error should mention dependency does not exist. Got: {}",
        combined
    );
}

// ===========================================================================
// wg edit --add-after --allow-phantom: explicit opt-in
// ===========================================================================

#[test]
fn edit_add_after_allow_phantom_succeeds() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("task-a", "Task A")]);

    wg_ok(
        &wg_dir,
        &["edit", "task-a", "--add-after", "ghost", "--allow-phantom"],
    );

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let a = graph.get_task("task-a").unwrap();
    assert!(a.after.contains(&"ghost".to_string()));
}

// ===========================================================================
// wg publish: batch with cross-references works
// ===========================================================================

#[test]
fn publish_batch_with_cross_refs_works() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    // Create two paused tasks that reference each other
    wg_ok(
        &wg_dir,
        &[
            "add", "Task A", "--id", "batch-a", "--paused", "--after", "batch-b",
        ],
    );
    wg_ok(&wg_dir, &["add", "Task B", "--id", "batch-b", "--paused"]);

    // Publish should succeed because both tasks exist at publish time
    wg_ok(&wg_dir, &["publish", "batch-a"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let a = graph.get_task("batch-a").unwrap();
    assert!(!a.paused, "batch-a should be unpaused after publish");
}

// ===========================================================================
// wg publish: batch with dangling reference errors
// ===========================================================================

#[test]
fn publish_with_dangling_ref_fails() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    // Create a paused task with a phantom dependency that is never created
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Dangling Task",
            "--id",
            "dangling",
            "--paused",
            "--after",
            "never-created",
        ],
    );

    // Publish should fail because "never-created" doesn't exist
    let (stdout, stderr) = wg_fail(&wg_dir, &["publish", "dangling"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("never-created"),
        "Error should reference the dangling dependency. Got: {}",
        combined
    );
}

// ===========================================================================
// Retroactive backlink repair
// ===========================================================================

#[test]
fn retroactive_backlink_repair_on_task_creation() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    // Create task A with phantom dep on B (using --allow-phantom)
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Task A",
            "--id",
            "task-a",
            "--after",
            "task-b",
            "--allow-phantom",
            "--no-place",
        ],
    );

    // Now create task B — the system should retroactively add 'task-a' to B's before list
    wg_ok(&wg_dir, &["add", "Task B", "--id", "task-b", "--no-place"]);

    let graph = load_graph(graph_path(&wg_dir)).unwrap();
    let b = graph.get_task("task-b").unwrap();
    assert!(
        b.before.contains(&"task-a".to_string()),
        "task-b.before should contain 'task-a' via retroactive repair. Got: {:?}",
        b.before
    );
}

// ===========================================================================
// phantom_blockers() query
// ===========================================================================

#[test]
fn phantom_blockers_query_detects_phantoms() {
    let mut graph = WorkGraph::new();
    let mut task = make_task("blocked", "Blocked Task");
    task.after = vec!["real-dep".to_string(), "phantom-dep".to_string()];
    let real = make_task("real-dep", "Real Dep");

    graph.add_node(Node::Task(task));
    graph.add_node(Node::Task(real));

    let phantoms = workgraph::query::phantom_blockers(graph.get_task("blocked").unwrap(), &graph);
    assert_eq!(phantoms, vec!["phantom-dep".to_string()]);
}

#[test]
fn phantom_blockers_query_empty_when_all_deps_exist() {
    let mut graph = WorkGraph::new();
    let mut task = make_task("child", "Child");
    task.after = vec!["parent".to_string()];
    let parent = make_task("parent", "Parent");

    graph.add_node(Node::Task(task));
    graph.add_node(Node::Task(parent));

    let phantoms = workgraph::query::phantom_blockers(graph.get_task("child").unwrap(), &graph);
    assert!(phantoms.is_empty());
}

// ===========================================================================
// why-blocked: phantom labeling
// ===========================================================================

#[test]
fn why_blocked_labels_phantom_deps() {
    let tmp = TempDir::new().unwrap();
    // Set up a graph with a task that has a phantom dependency
    let mut graph = WorkGraph::new();
    let mut task = make_task("my-task", "My Task");
    task.after = vec!["phantom-blocker".to_string()];
    graph.add_node(Node::Task(task));

    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();

    let output = wg_cmd(&wg_dir, &["why-blocked", "my-task"]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("DOES NOT EXIST") || stdout.contains("phantom"),
        "why-blocked should label phantom deps. Got: {}",
        stdout
    );
}

#[test]
fn why_blocked_json_includes_phantom_field() {
    let tmp = TempDir::new().unwrap();
    let mut graph = WorkGraph::new();
    let mut task = make_task("my-task", "My Task");
    task.after = vec!["phantom-blocker".to_string()];
    graph.add_node(Node::Task(task));

    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();

    let output = wg_cmd(&wg_dir, &["--json", "why-blocked", "my-task"]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("\"phantom\"") || stdout.contains("DOES NOT EXIST"),
        "why-blocked --json should include phantom info. Got: {}",
        stdout
    );
}

// ===========================================================================
// wg check: reports phantom edges as errors
// ===========================================================================

#[test]
fn check_reports_phantom_edges() {
    let tmp = TempDir::new().unwrap();
    let mut graph = WorkGraph::new();
    let mut task = make_task("task-with-phantom", "Task");
    task.after = vec!["nonexistent".to_string()];
    graph.add_node(Node::Task(task));

    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();

    let (stdout, stderr) = wg_fail(&wg_dir, &["check"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("nonexistent") && combined.contains("not found"),
        "wg check should report phantom edge. Got: {}",
        combined
    );
}

// ===========================================================================
// Error message includes hint about --paused and --allow-phantom
// ===========================================================================

#[test]
fn error_message_includes_hints() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![]);

    let (stdout, stderr) = wg_fail(&wg_dir, &["add", "Task", "--after", "nope", "--no-place"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("--paused") && combined.contains("--allow-phantom"),
        "Error should mention --paused and --allow-phantom hints. Got: {}",
        combined
    );
}
