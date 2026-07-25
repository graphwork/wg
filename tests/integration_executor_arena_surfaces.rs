use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

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
        .env_remove("WG_DIR")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .output()
        .expect("spawn wg")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn normal_help_and_config_hide_retired_executor_arena_choices() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let wg_dir = tmp.path().join(".wg");

    let init = wg_cmd(&wg_dir, &home, &["init", "--route", "pi", "--no-agency"]);
    assert!(init.status.success(), "{}", text(&init.stderr));

    let help = wg_cmd(&wg_dir, &home, &["--help"]);
    let help = text(&help.stdout);
    assert!(
        !help.contains("executors"),
        "hidden expert command leaked: {help}"
    );

    let config = wg_cmd(&wg_dir, &home, &["config", "--show"]);
    assert!(config.status.success(), "{}", text(&config.stderr));
    let config = text(&config.stdout);
    assert!(config.contains("owner = \"Pi\""));
    assert!(!config.contains("[executor choices]"));
    for retired in ["claude", "codex", "native", "opencode", "aider", "nex"] {
        assert!(
            !config.contains(&format!(" = \"{retired}\"")),
            "retired choice leaked: {retired}\n{config}"
        );
    }
}
