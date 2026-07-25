//! Integration tests for `wg init` model/executor selection.
//!
//! As of `simplify-executor-taxonomy`, `wg init` derives the handler
//! from the model spec's provider prefix. The legacy `--executor` /
//! `-x` flag is still accepted (with a deprecation warning) for one
//! release. These tests cover both the new (`-m`) and legacy (`-x`)
//! invocations.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

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

fn wg_cmd_in(dir: &Path, args: &[&str]) -> std::process::Output {
    // Isolate HOME so the developer's real `~/.wg/config.toml` (which may
    // pin a non-default model/route) does not leak into `wg init` defaults
    // and contaminate the test assertion. Other integration tests
    // (`integration_setup_routes.rs`, `smoke_native_executor.rs`) already
    // do this; `integration_init.rs` was the lone holdout.
    let fake_home = dir.join("_fake_home");
    std::fs::create_dir_all(&fake_home)
        .unwrap_or_else(|e| panic!("Failed to create fake home dir: {}", e));
    Command::new(wg_binary())
        .current_dir(dir)
        .env("HOME", &fake_home)
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn assert_lockstep_agent_guides(project_dir: &Path) {
    let claude_md = std::fs::read(project_dir.join("CLAUDE.md")).expect("CLAUDE.md should exist");
    let agents_md = std::fs::read(project_dir.join("AGENTS.md")).expect("AGENTS.md should exist");

    assert_eq!(
        claude_md, agents_md,
        "CLAUDE.md and AGENTS.md should be byte-for-byte identical"
    );

    let body = String::from_utf8(claude_md).expect("agent guide should be UTF-8");
    assert!(body.contains("wg agent-guide"));
    assert!(body.contains("layer-2"));
    assert!(body.contains("wg quickstart"));
    assert!(body.contains("WorksGood"));
    assert!(body.contains("<!-- worksgood-managed-guide:v1:start -->"));
    assert!(body.contains("<!-- worksgood-managed-guide:v1:end -->"));
    assert!(
        !body.to_ascii_lowercase().contains("workgraph"),
        "fresh guides must not contain retired branding: {body}"
    );
}

// ---------------------------------------------------------------------------
// test_init_without_flags_is_graph_only
// ---------------------------------------------------------------------------

/// `wg init` with no flags creates a graph but selects no LLM route.
#[test]
fn test_init_without_flags_is_graph_only() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(tmp.path(), &["init"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg init with no inputs should remain graph-only.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    let wg_dir = tmp.path().join(".wg");
    assert!(wg_dir.exists(), ".wg directory should be created");
    assert_lockstep_agent_guides(tmp.path());

    assert!(stdout.contains("graph-only"));
    let config_text = std::fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert!(
        !config_text
            .lines()
            .any(|line| line.trim().starts_with("model =")),
        "graph-only init must not persist a route: {config_text}"
    );
}

// ---------------------------------------------------------------------------
// test_init_with_executor_claude_succeeds
// ---------------------------------------------------------------------------

/// Legacy `wg init --executor claude` must still succeed (deprecated, but
/// supported for one release). The dispatcher's resolved handler must
/// be claude — verified through `parse_model_spec` rather than the
/// (now-stripped) `coordinator.executor` field.
#[test]
fn test_init_with_executor_claude_is_rejected() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(tmp.path(), &["init", "--executor", "claude"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stdout: {stdout}");
    assert!(stderr.contains("Pi is the sole LLM handler"), "{stderr}");
}

#[test]
fn test_init_routes_write_lockstep_agent_guides() {
    for (route, extra_args) in [("pi", Vec::<&str>::new())] {
        let tmp = TempDir::new().unwrap();
        let mut args = vec!["init", "--route", route, "--no-agency"];
        args.extend(extra_args);

        let output = wg_cmd_in(tmp.path(), &args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "wg {:?} should succeed.\nstdout: {}\nstderr: {}",
            args,
            stdout,
            stderr
        );

        assert_lockstep_agent_guides(tmp.path());
    }
}

// ---------------------------------------------------------------------------
// test_init_endpoint_only_still_requires_executor
// ---------------------------------------------------------------------------

/// `wg init -e https://example.com` (endpoint only, no model + no
/// executor + no route) must fail with a helpful error pointing at the
/// new `-m provider:model` flow. An endpoint alone is ambiguous —
/// without a model, wg can't pick a handler.
#[test]
fn test_init_endpoint_only_still_requires_executor() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(tmp.path(), &["init", "-e", "https://example.com"]);

    assert!(
        !output.status.success(),
        "wg init with only -e (no -m, no -x, no --route) should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stderr, stdout);

    // Error must offer the new model-spec flow as the migration target.
    assert!(
        combined.contains("configure the provider in Pi"),
        "error must show the new model+route flow. Got:\n{}",
        combined
    );
}

// ---------------------------------------------------------------------------
// test_init_executor_and_endpoint_succeeds
// ---------------------------------------------------------------------------

/// Legacy `wg init --executor shell -e <url>` must still succeed
/// (deprecated). `shell` is special — it's an exec_mode rather than an
/// LLM handler, so `coordinator.executor = "shell"` is preserved
/// (`strip_redundant_executor_keys` only strips when the model spec
/// implies the same handler, which shell never does).
#[test]
fn test_init_executor_and_endpoint_are_rejected() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(
        tmp.path(),
        &["init", "--executor", "shell", "-e", "http://127.0.0.1:9999"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stdout: {stdout}");
    assert!(stderr.contains("configure the provider in Pi"), "{stderr}");
    assert!(!tmp.path().join(".wg/config.toml").exists());
}
