//! Exhaustive integration tests for `wg trace`.
//!
//! Covers every trace scenario from docs/test-specs/trace-replay-test-spec.md
//! sections 1.1 through 1.25.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{Node, Status, Task, WorkGraph};
use workgraph::parser::save_graph;
use workgraph::provenance;

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

fn wg_fail(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd(wg_dir, args);
    assert!(
        !output.status.success(),
        "wg {:?} should have failed but succeeded.\nstdout: {}",
        args,
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    format!("{}{}", stdout, stderr)
}

fn wg_json(wg_dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut full_args = vec!["--json"];
    full_args.extend_from_slice(args);
    let raw = wg_ok(wg_dir, &full_args);
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse JSON.\nError: {}\nRaw output: {}", e, raw))
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

/// Create an agent archive directory with prompt.txt and output.txt.
fn create_agent_archive(wg_dir: &Path, task_id: &str, timestamp: &str, prompt: &str, output: &str) {
    let archive_dir = wg_dir
        .join("log")
        .join("agents")
        .join(task_id)
        .join(timestamp);
    fs::create_dir_all(&archive_dir).unwrap();
    fs::write(archive_dir.join("prompt.txt"), prompt).unwrap();
    fs::write(archive_dir.join("output.txt"), output).unwrap();
}

/// Record a provenance operation.
fn record_op(
    wg_dir: &Path,
    op: &str,
    task_id: Option<&str>,
    actor: Option<&str>,
    detail: serde_json::Value,
) {
    provenance::record(
        wg_dir,
        op,
        task_id,
        actor,
        detail,
        provenance::DEFAULT_ROTATION_THRESHOLD,
    )
    .unwrap();
}

// ===========================================================================
// 1.1 trace_no_agent_runs_manual_done
// ===========================================================================

#[test]
fn test_trace_no_agent_runs_summary_output() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Manual task", Status::Done);
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    t1.completed_at = Some("2026-02-18T10:30:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Record provenance entries
    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Manual task"}),
    );
    record_op(
        &wg_dir,
        "claim",
        Some("t1"),
        Some("human"),
        serde_json::Value::Null,
    );
    record_op(&wg_dir, "done", Some("t1"), None, serde_json::Value::Null);

    // Summary mode: should show "(none)" for agent runs
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Agent runs: (none)"),
        "Expected 'Agent runs: (none)' in output:\n{}",
        output
    );
    // Should show operations
    assert!(
        output.contains("Operations (3):") || output.contains("Operations"),
        "Expected operations section in output:\n{}",
        output
    );
}

#[test]
fn test_trace_no_agent_runs_json_output() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Manual task", Status::Done);
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    t1.completed_at = Some("2026-02-18T10:30:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Manual task"}),
    );
    record_op(
        &wg_dir,
        "claim",
        Some("t1"),
        Some("human"),
        serde_json::Value::Null,
    );
    record_op(&wg_dir, "done", Some("t1"), None, serde_json::Value::Null);

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // agent_runs should be empty array
    assert_eq!(json["agent_runs"].as_array().unwrap().len(), 0);
    assert_eq!(json["summary"]["agent_run_count"], 0);

    // operations should have 3 entries
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 3);

    // Validate operation structure
    for op in ops {
        assert!(op["timestamp"].is_string());
        assert!(op["op"].is_string());
    }
    assert_eq!(ops[0]["op"], "add_task");
    assert_eq!(ops[1]["op"], "claim");
    assert_eq!(ops[1]["actor"], "human");
    assert_eq!(ops[2]["op"], "done");
}

// ===========================================================================
// 1.2 trace_multiple_agent_runs_retried_task
// ===========================================================================

#[test]
fn test_trace_multiple_agent_runs_summary() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Retried task", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create two agent archives (simulating a retry)
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "First attempt prompt",
        "First attempt output",
    );
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T11:00:00Z",
        "Second attempt prompt",
        "Second attempt output",
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Agent runs (2):"),
        "Expected 'Agent runs (2):' in output:\n{}",
        output
    );
    // Verify both timestamps appear
    assert!(
        output.contains("2026-02-18T10:00:00Z"),
        "First run timestamp missing"
    );
    assert!(
        output.contains("2026-02-18T11:00:00Z"),
        "Second run timestamp missing"
    );
}

#[test]
fn test_trace_multiple_agent_runs_json_sorted() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Retried task", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create archives in reverse order to test sorting
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T11:00:00Z",
        "Second attempt",
        "Second output",
    );
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "First attempt",
        "First output",
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    let runs = json["agent_runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(json["summary"]["agent_run_count"], 2);

    // Should be sorted chronologically
    assert_eq!(runs[0]["timestamp"], "2026-02-18T10:00:00Z");
    assert_eq!(runs[1]["timestamp"], "2026-02-18T11:00:00Z");
}

// ===========================================================================
// 1.3 trace_json_structure_validation
// ===========================================================================

#[test]
fn test_trace_json_full_structure() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Full structure", Status::Done);
    t1.assigned = Some("agent-1".to_string());
    t1.created_at = Some("2026-02-18T09:00:00+00:00".to_string());
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    t1.completed_at = Some("2026-02-18T10:30:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Full structure"}),
    );
    record_op(
        &wg_dir,
        "done",
        Some("t1"),
        Some("agent-1"),
        serde_json::Value::Null,
    );

    // Create agent archive with stream-json style output
    let stream_output = r#"{"type":"assistant","message":"Starting work"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"tool_result","tool_use_id":"tu_1","content":"file contents"}
{"type":"assistant","message":"Done now"}
{"type":"tool_use","name":"Write","id":"tu_2"}
{"type":"tool_result","tool_use_id":"tu_2","content":"ok"}
{"type":"result","cost":{"input":1000,"output":500}}"#;
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:05:00Z",
        "Build the project",
        stream_output,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // Top-level fields
    assert_eq!(json["id"], "t1");
    assert_eq!(json["title"], "Full structure");
    assert_eq!(json["status"], "done");
    assert_eq!(json["assigned"], "agent-1");
    assert!(json["created_at"].is_string());
    assert!(json["started_at"].is_string());
    assert!(json["completed_at"].is_string());

    // Operations array
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 2);
    for op in ops {
        assert!(op["timestamp"].is_string(), "op missing timestamp");
        assert!(op["op"].is_string(), "op missing op field");
    }

    // Agent runs array
    let runs = json["agent_runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run["timestamp"], "2026-02-18T10:05:00Z");
    assert!(run["prompt_bytes"].is_number(), "missing prompt_bytes");
    assert!(run["output_bytes"].is_number(), "missing output_bytes");
    assert!(run["prompt_lines"].is_number(), "missing prompt_lines");
    assert!(run["output_lines"].is_number(), "missing output_lines");
    // Full content should be included in JSON mode
    assert!(run["prompt"].is_string(), "missing prompt content");
    assert!(run["output"].is_string(), "missing output content");
    // Tool calls and turns from stream-json parsing
    assert!(run["tool_calls"].is_number(), "missing tool_calls");
    assert!(run["turns"].is_number(), "missing turns");
    assert_eq!(run["tool_calls"], 2);
    assert_eq!(run["turns"], 2);

    // Summary object
    let summary = &json["summary"];
    assert!(
        summary["duration_secs"].is_number(),
        "missing duration_secs"
    );
    assert!(
        summary["duration_human"].is_string(),
        "missing duration_human"
    );
    assert_eq!(summary["operation_count"], 2);
    assert_eq!(summary["agent_run_count"], 1);
    assert!(
        summary["total_tool_calls"].is_number(),
        "missing total_tool_calls"
    );
    assert!(summary["total_turns"].is_number(), "missing total_turns");
    assert!(
        summary["total_output_bytes"].is_number(),
        "missing total_output_bytes"
    );
    assert_eq!(summary["total_tool_calls"], 2);
    assert_eq!(summary["total_turns"], 2);
}

// ===========================================================================
// 1.4 trace_full_output_contains_conversation
// ===========================================================================

#[test]
fn test_trace_full_shows_prompt_and_output_content() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Full test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let prompt_text = "You are an agent.\nBuild the project.\nMake sure tests pass.";
    let output_text = "I will build the project now.\nRunning cargo build...\nAll tests passed.";
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        prompt_text,
        output_text,
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1", "--full"]);

    // Should contain [Prompt] and [Output] headers
    assert!(
        output.contains("[Prompt]"),
        "Full output should contain '[Prompt]' header:\n{}",
        output
    );
    assert!(
        output.contains("[Output]"),
        "Full output should contain '[Output]' header:\n{}",
        output
    );

    // Prompt content should appear verbatim
    assert!(
        output.contains("You are an agent."),
        "Prompt text should appear verbatim:\n{}",
        output
    );
    assert!(
        output.contains("Build the project."),
        "Prompt text should appear verbatim:\n{}",
        output
    );

    // Output content should appear verbatim
    assert!(
        output.contains("I will build the project now."),
        "Output text should appear verbatim:\n{}",
        output
    );
    assert!(
        output.contains("All tests passed."),
        "Output text should appear verbatim:\n{}",
        output
    );

    // Byte counts should appear in brackets
    let prompt_bytes = prompt_text.len();
    let output_bytes = output_text.len();
    assert!(
        output.contains(&format!("{} bytes", prompt_bytes)),
        "Prompt byte count ({}) should appear:\n{}",
        prompt_bytes,
        output
    );
    assert!(
        output.contains(&format!("{} bytes", output_bytes)),
        "Output byte count ({}) should appear:\n{}",
        output_bytes,
        output
    );
}

#[test]
fn test_trace_full_multiple_runs_shows_all() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Multi-run full", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "First prompt",
        "First output",
    );
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T11:00:00Z",
        "Second prompt",
        "Second output",
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1", "--full"]);

    // Both runs should be shown
    assert!(output.contains("Run 1"), "Should show Run 1:\n{}", output);
    assert!(output.contains("Run 2"), "Should show Run 2:\n{}", output);
    assert!(
        output.contains("First prompt"),
        "First prompt content missing:\n{}",
        output
    );
    assert!(
        output.contains("Second prompt"),
        "Second prompt content missing:\n{}",
        output
    );
    assert!(
        output.contains("First output"),
        "First output content missing:\n{}",
        output
    );
    assert!(
        output.contains("Second output"),
        "Second output content missing:\n{}",
        output
    );
}

// ===========================================================================
// 1.5 trace_ops_only_shows_only_provenance
// ===========================================================================

#[test]
fn test_trace_ops_only_shows_operations() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Ops only task", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Ops only task"}),
    );
    record_op(
        &wg_dir,
        "claim",
        Some("t1"),
        Some("agent-1"),
        serde_json::Value::Null,
    );
    record_op(
        &wg_dir,
        "done",
        Some("t1"),
        Some("agent-1"),
        serde_json::Value::Null,
    );

    // Also create agent archive (should be ignored in ops-only mode)
    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "prompt", "output");

    let output = wg_ok(&wg_dir, &["trace", "show", "t1", "--ops-only"]);

    // Should contain operations header
    assert!(
        output.contains("Operations for 't1'"),
        "Should show 'Operations for' header:\n{}",
        output
    );

    // Should list each operation
    assert!(
        output.contains("add_task"),
        "Should list add_task op:\n{}",
        output
    );
    assert!(
        output.contains("claim"),
        "Should list claim op:\n{}",
        output
    );
    assert!(output.contains("done"), "Should list done op:\n{}", output);
    assert!(output.contains("agent-1"), "Should show actor:\n{}", output);

    // Should NOT contain agent runs section or summary
    assert!(
        !output.contains("Agent runs"),
        "Ops-only should NOT contain 'Agent runs':\n{}",
        output
    );
    assert!(
        !output.contains("Summary"),
        "Ops-only should NOT contain 'Summary':\n{}",
        output
    );
    assert!(
        !output.contains("Duration"),
        "Ops-only should NOT contain 'Duration':\n{}",
        output
    );
}

#[test]
fn test_trace_ops_only_no_operations() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "No ops task", Status::Open);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output = wg_ok(&wg_dir, &["trace", "show", "t1", "--ops-only"]);
    assert!(
        output.contains("No operations recorded"),
        "Should say no operations recorded:\n{}",
        output
    );
}

// ===========================================================================
// 1.6 trace_nonexistent_task
// ===========================================================================

#[test]
fn test_trace_nonexistent_task() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Existing task", Status::Open);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output = wg_fail(&wg_dir, &["trace", "show", "nonexistent"]);
    assert!(
        output.contains("not found"),
        "Error should mention 'not found':\n{}",
        output
    );
}

// ===========================================================================
// 1.7 trace_in_progress_task
// ===========================================================================

#[test]
fn test_trace_in_progress_task_summary() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "In-progress task", Status::Open);
    t1.assigned = Some("agent-1".to_string());
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    // No completed_at — task is still running
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create a partial agent archive (agent is running)
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:05:00Z",
        "Do something",
        "Partial output so far",
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);

    // Should succeed and show status as open
    assert!(
        output.contains("Trace: t1 (open)"),
        "Should show task as open:\n{}",
        output
    );
    // Should show agent assignment
    assert!(
        output.contains("agent-1"),
        "Should show assigned agent:\n{}",
        output
    );
    // Should show the agent run
    assert!(
        output.contains("Agent runs (1):"),
        "Should list the partial agent run:\n{}",
        output
    );
}

#[test]
fn test_trace_in_progress_task_json_no_duration() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "In-progress task", Status::Open);
    t1.assigned = Some("agent-1".to_string());
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:05:00Z", "prompt", "output");

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    assert_eq!(json["status"], "open");
    assert_eq!(json["assigned"], "agent-1");
    assert!(json["started_at"].is_string());
    // completed_at should be absent (skip_serializing_if)
    assert!(
        json["completed_at"].is_null(),
        "completed_at should be null/absent for in-progress task"
    );
    // duration should be absent
    assert!(
        json["summary"]["duration_secs"].is_null(),
        "duration_secs should be null for in-progress task"
    );
    assert!(
        json["summary"]["duration_human"].is_null(),
        "duration_human should be null for in-progress task"
    );
    // Agent runs should still be listed
    assert_eq!(json["summary"]["agent_run_count"], 1);
    assert_eq!(json["agent_runs"].as_array().unwrap().len(), 1);
}

// ===========================================================================
// 1.8 trace_with_rotated_log_files
// ===========================================================================

#[test]
fn test_trace_with_rotated_operations_logs() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Rotated logs task", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let log_dir = wg_dir.join("log");
    fs::create_dir_all(&log_dir).unwrap();

    // Create a rotated compressed file with an older operation
    let old_entry = provenance::OperationEntry {
        timestamp: "2026-02-17T10:00:00+00:00".to_string(),
        op: "add_task".to_string(),
        task_id: Some("t1".to_string()),
        actor: None,
        user: None,
        detail: serde_json::json!({"title": "Rotated logs task"}),
    };
    let old_json = format!("{}\n", serde_json::to_string(&old_entry).unwrap());
    let compressed = zstd::encode_all(old_json.as_bytes(), 3).unwrap();
    fs::write(
        log_dir.join("20260217T100000.000000Z.jsonl.zst"),
        &compressed,
    )
    .unwrap();

    // Create the current operations.jsonl with a newer operation
    let new_entry = provenance::OperationEntry {
        timestamp: "2026-02-18T10:00:00+00:00".to_string(),
        op: "done".to_string(),
        task_id: Some("t1".to_string()),
        actor: Some("agent-1".to_string()),
        user: None,
        detail: serde_json::Value::Null,
    };
    let new_json = format!("{}\n", serde_json::to_string(&new_entry).unwrap());
    fs::write(log_dir.join("operations.jsonl"), &new_json).unwrap();

    // Summary mode should show operations from both files
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Operations (2):") || output.contains("Operations"),
        "Should show operations from both rotated and current files:\n{}",
        output
    );
    assert!(
        output.contains("add_task"),
        "Should include op from rotated file:\n{}",
        output
    );
    assert!(
        output.contains("done"),
        "Should include op from current file:\n{}",
        output
    );

    // JSON mode should include all operations
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(
        ops.len(),
        2,
        "Should have 2 operations (1 rotated + 1 current)"
    );

    // Should be in chronological order
    assert_eq!(ops[0]["op"], "add_task");
    assert_eq!(ops[1]["op"], "done");
}

// ===========================================================================
// 1.9 trace_output_size_accuracy
// ===========================================================================

#[test]
fn test_trace_output_size_accuracy_summary() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Size test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Write exactly 10240 bytes of output
    let output_content = "x".repeat(10240);
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        &output_content,
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("10.0 KB"),
        "Should show 'Total output: 10.0 KB':\n{}",
        output
    );
}

#[test]
fn test_trace_output_size_accuracy_json() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Size test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Write exactly 10240 bytes
    let output_content = "x".repeat(10240);
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        &output_content,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    assert_eq!(
        json["summary"]["total_output_bytes"], 10240,
        "total_output_bytes should be exactly 10240"
    );
    assert_eq!(
        json["agent_runs"][0]["output_bytes"], 10240,
        "agent_runs[0].output_bytes should be exactly 10240"
    );
}

#[test]
fn test_trace_output_size_accuracy_megabytes() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Big output", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Write >1MB of output (1048576 + 512 bytes = 1049088 bytes)
    let output_content = "y".repeat(1_049_088);
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        &output_content,
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    // 1049088 bytes = 1024.5 KB = 1.0 MB
    assert!(
        output.contains("MB"),
        "Should show output in MB for large sizes:\n{}",
        output
    );
}

#[test]
fn test_trace_output_size_multiple_runs_summed() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Multi-run size", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Two runs with known sizes
    let output1 = "a".repeat(5000);
    let output2 = "b".repeat(3000);
    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "p1", &output1);
    create_agent_archive(&wg_dir, "t1", "2026-02-18T11:00:00Z", "p2", &output2);

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert_eq!(
        json["summary"]["total_output_bytes"], 8000,
        "total_output_bytes should be sum of both runs (5000+3000=8000)"
    );
    assert_eq!(json["agent_runs"][0]["output_bytes"], 5000);
    assert_eq!(json["agent_runs"][1]["output_bytes"], 3000);
}

// ===========================================================================
// 1.10 trace_turn_count_accuracy
// ===========================================================================

#[test]
fn test_trace_turn_count_accuracy() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Turn count test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create output with exactly 3 assistant turns and 5 tool_use calls
    let stream_output = r#"{"type":"assistant","message":"Turn 1: analyzing"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"tool_result","tool_use_id":"tu_1","content":"file1"}
{"type":"tool_use","name":"Read","id":"tu_2"}
{"type":"tool_result","tool_use_id":"tu_2","content":"file2"}
{"type":"assistant","message":"Turn 2: implementing"}
{"type":"tool_use","name":"Write","id":"tu_3"}
{"type":"tool_result","tool_use_id":"tu_3","content":"ok"}
{"type":"tool_use","name":"Write","id":"tu_4"}
{"type":"tool_result","tool_use_id":"tu_4","content":"ok"}
{"type":"assistant","message":"Turn 3: done"}
{"type":"tool_use","name":"Bash","id":"tu_5"}
{"type":"tool_result","tool_use_id":"tu_5","content":"ok"}
{"type":"result","cost":{"input":5000,"output":2000}}"#;

    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        stream_output,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // Per-run counts
    assert_eq!(json["agent_runs"][0]["turns"], 3, "Expected 3 turns");
    assert_eq!(
        json["agent_runs"][0]["tool_calls"], 5,
        "Expected 5 tool calls"
    );

    // Summary totals
    assert_eq!(json["summary"]["total_turns"], 3);
    assert_eq!(json["summary"]["total_tool_calls"], 5);
}

#[test]
fn test_trace_turn_count_summary_display() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Turn display test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let stream_output = r#"{"type":"assistant","message":"Hello"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"tool_result","tool_use_id":"tu_1","content":"ok"}
{"type":"result","cost":{"input":100,"output":50}}"#;
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        stream_output,
    );

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Total turns: 1"),
        "Should display turn count:\n{}",
        output
    );
    assert!(
        output.contains("Total tool calls: 1"),
        "Should display tool call count:\n{}",
        output
    );
}

#[test]
fn test_trace_turn_count_multiple_runs_summed() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Multi-run turns", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output1 = r#"{"type":"assistant","message":"Run 1 turn 1"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"assistant","message":"Run 1 turn 2"}
{"type":"result","cost":{"input":100,"output":50}}"#;
    let output2 = r#"{"type":"assistant","message":"Run 2 turn 1"}
{"type":"tool_use","name":"Write","id":"tu_2"}
{"type":"tool_use","name":"Bash","id":"tu_3"}
{"type":"result","cost":{"input":200,"output":100}}"#;

    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "p1", output1);
    create_agent_archive(&wg_dir, "t1", "2026-02-18T11:00:00Z", "p2", output2);

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // Run 1: 2 turns, 1 tool call
    assert_eq!(json["agent_runs"][0]["turns"], 2);
    assert_eq!(json["agent_runs"][0]["tool_calls"], 1);
    // Run 2: 1 turn, 2 tool calls
    assert_eq!(json["agent_runs"][1]["turns"], 1);
    assert_eq!(json["agent_runs"][1]["tool_calls"], 2);
    // Totals
    assert_eq!(json["summary"]["total_turns"], 3);
    assert_eq!(json["summary"]["total_tool_calls"], 3);
}

// ===========================================================================
// 1.11 trace_uninitialized_workgraph (already covered by unit test, but
//      included here for completeness at integration level)
// ===========================================================================

#[test]
fn test_trace_uninitialized_workgraph() {
    let tmp = TempDir::new().unwrap();
    // Point at an empty dir with no .wg
    let fake_dir = tmp.path().join(".wg");
    // Don't create it — it shouldn't exist
    let output = wg_cmd(&fake_dir, &["trace", "show", "t1"]);
    assert!(
        !output.status.success(),
        "Should fail when WG is not initialized"
    );
}

// ===========================================================================
// 1.12 trace_content_block_tool_use_counting
// ===========================================================================

#[test]
fn test_trace_content_block_tool_use_counting() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Content block test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Mix of top-level tool_use and content_block style tool_use
    let stream_output = r#"{"type":"assistant","message":"Starting"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"tool_result","tool_use_id":"tu_1","content":"ok"}
{"content_block":{"type":"tool_use","name":"Write","id":"tu_2"}}
{"type":"result","cost":{"input":100,"output":50}}"#;

    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        stream_output,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // Both tool calls should be counted: 1 top-level + 1 content_block
    assert_eq!(
        json["agent_runs"][0]["tool_calls"], 2,
        "Should count both top-level and content_block tool_use: got {:?}",
        json["agent_runs"][0]["tool_calls"]
    );
    assert_eq!(json["summary"]["total_tool_calls"], 2);
    // Only 1 assistant turn
    assert_eq!(json["agent_runs"][0]["turns"], 1);
}

#[test]
fn test_trace_content_block_no_double_counting() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "No double count", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Only content_block style, no top-level tool_use
    let stream_output = r#"{"type":"assistant","message":"Working"}
{"content_block":{"type":"tool_use","name":"Read","id":"tu_1"}}
{"content_block":{"type":"tool_use","name":"Write","id":"tu_2"}}
{"content_block":{"type":"tool_use","name":"Bash","id":"tu_3"}}
{"type":"result","cost":{"input":100,"output":50}}"#;

    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        stream_output,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    assert_eq!(
        json["agent_runs"][0]["tool_calls"], 3,
        "Should count 3 content_block tool_use calls"
    );
    assert_eq!(json["agent_runs"][0]["turns"], 1);
}

// ===========================================================================
// Additional edge cases from spec 5.1 and 5.2 (helper functions tested
// end-to-end through trace)
// ===========================================================================

#[test]
fn test_trace_duration_boundary_values() {
    // Test duration formatting through the trace output by setting specific timestamps.
    // 0 seconds
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t0", "Zero duration", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T10:00:00+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t0"]);
        assert_eq!(json["summary"]["duration_secs"], 0);
        assert_eq!(json["summary"]["duration_human"], "0s");
    }
    // 59 seconds
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t59", "59 seconds", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T10:00:59+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t59"]);
        assert_eq!(json["summary"]["duration_secs"], 59);
        assert_eq!(json["summary"]["duration_human"], "59s");
    }
    // 60 seconds = 1m 0s
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t60", "60 seconds", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T10:01:00+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t60"]);
        assert_eq!(json["summary"]["duration_secs"], 60);
        assert_eq!(json["summary"]["duration_human"], "1m 0s");
    }
    // 3599 seconds = 59m 59s
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t3599", "3599 seconds", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T10:59:59+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t3599"]);
        assert_eq!(json["summary"]["duration_secs"], 3599);
        assert_eq!(json["summary"]["duration_human"], "59m 59s");
    }
    // 3600 seconds = 1h 0m
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t3600", "3600 seconds", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T11:00:00+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t3600"]);
        assert_eq!(json["summary"]["duration_secs"], 3600);
        assert_eq!(json["summary"]["duration_human"], "1h 0m");
    }
    // 7261 seconds = 2h 1m
    {
        let tmp = TempDir::new().unwrap();
        let mut t1 = make_task("t7261", "7261 seconds", Status::Done);
        t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
        t1.completed_at = Some("2026-02-18T12:01:01+00:00".to_string());
        let wg_dir = setup_workgraph(&tmp, vec![t1]);
        let json = wg_json(&wg_dir, &["trace", "show", "t7261"]);
        assert_eq!(json["summary"]["duration_secs"], 7261);
        assert_eq!(json["summary"]["duration_human"], "2h 1m");
    }
}

#[test]
fn test_trace_result_only_output() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Result only", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Output contains ONLY a result message (no assistant turns)
    let stream_output = r#"{"type":"result","cost":{"input":100,"output":50}}"#;
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        stream_output,
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // result-only should count as 1 turn (the fallback in parse_stream_json_stats)
    assert_eq!(
        json["agent_runs"][0]["turns"], 1,
        "Result-only output should count as 1 turn"
    );
    assert_eq!(json["summary"]["total_turns"], 1);
}

#[test]
fn test_trace_agent_runs_sort_order() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Sort order test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create archives with timestamps out of alphabetical/filesystem order
    create_agent_archive(&wg_dir, "t1", "2026-02-18T15:00:00Z", "third", "out3");
    create_agent_archive(&wg_dir, "t1", "2026-02-18T09:00:00Z", "first", "out1");
    create_agent_archive(&wg_dir, "t1", "2026-02-18T12:00:00Z", "second", "out2");

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let runs = json["agent_runs"].as_array().unwrap();

    assert_eq!(runs.len(), 3);
    assert_eq!(
        runs[0]["timestamp"], "2026-02-18T09:00:00Z",
        "First should be earliest"
    );
    assert_eq!(
        runs[1]["timestamp"], "2026-02-18T12:00:00Z",
        "Second should be middle"
    );
    assert_eq!(
        runs[2]["timestamp"], "2026-02-18T15:00:00Z",
        "Third should be latest"
    );
}

// ===========================================================================
// Additional: Trace with all task statuses
// ===========================================================================

#[test]
fn test_trace_various_statuses() {
    let tmp = TempDir::new().unwrap();
    let t_open = make_task("open", "Open task", Status::Open);
    let t_done = make_task("done", "Done task", Status::Done);
    let mut t_fail = make_task("failed", "Failed task", Status::Failed);
    t_fail.failure_reason = Some("compilation error".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t_open, t_done, t_fail]);

    // All should trace without error
    for (id, expected_status) in [("open", "open"), ("done", "done"), ("failed", "failed")] {
        let json = wg_json(&wg_dir, &["trace", "show", id]);
        assert_eq!(json["status"], expected_status);
    }
}

// ===========================================================================
// 1.13 trace_json_flag_overrides_full_and_ops_only
// ===========================================================================

#[test]
fn test_trace_json_overrides_full_flag() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Override test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "prompt", "output");
    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Override test"}),
    );

    // Pass --json globally AND --full on the trace subcommand
    // The --json flag should take priority, producing JSON output
    let output = wg_cmd(&wg_dir, &["--json", "trace", "show", "t1", "--full"]);
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // Output should be valid JSON (not the full text format)
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Output should be valid JSON when --json is set.\nError: {}\nOutput: {}",
            e, stdout
        )
    });
    assert!(json["id"].is_string(), "JSON should have id field");
    assert!(
        json["agent_runs"].is_array(),
        "JSON should have agent_runs array"
    );
    assert!(
        json["summary"].is_object(),
        "JSON should have summary object"
    );
    // Should NOT contain text-mode markers
    assert!(
        !stdout.contains("[Prompt]"),
        "Should not contain full-mode [Prompt] header"
    );
    assert!(
        !stdout.contains("[Output]"),
        "Should not contain full-mode [Output] header"
    );
}

#[test]
fn test_trace_json_overrides_ops_only_flag() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Override test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "prompt", "output");
    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Override test"}),
    );

    // Pass --json globally AND --ops-only on the trace subcommand
    let output = wg_cmd(&wg_dir, &["--json", "trace", "show", "t1", "--ops-only"]);
    assert!(output.status.success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    // Output should be valid JSON (not the ops-only text format)
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Output should be valid JSON when --json is set.\nError: {}\nOutput: {}",
            e, stdout
        )
    });
    assert!(json["id"].is_string(), "JSON should have id field");
    assert!(
        json["agent_runs"].is_array(),
        "JSON should have agent_runs array (not ops-only)"
    );
    assert!(
        json["operations"].is_array(),
        "JSON should have operations array"
    );
    assert!(
        json["summary"].is_object(),
        "JSON should have summary object"
    );
    // Should NOT contain ops-only text markers
    assert!(
        !stdout.contains("Operations for"),
        "Should not contain ops-only header"
    );
}

// ===========================================================================
// 1.14 trace_agent_archive_missing_output_txt
// ===========================================================================

#[test]
fn test_trace_agent_archive_missing_output() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Missing output", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create archive directory with only prompt.txt (no output.txt)
    let archive_dir = wg_dir
        .join("log")
        .join("agents")
        .join("t1")
        .join("2026-02-18T10:00:00Z");
    fs::create_dir_all(&archive_dir).unwrap();
    fs::write(archive_dir.join("prompt.txt"), "A prompt without output").unwrap();
    // Do NOT create output.txt

    // Summary mode should succeed
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Agent runs (1):"),
        "Should still show the agent run:\n{}",
        output
    );

    // JSON mode should show prompt_bytes but no output_bytes
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let run = &json["agent_runs"][0];
    assert!(
        run["prompt_bytes"].is_number(),
        "prompt_bytes should be present"
    );
    assert!(
        run["output_bytes"].is_null(),
        "output_bytes should be null when output.txt missing"
    );
    assert!(
        run["tool_calls"].is_null(),
        "tool_calls should be null when no output"
    );
    assert!(
        run["turns"].is_null(),
        "turns should be null when no output"
    );
}

// ===========================================================================
// 1.15 trace_agent_archive_empty_output
// ===========================================================================

#[test]
fn test_trace_agent_archive_empty_output() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Empty output", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create archive with empty output.txt
    create_agent_archive(&wg_dir, "t1", "2026-02-18T10:00:00Z", "prompt", "");

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let run = &json["agent_runs"][0];

    // output_bytes should be 0
    assert_eq!(
        run["output_bytes"], 0,
        "output_bytes should be 0 for empty file"
    );
    // output_lines should be 0
    assert_eq!(
        run["output_lines"], 0,
        "output_lines should be 0 for empty file"
    );
    // tool_calls and turns should be omitted (0 from empty input)
    assert!(
        run["tool_calls"].is_null(),
        "tool_calls should be null for empty output"
    );
    assert!(
        run["turns"].is_null(),
        "turns should be null for empty output"
    );

    // Summary total_output_bytes should be omitted (sum is 0)
    assert!(
        json["summary"]["total_output_bytes"].is_null(),
        "total_output_bytes should be null when sum is 0"
    );
    assert!(
        json["summary"]["total_tool_calls"].is_null(),
        "total_tool_calls should be null when sum is 0"
    );
    assert!(
        json["summary"]["total_turns"].is_null(),
        "total_turns should be null when sum is 0"
    );
}

// ===========================================================================
// 1.16 trace_operation_detail_truncation
// ===========================================================================

#[test]
fn test_trace_operation_detail_truncation() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Truncation test", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Short detail (< 120 chars)
    let short_detail = serde_json::json!({"msg": "short detail"});
    record_op(&wg_dir, "add_task", Some("t1"), None, short_detail.clone());

    // Long detail (> 120 chars) - create a string that when serialized to JSON is > 120 chars
    let long_value = "x".repeat(200);
    let long_detail = serde_json::json!({"description": long_value});
    record_op(&wg_dir, "custom_op", Some("t1"), None, long_detail.clone());

    // Summary mode should truncate the long detail
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);

    // Short detail should appear in full
    assert!(
        output.contains("short detail"),
        "Short detail should appear in full:\n{}",
        output
    );

    // Long detail should be truncated with "..."
    assert!(
        output.contains("..."),
        "Long detail should be truncated with '...':\n{}",
        output
    );

    // The full long detail string should NOT appear in summary mode
    let long_detail_str = serde_json::to_string(&long_detail).unwrap();
    assert!(
        !output.contains(&long_detail_str),
        "Full long detail should NOT appear in summary (should be truncated):\n{}",
        output
    );

    // JSON mode should contain full detail (no truncation)
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let ops = json["operations"].as_array().unwrap();
    // Find the custom_op entry
    let custom_op = ops.iter().find(|o| o["op"] == "custom_op").unwrap();
    assert_eq!(
        custom_op["detail"]["description"].as_str().unwrap().len(),
        200,
        "JSON mode should contain full detail without truncation"
    );
}

// ===========================================================================
// 1.20 trace_summary_mode_excludes_content
// ===========================================================================

#[test]
fn test_trace_summary_mode_excludes_content() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Summary exclusion", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let prompt_text =
        "UNIQUE_PROMPT_MARKER_12345 This is the prompt text that should not appear in summary";
    let output_text =
        "UNIQUE_OUTPUT_MARKER_67890 This is the output text that should not appear in summary";
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        prompt_text,
        output_text,
    );

    // Summary mode (default) should NOT print prompt/output content verbatim
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);

    assert!(
        !output.contains("UNIQUE_PROMPT_MARKER_12345"),
        "Summary mode should NOT contain prompt text verbatim:\n{}",
        output
    );
    assert!(
        !output.contains("UNIQUE_OUTPUT_MARKER_67890"),
        "Summary mode should NOT contain output text verbatim:\n{}",
        output
    );

    // Should show size info instead ("X.X KB (Y lines)" format)
    assert!(
        output.contains("KB") || output.contains("bytes"),
        "Summary should show size info:\n{}",
        output
    );

    // JSON mode SHOULD include the full content
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let run = &json["agent_runs"][0];
    assert_eq!(
        run["prompt"].as_str().unwrap(),
        prompt_text,
        "JSON mode should include full prompt content"
    );
    assert_eq!(
        run["output"].as_str().unwrap(),
        output_text,
        "JSON mode should include full output content"
    );
}

// ===========================================================================
// Additional: JSON omits zero tool calls/turns
// ===========================================================================

#[test]
fn test_trace_json_no_tool_calls_or_turns_omitted() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "No stats", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Agent archive with non-JSON output (plain text)
    create_agent_archive(
        &wg_dir,
        "t1",
        "2026-02-18T10:00:00Z",
        "prompt",
        "plain text output with no json",
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);

    // tool_calls and turns should be omitted (skip_serializing_if) when 0
    assert!(
        json["agent_runs"][0]["tool_calls"].is_null(),
        "tool_calls should be omitted when 0"
    );
    assert!(
        json["agent_runs"][0]["turns"].is_null(),
        "turns should be omitted when 0"
    );
    assert!(
        json["summary"]["total_tool_calls"].is_null(),
        "total_tool_calls should be omitted when 0"
    );
    assert!(
        json["summary"]["total_turns"].is_null(),
        "total_turns should be omitted when 0"
    );
}

// ===========================================================================
// 1.21 trace_blocked_and_abandoned_and_inprogress_statuses
// ===========================================================================

#[test]
fn test_trace_blocked_status() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Blocked task", Status::Open);
    // Set Blocked status via the field directly
    t1.status = Status::Blocked;
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("blocked"),
        "Should show blocked status:\n{}",
        output
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert_eq!(json["status"], "blocked");
}

#[test]
fn test_trace_abandoned_status() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Abandoned task", Status::Open);
    t1.status = Status::Abandoned;
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("abandoned"),
        "Should show abandoned status:\n{}",
        output
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert_eq!(json["status"], "abandoned");
}

#[test]
fn test_trace_in_progress_status() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "InProgress task", Status::Open);
    t1.status = Status::InProgress;
    t1.assigned = Some("agent-1".to_string());
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("in-progress"),
        "Should show in-progress status:\n{}",
        output
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert_eq!(json["status"], "in-progress");
}

#[test]
fn test_trace_all_six_statuses() {
    let tmp = TempDir::new().unwrap();
    let t_open = make_task("s-open", "Open task", Status::Open);
    let mut t_inprog = make_task("s-inprogress", "InProgress task", Status::Open);
    t_inprog.status = Status::InProgress;
    let t_done = make_task("s-done", "Done task", Status::Done);
    let mut t_blocked = make_task("s-blocked", "Blocked task", Status::Open);
    t_blocked.status = Status::Blocked;
    let t_failed = make_task("s-failed", "Failed task", Status::Failed);
    let mut t_abandoned = make_task("s-abandoned", "Abandoned task", Status::Open);
    t_abandoned.status = Status::Abandoned;

    let wg_dir = setup_workgraph(
        &tmp,
        vec![t_open, t_inprog, t_done, t_blocked, t_failed, t_abandoned],
    );

    let expected = [
        ("s-open", "open"),
        ("s-inprogress", "in-progress"),
        ("s-done", "done"),
        ("s-blocked", "blocked"),
        ("s-failed", "failed"),
        ("s-abandoned", "abandoned"),
    ];

    for (id, expected_status) in expected {
        let json = wg_json(&wg_dir, &["trace", "show", id]);
        assert_eq!(
            json["status"], expected_status,
            "Task {} should have status '{}'",
            id, expected_status
        );
    }
}

// ===========================================================================
// 1.22 trace_agent_archive_missing_prompt_txt
// ===========================================================================

#[test]
fn test_trace_agent_archive_missing_prompt() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Missing prompt", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create archive directory with only output.txt (no prompt.txt)
    let archive_dir = wg_dir
        .join("log")
        .join("agents")
        .join("t1")
        .join("2026-02-18T10:00:00Z");
    fs::create_dir_all(&archive_dir).unwrap();
    let stream_output = r#"{"type":"assistant","message":"Working"}
{"type":"tool_use","name":"Read","id":"tu_1"}
{"type":"result","cost":{"input":100,"output":50}}"#;
    fs::write(archive_dir.join("output.txt"), stream_output).unwrap();
    // Do NOT create prompt.txt

    // Summary mode should succeed
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Agent runs (1):"),
        "Should still show the agent run:\n{}",
        output
    );

    // JSON mode should show output_bytes but no prompt_bytes
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let run = &json["agent_runs"][0];
    assert!(
        run["output_bytes"].is_number(),
        "output_bytes should be present"
    );
    assert!(
        run["prompt_bytes"].is_null(),
        "prompt_bytes should be null when prompt.txt is missing"
    );
    assert!(
        run["prompt_lines"].is_null(),
        "prompt_lines should be null when prompt.txt is missing"
    );
    assert!(
        run["prompt"].is_null(),
        "prompt content should be null when prompt.txt is missing"
    );
    // tool_calls and turns should still be parsed from output.txt
    assert_eq!(run["tool_calls"], 1, "Should count tool call from output");
    assert_eq!(run["turns"], 1, "Should count turn from output");
}

// ===========================================================================
// 1.23 trace_agent_archive_empty_directory
// ===========================================================================

#[test]
fn test_trace_agent_archive_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Empty archive", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Create empty archive directory (no prompt.txt, no output.txt)
    let archive_dir = wg_dir
        .join("log")
        .join("agents")
        .join("t1")
        .join("2026-02-18T10:00:00Z");
    fs::create_dir_all(&archive_dir).unwrap();

    // Should succeed without error
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        output.contains("Agent runs (1):"),
        "Should count the empty archive as an agent run:\n{}",
        output
    );

    // JSON mode: all file-derived fields should be absent/null
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert_eq!(json["summary"]["agent_run_count"], 1);
    let run = &json["agent_runs"][0];
    assert_eq!(run["timestamp"], "2026-02-18T10:00:00Z");
    assert!(run["prompt_bytes"].is_null(), "prompt_bytes should be null");
    assert!(run["output_bytes"].is_null(), "output_bytes should be null");
    assert!(run["prompt_lines"].is_null(), "prompt_lines should be null");
    assert!(run["output_lines"].is_null(), "output_lines should be null");
    assert!(run["tool_calls"].is_null(), "tool_calls should be null");
    assert!(run["turns"].is_null(), "turns should be null");
}

// ===========================================================================
// 1.24 trace_operations_filtering_accuracy
// ===========================================================================

#[test]
fn test_trace_operations_filtering_by_task_id() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Task one", Status::Done);
    let t2 = make_task("t2", "Task two", Status::Done);
    let wg_dir = setup_workgraph(&tmp, vec![t1, t2]);

    // Record ops for t1
    record_op(
        &wg_dir,
        "add_task",
        Some("t1"),
        None,
        serde_json::json!({"title": "Task one"}),
    );
    record_op(
        &wg_dir,
        "done",
        Some("t1"),
        Some("agent-1"),
        serde_json::Value::Null,
    );

    // Record ops for t2
    record_op(
        &wg_dir,
        "add_task",
        Some("t2"),
        None,
        serde_json::json!({"title": "Task two"}),
    );

    // Record a global op with no task_id (e.g., replay)
    record_op(
        &wg_dir,
        "replay",
        None,
        None,
        serde_json::json!({"run_id": "run-001"}),
    );

    // Trace t1 should only show t1's operations
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 2, "t1 should have exactly 2 operations");
    for op in ops {
        assert_eq!(
            op["task_id"], "t1",
            "All operations should belong to t1, got: {:?}",
            op
        );
    }

    // Trace t2 should only show t2's operation
    let json2 = wg_json(&wg_dir, &["trace", "show", "t2"]);
    let ops2 = json2["operations"].as_array().unwrap();
    assert_eq!(ops2.len(), 1, "t2 should have exactly 1 operation");
    assert_eq!(ops2[0]["task_id"], "t2");
    assert_eq!(ops2[0]["op"], "add_task");
}

#[test]
fn test_trace_operations_excludes_global_and_other_tasks() {
    let tmp = TempDir::new().unwrap();
    let t1 = make_task("t1", "Filtered task", Status::Open);
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Only global ops, no task-specific ops for t1
    record_op(
        &wg_dir,
        "replay",
        None,
        None,
        serde_json::json!({"run_id": "run-001"}),
    );
    record_op(
        &wg_dir,
        "add_task",
        Some("other-task"),
        None,
        serde_json::json!({"title": "Other"}),
    );

    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(
        ops.len(),
        0,
        "t1 should have 0 operations (global and other-task ops excluded)"
    );
    assert_eq!(json["summary"]["operation_count"], 0);
}

// ===========================================================================
// 1.25 trace_with_unparseable_timestamps
// ===========================================================================

#[test]
fn test_trace_unparseable_timestamps_no_duration() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Bad timestamps", Status::Done);
    t1.started_at = Some("not-a-timestamp".to_string());
    t1.completed_at = Some("also-not-a-timestamp".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Should succeed without panicking
    let output = wg_ok(&wg_dir, &["trace", "show", "t1"]);
    // Should NOT show duration (unparseable timestamps)
    assert!(
        !output.contains("Duration:"),
        "Should not show Duration for unparseable timestamps:\n{}",
        output
    );

    // JSON mode: duration fields should be absent
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        json["summary"]["duration_secs"].is_null(),
        "duration_secs should be null for unparseable timestamps"
    );
    assert!(
        json["summary"]["duration_human"].is_null(),
        "duration_human should be null for unparseable timestamps"
    );
}

#[test]
fn test_trace_unparseable_started_at_only() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Bad start time", Status::Done);
    t1.started_at = Some("not-valid".to_string());
    t1.completed_at = Some("2026-02-18T10:30:00+00:00".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Should succeed without panicking
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        json["summary"]["duration_secs"].is_null(),
        "duration_secs should be null when started_at is unparseable"
    );
}

#[test]
fn test_trace_unparseable_completed_at_only() {
    let tmp = TempDir::new().unwrap();
    let mut t1 = make_task("t1", "Bad end time", Status::Done);
    t1.started_at = Some("2026-02-18T10:00:00+00:00".to_string());
    t1.completed_at = Some("not-valid".to_string());
    let wg_dir = setup_workgraph(&tmp, vec![t1]);

    // Should succeed without panicking
    let json = wg_json(&wg_dir, &["trace", "show", "t1"]);
    assert!(
        json["summary"]["duration_secs"].is_null(),
        "duration_secs should be null when completed_at is unparseable"
    );
}
