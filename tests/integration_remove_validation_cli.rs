//! Tests for removal of the policy-style `--validation` CLI flag.
//!
//! Human criteria remain in `## Validation`. The distinct, exact
//! `--validation-command` setting configures host-run deterministic evidence
//! and must remain public without reviving the retired policy surface.

use std::process::Command;

fn wg_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // /deps
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("wg");
    p
}

fn wg_cmd(bin: &std::path::Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_MODEL")
        .env_remove("WG_TIER");
    cmd
}

/// `wg add --help` must not advertise the retired policy flag
/// `--validation`; the exact `--validation-command` execution setting is a
/// distinct deterministic evidence surface.
#[test]
fn test_cli_add_no_validation_flag() {
    let bin = wg_bin();
    if !bin.exists() {
        eprintln!("wg binary not built; skipping test");
        return;
    }
    let out = wg_cmd(&bin)
        .args(["add", "--help"])
        .output()
        .expect("run wg add --help");
    let help =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);

    assert!(
        !help.contains("--validation ") && !help.contains("--validation="),
        "wg add --help must not mention retired --validation policy flag, got:\n{}",
        help
    );
    assert!(
        help.contains("--validation-command"),
        "wg add --help must expose deterministic validation command configuration, got:\n{}",
        help
    );
    assert!(
        !help.contains("--validator-agent"),
        "wg add --help must not mention --validator-agent flag"
    );
    assert!(
        !help.contains("--validator-model"),
        "wg add --help must not mention --validator-model flag"
    );
}

/// `wg edit --help` must not advertise retired `--validation`, while the
/// deterministic command setting remains editable.
#[test]
fn test_cli_edit_no_validation_flag() {
    let bin = wg_bin();
    if !bin.exists() {
        eprintln!("wg binary not built; skipping test");
        return;
    }
    let out = wg_cmd(&bin)
        .args(["edit", "--help"])
        .output()
        .expect("run wg edit --help");
    let help =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);

    assert!(
        !help.contains("--validation ") && !help.contains("--validation="),
        "wg edit --help must not mention retired --validation policy flag"
    );
    assert!(help.contains("--validation-command"), "{help}");
}

/// Quickstart output must mention the `## Validation` section convention but
/// must NOT advertise the `--validation` CLI flag.
#[test]
fn test_quickstart_no_validation_flag() {
    let bin = wg_bin();
    if !bin.exists() {
        eprintln!("wg binary not built; skipping test");
        return;
    }
    let out = wg_cmd(&bin)
        .arg("quickstart")
        .output()
        .expect("run wg quickstart");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);

    assert!(
        !text.contains("--validation"),
        "wg quickstart must not advertise --validation flag, got:\n{}",
        text
    );
    // Still mentions the markdown section by name
    assert!(
        text.contains("## Validation") || text.contains("Validation section"),
        "wg quickstart must still mention the ## Validation section convention; got:\n{}",
        text
    );
}

/// Prompts assembled for spawned agents must not contain the `--validation`
/// flag string. Validation criteria flow through the `## Validation` section
/// of task descriptions, read by the agency evaluator.
#[test]
fn test_executor_prompt_no_validation_flag() {
    let guide = worksgood::service::executor::DEFAULT_WG_GUIDE;
    assert!(
        !guide.contains("--validation"),
        "DEFAULT_WG_GUIDE must not contain --validation flag, got:\n{}",
        guide
    );

    let guidance =
        worksgood::service::executor::build_decomposition_guidance("multi-step task", "task-1", 10);
    assert!(
        !guidance.contains("--validation"),
        "build_decomposition_guidance output must not contain --validation flag, got:\n{}",
        guidance
    );
}

#[test]
fn test_validation_command_round_trips_and_can_be_cleared() {
    let bin = wg_bin();
    if !bin.exists() {
        eprintln!("wg binary not built; skipping test");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let init = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["init", "--executor", "shell"])
        .output()
        .expect("wg init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let add = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args([
            "add",
            "deterministic evidence",
            "--id",
            "deterministic-evidence",
            "--validation-command",
            "printf exact",
        ])
        .output()
        .expect("wg add --validation-command");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let show = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["--json", "show", "deterministic-evidence"])
        .output()
        .expect("wg show");
    let value: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(
        value["validation_commands"],
        serde_json::json!(["printf exact"])
    );

    let clear = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["edit", "deterministic-evidence", "--validation-command", ""])
        .output()
        .expect("wg edit --validation-command clear");
    assert!(
        clear.status.success(),
        "{}",
        String::from_utf8_lossy(&clear.stderr)
    );
    let show = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["--json", "show", "deterministic-evidence"])
        .output()
        .expect("wg show after clear");
    let value: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert!(
        value.get("validation_commands").is_none()
            || value["validation_commands"]
                .as_array()
                .is_some_and(Vec::is_empty)
    );
}

/// `wg add 'test' --validation=llm` either errors with unknown-flag OR is
/// accepted as a no-op with a deprecation warning. Either is acceptable
/// per the task spec.
#[test]
fn test_cli_add_validation_flag_is_noop_or_unknown() {
    let bin = wg_bin();
    if !bin.exists() {
        eprintln!("wg binary not built; skipping test");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    // Initialize WG so flag-acceptance path can succeed without
    // hitting the "WG not initialized" gate.
    let init = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["init", "--executor", "shell"])
        .output()
        .expect("wg init");
    assert!(
        init.status.success(),
        "wg init failed: stderr={}",
        String::from_utf8_lossy(&init.stderr)
    );

    let out = wg_cmd(&bin)
        .current_dir(tmp.path())
        .args(["add", "smoke-test", "--validation=llm"])
        .output()
        .expect("run wg add --validation=llm");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{}{}", stdout, stderr);

    let unknown_flag =
        combined.contains("unexpected argument") || combined.contains("unrecognized");
    let deprecation_warning =
        combined.to_lowercase().contains("deprecated") || combined.contains("ignored");

    assert!(
        unknown_flag || deprecation_warning,
        "expected either unknown-flag error or deprecation warning, got:\nstdout={}\nstderr={}",
        stdout,
        stderr
    );
}
