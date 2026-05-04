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
    Command::new(wg_binary())
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

// ---------------------------------------------------------------------------
// test_init_requires_executor
// ---------------------------------------------------------------------------

/// `wg init` with no flags at all must fail with a helpful error
/// pointing the user at the new `-m provider:model` flow (or `--route`).
#[test]
fn test_init_requires_executor() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(tmp.path(), &["init"]);

    assert!(
        !output.status.success(),
        "wg init with no inputs should fail with a helpful error"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stderr, stdout);

    // Error must offer at least one example using the new model-spec form.
    assert!(
        combined.contains("-m claude:opus")
            || combined.contains("provider:model")
            || combined.contains("--route"),
        "error must show the new model+route flow. Got:\n{}",
        combined
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
fn test_init_with_executor_claude_succeeds() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(tmp.path(), &["init", "--executor", "claude"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg init --executor claude should still succeed (deprecated).\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // Legacy invocations must emit a deprecation warning.
    assert!(
        stderr.contains("deprecated"),
        "legacy --executor invocation should emit a deprecation warning, got: {}",
        stderr
    );

    let wg_dir = tmp.path().join(".wg");
    assert!(wg_dir.exists(), ".wg directory should be created");

    let config = workgraph::config::Config::load(&wg_dir).expect("config.toml should be loadable");
    // The handler is now derived from the model spec. The fresh config
    // should have claude:* set as the model — that's what the route
    // populates for `--executor claude`.
    let agent_model = &config.agent.model;
    assert!(
        agent_model.starts_with("claude:")
            || agent_model == "claude"
            || agent_model.is_empty()
            || workgraph::dispatch::handler_for_model(agent_model)
                == workgraph::dispatch::ExecutorKind::Claude,
        "agent.model must imply the claude handler, got: {:?}",
        agent_model
    );
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
        combined.contains("provider:model")
            || combined.contains("-m claude:opus")
            || combined.contains("--route"),
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
fn test_init_executor_and_endpoint_succeeds() {
    let tmp = TempDir::new().unwrap();

    let output = wg_cmd_in(
        tmp.path(),
        &["init", "--executor", "shell", "-e", "http://127.0.0.1:9999"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "wg init --executor shell -e http://... should succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    let wg_dir = tmp.path().join(".wg");
    let config = workgraph::config::Config::load(&wg_dir).expect("config.toml should be loadable");

    assert_eq!(
        config.coordinator.executor.as_deref(),
        Some("shell"),
        "coordinator.executor should be 'shell' (no model implies it)"
    );

    let default_ep = config
        .llm_endpoints
        .endpoints
        .iter()
        .find(|e| e.is_default)
        .expect("a default endpoint should be written");
    assert_eq!(
        default_ep.url.as_deref(),
        Some("http://127.0.0.1:9999"),
        "endpoint URL should be persisted"
    );
}
