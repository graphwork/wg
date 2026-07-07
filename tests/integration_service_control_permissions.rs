//! Regression tests for role-aware service-control permissions.

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

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(wg_binary());
    cmd.arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_MODEL")
        .env_remove("WG_TIER")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_CHAT_REF")
        .env_remove("WG_CHAT_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn worker_context_cannot_restart_service() {
    let temp = TempDir::new().unwrap();
    let wg_dir = temp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    let output = wg_cmd(&wg_dir, &["service", "restart"])
        .env("WG_TASK_ID", "ordinary-worker-task")
        .env("WG_AGENT_ID", "agent-worker")
        .output()
        .expect("run wg service restart");

    assert!(
        !output.status.success(),
        "worker restart should fail\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("worker agents cannot control the WG service"),
        "stderr should explain worker service-control denial, got: {stderr}"
    );
    assert!(
        stderr.contains("Chat agents may run service-control commands when user-directed"),
        "stderr should explain chat-agent exception, got: {stderr}"
    );
}

#[test]
fn worker_context_can_read_service_status() {
    let temp = TempDir::new().unwrap();
    let wg_dir = temp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    let output = wg_cmd(&wg_dir, &["service", "status"])
        .env("WG_TASK_ID", "ordinary-worker-task")
        .env("WG_AGENT_ID", "agent-worker")
        .output()
        .expect("run wg service status");

    assert!(
        output.status.success(),
        "worker status should remain available\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Service: not running"),
        "status output should remain the normal read-only response"
    );
}

#[test]
fn human_context_can_read_service_status_unchanged() {
    let temp = TempDir::new().unwrap();
    let wg_dir = temp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    let output = wg_cmd(&wg_dir, &["service", "status"])
        .output()
        .expect("run wg service status");

    assert!(
        output.status.success(),
        "human status should remain available\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Service: not running"),
        "status output should remain unchanged for non-agent shells"
    );
}
