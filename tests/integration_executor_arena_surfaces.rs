use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const CHOICES: &[&str] = &[
    "native",
    "claude",
    "codex",
    "shell",
    "opencode",
    "aider",
    "goose",
    "qwen",
    "cline",
    "gemini",
    "crush",
    "amplifier",
];

fn wg_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_wg") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/wg")
}

fn wg_cmd(wg_dir: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env_remove("WG_DIR")
        .env_remove("WG_PROJECT_ROOT")
        .env_remove("WG_WORKTREE_PATH")
        .env_remove("WG_EXECUTOR_TYPE")
        .env_remove("WG_MODEL")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .output()
        .expect("spawn wg")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_choices(surface: &str, output: &str) {
    for choice in CHOICES {
        assert!(
            output.contains(choice),
            "{surface} should mention executor choice `{choice}`; output:\n{output}"
        );
    }
}

#[test]
fn config_list_and_discovery_surface_executor_arena_choices() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config")).unwrap();
    let wg_dir = tmp.path().join(".wg");

    let init = wg_cmd(
        &wg_dir,
        &home,
        &["init", "-m", "claude:opus", "--no-agency"],
    );
    assert!(
        init.status.success(),
        "wg init failed\nstdout:\n{}\nstderr:\n{}",
        stdout(&init),
        stderr(&init)
    );

    let discovery = wg_cmd(&wg_dir, &home, &["executors", "--all"]);
    assert!(
        discovery.status.success(),
        "wg executors --all failed\nstdout:\n{}\nstderr:\n{}",
        stdout(&discovery),
        stderr(&discovery)
    );
    assert_choices("wg executors --all", &stdout(&discovery));

    let config_show = wg_cmd(&wg_dir, &home, &["config", "--show"]);
    assert!(
        config_show.status.success(),
        "wg config --show failed\nstdout:\n{}\nstderr:\n{}",
        stdout(&config_show),
        stderr(&config_show)
    );
    let show = stdout(&config_show);
    assert!(show.contains("[executor choices]"), "config show:\n{show}");
    assert!(show.contains("stable_external"), "config show:\n{show}");
    assert!(show.contains("provider_specific"), "config show:\n{show}");
    assert!(
        show.contains("experimental_external"),
        "config show:\n{show}"
    );
    assert_choices("wg config --show", &show);

    let config_list = wg_cmd(&wg_dir, &home, &["config", "--list"]);
    assert!(
        config_list.status.success(),
        "wg config --list failed\nstdout:\n{}\nstderr:\n{}",
        stdout(&config_list),
        stderr(&config_list)
    );
    let list = stdout(&config_list);
    assert!(list.contains("[executor choices]"), "config list:\n{list}");
    assert_choices("wg config --list", &list);
}
