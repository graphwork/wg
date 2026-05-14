//! Integration tests for the placement structured output system.
//!
//! Tests the end-to-end flow: JSONL stream → `wg apply-placement` CLI →
//! graph updated, covering valid edits, no-ops, unparseable output,
//! empty output (agent death), and multi-line output.

use std::fs;
use std::io::Write;
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

fn make_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::Open,
        ..Task::default()
    }
}

/// Set up a temp dir with a `.wg/` directory containing a graph.
fn setup_graph(tasks: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();

    let mut graph = WorkGraph::new();
    for &(id, title) in tasks {
        graph.add_node(Node::Task(make_task(id, title)));
    }
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();
    tmp
}

/// Write a JSONL stream file simulating Claude assistant output.
fn write_stream_file(dir: &Path, text_content: &str) -> PathBuf {
    let stream_path = dir.join("raw_stream.jsonl");
    let mut f = fs::File::create(&stream_path).unwrap();

    // System event
    writeln!(
        f,
        r#"{{"type":"system","system":"You are a placement agent"}}"#
    )
    .unwrap();

    // Assistant event with text content
    let escaped = serde_json::to_string(text_content).unwrap();
    let escaped_inner = &escaped[1..escaped.len() - 1];
    writeln!(
        f,
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
        escaped_inner
    )
    .unwrap();

    // Result event
    writeln!(
        f,
        r#"{{"type":"result","usage":{{"input_tokens":100,"output_tokens":50}}}}"#
    )
    .unwrap();

    f.flush().unwrap();
    stream_path
}

/// Run `wg apply-placement` and return (success, stderr).
fn run_apply_placement(wg_dir: &Path, output_dir: &Path, source_task_id: &str) -> (bool, String) {
    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args([
            "apply-placement",
            output_dir.to_str().unwrap(),
            source_task_id,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), stderr)
}

// ---------------------------------------------------------------------------
// Test 1: Valid wg edit command → edges applied to graph
// ---------------------------------------------------------------------------

#[test]
fn test_placement_edit_applied_to_graph() {
    let tmp = setup_graph(&[
        ("my-task", "My Task"),
        ("dep-a", "Dependency A"),
        ("dep-b", "Dependency B"),
    ]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "I'll analyze the task dependencies.\n\nAfter reviewing, this task should depend on dep-a and come before dep-b.\n\nwg edit my-task --after dep-a --before dep-b",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "apply-placement should succeed. stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("applied placement"),
        "Should report applied placement. stderr: {}",
        stderr
    );

    // Verify graph was updated
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert!(
        task.after.contains(&"dep-a".to_string()),
        "Task should have dep-a in after list, got: {:?}",
        task.after
    );
    assert!(
        task.before.contains(&"dep-b".to_string()),
        "Task should have dep-b in before list, got: {:?}",
        task.before
    );

    // Verify log entry was added
    assert!(
        task.log
            .iter()
            .any(|l| l.message.contains("Placement applied")),
        "Task should have a placement log entry"
    );
}

// ---------------------------------------------------------------------------
// Test 2: no-op output → no graph changes
// ---------------------------------------------------------------------------

#[test]
fn test_placement_noop_no_changes() {
    let tmp = setup_graph(&[("my-task", "My Task")]);
    let wg_dir = tmp.path().join(".wg");

    // Capture graph state before
    let graph_before = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let after_before = graph_before.get_task("my-task").unwrap().after.clone();

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "The task doesn't need any dependency changes.\n\nno-op",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "apply-placement with no-op should succeed. stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("no-op"),
        "Should report no-op. stderr: {}",
        stderr
    );

    // Verify graph was NOT changed
    let graph_after = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let after_after = graph_after.get_task("my-task").unwrap().after.clone();
    assert_eq!(
        after_before, after_after,
        "Graph should not be modified on no-op"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Garbage/unparseable output → non-zero exit (failure)
// ---------------------------------------------------------------------------

#[test]
fn test_placement_unparseable_fails() {
    let tmp = setup_graph(&[("my-task", "My Task")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "I'm confused about what to do.\n\nHere is some random text that isn't a command.",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(!success, "apply-placement with garbage output should fail");
    assert!(
        stderr.contains("unparseable") || stderr.contains("FAILED"),
        "Should report unparseable error. stderr: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test 4: Agent death (empty stream / no assistant output) → failure
// ---------------------------------------------------------------------------

#[test]
fn test_placement_empty_output_fails() {
    let tmp = setup_graph(&[("my-task", "My Task")]);
    let wg_dir = tmp.path().join(".wg");

    // Create stream with NO assistant output (simulating agent crash/death)
    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    let stream_path = output_dir.join("raw_stream.jsonl");
    let mut f = fs::File::create(&stream_path).unwrap();
    writeln!(
        f,
        r#"{{"type":"system","system":"You are a placement agent"}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"type":"result","usage":{{"input_tokens":100,"output_tokens":0}}}}"#
    )
    .unwrap();
    f.flush().unwrap();

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(!success, "apply-placement with empty output should fail");
    assert!(
        stderr.contains("no text output") || stderr.contains("FAILED"),
        "Should report empty output error. stderr: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test 5: Multi-line output, only last line is the command → works
// ---------------------------------------------------------------------------

#[test]
fn test_placement_multiline_last_line_command() {
    let tmp = setup_graph(&[("my-task", "My Task"), ("prereq", "Prerequisite")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "Let me analyze the dependency graph carefully.\n\
         \n\
         Looking at the active tasks:\n\
         - prereq (Prerequisite) - this is clearly a dependency\n\
         - other-task (Other) - not related\n\
         \n\
         The task 'my-task' should wait for 'prereq' to complete first.\n\
         This is because prereq sets up the foundation that my-task needs.\n\
         \n\
         wg edit my-task --after prereq",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "Multi-line output with command on last line should succeed. stderr: {}",
        stderr
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert!(
        task.after.contains(&"prereq".to_string()),
        "Task should have prereq in after list, got: {:?}",
        task.after
    );
}

// ---------------------------------------------------------------------------
// Test 6: Wrong task ID in command → failure (security guard)
// ---------------------------------------------------------------------------

#[test]
fn test_placement_wrong_task_id_fails() {
    let tmp = setup_graph(&[("my-task", "My Task"), ("other-task", "Other Task")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(&output_dir, "wg edit other-task --after my-task");

    let (success, _stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(!success, "apply-placement with wrong task ID should fail");
}

// ---------------------------------------------------------------------------
// Test 7: Comma-separated deps → all applied
// ---------------------------------------------------------------------------

#[test]
fn test_placement_comma_separated_deps() {
    let tmp = setup_graph(&[
        ("my-task", "My Task"),
        ("dep-1", "Dep 1"),
        ("dep-2", "Dep 2"),
        ("dep-3", "Dep 3"),
    ]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(&output_dir, "wg edit my-task --after dep-1,dep-2,dep-3");

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "Comma-separated deps should work. stderr: {}",
        stderr
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert_eq!(
        task.after,
        vec![
            "dep-1".to_string(),
            "dep-2".to_string(),
            "dep-3".to_string()
        ],
        "All comma-separated deps should be applied"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Backtick-wrapped command → accepted
// ---------------------------------------------------------------------------

#[test]
fn test_placement_backtick_wrapped_command() {
    let tmp = setup_graph(&[("my-task", "My Task"), ("foundation", "Foundation")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "Based on my analysis:\n\n`wg edit my-task --after foundation`",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "Backtick-wrapped command should work. stderr: {}",
        stderr
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert!(
        task.after.contains(&"foundation".to_string()),
        "Backtick-wrapped command should be applied"
    );
}

// ---------------------------------------------------------------------------
// Test 9: No-op case insensitive
// ---------------------------------------------------------------------------

#[test]
fn test_placement_noop_case_insensitive() {
    let tmp = setup_graph(&[("my-task", "My Task")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(&output_dir, "No changes needed.\n\nNo-Op");

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "Case-insensitive no-op should succeed. stderr: {}",
        stderr
    );
    assert!(stderr.contains("no-op"));
}

// ---------------------------------------------------------------------------
// Test 10: Trailing whitespace after command → still works
// ---------------------------------------------------------------------------

#[test]
fn test_placement_trailing_whitespace() {
    let tmp = setup_graph(&[("my-task", "My Task"), ("dep", "Dep")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(
        &output_dir,
        "Reasoning here.\n\nwg edit my-task --after dep\n\n  \n",
    );

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "Trailing whitespace should be ignored. stderr: {}",
        stderr
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert!(task.after.contains(&"dep".to_string()));
}

// ---------------------------------------------------------------------------
// Test 11: --blocked-by alias works (same as --after)
// ---------------------------------------------------------------------------

#[test]
fn test_placement_blocked_by_alias() {
    let tmp = setup_graph(&[("my-task", "My Task"), ("blocker", "Blocker")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(&output_dir, "wg edit my-task --blocked-by blocker");

    let (success, stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(
        success,
        "--blocked-by alias should work. stderr: {}",
        stderr
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("my-task").unwrap();
    assert!(
        task.after.contains(&"blocker".to_string()),
        "--blocked-by should map to after. got: {:?}",
        task.after
    );
}

// ---------------------------------------------------------------------------
// Test 12: wg edit with no edges → failure
// ---------------------------------------------------------------------------

#[test]
fn test_placement_edit_no_edges_fails() {
    let tmp = setup_graph(&[("my-task", "My Task")]);
    let wg_dir = tmp.path().join(".wg");

    let output_dir = tmp.path().join("agent-output");
    fs::create_dir_all(&output_dir).unwrap();
    write_stream_file(&output_dir, "wg edit my-task");

    let (success, _stderr) = run_apply_placement(&wg_dir, &output_dir, "my-task");
    assert!(!success, "wg edit with no --after or --before should fail");
}
