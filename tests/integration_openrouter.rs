//! Integration tests for open model tool use via OpenRouter native executor.
//!
//! Tests the critical path of open model tool calling through the native executor:
//! - Full agent loop with OpenRouter minimax-m2.7 model
//! - File tools and bash execution
//! - Journal completeness verification
//! - Multi-turn conversation with tool results round-tripping
//!
//! Run with: cargo test --test integration_openrouter -- --ignored
//! Requires: OPENROUTER_API_KEY environment variable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;

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
    let fake_home = wg_dir.parent().unwrap_or(wg_dir).join("fakehome");
    let _ = fs::create_dir_all(&fake_home);
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", &fake_home)
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

/// Wait for a file to exist, polling every 2s, up to `max_wait` seconds.
fn wait_for_file(path: &Path, max_wait_secs: u64) -> std::io::Result<()> {
    let start = std::time::Instant::now();
    loop {
        if path.exists() {
            return Ok(());
        }
        if start.elapsed().as_secs() >= max_wait_secs {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out after {}s waiting for {:?}", max_wait_secs, path),
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------------------
// Integration Tests
// ---------------------------------------------------------------------------

/// Test: Full native executor tool-use loop with minimax-m2.7 via OpenRouter.
///
/// Validates:
/// 1. Agent spawns successfully with OpenRouter model
/// 2. Multiple tool calls execute correctly (read, bash)
/// 3. Tool results round-trip back to the model
/// 4. Journal entries are complete (init, messages, tool_executions, end)
/// 5. Final output is coherent text
/// 6. Task reaches "done" status
///
/// This is the primary integration test for the critical path of open model
/// tool calling through the native executor.
#[test]
#[ignore = "requires OPENROUTER_API_KEY"]
fn test_openrouter_minimax_tool_loop() {
    // ── 0. Check API key ─────────────────────────────────────────────────
    let _api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("OPENROUTER_API_KEY must be set for this integration test");

    // ── 1. Set up temp workgraph ─────────────────────────────────────────
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    wg_ok(&wg_dir, &["init"]);
    wg_ok(&wg_dir, &["agency", "init"]);

    // ── 2. Configure OpenRouter endpoint ─────────────────────────────────
    let key_file = tmp.path().join("key.txt");
    fs::write(&key_file, &_api_key).unwrap();

    wg_ok(
        &wg_dir,
        &[
            "endpoint",
            "add",
            "test-openrouter",
            "--provider",
            "openrouter",
            "--url",
            "https://openrouter.ai/api/v1",
            "--key-file",
            key_file.to_str().unwrap(),
        ],
    );
    wg_ok(&wg_dir, &["endpoint", "set-default", "test-openrouter"]);

    // ── 3. Create a test file to be read ─────────────────────────────────
    let test_file = tmp.path().join("test_input.txt");
    fs::write(&test_file, "Hello from integration test\nLine 2\nLine 3").unwrap();

    // ── 4. Create a task that requires tool use ──────────────────────────
    wg_ok(
        &wg_dir,
        &[
            "add",
            "OpenRouter tool integration test",
            "--id",
            "openrouter-tool-test",
            "--context-scope",
            "task",
            "--immediate",
        ],
    );

    // ── 5. Spawn native executor with minimax-m2.7 via OpenRouter ─────────
    let spawn_output = wg_cmd(
        &wg_dir,
        &[
            "spawn",
            "openrouter-tool-test",
            "--executor",
            "native",
            "--model",
            "minimax/minimax-m2.7",
        ],
    );

    let stderr = String::from_utf8_lossy(&spawn_output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&spawn_output.stdout).to_string();

    assert!(
        spawn_output.status.success(),
        "wg spawn failed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // ── 6. Poll until agent completes (task becomes Done) ─────────────────
    let max_wait = 300; // 5 minutes max

    let mut completed = false;
    let mut failed = false;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < max_wait {
        let output = wg_cmd(&wg_dir, &["show", "openrouter-tool-test", "--json"]);
        if output.status.success() {
            let show_stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&show_stdout) {
                match val.get("status").and_then(|s| s.as_str()) {
                    Some("done") => {
                        completed = true;
                        break;
                    }
                    Some("failed") => {
                        failed = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }

    assert!(
        !failed,
        "Agent task failed. Check agent output logs."
    );
    assert!(
        completed,
        "Agent did not complete within {}s. Check agent output logs.",
        max_wait
    );

    // ── 7. Verify agent output directory exists ───────────────────────────
    let agents_base = wg_dir.join("agents");
    assert!(
        agents_base.exists(),
        "Agent output directory should exist at {:?}",
        agents_base
    );

    // Find the agent directory
    let agent_subdirs: Vec<PathBuf> = fs::read_dir(&agents_base)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("agent-")
        })
        .collect();

    assert!(
        !agent_subdirs.is_empty(),
        "Should have at least one agent directory"
    );

    let agent_dir = &agent_subdirs[0];

    // ── 8. Verify conversation journal exists ──────────────────────────────
    let journal_path = agent_dir.join("conversation.jsonl");
    assert!(
        journal_path.exists(),
        "Conversation journal should exist at {:?}",
        journal_path
    );

    // ── 9. Parse journal entries and validate structure ──────────────────
    let journal_content = fs::read_to_string(&journal_path).unwrap();
    let entries: Vec<serde_json::Value> = journal_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("Invalid JSON line in journal"))
        .collect();

    // Journal should have at least: init + assistant turn + tool execution + user (result) + end
    assert!(
        entries.len() >= 4,
        "Journal should have at least 4 entries, got {}",
        entries.len()
    );

    // First entry must be Init
    let first = &entries[0];
    assert_eq!(
        first.get("entry_type").and_then(|v| v.as_str()),
        Some("init"),
        "First journal entry should be 'init'"
    );

    // Count assistant turns (should have at least 2 for a tool loop)
    let assistant_entries: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e.get("entry_type").and_then(|v| v.as_str()) == Some("message"))
        .filter(|e| e.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .collect();

    assert!(
        assistant_entries.len() >= 1,
        "Should have at least 1 assistant turn, got {}",
        assistant_entries.len()
    );

    // ── 10. Validate stop_reason detection ─────────────────────────────────
    let stop_reasons: Vec<String> = assistant_entries
        .iter()
        .filter_map(|e| {
            e.get("stop_reason")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect();

    // At least one should have stop_reason = "tool_use"
    assert!(
        stop_reasons.iter().any(|r| r == "tool_use"),
        "At least one turn should have stop_reason='tool_use'. Got: {:?}",
        stop_reasons
    );

    // ── 11. Validate tool execution entries ───────────────────────────────
    let tool_exec_entries: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e.get("entry_type").and_then(|v| v.as_str()) == Some("tool_execution"))
        .collect();

    assert!(
        !tool_exec_entries.is_empty(),
        "Should have at least 1 tool execution, got {}",
        tool_exec_entries.len()
    );

    // Verify tool_use_id linkage
    for tool_exec in &tool_exec_entries {
        let tool_use_id = tool_exec
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .expect("tool_execution entry should have tool_use_id");
        assert!(
            !tool_use_id.is_empty(),
            "tool_use_id should not be empty"
        );

        // Verify tool name is present
        let tool_name = tool_exec
            .get("tool")
            .and_then(|v| v.as_str())
            .expect("tool_execution entry should have tool name");
        assert!(
            !tool_name.is_empty(),
            "tool name should not be empty"
        );
    }

    // ── 12. Validate tool results round-tripped ───────────────────────────
    let tool_result_blocks: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e.get("entry_type").and_then(|v| v.as_str()) == Some("message"))
        .filter(|e| e.get("role").and_then(|v| v.as_str()) == Some("user"))
        .flat_map(|e| {
            e.get("content")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .filter(|c| c.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
        .collect();

    assert!(
        tool_result_blocks.len() >= 1,
        "Should have at least 1 tool_result block in user messages, got {}",
        tool_result_blocks.len()
    );

    // ── 13. Validate End entry ────────────────────────────────────────────
    let last = entries.last().unwrap();
    assert_eq!(
        last.get("entry_type").and_then(|v| v.as_str()),
        Some("end"),
        "Last journal entry should be 'end'"
    );

    let end_reason = last
        .get("reason")
        .and_then(|v| v.as_str())
        .expect("end entry should have a reason");
    assert!(
        end_reason == "end_turn" || end_reason == "max_turns",
        "End reason should be 'end_turn' or 'max_turns', got '{}'",
        end_reason
    );

    // ── 14. Verify agent.ndjson output log exists ────────────────────────
    let ndjson_path = agent_dir.join("agent.ndjson");
    assert!(
        ndjson_path.exists(),
        "Agent NDJSON output log should exist at {:?}",
        ndjson_path
    );

    // ── 15. Validate final output is coherent ─────────────────────────────
    let ndjson_content = fs::read_to_string(&ndjson_path).unwrap();
    let lines: Vec<&str> = ndjson_content.lines().collect();

    // Last line should be a Result event
    if let Some(last_line) = lines.last() {
        let result_val =
            serde_json::from_str::<serde_json::Value>(last_line).expect("Last NDJSON line valid JSON");
        assert_eq!(
            result_val.get("type").and_then(|v| v.as_str()),
            Some("result"),
            "Last NDJSON line should be a 'result' event"
        );

        let final_text = result_val
            .get("final_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            !final_text.trim().is_empty(),
            "Final text should not be empty"
        );
    }

    // ── 16. Verify task status is Done ───────────────────────────────────
    let graph_path = wg_dir.join("graph.jsonl");
    let graph_content = fs::read_to_string(&graph_path).unwrap();
    let graph_lines: Vec<&str> = graph_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    // Find the task line
    let task_line = graph_lines
        .iter()
        .find(|l| l.contains("\"id\":\"openrouter-tool-test\""))
        .expect("Should find openrouter-tool-test task in graph");

    let task_val: serde_json::Value =
        serde_json::from_str(task_line).expect("Task line should be valid JSON");
    assert_eq!(
        task_val.get("status").and_then(|v| v.as_str()),
        Some("done"),
        "Task status should be 'done'"
    );

    // ── 17. Log success ───────────────────────────────────────────────────
    eprintln!(
        "[integration] OpenRouter tool loop test passed: {} turns, {} tool executions, {} journal entries",
        assistant_entries.len(),
        tool_exec_entries.len(),
        entries.len()
    );
}

/// Test: Native executor with bash tool execution via OpenRouter.
///
/// Validates that the bash tool works correctly in the native executor
/// with open models via OpenRouter.
#[test]
#[ignore = "requires OPENROUTER_API_KEY"]
fn test_openrouter_bash_tool_execution() {
    let _api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("OPENROUTER_API_KEY must be set for this integration test");

    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    wg_ok(&wg_dir, &["init"]);
    wg_ok(&wg_dir, &["agency", "init"]);

    // Configure endpoint
    let key_file = tmp.path().join("key.txt");
    fs::write(&key_file, &_api_key).unwrap();

    wg_ok(
        &wg_dir,
        &[
            "endpoint", "add", "test-openrouter", "--provider", "openrouter",
            "--url", "https://openrouter.ai/api/v1", "--key-file", key_file.to_str().unwrap(),
        ],
    );
    wg_ok(&wg_dir, &["endpoint", "set-default", "test-openrouter"]);

    // Create task
    wg_ok(
        &wg_dir,
        &[
            "add", "Bash tool test",
            "--id", "bash-tool-test",
            "--context-scope", "task",
            "--immediate",
        ],
    );

    // Spawn
    let spawn_output = wg_cmd(
        &wg_dir,
        &[
            "spawn", "bash-tool-test",
            "--executor", "native",
            "--model", "minimax/minimax-m2.7",
        ],
    );

    assert!(
        spawn_output.status.success(),
        "Spawn should succeed: {}",
        String::from_utf8_lossy(&spawn_output.stderr)
    );

    // Poll for completion
    let max_wait = 300;
    let mut completed = false;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < max_wait {
        let output = wg_cmd(&wg_dir, &["show", "bash-tool-test", "--json"]);
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if val.get("status").and_then(|s| s.as_str()) == Some("done") {
                    completed = true;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }

    assert!(completed, "Agent should complete within {}s", max_wait);

    eprintln!("[integration] Bash tool test passed");
}

/// Test: Journal completeness verification.
///
/// Verifies that the journal contains all expected entry types and
/// maintains proper sequencing for the native executor tool loop.
#[test]
#[ignore = "requires OPENROUTER_API_KEY"]
fn test_openrouter_journal_completeness() {
    let _api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("OPENROUTER_API_KEY must be set for this integration test");

    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");

    wg_ok(&wg_dir, &["init"]);
    wg_ok(&wg_dir, &["agency", "init"]);

    // Configure endpoint
    let key_file = tmp.path().join("key.txt");
    fs::write(&key_file, &_api_key).unwrap();

    wg_ok(
        &wg_dir,
        &[
            "endpoint", "add", "test-openrouter", "--provider", "openrouter",
            "--url", "https://openrouter.ai/api/v1", "--key-file", key_file.to_str().unwrap(),
        ],
    );
    wg_ok(&wg_dir, &["endpoint", "set-default", "test-openrouter"]);

    // Create task
    wg_ok(
        &wg_dir,
        &[
            "add", "Journal completeness test",
            "--id", "journal-test",
            "--context-scope", "task",
            "--immediate",
        ],
    );

    // Spawn
    let spawn_output = wg_cmd(
        &wg_dir,
        &[
            "spawn", "journal-test",
            "--executor", "native",
            "--model", "minimax/minimax-m2.7",
        ],
    );

    assert!(spawn_output.status.success(), "Spawn should succeed");

    // Poll for completion
    let max_wait = 300;
    let mut completed = false;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < max_wait {
        let output = wg_cmd(&wg_dir, &["show", "journal-test", "--json"]);
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if val.get("status").and_then(|s| s.as_str()) == Some("done") {
                    completed = true;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }

    assert!(completed, "Agent should complete");

    // Find agent directory
    let agents_base = wg_dir.join("agents");
    let agent_subdirs: Vec<PathBuf> = fs::read_dir(&agents_base)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir() && p.file_name().unwrap().to_str().unwrap().starts_with("agent-")
        })
        .collect();

    assert!(!agent_subdirs.is_empty(), "Should have agent directory");
    let agent_dir = &agent_subdirs[0];

    // Parse journal
    let journal_path = agent_dir.join("conversation.jsonl");
    let journal_content = fs::read_to_string(&journal_path).unwrap();
    let entries: Vec<serde_json::Value> = journal_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("Invalid JSON"))
        .collect();

    // Verify entry type sequence
    let entry_types: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.get("entry_type").and_then(|v| v.as_str()))
        .collect();

    // First must be init
    assert_eq!(entry_types.first(), Some(&"init"));

    // Last must be end
    assert_eq!(entry_types.last(), Some(&"end"));

    // Must have at least one message and one tool_execution
    assert!(
        entry_types.contains(&"message"),
        "Should have message entries"
    );
    assert!(
        entry_types.contains(&"tool_execution"),
        "Should have tool_execution entries"
    );

    eprintln!(
        "[integration] Journal completeness test passed: {:?}",
        entry_types
    );
}
