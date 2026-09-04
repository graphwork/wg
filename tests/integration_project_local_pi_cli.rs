use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn project(tmp: &TempDir) -> (PathBuf, PathBuf) {
    let root = tmp.path().join("project");
    let graph = root.join(".wg");
    fs::create_dir_all(&graph).unwrap();
    worksgood::parser::save_graph(
        &worksgood::graph::WorkGraph::new(),
        &graph.join("graph.jsonl"),
    )
    .unwrap();
    (root, graph)
}

fn wg(home: &Path, graph: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wg"));
    command
        .current_dir(graph.parent().unwrap())
        .arg("--dir")
        .arg(graph)
        .args(args)
        .env("HOME", home);
    for key in [
        "WG_DIR",
        "WG_PROJECT_ROOT",
        "WG_TASK_ID",
        "WG_AGENT_ID",
        "WG_WORKER_CONTROL_MODE",
        "OPENROUTER_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ] {
        command.env_remove(key);
    }
    command.output().unwrap()
}

#[test]
fn setup_defaults_to_authoritative_project_file_without_machine_state() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let output = wg(
        &home,
        &graph,
        &[
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:openai-codex:gpt-5.6-sol",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project_config = fs::read_to_string(root.join("worksgood.toml")).unwrap();
    assert!(project_config.contains("schema_version = 1"));
    assert!(project_config.contains("pi:openai-codex:gpt-5.6-sol"));
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
    assert!(!home.join(".pi").exists());
}

#[cfg(unix)]
#[test]
fn interactive_setup_announces_project_default_before_first_prompt() {
    if Command::new("script").arg("--version").output().is_err() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let command_line = format!(
        "{} --dir {} setup",
        env!("CARGO_BIN_EXE_wg"),
        graph.display()
    );
    let mut child = Command::new("script")
        .current_dir(&root)
        .args(["-qec", &command_line, "/dev/null"])
        .env("HOME", &home)
        .env_remove("WG_DIR")
        .env_remove("WG_PROJECT_ROOT")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    child.stdin.take().unwrap().write_all(b"\x03").unwrap();
    let output = child.wait_with_output().unwrap();
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(terminal.contains("Scope: project only"), "{terminal}");
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
}

#[test]
fn profile_select_and_deprecated_use_preserve_project_guardrails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    fs::write(
        root.join("worksgood.toml"),
        "schema_version = 1\n[dispatcher]\nmax_agents = 7\n[dispatcher.resource_management]\ndisk_sentinel_enabled = false\n",
    )
    .unwrap();
    let selected = wg(&home, &graph, &["profile", "select", "pi", "--no-reload"]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let after_select = fs::read_to_string(root.join("worksgood.toml")).unwrap();
    assert!(after_select.contains("max_agents = 7"));
    assert!(after_select.contains("disk_sentinel_enabled = false"));
    assert!(after_select.contains("[profile_origin]"));
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());

    let used = wg(&home, &graph, &["profile", "use", "pi", "--no-reload"]);
    assert!(used.status.success());
    assert!(String::from_utf8_lossy(&used.stderr).contains("deprecated"));
    assert_eq!(
        after_select,
        fs::read_to_string(root.join("worksgood.toml")).unwrap()
    );
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
}

#[test]
fn every_exposed_global_route_rewrite_is_refused_before_mutation() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (_root, graph) = project(&tmp);
    for args in [
        vec!["config", "set", "agent.model", "pi:test:model", "--global"],
        vec![
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:test:model",
            "--scope",
            "global",
            "--yes",
        ],
        vec![
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:test:model",
            "--scope",
            "both",
            "--yes",
        ],
    ] {
        let output = wg(&home, &graph, &args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("WG-GLOBAL-CONFIG-WRITE-REFUSED"));
        assert!(!home.join(".wg/config.toml").exists());
        assert!(!home.join(".wg/active-profile").exists());
    }
}
