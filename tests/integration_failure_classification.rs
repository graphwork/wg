//! Integration tests for failure classification (Option A: wg fail --class).
//!
//! Verifies the end-to-end path:
//!   raw_stream.jsonl → classify-failure (wg CLI) → wg fail --class → graph.jsonl → wg show

use std::fs;
use std::io::Write;
use tempfile::TempDir;
use workgraph::graph::{FailureClass, Node, Status, Task, WorkGraph};
use workgraph::parser::{load_graph, save_graph};

/// Initialize a workgraph for CLI invocation (creates .wg/graph.jsonl).
fn setup_wg(project_dir: &std::path::Path, tasks: Vec<Task>) -> std::path::PathBuf {
    let wg_dir = project_dir.join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let mut g = WorkGraph::new();
    for t in tasks {
        g.add_node(Node::Task(t));
    }
    save_graph(&g, &wg_dir.join("graph.jsonl")).unwrap();
    wg_dir
}

fn make_in_progress_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::InProgress,
        ..Task::default()
    }
}

/// Run a `wg` subcommand inside `dir` using `cargo run` and return (stdout, stderr, exit_code).
fn run_wg(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wg"));
    cmd.current_dir(dir).args(args);
    let output = cmd.output().expect("Failed to run wg binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Verify FailureClass::Display outputs the expected kebab strings.
#[test]
fn test_failure_class_display() {
    use FailureClass::*;
    assert_eq!(ApiError400Document.to_string(), "api-error-400-document");
    assert_eq!(ApiError429RateLimit.to_string(), "api-error-429-rate-limit");
    assert_eq!(ApiError5xxTransient.to_string(), "api-error-5xx-transient");
    assert_eq!(AgentHardTimeout.to_string(), "agent-hard-timeout");
    assert_eq!(AgentExitNonzero.to_string(), "agent-exit-nonzero");
    assert_eq!(WrapperInternal.to_string(), "wrapper-internal");
}

/// Verify legacy rows (no failure_class field in JSON) deserialize with None.
#[test]
fn test_legacy_row_failure_class_defaults_to_none() {
    // Minimal legacy task JSON without failure_class field (uses kind:"task" format)
    let jsonl = r#"{"kind":"task","id":"leg","title":"Legacy","status":"failed","failure_reason":"old error"}"#;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("graph.jsonl");
    fs::write(&path, jsonl).unwrap();

    let graph = load_graph(&path).unwrap();
    let task = graph.get_task("leg").unwrap();
    assert_eq!(task.status, Status::Failed);
    assert_eq!(task.failure_class, None, "Legacy rows must default to None");
    assert_eq!(task.failure_reason.as_deref(), Some("old error"));
}

/// Verify all FailureClass variants survive round-trip through graph.jsonl.
#[test]
fn test_failure_class_serde_round_trip() {
    use FailureClass::*;

    let classes = [
        ApiError400Document,
        ApiError429RateLimit,
        ApiError5xxTransient,
        AgentHardTimeout,
        AgentExitNonzero,
        WrapperInternal,
    ];

    for class in classes {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let task_id = format!("t-{}", class);
        let mut task = make_in_progress_task(&task_id, "test");
        task.status = Status::Failed;
        task.failure_reason = Some("test failure".to_string());
        task.failure_class = Some(class);

        fs::create_dir_all(dir).unwrap();
        let path = dir.join("graph.jsonl");
        let mut g = WorkGraph::new();
        g.add_node(Node::Task(task));
        save_graph(&g, &path).unwrap();

        let loaded = load_graph(&path).unwrap();
        let loaded_task = loaded.get_task(&task_id).unwrap();
        assert_eq!(
            loaded_task.failure_class,
            Some(class),
            "Round-trip failed for {:?}",
            class
        );
    }
}

/// Verify `wg fail --class api-error-400-document` round-trips through graph.jsonl.
#[test]
fn test_wg_fail_with_class_persists() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path();
    let wg_dir = setup_wg(project, vec![make_in_progress_task("t1", "Test task")]);

    let (stdout, stderr, code) = run_wg(
        project,
        &[
            "fail",
            "t1",
            "--class",
            "api-error-400-document",
            "--reason",
            "Could not process PDF",
        ],
    );
    assert_eq!(code, 0, "wg fail should exit 0. stdout={stdout} stderr={stderr}");

    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("t1").unwrap();
    assert_eq!(task.status, Status::Failed);
    assert_eq!(
        task.failure_class,
        Some(FailureClass::ApiError400Document),
        "failure_class should be api-error-400-document"
    );
    assert_eq!(
        task.failure_reason.as_deref(),
        Some("Could not process PDF")
    );
}

/// Verify `wg show` output contains the failure_class and operator hint.
#[test]
fn test_wg_show_renders_failure_class() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path();
    setup_wg(project, vec![make_in_progress_task("t1", "Test task")]);

    // Mark as failed with class
    let (_, _, code) = run_wg(
        project,
        &[
            "fail",
            "t1",
            "--class",
            "api-error-400-document",
            "--reason",
            "PDF error",
        ],
    );
    assert_eq!(code, 0);

    let (stdout, stderr, code) = run_wg(project, &["show", "t1"]);
    assert_eq!(code, 0, "wg show should exit 0. stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("api-error-400-document"),
        "wg show should contain failure_class. Got: {stdout}"
    );
    assert!(
        stdout.contains("fix the input"),
        "wg show should contain operator hint. Got: {stdout}"
    );
}

/// Verify `wg classify-failure` prints the correct class for a pdf-400 stream.
#[test]
fn test_classify_failure_subcommand_pdf_400() {
    let tmp = TempDir::new().unwrap();
    let raw_stream = tmp.path().join("raw_stream.jsonl");
    let mut f = fs::File::create(&raw_stream).unwrap();
    writeln!(
        f,
        r#"{{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":400,"message":"Could not process PDF"}}"#
    )
    .unwrap();
    drop(f);

    // Run classify-failure without a real workgraph dir (it doesn't need one)
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wg"));
    cmd.current_dir(tmp.path())
        .args(["classify-failure", "--raw-stream", &raw_stream.to_string_lossy(), "--exit-code", "1"]);
    let output = cmd.output().expect("Failed to run wg binary");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        stdout, "api-error-400-document",
        "classify-failure should output the class. Got: {stdout}"
    );
}

/// Verify `wg classify-failure` outputs agent-hard-timeout for exit code 124.
#[test]
fn test_classify_failure_subcommand_hard_timeout() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wg"));
    cmd.current_dir(tmp.path())
        .args(["classify-failure", "--exit-code", "124"]);
    let output = cmd.output().expect("Failed to run wg binary");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        stdout, "agent-hard-timeout",
        "classify-failure should output agent-hard-timeout for exit 124. Got: {stdout}"
    );
}
