//! The attended concierge lifecycle must read as the operator's own tool, not
//! a research artifact. Before the role plan is printed, `run_lifecycle`
//! prints a plain operator-framing block (no emoji, no internal codenames):
//! you are the operator; what each tier drives; routes are re-changeable; Pi
//! owns authentication and model selection.
//!
//! This drives the real `worksgood` binary through the attended-flow fixture
//! convention used by the concierge smoke scenarios (a fake `pi` on PATH, an
//! isolated `HOME`, `--dry-run` so nothing is written) and asserts the block
//! prints when the lifecycle reaches the plan stage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fake_pi_bin(scratch: &Path) -> PathBuf {
    let bin = scratch.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pi = bin.join("pi");
    std::fs::write(
        &pi,
        "#!/bin/sh\n# Discovery/readiness fixture. No model call.\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

#[test]
fn attended_lifecycle_prints_operator_framing_before_the_plan() {
    let scratch = tempfile::TempDir::new().unwrap();
    let project = scratch.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let git_ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !git_ok {
        // The framing assert is about the lifecycle print path, not git.
        std::fs::create_dir_all(project.join(".git")).unwrap();
    }
    let home = scratch.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let fake_bin = fake_pi_bin(scratch.path());

    let path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_worksgood"))
        .arg("--project")
        .arg(&project)
        .args([
            "setup",
            "--model",
            "pi:openrouter:deepseek/deepseek-v4-flash",
            "--dry-run",
        ])
        .env("HOME", &home)
        .env("WG_GLOBAL_DIR", home.join(".wg"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("PATH", format!("{}:{path}", fake_bin.display()))
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_AGENT_ROLE")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_MODEL")
        .env_remove("WG_TIER")
        .output()
        .expect("spawn worksgood setup --dry-run");
    assert!(
        output.status.success(),
        "dry-run setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The framing block prints, in full, before the role plan.
    let plan_at = stdout
        .find("Immutable redacted plan:")
        .expect("dry-run output should reach the plan stage");
    for expected in [
        "You are the operator: this system is yours, not a research artifact.",
        "The graph is your durable record of the work done here.",
        "strong tier runs your workers and heavy generative roles",
        "weak tier runs the cheap, recoverable one-shots",
        "Pointing both tiers at the same model is a valid choice.",
        "Routes are choices, not commitments",
        "Pi owns authentication and model selection",
    ] {
        let at = stdout.find(expected).unwrap_or_else(|| {
            panic!("framing line missing from attended setup output: {expected}")
        });
        assert!(
            at < plan_at,
            "framing line printed after the plan: {expected}"
        );
    }
    // Plain style: no emoji in the framing block.
    let framing = &stdout[..plan_at];
    assert!(
        !framing.chars().any(|c| c as u32 >= 0x1F300),
        "emoji found in framing block: {framing}"
    );
}
