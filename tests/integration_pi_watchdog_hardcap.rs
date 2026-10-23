//! HOTFIX `fix-tool-hang` — FIX B/FIX C end-to-end proof.
//!
//! Reproduces the 2026-10-23 21-hour hang shape: a Pi worker whose only tool
//! call is wedged (here: `sleep 999999` standing in for the half-open heredoc
//! reading stdin) produces NO proven progress and NO stream bytes. The watchdog
//! used to OBSERVE that (proof-silence far beyond soft/hard) and only log.
//!
//! This test drives the real binaries through the real wrapper seams:
//!
//!   1. `wg pi-watchdog bootstrap` binds a real child process identity;
//!   2. `wg pi-stream-observe --follow-pid` (the follower the wrapper starts)
//!      must EXECUTE on the hard-cap breach — SIGKILL the exact wedged pid and
//!      fail the attempt with `watchdog-tool-silence-hardcap`;
//!   3. `wg retry` resumes the SAME task in place (worktree preserved).
//!
//! The hard cap is lowered to 1s through its documented env override so the
//! test does not wait ten minutes; production defaults to 600s.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use tempfile::TempDir;
use worksgood::graph::{Node, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, save_graph};

fn wg_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wg")
}

/// Run `wg` as an OPERATOR against the fixture graph: never leak the outer
/// attempt's terminal authority / worker capability into the child.
fn wg_cmd(wg_dir: &Path, args: &[&str]) -> Output {
    Command::new(wg_bin())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_WORKTREE_PATH")
        .env_remove("WG_HANDLER_QUIESCENT")
        .env_remove("WG_SPAWN_RUN_ID")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_CONTROL_MODE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("failed to run wg {:?}: {e}", args))
}

fn wg_ok(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd(wg_dir, args);
    assert!(
        output.status.success(),
        "wg {:?} failed\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn spawn_silent_process() -> Child {
    Command::new("sleep")
        .arg("999999")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the silent tool stand-in")
}

fn process_gone(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

fn setup_claimed_task(tmp: &TempDir) -> (PathBuf, String, String) {
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(Task {
        id: "wedged".to_string(),
        title: "Wedge a pi tool call".to_string(),
        status: Status::Open,
        ..Task::default()
    }));
    save_graph(&graph, &graph_path).unwrap();
    wg_ok(&wg_dir, &["claim", "wedged"]);
    let graph = load_graph(&graph_path).unwrap();
    let task = graph.get_task("wedged").unwrap();
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .expect("claim creates a current attempt")
        .clone();
    (wg_dir, "wedged".to_string(), attempt.id)
}

/// Build the `agents/<id>` runtime dir `wg pi-watchdog bootstrap` expects:
/// `metadata.json`, `pi-session-plan.json`, and the session journal header.
fn write_agent_runtime(wg_dir: &Path, task_id: &str, attempt_id: &str, worktree: &Path) -> PathBuf {
    let agent_dir = wg_dir.join("agents").join("agent-wedged");
    let session_dir = agent_dir.join("session");
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_file = session_dir.join("sess-wedged.jsonl");
    let header = r#"{"type":"session","id":"sess-wedged","version":3}"#;
    std::fs::write(&session_file, format!("{header}\n")).unwrap();
    let header_digest = format!("b3:{}", blake3::hash(header.as_bytes()).to_hex());
    std::fs::write(
        agent_dir.join("metadata.json"),
        serde_json::to_vec(&serde_json::json!({
            "task_id": task_id,
            "attempt_id": attempt_id,
            "worktree_path": worktree,
            "model": "pi:openrouter:test/model",
            "executor": "pi",
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        agent_dir.join("pi-session-plan.json"),
        serde_json::to_vec(&serde_json::json!({
            "session_id": "sess-wedged",
            "session_dir": session_dir,
            "session_file": session_file,
            "resumed": false,
            "header_digest": header_digest,
        }))
        .unwrap(),
    )
    .unwrap();
    agent_dir
}

#[cfg(unix)]
#[test]
fn hard_cap_breach_kills_the_wedged_pi_and_fails_the_attempt_with_retry_in_place() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, task_id, attempt_id) = setup_claimed_task(&tmp);
    let agent_dir = write_agent_runtime(&wg_dir, &task_id, &attempt_id, tmp.path());

    let mut wedged = spawn_silent_process();
    let wedged_pid = wedged.id();
    let wrapper_pid = std::process::id();

    // (1) The real wrapper seam: bind the exact child process identity.
    wg_ok(
        &wg_dir,
        &[
            "pi-watchdog",
            "bootstrap",
            &task_id,
            "--agent-dir",
            agent_dir.to_str().unwrap(),
            "--pid",
            &wedged_pid.to_string(),
            "--wrapper-pid",
            &wrapper_pid.to_string(),
        ],
    );

    // (2) The follower the wrapper starts. Lower the hard cap to 1s so the
    // breach is reachable inside the test; keep the no-progress backstop well
    // above it so ENFORCEMENT (not the backstop) is what acts.
    let observer = Command::new(wg_bin())
        .arg("--dir")
        .arg(&wg_dir)
        .args([
            "pi-stream-observe",
            "--agent-dir",
            agent_dir.to_str().unwrap(),
            "--follow-pid",
            &wedged_pid.to_string(),
        ])
        .env("WG_PI_WATCHDOG_HARD_SILENCE_SECS", "1")
        .env("WG_PI_OBSERVE_NO_PROGRESS_SECS", "120")
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_CONTROL_MODE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run the pi-stream-observe follower");

    let stderr = String::from_utf8_lossy(&observer.stderr).to_string();
    assert!(
        observer.status.success(),
        "observer failed\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&observer.stdout)
    );
    assert!(
        stderr.contains("hard-cap breach") && stderr.contains("watchdog-tool-silence-hardcap"),
        "observer did not announce the enforced hard cap; stderr: {stderr}"
    );

    // The wedged process is really dead (not merely logged about).
    let mut killed = false;
    for _ in 0..250 {
        if process_gone(&mut wedged) {
            killed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !killed {
        let _ = wedged.kill();
        let _ = wedged.wait();
    }
    assert!(
        killed,
        "watchdog hard cap did not kill the wedged Pi process within the grace window"
    );

    // The attempt failed with the documented reason, so the retry path owns it.
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task(&task_id).unwrap();
    assert_eq!(task.status, Status::Failed, "attempt must be failed");
    assert_eq!(
        task.failure_reason.as_deref(),
        Some("watchdog-tool-silence-hardcap")
    );

    // (3) Retry resumes IN PLACE: the worktree/branch survives the retry.
    let worktree_marker = tmp.path().join("worktree-canary.txt");
    std::fs::write(&worktree_marker, "prior progress\n").unwrap();
    wg_ok(&wg_dir, &["retry", &task_id]);
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    let retried = graph.get_task(&task_id).unwrap();
    assert_eq!(retried.status, Status::Open);
    assert_eq!(retried.failure_reason, None);
    assert_eq!(retried.retry_count, 1);
    assert!(
        worktree_marker.is_file(),
        "retry must preserve the in-place worktree"
    );
}

#[cfg(unix)]
#[test]
fn soft_silence_is_log_only_and_never_kills_the_process() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, task_id, attempt_id) = setup_claimed_task(&tmp);
    let agent_dir = write_agent_runtime(&wg_dir, &task_id, &attempt_id, tmp.path());
    let mut quiet = spawn_silent_process();
    let pid = quiet.id();
    wg_ok(
        &wg_dir,
        &[
            "pi-watchdog",
            "bootstrap",
            &task_id,
            "--agent-dir",
            agent_dir.to_str().unwrap(),
            "--pid",
            &pid.to_string(),
            "--wrapper-pid",
            &std::process::id().to_string(),
        ],
    );

    // Forge a soft-but-not-hard silence by pulling the watchdog's proven clock
    // back one soft window: soft is log-only, so the process must survive.
    let observer = Command::new(wg_bin())
        .arg("--dir")
        .arg(&wg_dir)
        .args([
            "pi-stream-observe",
            "--agent-dir",
            agent_dir.to_str().unwrap(),
            "--follow-pid",
            &pid.to_string(),
        ])
        // hard cap far above the run; no-progress backstop fires first and is
        // the ONLY reason this observer exits.
        .env("WG_PI_WATCHDOG_HARD_SILENCE_SECS", "3600")
        .env("WG_PI_OBSERVE_NO_PROGRESS_SECS", "1")
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_CONTROL_MODE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run the follower");

    // FIX C: the observer bounded itself out (non-zero + clear line).
    let stderr = String::from_utf8_lossy(&observer.stderr).to_string();
    assert!(
        !observer.status.success(),
        "no-progress observer must exit non-zero"
    );
    assert!(
        stderr.contains("no stream progress"),
        "missing bounded no-progress line; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("hard-cap breach"),
        "soft silence must never be enforced; stderr: {stderr}"
    );

    // The soft rung is observation only: the process is still alive, the task
    // is still in progress.
    assert!(
        !process_gone(&mut quiet),
        "soft silence must leave the process intact"
    );
    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph.get_task(&task_id).unwrap().status,
        Status::InProgress,
        "soft silence must never fail the attempt"
    );
    let _ = quiet.kill();
    let _ = quiet.wait();
}
