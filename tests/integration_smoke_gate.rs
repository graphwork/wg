//! Smoke harness and current completion-lifecycle integration tests.
//!
//! The smoke harness owns process execution and reports pass/fail/skip.  Task
//! completion is publication-derived; the retired direct-`done` smoke/bypass
//! flags are deliberately rejected.  These tests therefore exercise the real
//! harness for positive and negative gate outcomes, and separately exercise
//! the current reasoned operator-recovery completion valve through the real
//! `wg` binary.

#[path = "common/isolated_cli.rs"]
mod isolated_cli;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::TempDir;
use worksgood::graph::Status;
use worksgood::parser::load_graph;
use worksgood::smoke::{GateReport, Manifest, run_scenarios};

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("current exe path");
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

fn wg_cmd_with_env(wg_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let fixture_root = wg_dir.parent().unwrap_or(wg_dir);
    let mut cmd = isolated_cli::command(&wg_binary(), fixture_root);
    isolated_cli::assert_worker_authority_is_absent(&cmd);
    cmd.arg("--dir").arg(wg_dir).args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|error| panic!("Failed to run wg {args:?}: {error}"))
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod script");
}

fn make_pass_script(dir: &Path, name: &str) {
    write_executable(&dir.join(name), "#!/usr/bin/env bash\nexit 0\n");
}

fn make_fail_script(dir: &Path, name: &str, message: &str) {
    write_executable(
        &dir.join(name),
        &format!(
            "#!/usr/bin/env bash\necho '{}' >&2\nexit 7\n",
            message.replace('\'', "'\\''")
        ),
    );
}

fn make_skip_script(dir: &Path, name: &str, reason: &str) {
    write_executable(
        &dir.join(name),
        &format!(
            "#!/usr/bin/env bash\necho '{}' >&2\nexit 77\n",
            reason.replace('\'', "'\\''")
        ),
    );
}

fn run_manifest(path: &Path, owner: Option<&str>) -> GateReport {
    let manifest = Manifest::load_from(path).expect("load synthetic manifest");
    let scenarios: Vec<_> = match owner {
        Some(task_id) => manifest.scenarios_for_task(task_id),
        None => manifest.scenarios.iter().collect(),
    };
    run_scenarios(&scenarios, path.parent().unwrap())
}

fn init_with_task(tmp: &Path, task_id: &str) -> PathBuf {
    let wg_dir = tmp.join(".wg");
    let init = wg_cmd_with_env(&wg_dir, &["init", "--route", "pi"], &[]);
    assert!(
        init.status.success(),
        "wg init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::write(
        wg_dir.join("config.toml"),
        "[agency]\nauto_assign = false\nauto_evaluate = false\nflip_enabled = false\n",
    )
    .unwrap();
    let add = wg_cmd_with_env(&wg_dir, &["add", "Task under test", "--id", task_id], &[]);
    assert!(
        add.status.success(),
        "wg add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let publish = wg_cmd_with_env(&wg_dir, &["publish", task_id, "--only"], &[]);
    assert!(
        publish.status.success(),
        "wg publish failed: {}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let claim = wg_cmd_with_env(&wg_dir, &["claim", task_id], &[]);
    assert!(
        claim.status.success(),
        "wg claim failed: {}",
        String::from_utf8_lossy(&claim.stderr)
    );
    wg_dir
}

#[test]
fn test_smoke_harness_blocks_when_owned_scenario_fails() {
    let tmp = TempDir::new().unwrap();
    make_fail_script(tmp.path(), "always_fails.sh", "intentional smoke failure");
    let manifest_path = tmp.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
[[scenario]]
name = "always_fails"
script = "always_fails.sh"
owners = ["fence-task"]
timeout_seconds = 10
"#,
    )
    .unwrap();

    let report = run_manifest(&manifest_path, Some("fence-task"));
    assert!(
        report.blocks_done(),
        "a failing scenario must block the gate"
    );
    assert_eq!(report.failures().len(), 1);
    let rendered = report.render();
    assert!(rendered.contains("always_fails"));
    assert!(rendered.contains("intentional smoke failure"));
}

#[test]
fn test_smoke_harness_accepts_pass_and_loud_skip_for_owner_only() {
    let tmp = TempDir::new().unwrap();
    make_pass_script(tmp.path(), "ok.sh");
    make_skip_script(tmp.path(), "skipme.sh", "endpoint unreachable");
    make_fail_script(tmp.path(), "other_fail.sh", "must not run");
    let manifest_path = tmp.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
[[scenario]]
name = "happy_pass"
script = "ok.sh"
owners = ["happy-task"]

[[scenario]]
name = "happy_skip"
script = "skipme.sh"
owners = ["happy-task"]

[[scenario]]
name = "other_owner_fail"
script = "other_fail.sh"
owners = ["some-other-task"]
"#,
    )
    .unwrap();

    let report = run_manifest(&manifest_path, Some("happy-task"));
    assert!(!report.blocks_done(), "pass plus loud skip must not block");
    assert_eq!(report.passes().len(), 1);
    assert_eq!(report.skips().len(), 1);
    assert!(!report.render().contains("other_owner_fail"));
}

#[test]
fn test_full_smoke_harness_surfaces_foreign_owned_failure() {
    let tmp = TempDir::new().unwrap();
    make_pass_script(tmp.path(), "ok.sh");
    make_fail_script(tmp.path(), "other_fail.sh", "from elsewhere");
    let manifest_path = tmp.path().join("manifest.toml");
    fs::write(
        &manifest_path,
        r#"
[[scenario]]
name = "owned_pass"
script = "ok.sh"
owners = ["all-or-nothing"]

[[scenario]]
name = "foreign_fail"
script = "other_fail.sh"
owners = ["unrelated-task"]
"#,
    )
    .unwrap();

    let owned = run_manifest(&manifest_path, Some("all-or-nothing"));
    assert!(!owned.blocks_done());
    let full = run_manifest(&manifest_path, None);
    assert!(full.blocks_done());
    assert!(full.render().contains("foreign_fail"));
}

#[test]
fn test_legacy_smoke_bypass_flags_are_rejected_for_agents_and_humans() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_with_task(tmp.path(), "legacy-bypass");

    for env in [&[][..], &[("WG_AGENT_ID", "test-agent")][..]] {
        let output = wg_cmd_with_env(&wg_dir, &["done", "legacy-bypass", "--skip-smoke"], env);
        assert!(
            !output.status.success(),
            "retired bypass must never succeed"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("legacy wg done bypass/merge/cycle flags are not supported"),
            "retired flag rejection should be explicit: {stderr}"
        );
    }

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph.get_task("legacy-bypass").unwrap().status,
        Status::InProgress
    );
}

#[test]
fn test_smoke_current_operator_acceptance_refuses_worker_then_allows_human() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_with_task(tmp.path(), "operator-recovery");
    let args = [
        "done",
        "operator-recovery",
        "--operator-accept",
        "--reason",
        "integration fixture recovery",
    ];

    let worker = wg_cmd_with_env(&wg_dir, &args, &[("WG_AGENT_ID", "test-agent")]);
    assert!(
        !worker.status.success(),
        "worker must not acquire operator authority"
    );
    let worker_error = String::from_utf8_lossy(&worker.stderr);
    assert!(
        worker_error.contains("operator acceptance is refused inside a worker process"),
        "worker rejection should remain explicit: {worker_error}"
    );
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph.get_task("operator-recovery").unwrap().status,
        Status::InProgress
    );

    let human = wg_cmd_with_env(&wg_dir, &args, &[]);
    assert!(
        human.status.success(),
        "reasoned human recovery should succeed: {}",
        String::from_utf8_lossy(&human.stderr)
    );
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph.get_task("operator-recovery").unwrap().status,
        Status::Done
    );
}

#[test]
fn smoke_helpers_refuse_direct_execution_without_subreaper_harness() {
    let tmp = TempDir::new().unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/smoke/scenarios/_helpers.sh");
    let mut command = isolated_cli::command(Path::new("bash"), tmp.path());
    let output = command
        .args(["-c", ". \"$1\"", "_"])
        .arg(&helper)
        .env("WG_SMOKE_ROOT", tmp.path().join("smoke-root"))
        .env_remove("WG_SMOKE_HARNESS_RUN_ID")
        .env_remove("WG_SMOKE_SUBREAPER_TOKEN")
        .output()
        .expect("source helper directly");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("direct/unsupported execution is refused")
    );
    assert!(
        !tmp.path().join("smoke-root").exists(),
        "direct refusal must happen before fixture initialization"
    );
}

#[test]
fn smoke_process_ownership_cleanup_real_entry_point() {
    let tmp = TempDir::new().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/smoke/scenarios/smoke_process_ownership_cleanup.sh");
    let wrapper = tmp.path().join("ownership-wrapper.sh");
    write_executable(
        &wrapper,
        &format!(
            "#!/usr/bin/env bash\nexport WG_BIN='{}'\nexport WG_SMOKE_ROOT='{}'\nexec bash '{}'\n",
            wg_binary().display(),
            tmp.path().join("smoke-root").display(),
            script.display()
        ),
    );
    let scenario = worksgood::smoke::Scenario {
        name: "smoke-process-ownership-cleanup-real-entry-point".to_string(),
        script: wrapper.to_string_lossy().to_string(),
        owners: vec!["fix-smoke-pi-process-leaks".to_string()],
        description: "exercise process ownership through the real Rust harness".to_string(),
        timeout_seconds: Some(180),
    };
    let report = run_scenarios(&[&scenario], tmp.path());
    assert!(
        matches!(
            report.results.as_slice(),
            [worksgood::smoke::ScenarioResult {
                outcome: worksgood::smoke::ScenarioOutcome::Pass,
                ..
            }]
        ),
        "ownership scenario failed through the real harness: {report:?}"
    );
}

#[test]
fn test_missing_smoke_manifest_is_an_empty_nonblocking_gate() {
    let tmp = TempDir::new().unwrap();
    let report = run_manifest(&tmp.path().join("no-such-manifest.toml"), Some("nobody"));
    assert!(report.results.is_empty());
    assert!(!report.blocks_done());
}
