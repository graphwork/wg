//! Integration tests for config CLI commands: init, show, update, and list.
//!
//! These tests invoke the real `wg` binary to verify that configuration
//! management commands work correctly end-to-end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::WorkGraph;
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

fn wg_cmd_with_home(wg_dir: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_MODEL")
        .env_remove("WG_TIER")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_TASK_ID")
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

fn wg_ok_with_home(wg_dir: &Path, home: &Path, args: &[&str]) -> String {
    let output = wg_cmd_with_home(wg_dir, home, args);
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

fn setup_workgraph(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = WorkGraph::new();
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

fn codex_one_line_config_args() -> Vec<&'static str> {
    vec![
        "config",
        "--local",
        "--model",
        "codex:gpt-5.5",
        "--coordinator-model",
        "codex:gpt-5.5",
        "--tier",
        "fast=codex:gpt-5.4-mini",
        "--tier",
        "standard=codex:gpt-5.4",
        "--tier",
        "premium=codex:gpt-5.5",
        "--set-model",
        "default",
        "codex:gpt-5.5",
        "--set-model",
        "task_agent",
        "codex:gpt-5.5",
        "--set-model",
        "evaluator",
        "codex:gpt-5.4-mini",
        "--set-model",
        "assigner",
        "codex:gpt-5.4-mini",
        "--flip-model",
        "codex:gpt-5.4-mini",
        "--auto-assign",
        "true",
        "--auto-evaluate",
        "true",
    ]
}

// ===========================================================================
// wg config --init
// ===========================================================================

#[test]
fn config_init_creates_file() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--init"]);
    assert!(output.contains("Created") || output.contains("already exists"));

    // Config file should exist
    let config_path = wg_dir.join("config.toml");
    assert!(
        config_path.exists(),
        "config.toml should be created by --init"
    );
}

#[test]
fn config_init_idempotent() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // First init
    wg_ok(&wg_dir, &["config", "--init"]);
    // Second init — should not fail
    let output = wg_ok(&wg_dir, &["config", "--init"]);
    assert!(output.contains("already exists"));
}

// ===========================================================================
// wg config --show
// ===========================================================================

#[test]
fn config_show_default() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--show"]);
    assert!(output.contains("workgraph Configuration"));
    assert!(output.contains("[agent]"));
    assert!(output.contains("[dispatcher]"));
    assert!(output.contains("executor"));
}

#[test]
fn config_show_json() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["--json", "config", "--show"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from config --show --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.get("agent").is_some());
    assert!(json.get("dispatcher").is_some());
}

#[test]
fn config_show_after_init() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // Init then show
    wg_ok(&wg_dir, &["config", "--init"]);
    let output = wg_ok(&wg_dir, &["config", "--show"]);
    assert!(output.contains("workgraph Configuration"));
    assert!(output.contains("executor"));
}

// ===========================================================================
// wg config --<setting> (update)
// ===========================================================================

#[test]
fn config_set_executor() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--executor", "shell"]);
    assert!(output.contains("Set agent.executor"));
    assert!(output.contains("shell"));

    // Verify it persisted
    let show = wg_ok(&wg_dir, &["config", "--show"]);
    assert!(show.contains("shell"));
}

#[test]
fn config_set_model() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--model", "claude:haiku"]);
    assert!(output.contains("Set agent.model"));
    assert!(output.contains("Set dispatcher.model"));
    assert!(output.contains("claude:haiku"));
}

#[test]
fn config_set_max_agents() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--max-agents", "5"]);
    assert!(output.contains("Set coordinator.max_agents"));
    assert!(output.contains("5"));
}

#[test]
fn config_set_multiple_values() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(
        &wg_dir,
        &[
            "config",
            "--executor",
            "native",
            "--model",
            "claude:sonnet",
            "--max-agents",
            "3",
        ],
    );
    assert!(output.contains("Set agent.executor"));
    assert!(output.contains("Set agent.model"));
    assert!(output.contains("Set coordinator.max_agents"));
}

#[test]
fn config_set_coordinator_executor() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--coordinator-executor", "native"]);
    assert!(output.contains("Set dispatcher.executor"));
}

#[test]
fn config_one_line_codex_overwrites_dispatcher_model_and_executor() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(fake_home.join(".config")).unwrap();

    fs::write(
        wg_dir.join("config.toml"),
        r#"[agent]
model = "claude:opus"

[dispatcher]
executor = "claude"
model = "claude:opus"
"#,
    )
    .unwrap();

    let args = codex_one_line_config_args();
    let output = wg_ok_with_home(&wg_dir, &fake_home, &args);
    assert!(
        output.contains("Set dispatcher.model = \"codex:gpt-5.5\""),
        "config output should name canonical dispatcher.model, got:\n{}",
        output
    );
    assert!(
        !output.contains("Set coordinator.model"),
        "config output should not present coordinator.model as the written key:\n{}",
        output
    );
    assert!(
        output.contains("Cleared deprecated dispatcher.executor"),
        "model write should clear a stale dispatcher.executor pin:\n{}",
        output
    );

    let saved = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert!(
        saved.contains("[dispatcher]") && saved.contains("model = \"codex:gpt-5.5\""),
        "saved config should write canonical dispatcher.model:\n{}",
        saved
    );
    assert!(
        !saved.contains("executor = \"claude\""),
        "stale dispatcher.executor must not keep masking the model:\n{}",
        saved
    );

    let merged = wg_ok_with_home(&wg_dir, &fake_home, &["config", "--merged"]);
    assert!(
        merged.contains("executor = \"codex\""),
        "effective merged config should resolve codex executor:\n{}",
        merged
    );
    assert!(
        merged.contains("model = \"codex:gpt-5.5\""),
        "effective merged config should resolve codex dispatcher model:\n{}",
        merged
    );

    let restart = wg_ok_with_home(&wg_dir, &fake_home, &["service", "restart"]);
    let _ = wg_cmd_with_home(&wg_dir, &fake_home, &["service", "stop"]);
    assert!(
        restart.contains("executor=codex, model=codex:gpt-5.5"),
        "service restart should use the daemon-facing codex route:\n{}",
        restart
    );
}

#[test]
fn config_set_guardrails() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(
        &wg_dir,
        &["config", "--max-child-tasks", "15", "--max-task-depth", "5"],
    );
    assert!(output.contains("Set guardrails.max_child_tasks_per_agent"));
    assert!(output.contains("Set guardrails.max_task_depth"));
}

// ===========================================================================
// wg config --list
// ===========================================================================

#[test]
fn config_list_shows_merged() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["config", "--list"]);
    assert!(output.contains("workgraph Configuration (merged)"));
    // Should show source annotations
    assert!(
        output.contains("default") || output.contains("local") || output.contains("global"),
        "Expected source annotation in config --list output, got: {}",
        output
    );
}

#[test]
fn config_list_json() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["--json", "config", "--list"]);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|e| {
        panic!(
            "Invalid JSON from config --list --json: {}\nOutput: {}",
            e, output
        )
    });
    assert!(json.is_array(), "Expected array from config --list --json");
    // Each entry should have key, value, source
    if let Some(arr) = json.as_array()
        && !arr.is_empty()
    {
        let first = &arr[0];
        assert!(first.get("key").is_some());
        assert!(first.get("value").is_some());
        assert!(first.get("source").is_some());
    }
}

#[test]
fn config_list_reflects_updates() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // Set a custom value
    wg_ok(&wg_dir, &["config", "--max-agents", "42"]);

    // List should show local source for that key
    let output = wg_ok(&wg_dir, &["config", "--list"]);
    assert!(
        output.contains("42"),
        "Expected updated value 42 in config --list, got: {}",
        output
    );
    assert!(
        output.contains("local"),
        "Expected 'local' source annotation for updated value, got: {}",
        output
    );
}

// ===========================================================================
// wg config (no flags = show)
// ===========================================================================

#[test]
fn config_no_flags_shows_config() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // Bare `wg config` should behave like `wg config --show`
    let output = wg_ok(&wg_dir, &["config"]);
    assert!(output.contains("workgraph Configuration"));
}
