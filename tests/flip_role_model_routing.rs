//! Regression tests for FLIP evaluation honoring configured agency role
//! models (bug-flip-role-model-routing).
//!
//! Before the fix, `run_flip` resolved the inference/comparison *metadata*
//! models as CLI > task-model > config, so an explicitly configured
//! `[models.flip_inference]` / `[models.flip_comparison]` was shadowed
//! whenever the source task carried a runtime model (almost always). The
//! actual LLM calls already routed through the configured role via
//! `run_lightweight_llm_call`; only the recorded metadata was wrong.
//!
//! These tests drive the `--flip --dry-run` CLI path, which prints the
//! resolved inference/comparison models and the `FLIP models:` stderr line
//! that carries their *source* — no live LLM call required.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use worksgood::graph::{LogEntry, Node, Status, Task, WorkGraph};
use worksgood::parser::save_graph;

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
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_TASK_ID")
        .env("WG_SMOKE_AGENT_OVERRIDE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

/// Build a WorkGraph in a temp dir, returning the `.wg` directory path.
fn setup_workgraph(tmp: &TempDir, tasks: Vec<Task>) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

/// Write a config.toml into the given `.wg` directory.
fn write_config(wg_dir: &Path, toml: &str) {
    std::fs::write(wg_dir.join("config.toml"), toml).unwrap();
}

/// A task with a spawn-log entry that records a runtime model — this is the
/// "actor task model" that previously shadowed the configured FLIP roles.
fn task_with_spawn_model(id: &str, model: &str) -> Task {
    Task {
        id: id.to_string(),
        title: id.to_string(),
        status: Status::Done,
        log: vec![LogEntry {
            timestamp: String::new(),
            actor: None,
            user: None,
            message: format!("Spawned by coordinator --executor claude --model {model}"),
        }],
        ..Task::default()
    }
}

/// FLIP requires `agency.flip_enabled = true`; freeform labels do not enable it.
const FLIP_ENABLED_CONFIG: &str = r#"
[agency]
flip_enabled = true
"#;

#[test]
fn test_flip_uses_configured_role_models_over_task_model() {
    // Regression: task_agent / runtime model is distinct from flip_inference
    // and flip_comparison. The FLIP dry run must report the configured FLIP
    // role models, not the task model.
    let tmp = TempDir::new().unwrap();
    let task = task_with_spawn_model("a", "openrouter:z-ai/glm-5.2");
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    write_config(
        &wg_dir,
        r#"
[agency]
flip_enabled = true

[models.flip_inference]
model = "openrouter:deepseek/deepseek-v4-flash"

[models.flip_comparison]
model = "openrouter:deepseek/deepseek-v4-flash"
"#,
    );

    let out = wg_cmd(&wg_dir, &["evaluate", "run", "a", "--flip", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg evaluate run --flip --dry-run failed.\nstderr: {}\nstdout: {}",
        stderr,
        stdout
    );

    // The configured role model must appear — not the task model.
    assert!(
        stdout.contains("Inference model: openrouter:deepseek/deepseek-v4-flash"),
        "FLIP inference model should be the configured role model, not the task model.\nstdout: {}",
        stdout
    );
    assert!(
        stdout.contains("Comparison model: openrouter:deepseek/deepseek-v4-flash"),
        "FLIP comparison model should be the configured role model, not the task model.\nstdout: {}",
        stdout
    );

    // The task model must NOT be reported as the FLIP models.
    assert!(
        !stdout.contains("Inference model: openrouter:z-ai/glm-5.2"),
        "FLIP inference model leaked the task model.\nstdout: {}",
        stdout
    );

    // stderr carries the source attribution.
    assert!(
        stderr.contains("inference='openrouter:deepseek/deepseek-v4-flash' (role/config)"),
        "FLIP inference source should be role/config, not task-model.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("comparison='openrouter:deepseek/deepseek-v4-flash' (role/config)"),
        "FLIP comparison source should be role/config, not task-model.\nstderr: {}",
        stderr
    );
}

#[test]
fn test_flip_falls_back_to_task_model_when_role_unconfigured() {
    // Preserve current fallback behavior: when no flip_inference/flip_comparison
    // role model is configured, the task model is used (source = task-model).
    let tmp = TempDir::new().unwrap();
    let task = task_with_spawn_model("a", "openrouter:z-ai/glm-5.2");
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    // flip_enabled only — no [models.flip_*] tables.
    write_config(&wg_dir, FLIP_ENABLED_CONFIG);

    let out = wg_cmd(&wg_dir, &["evaluate", "run", "a", "--flip", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg evaluate run --flip --dry-run failed.\nstderr: {}\nstdout: {}",
        stderr,
        stdout
    );

    assert!(
        stdout.contains("Inference model: openrouter:z-ai/glm-5.2"),
        "FLIP inference should fall back to task model when no role model configured.\nstdout: {}",
        stdout
    );
    assert!(
        stdout.contains("Comparison model: openrouter:z-ai/glm-5.2"),
        "FLIP comparison should fall back to task model when no role model configured.\nstdout: {}",
        stdout
    );
    assert!(
        stderr.contains("(task-model)"),
        "FLIP source should be task-model in fallback case.\nstderr: {}",
        stderr
    );
}

#[test]
fn test_flip_cli_override_wins_over_role_config() {
    // CLI --evaluator-model must still take precedence over both the
    // configured role model and the task model.
    let tmp = TempDir::new().unwrap();
    let task = task_with_spawn_model("a", "openrouter:z-ai/glm-5.2");
    let wg_dir = setup_workgraph(&tmp, vec![task]);

    write_config(
        &wg_dir,
        r#"
[agency]
flip_enabled = true

[models.flip_inference]
model = "openrouter:deepseek/deepseek-v4-flash"

[models.flip_comparison]
model = "openrouter:deepseek/deepseek-v4-flash"
"#,
    );

    let out = wg_cmd(
        &wg_dir,
        &[
            "evaluate",
            "run",
            "a",
            "--flip",
            "--dry-run",
            "--evaluator-model",
            "claude:sonnet",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg evaluate run --flip --dry-run failed.\nstderr: {}\nstdout: {}",
        stderr,
        stdout
    );

    assert!(
        stdout.contains("Inference model: claude:sonnet"),
        "CLI --evaluator-model should win over role config.\nstdout: {}",
        stdout
    );
    assert!(
        stdout.contains("Comparison model: claude:sonnet"),
        "CLI --evaluator-model should win over role config.\nstdout: {}",
        stdout
    );
    assert!(
        stderr.contains("(cli-override)"),
        "FLIP source should be cli-override.\nstderr: {}",
        stderr
    );
}
