use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use tempfile::TempDir;

use worksgood::graph::{Node, Task, WorkGraph, parse_delay};
use worksgood::parser::{load_graph, save_graph};

const ISOLATION_CHILD_ENV: &str = "WG_VERIFY_TIMEOUT_ISOLATION_CHILD";
const ISOLATION_SCRATCH_ENV: &str = "WG_VERIFY_TIMEOUT_ISOLATION_SCRATCH";

/// Build a `wg` subprocess at the same isolation boundary as a fresh shell.
///
/// Integration tests run inside worker/chat processes in CI and under WG's own
/// coordinator. A scratch project's CWD must not lose to an inherited WG_DIR,
/// and worker identity must not affect command behavior. Always use Cargo's
/// just-built binary rather than a possibly stale global installation.
fn wg_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wg"));
    command
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID");
    command
}

fn run_wg(project_root: &Path, args: &[&str]) -> Result<Output> {
    wg_command()
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to run built wg binary with args {args:?}"))
}

fn init_scratch(project_root: &Path) -> Result<()> {
    let output = run_wg(project_root, &["init", "--route", "claude-cli"])?;
    anyhow::ensure!(
        output.status.success(),
        "scratch init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn add_timeout_fixture(project_root: &Path, title: &str, timeout: &str) -> Result<()> {
    let output = run_wg(project_root, &["add", title, "--verify-timeout", timeout])?;
    anyhow::ensure!(
        output.status.success(),
        "failed to add {title:?} ({timeout}): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Populate the exact five fixtures whose old subprocesses polluted the live
/// graph when they inherited a parent WG_DIR.
fn populate_verify_timeout_fixtures(project_root: &Path) -> Result<()> {
    init_scratch(project_root)?;
    add_timeout_fixture(project_root, "Test CLI verify timeout", "999s")?;
    for timeout in ["30s", "5m", "2h", "1d"] {
        add_timeout_fixture(project_root, &format!("Test timeout {timeout}"), timeout)?;
    }
    Ok(())
}

#[test]
fn test_verify_timeout_parsing() -> Result<()> {
    // Test valid duration parsing
    assert_eq!(parse_delay("30s"), Some(30));
    assert_eq!(parse_delay("5m"), Some(300));
    assert_eq!(parse_delay("2h"), Some(7200));
    assert_eq!(parse_delay("1d"), Some(86400));

    // Test invalid duration parsing
    assert_eq!(parse_delay("invalid"), None);
    assert_eq!(parse_delay(""), None);
    assert_eq!(parse_delay("30x"), None);

    Ok(())
}

#[test]
fn test_verify_timeout_cli_basic() -> Result<()> {
    // Test the CLI --verify-timeout flag basic functionality
    let temp_dir = TempDir::new()?;
    let project_root = temp_dir.path();

    init_scratch(project_root)?;
    add_timeout_fixture(project_root, "Test CLI verify timeout", "999s")?;

    let list_output = run_wg(project_root, &["list"])?;
    anyhow::ensure!(
        list_output.status.success(),
        "wg list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_text = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_text.contains("Test CLI verify timeout"));

    Ok(())
}

#[test]
fn test_verify_timeout_duration_formats() -> Result<()> {
    // Test different duration formats through CLI
    let temp_dir = TempDir::new()?;
    let project_root = temp_dir.path();

    init_scratch(project_root)?;

    for timeout in ["30s", "5m", "2h", "1d"] {
        add_timeout_fixture(project_root, &format!("Test timeout {timeout}"), timeout)?;
    }

    Ok(())
}

/// Child entrypoint used by `test_inherited_wg_dir_cannot_pollute_parent_graph`.
/// It is a no-op in an ordinary test run; the parent re-executes this exact
/// integration-test binary with a sentinel worker environment.
#[test]
fn verify_timeout_isolation_child() -> Result<()> {
    if std::env::var_os(ISOLATION_CHILD_ENV).is_none() {
        return Ok(());
    }
    let scratch =
        std::env::var_os(ISOLATION_SCRATCH_ENV).context("isolation child missing scratch path")?;
    populate_verify_timeout_fixtures(Path::new(&scratch))
}

#[test]
fn test_inherited_wg_dir_cannot_pollute_parent_graph() -> Result<()> {
    let parent = TempDir::new()?;
    let parent_wg = parent.path().join(".wg");
    std::fs::create_dir_all(&parent_wg)?;

    let mut parent_graph = WorkGraph::new();
    parent_graph.add_node(Node::Task(Task {
        id: "sentinel-parent".to_string(),
        title: "Sentinel parent graph".to_string(),
        ..Task::default()
    }));
    let parent_graph_path = parent_wg.join("graph.jsonl");
    save_graph(&parent_graph, &parent_graph_path)?;

    let before = std::fs::read(&parent_graph_path)?;
    let before_hash = blake3::hash(&before);
    let scratch = TempDir::new()?;

    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", "verify_timeout_isolation_child", "--nocapture"])
        .env(ISOLATION_CHILD_ENV, "1")
        .env(ISOLATION_SCRATCH_ENV, scratch.path())
        .env("WG_DIR", &parent_wg)
        .env("WG_TASK_ID", "sentinel-parent-task")
        .env("WG_AGENT_ID", "sentinel-parent-agent")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "isolated child failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let after = std::fs::read(&parent_graph_path)?;
    assert_eq!(before, after, "sentinel parent graph content changed");
    assert_eq!(
        before_hash,
        blake3::hash(&after),
        "sentinel parent graph hash changed"
    );
    let parent_after = load_graph(&parent_graph_path)?;
    assert_eq!(parent_after.tasks().count(), 1);
    assert!(parent_after.get_task("sentinel-parent").is_some());
    assert!(
        parent_after.get_task(".coordinator").is_none(),
        "TUI/legacy bootstrap ghost appeared in the parent graph"
    );
    assert!(
        !parent_after
            .tasks()
            .any(|task| task.id.starts_with(".coordinator-")),
        "legacy coordinator task appeared in the parent graph"
    );

    let scratch_graph = load_graph(&scratch.path().join(".wg/graph.jsonl"))?;
    let fixtures: BTreeMap<_, _> = scratch_graph
        .tasks()
        .map(|task| (task.title.as_str(), task.verify_timeout.as_deref()))
        .collect();
    assert_eq!(
        fixtures,
        BTreeMap::from([
            ("Test CLI verify timeout", Some("999s")),
            ("Test timeout 30s", Some("30s")),
            ("Test timeout 5m", Some("5m")),
            ("Test timeout 2h", Some("2h")),
            ("Test timeout 1d", Some("1d")),
        ]),
        "all five timeout fixtures must land in the scratch graph"
    );
    assert!(scratch_graph.get_task(".coordinator").is_none());
    assert!(
        !scratch_graph
            .tasks()
            .any(|task| task.id.starts_with(".coordinator-"))
    );

    Ok(())
}

#[test]
fn test_verify_timeout_duration_conversion() -> Result<()> {
    // Test various time unit conversions
    assert_eq!(parse_delay("0s"), Some(0));
    assert_eq!(parse_delay("1s"), Some(1));
    assert_eq!(parse_delay("60s"), Some(60));
    assert_eq!(parse_delay("1m"), Some(60));
    assert_eq!(parse_delay("90m"), Some(5400));
    assert_eq!(parse_delay("1h"), Some(3600));
    assert_eq!(parse_delay("24h"), Some(86400));
    assert_eq!(parse_delay("1d"), Some(86400));
    assert_eq!(parse_delay("7d"), Some(604800));

    Ok(())
}

#[test]
fn test_verify_timeout_edge_cases() -> Result<()> {
    // Test edge cases in duration parsing
    assert_eq!(parse_delay("0s"), Some(0));
    assert_eq!(parse_delay("1s"), Some(1));

    // Test whitespace handling
    assert_eq!(parse_delay(" 30s "), Some(30));
    assert_eq!(parse_delay("5m "), Some(300));

    // Test invalid formats
    assert_eq!(parse_delay("30"), None); // No unit
    assert_eq!(parse_delay("s"), None); // No number
    assert_eq!(parse_delay("abc"), None); // Invalid number
    assert_eq!(parse_delay("30x"), None); // Invalid unit

    Ok(())
}
