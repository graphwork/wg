//! Tests for the deprecated --provider CLI flags.
//!
//! Validates that all deprecated --provider flags:
//! 1. Still succeed (backward compat)
//! 2. Emit a deprecation warning on stderr
//! 3. Suggest the provider:model format replacement

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

fn setup_workgraph(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = WorkGraph::new();
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

/// Assert command succeeds and stderr contains the deprecation warning.
fn assert_deprecated_warning(output: &std::process::Output, args: &[&str], expected_substr: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg {:?} should succeed (backward compat).\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    assert!(
        stderr.contains("deprecated"),
        "wg {:?} should emit deprecation warning on stderr.\nstderr: {}",
        args,
        stderr
    );
    assert!(
        stderr.contains(expected_substr),
        "wg {:?} stderr should mention '{}' replacement.\nstderr: {}",
        args,
        expected_substr,
        stderr
    );
}

// ===========================================================================
// wg add --provider
// ===========================================================================

#[test]
fn add_provider_flag_emits_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["add", "test-task", "--provider", "openrouter"];
    let output = wg_cmd(&wg_dir, &args);
    assert_deprecated_warning(&output, &args, "provider:model");
}

#[test]
fn add_provider_flag_suggests_claude_for_anthropic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["add", "test-task-anthropic", "--provider", "anthropic"];
    let output = wg_cmd(&wg_dir, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg add --provider anthropic should succeed.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("claude:MODEL"),
        "Should suggest claude: prefix for anthropic.\nstderr: {}",
        stderr
    );
}

// ===========================================================================
// wg update --provider
// ===========================================================================

#[test]
fn update_provider_flag_emits_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // Create a task first
    let create_output = wg_cmd(&wg_dir, &["add", "update-target"]);
    assert!(
        create_output.status.success(),
        "Failed to create task for update test"
    );

    let args = ["edit", "update-target", "--provider", "openai"];
    let output = wg_cmd(&wg_dir, &args);
    assert_deprecated_warning(&output, &args, "provider:model");
}

// ===========================================================================
// wg config --coordinator-provider
// ===========================================================================

#[test]
fn config_coordinator_provider_flag_emits_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["config", "--coordinator-provider", "openrouter"];
    let output = wg_cmd(&wg_dir, &args);
    assert_deprecated_warning(&output, &args, "--dispatcher-model");
}

#[test]
fn config_coordinator_provider_flag_suggests_claude_for_anthropic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["config", "--coordinator-provider", "anthropic"];
    let output = wg_cmd(&wg_dir, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg config --coordinator-provider anthropic should succeed.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("claude:"),
        "Should suggest claude: prefix for anthropic.\nstderr: {}",
        stderr
    );
}

// ===========================================================================
// wg config --set-provider
// ===========================================================================

#[test]
fn config_set_provider_flag_emits_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["config", "--set-provider", "evaluator", "openrouter"];
    let output = wg_cmd(&wg_dir, &args);
    assert_deprecated_warning(&output, &args, "--set-model");
}

#[test]
fn config_set_provider_flag_suggests_claude_for_anthropic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["config", "--set-provider", "default", "anthropic"];
    let output = wg_cmd(&wg_dir, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg config --set-provider default anthropic should succeed.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("claude:MODEL"),
        "Should suggest claude: prefix for anthropic.\nstderr: {}",
        stderr
    );
}

// ===========================================================================
// wg config --role-provider
// ===========================================================================

#[test]
fn config_role_provider_flag_emits_deprecation_warning() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let args = ["config", "--role-provider", "triage=openai"];
    let output = wg_cmd(&wg_dir, &args);
    assert_deprecated_warning(&output, &args, "--set-model");
}
