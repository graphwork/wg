//! Integration tests for `wg exec` — the interactive CLI command.
//!
//! Tests cover:
//! 1. Dry-run mode (interactive + shell) — prints context/env without executing
//! 2. Task claiming — shell exec transitions open → in-progress → done/failed
//! 3. Invalid task — graceful error on nonexistent task ID
//! 4. Already done/failed — appropriate error messages
//! 5. Env var assembly — WG_TASK_ID, WG_EXECUTOR_TYPE, WG_MODEL etc.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{Node, Status, Task, WorkGraph};
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

// ===========================================================================
// 1. Dry-run mode (interactive) — prints env vars + assembled prompt
// ===========================================================================

#[test]
fn exec_dry_run_prints_env_vars_and_prompt() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("dry-task", "Dry run test task", Status::Open);
    task.description = Some("This is a test task description.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(&wg_dir, &["exec", "dry-task", "--dry-run"]);

    // Should contain env var section
    assert!(
        output.contains("Environment Variables"),
        "dry-run should print env vars header. Got:\n{}",
        output
    );
    assert!(
        output.contains("WG_TASK_ID=dry-task"),
        "dry-run should show WG_TASK_ID. Got:\n{}",
        output
    );
    assert!(
        output.contains("Assembled Prompt"),
        "dry-run should print assembled prompt header. Got:\n{}",
        output
    );

    // Task should remain open (not claimed)
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("dry-task").unwrap();
    assert_eq!(
        t.status,
        Status::Open,
        "dry-run should not change task status"
    );
}

#[test]
fn exec_dry_run_shell_mode_prints_command() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("shell-dry", "Shell dry run", Status::Open);
    task.exec = Some("echo hello world".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(&wg_dir, &["exec", "shell-dry", "--shell", "--dry-run"]);

    // Should show the command
    assert!(
        output.contains("echo hello world"),
        "shell dry-run should show exec command. Got:\n{}",
        output
    );
    // Should mention "Would execute" (not actual execution output)
    assert!(
        output.contains("Would execute"),
        "shell dry-run should say 'Would execute'. Got:\n{}",
        output
    );

    // Task still open
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("shell-dry").unwrap();
    assert_eq!(t.status, Status::Open);
}

#[test]
fn exec_dry_run_includes_task_description_in_prompt() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("desc-task", "Task with description", Status::Open);
    task.description = Some("Implement the frobnicator module.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(&wg_dir, &["exec", "desc-task", "--dry-run"]);

    assert!(
        output.contains("Implement the frobnicator module"),
        "dry-run prompt should contain task description. Got:\n{}",
        output
    );
}

// ===========================================================================
// 2. Task claiming — shell exec transitions status
// ===========================================================================

#[test]
fn exec_shell_claims_open_task_and_runs_command() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("claim-t", "Claimable shell task", Status::Open);
    task.exec = Some("echo claimed-successfully".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(&wg_dir, &["exec", "claim-t", "--shell"]);

    // Should show it claimed and executed
    assert!(
        output.contains("claimed-successfully"),
        "should show command output. Got:\n{}",
        output
    );

    // Task should be done after successful execution
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("claim-t").unwrap();
    assert_eq!(
        t.status,
        Status::Done,
        "successful shell exec should mark task done"
    );
    assert!(t.started_at.is_some(), "should record start time");
    assert!(t.completed_at.is_some(), "should record completion time");
}

#[test]
fn exec_shell_failed_command_marks_task_failed() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("fail-t", "Failing shell task", Status::Open);
    task.exec = Some("exit 42".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_cmd(&wg_dir, &["exec", "fail-t", "--shell"]);

    // Command should fail
    assert!(
        !output.status.success(),
        "wg exec should fail when command exits non-zero"
    );

    // Task should be failed
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("fail-t").unwrap();
    assert_eq!(
        t.status,
        Status::Failed,
        "failed shell exec should mark task failed"
    );
    assert!(
        t.failure_reason.as_ref().unwrap().contains("42"),
        "failure reason should contain exit code"
    );
}

// ===========================================================================
// 3. Invalid task — graceful error on nonexistent task
// ===========================================================================

#[test]
fn exec_nonexistent_task_errors_gracefully() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("real", "Real task", Status::Open)]);

    let output = wg_cmd(&wg_dir, &["exec", "nonexistent-task", "--dry-run"]);

    assert!(
        !output.status.success(),
        "exec on nonexistent task should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("not found"),
        "should mention 'not found'. Got:\n{}",
        stderr
    );
}

#[test]
fn exec_shell_nonexistent_task_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp, vec![make_task("real", "Real task", Status::Open)]);

    let output = wg_cmd(&wg_dir, &["exec", "ghost-task", "--shell"]);

    assert!(
        !output.status.success(),
        "shell exec on nonexistent task should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("not found"),
        "should mention 'not found'. Got:\n{}",
        stderr
    );
}

// ===========================================================================
// 4. Already done / failed / abandoned — appropriate errors
// ===========================================================================

#[test]
fn exec_already_done_task_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![make_task("done-t", "Already done", Status::Done)],
    );

    // Interactive mode: should bail on done tasks
    let output = wg_cmd(&wg_dir, &["exec", "done-t", "--dry-run"]);
    assert!(!output.status.success(), "exec on done task should fail");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("already done"),
        "should say 'already done'. Got:\n{}",
        stderr
    );
}

#[test]
fn exec_shell_already_done_task_errors() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("done-s", "Done shell", Status::Done);
    task.exec = Some("echo nope".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_cmd(&wg_dir, &["exec", "done-s", "--shell"]);
    assert!(
        !output.status.success(),
        "shell exec on done task should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("already done"),
        "should say 'already done'. Got:\n{}",
        stderr
    );
}

#[test]
fn exec_failed_task_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![make_task("fail-t2", "Failed task", Status::Failed)],
    );

    let output = wg_cmd(&wg_dir, &["exec", "fail-t2", "--dry-run"]);
    assert!(!output.status.success(), "exec on failed task should fail");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("failed") || stderr.contains("retry"),
        "should mention failure or retry. Got:\n{}",
        stderr
    );
}

#[test]
fn exec_in_progress_task_prints_note() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("ip-task", "In-progress task", Status::InProgress);
    task.assigned = Some("other-agent".to_string());
    task.description = Some("Some work being done.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    // Interactive dry-run on in-progress task should succeed with a note
    let output = wg_cmd(&wg_dir, &["exec", "ip-task", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        output.status.success(),
        "exec dry-run on in-progress task should succeed. stderr:\n{}",
        stderr
    );
    // Should print a note about already in-progress
    assert!(
        stderr.contains("already in-progress"),
        "should note task is already in-progress. stderr:\n{}",
        stderr
    );
    // But should still show the context
    assert!(
        stdout.contains("WG_TASK_ID=ip-task"),
        "should still show env vars even for in-progress task. stdout:\n{}",
        stdout
    );
}

// ===========================================================================
// 5. Env var assembly — verify expected vars in dry-run output
// ===========================================================================

#[test]
fn exec_dry_run_contains_all_expected_env_vars() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("env-task", "Env var test", Status::Open);
    task.description = Some("Testing env var assembly.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(&wg_dir, &["exec", "env-task", "--dry-run"]);

    // Required env vars
    assert!(
        output.contains("WG_TASK_ID=env-task"),
        "missing WG_TASK_ID. Got:\n{}",
        output
    );
    assert!(
        output.contains("WG_AGENT_ID="),
        "missing WG_AGENT_ID. Got:\n{}",
        output
    );
    assert!(
        output.contains("WG_EXECUTOR_TYPE=claude"),
        "missing WG_EXECUTOR_TYPE. Got:\n{}",
        output
    );
    assert!(
        output.contains("WG_USER="),
        "missing WG_USER. Got:\n{}",
        output
    );
}

#[test]
fn exec_dry_run_with_model_flag_sets_wg_model() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("model-task", "Model flag test", Status::Open);
    task.description = Some("Testing model override.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(
        &wg_dir,
        &["exec", "model-task", "--dry-run", "--model", "opus"],
    );

    assert!(
        output.contains("WG_MODEL=opus"),
        "should contain WG_MODEL=opus when --model flag specified. Got:\n{}",
        output
    );
}

#[test]
fn exec_dry_run_with_actor_flag_sets_agent_id() {
    let tmp = TempDir::new().unwrap();
    let mut task = make_task("actor-task", "Actor flag test", Status::Open);
    task.description = Some("Testing actor override.".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    let output = wg_ok(
        &wg_dir,
        &["exec", "actor-task", "--dry-run", "--actor", "my-agent"],
    );

    assert!(
        output.contains("WG_AGENT_ID=exec-my-agent"),
        "should set WG_AGENT_ID based on actor flag. Got:\n{}",
        output
    );
}

// ===========================================================================
// Additional: set/clear exec commands
// ===========================================================================

#[test]
fn exec_set_and_clear_command() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(
        &tmp,
        vec![make_task("setclear", "Set/clear test", Status::Open)],
    );

    // Set exec command
    let output = wg_ok(&wg_dir, &["exec", "setclear", "--set", "echo test-cmd"]);
    assert!(
        output.contains("Set exec command"),
        "should confirm setting exec command. Got:\n{}",
        output
    );

    // Verify it was set
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("setclear").unwrap();
    assert_eq!(t.exec, Some("echo test-cmd".to_string()));

    // Clear exec command
    let output = wg_ok(&wg_dir, &["exec", "setclear", "--clear"]);
    assert!(
        output.contains("Cleared exec command"),
        "should confirm clearing exec command. Got:\n{}",
        output
    );

    // Verify it was cleared
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let t = graph.get_task("setclear").unwrap();
    assert!(t.exec.is_none());
}

// ===========================================================================
// Dependency context in dry-run
// ===========================================================================

#[test]
fn exec_dry_run_includes_dependency_context() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");

    let mut graph = WorkGraph::new();

    // Create a done dependency task with artifacts
    let mut dep = Task {
        id: "dep-task".to_string(),
        title: "Dependency task".to_string(),
        status: Status::Done,
        description: Some("I produced the widget module.".to_string()),
        ..Task::default()
    };
    dep.artifacts.push("src/widget.rs".to_string());
    graph.add_node(Node::Task(dep));

    // Create the target task that depends on dep-task
    let mut target = Task {
        id: "target-task".to_string(),
        title: "Target task".to_string(),
        status: Status::Open,
        description: Some("Use the widget module.".to_string()),
        ..Task::default()
    };
    target.after.push("dep-task".to_string());
    graph.add_node(Node::Task(target));

    save_graph(&graph, &graph_path).unwrap();

    let output = wg_ok(&wg_dir, &["exec", "target-task", "--dry-run"]);

    // The assembled prompt should mention the dependency
    assert!(
        output.contains("dep-task") || output.contains("Dependency task"),
        "dry-run prompt should include dependency context. Got:\n{}",
        output
    );
}

#[test]
fn exec_no_workgraph_initialized_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    // Don't create the directory or graph file

    let output = wg_cmd(&wg_dir, &["exec", "any-task", "--dry-run"]);
    assert!(
        !output.status.success(),
        "exec without initialized WG should fail"
    );
}
