//! Validation tests for `simplify-executor-taxonomy`.
//!
//! User-facing concept: the user picks a `(model, endpoint)` pair. wg
//! derives the handler / wire protocol / executor from the model's
//! provider prefix.
//!
//! These tests pin down:
//! 1. `wg init -m <provider>:<model>` works without `--executor` / `-x`
//! 2. `wg init -m local:... -e <url>` routes to the nex / native handler
//! 3. Legacy `wg init -x <executor>` still works but emits a deprecation
//!    warning (one release of grace)
//! 4. Legacy `[agent].executor` / `[dispatcher].executor` config keys
//!    still load but emit a deprecation warning
//! 5. `dispatch::handler_for_model` is the single source of truth
//! 6. `--help` text no longer markets `--executor` as a primary flag

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Locate the freshly-built `wg` binary so these tests don't depend on
/// the global `cargo install` having been run.
fn wg_binary() -> std::path::PathBuf {
    // CARGO_BIN_EXE_<name> is set for integration tests; falls back to
    // target/debug/wg for local runs that bypass the env var.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_wg") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/wg")
}

fn wg_init(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(dir)
        .arg("init")
        .args(args)
        // Skip agency to keep the tests fast and focused.
        .arg("--no-agency")
        .output()
        .expect("spawn wg init")
}

fn read_config_toml(wg_dir: &Path) -> String {
    std::fs::read_to_string(wg_dir.join("config.toml"))
        .unwrap_or_else(|_| String::new())
}

// --------------------------------------------------------------------
// 1. test_init_without_executor_flag_uses_model_prefix
// --------------------------------------------------------------------
#[test]
fn test_init_without_executor_flag_uses_model_prefix() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");

    let out = wg_init(&wg_dir, &["-m", "claude:opus"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`wg init -m claude:opus` (no -x) must succeed, got status={:?} stderr={}",
        out.status,
        stderr
    );

    // No deprecation warning should fire for the new path.
    assert!(
        !stderr.contains("`--executor"),
        "no --executor was passed, deprecation warning should not appear: {}",
        stderr
    );

    // The config should reflect a claude-handler setup. Model is set; we
    // accept either an explicit `executor = "claude"` or no executor key
    // (both resolve to claude via handler_for_model). What matters is
    // the provider prefix is preserved in the model field.
    let config = read_config_toml(&wg_dir);
    assert!(
        config.contains("opus"),
        "config.toml must contain the model spec: {}",
        config
    );
}

// --------------------------------------------------------------------
// 2. test_init_with_local_model_routes_to_nex
// --------------------------------------------------------------------
#[test]
fn test_init_with_local_model_routes_to_nex() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");

    let out = wg_init(
        &wg_dir,
        &[
            "-m",
            "local:qwen3-coder",
            "-e",
            "https://lambda01.example.com/v1",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`wg init -m local:qwen3-coder -e <url>` must succeed, got status={:?} stderr={}",
        out.status,
        stderr,
    );

    let config = read_config_toml(&wg_dir);
    // The endpoint URL must be present in config.toml.
    assert!(
        config.contains("lambda01.example.com"),
        "config.toml must contain the endpoint URL: {}",
        config
    );
    // The model name must be present.
    assert!(
        config.contains("qwen3-coder"),
        "config.toml must contain the model: {}",
        config
    );
    // Critically: the executor for `local:` models must resolve to native
    // (nex). We verify via the dispatch resolver below; here we just check
    // the persisted config picked up the local provider somehow. Either
    // `provider = "local"` (endpoint config) or `executor = "native"`
    // counts as evidence — both indicate the local-model routing landed.
    assert!(
        config.contains("\"local\"") || config.contains("\"native\""),
        "config.toml must reflect local/native handler routing: {}",
        config
    );
}

// --------------------------------------------------------------------
// 3. test_legacy_executor_flag_warns_and_works
// --------------------------------------------------------------------
#[test]
fn test_legacy_executor_flag_warns_and_works() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");

    let out = wg_init(&wg_dir, &["-x", "claude", "-m", "claude:opus"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`wg init -x claude -m claude:opus` must still succeed, got: {}",
        stderr
    );
    assert!(
        stderr.contains("deprecated"),
        "legacy `-x` invocation must emit a deprecation warning, got: {}",
        stderr
    );
    assert!(
        stderr.contains("--executor") || stderr.contains("`-x"),
        "deprecation warning must reference the deprecated flag, got: {}",
        stderr
    );
    let config = read_config_toml(&wg_dir);
    assert!(
        config.contains("opus"),
        "legacy invocation still produces a working config: {}",
        config
    );
}

// --------------------------------------------------------------------
// 4. test_legacy_executor_field_warns_on_load
// --------------------------------------------------------------------
#[test]
fn test_legacy_executor_field_warns_on_load() {
    let warnings = workgraph::config::deprecated_executor_warnings_for_toml(
        r#"
[agent]
executor = "claude"
model = "claude:opus"
"#,
    );
    assert!(
        warnings.iter().any(|w| w.contains("agent")),
        "[agent].executor must produce a deprecation warning, got: {:?}",
        warnings
    );

    let warnings = workgraph::config::deprecated_executor_warnings_for_toml(
        r#"
[dispatcher]
executor = "native"
"#,
    );
    assert!(
        warnings.iter().any(|w| w.contains("dispatcher")),
        "[dispatcher].executor must produce a deprecation warning, got: {:?}",
        warnings
    );

    // A clean config (model only, no executor) emits no warning.
    let warnings = workgraph::config::deprecated_executor_warnings_for_toml(
        r#"
[agent]
model = "claude:opus"
"#,
    );
    assert!(
        warnings.is_empty(),
        "model-only config must not emit any executor warning, got: {:?}",
        warnings
    );
}

// --------------------------------------------------------------------
// 5. test_handler_for_model_is_single_source_of_truth
// --------------------------------------------------------------------
#[test]
fn test_handler_for_model_is_single_source_of_truth() {
    use workgraph::dispatch::{ExecutorKind, handler_for_model};

    // Anthropic models → claude handler.
    assert_eq!(handler_for_model("claude:opus"), ExecutorKind::Claude);
    assert_eq!(handler_for_model("claude:sonnet-4-6"), ExecutorKind::Claude);
    // Bare aliases default to claude (historical convention).
    assert_eq!(handler_for_model("opus"), ExecutorKind::Claude);

    // OAI-compat / local / openrouter → native (nex) handler.
    // `nex:*` is the canonical prefix matching the `wg nex` subcommand.
    assert_eq!(handler_for_model("nex:qwen3-coder"), ExecutorKind::Native);
    // `local:` and `oai-compat:` are deprecated aliases for `nex:` —
    // they still route to the same handler for one release.
    assert_eq!(handler_for_model("local:qwen3-coder"), ExecutorKind::Native);
    assert_eq!(
        handler_for_model("openrouter:anthropic/claude-opus-4-6"),
        ExecutorKind::Native
    );
    assert_eq!(handler_for_model("oai-compat:gpt-5"), ExecutorKind::Native);

    // codex → codex handler.
    assert_eq!(handler_for_model("codex:gpt-5"), ExecutorKind::Codex);
}

// --------------------------------------------------------------------
// 6. test_no_executor_in_user_facing_help
// --------------------------------------------------------------------
#[test]
fn test_no_executor_in_user_facing_help() {
    // `wg init --help` should foreground -m / -e and either omit
    // `--executor` or mark it deprecated. We assert the soft form:
    // either the help no longer mentions `--executor` at all, OR every
    // mention sits next to "deprecated".
    let out = Command::new(wg_binary())
        .args(["init", "--help"])
        .output()
        .expect("spawn wg init --help");
    assert!(
        out.status.success(),
        "wg init --help must succeed: {:?}",
        out
    );
    let help = String::from_utf8_lossy(&out.stdout);

    // --model and --endpoint must be foregrounded as the primary flags.
    assert!(
        help.contains("--model") || help.contains("-m"),
        "wg init --help must mention --model / -m: {}",
        help
    );
    assert!(
        help.contains("--endpoint") || help.contains("-e"),
        "wg init --help must mention --endpoint / -e: {}",
        help
    );

    // If --executor is mentioned, it must be marked deprecated.
    if help.contains("--executor") || help.contains("-x") {
        assert!(
            help.to_lowercase().contains("deprecated"),
            "if wg init --help still mentions --executor, it must be \
             marked deprecated: {}",
            help
        );
    }
}
