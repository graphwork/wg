use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;
use workgraph::config::{Config, DispatchRole};
use workgraph::nex_runtime::{
    NexRuntimeResolveInput, load_config, resolve_standalone, resolve_wg_autonomous,
};

fn wg_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_wg") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/wg")
}

fn nex_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_nex") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/nex")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("path has parent")).expect("create parent dir");
    fs::write(path, body).expect("write fixture");
}

fn write_model_config(path: &Path, model_id: &str) {
    write(
        path,
        &format!(
            r#"
[models.task_agent]
model = "nex:{model_id}"
"#
        ),
    );
}

fn write_fake_llm(path: &Path, marker: &str) {
    write(path, &format!("{marker}\n"));
}

fn isolated_command(binary: PathBuf, home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.env_clear();
    cmd.env("HOME", home);
    cmd.env("USER", "wg-test");
    cmd.env("TERM", "xterm-256color");
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.current_dir(cwd);
    cmd
}

fn list_journals(root: &Path, child: &str) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let base = root.join(child);
    let Ok(entries) = fs::read_dir(&base) else {
        return out;
    };
    for entry in entries.flatten() {
        let journal = entry.path().join("conversation.jsonl");
        if journal.is_file() {
            out.insert(journal);
        }
    }
    out
}

fn standalone_journals(state_root: &Path) -> BTreeSet<PathBuf> {
    list_journals(state_root, "sessions")
}

fn wg_chat_journals(wg_dir: &Path) -> BTreeSet<PathBuf> {
    list_journals(wg_dir, "chat")
}

fn journal_init_model(journal: &Path) -> String {
    let text = fs::read_to_string(journal).expect("read journal");
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).expect("journal line is json");
        if value.get("entry_type").and_then(Value::as_str) == Some("init") {
            return value
                .get("model")
                .and_then(Value::as_str)
                .expect("init has model")
                .to_string();
        }
    }
    panic!("journal has no init entry: {}", journal.display());
}

fn journal_contains(journal: &Path, needle: &str) -> bool {
    fs::read_to_string(journal)
        .expect("read journal")
        .contains(needle)
}

fn base_nex_args(prompt: &str) -> Vec<OsString> {
    vec![
        "--autonomous".into(),
        "--no-mcp".into(),
        "--minimal-tools".into(),
        "--max-turns".into(),
        "4".into(),
        prompt.into(),
    ]
}

fn run_standalone_nex(
    home: &Path,
    cwd: &Path,
    state_root: &Path,
    fake_llm: &Path,
    args: Vec<OsString>,
    envs: &[(&str, OsString)],
) -> (PathBuf, String) {
    let before = standalone_journals(state_root);
    let mut cmd = isolated_command(nex_binary(), home, cwd);
    cmd.env("WG_FAKE_LLM", fake_llm);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.args(args);

    let output = cmd.output().expect("spawn standalone nex");
    let text = output_text(&output);
    assert!(output.status.success(), "nex failed:\n{text}");

    let after = standalone_journals(state_root);
    let new: Vec<_> = after.difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "expected one new standalone journal under {}, before={before:?}, after={after:?}, output:\n{text}",
        state_root.display()
    );
    (new[0].clone(), text)
}

fn run_wg_nex_autonomous(
    home: &Path,
    cwd: &Path,
    wg_dir: &Path,
    fake_llm: &Path,
    prompt: &str,
) -> (PathBuf, String) {
    let before = wg_chat_journals(wg_dir);
    let mut cmd = isolated_command(wg_binary(), home, cwd);
    cmd.env("WG_FAKE_LLM", fake_llm);
    cmd.args([
        "nex",
        "--autonomous",
        "--no-mcp",
        "--minimal-tools",
        "--max-turns",
        "4",
        prompt,
    ]);

    let output = cmd.output().expect("spawn wg nex");
    let text = output_text(&output);
    assert!(output.status.success(), "wg nex failed:\n{text}");

    let after = wg_chat_journals(wg_dir);
    let new: Vec<_> = after.difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "expected one new wg chat journal under {}, before={before:?}, after={after:?}, output:\n{text}",
        wg_dir.display()
    );
    (new[0].clone(), text)
}

#[test]
fn standalone_nex_help_exposes_shared_options() {
    let output = Command::new(nex_binary())
        .arg("--help")
        .output()
        .expect("spawn nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: nex"),
        "standalone help should render as nex, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
    ] {
        assert!(
            text.contains(flag),
            "standalone nex help missing {flag}:\n{text}"
        );
    }
}

#[test]
fn wg_nex_help_keeps_compatibility_options() {
    let output = Command::new(wg_binary())
        .args(["nex", "--help"])
        .output()
        .expect("spawn wg nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "wg nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: wg nex") || text.contains("Usage: wg [OPTIONS] nex"),
        "wg nex help should render as a wg subcommand, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
    ] {
        assert!(text.contains(flag), "wg nex help missing {flag}:\n{text}");
    }
}

#[test]
fn standalone_nex_binary_starts_fresh_project_sessions_with_isolated_home_and_cwd() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("src").join("deep");
    let project_nex = project.join(".nex");
    fs::create_dir_all(&project_nex).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(home.join(".nex")).unwrap();
    let fake = tmp.path().join("fake-llm.txt");
    write_fake_llm(&fake, "FRESH_SESSION_MARKER");

    let (first, first_output) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("first fresh session");
            args.splice(0..0, ["--model".into(), "fresh-model".into()]);
            args
        },
        &[],
    );
    assert!(
        journal_contains(&first, "FRESH_SESSION_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{first_output}"
    );

    let (second, second_output) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("second fresh session");
            args.splice(0..0, ["--model".into(), "fresh-model".into()]);
            args
        },
        &[],
    );
    assert!(
        journal_contains(&second, "FRESH_SESSION_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{second_output}"
    );

    assert_ne!(first, second, "bare standalone nex must not auto-resume");
    assert_eq!(standalone_journals(&project_nex).len(), 2);
    assert!(
        standalone_journals(&home.join(".nex")).is_empty(),
        "project .nex should own state when discovered from cwd"
    );
    assert_eq!(journal_init_model(&first), "fresh-model");
    assert_eq!(journal_init_model(&second), "fresh-model");
    assert!(journal_contains(&first, "first fresh session"));
    assert!(journal_contains(&second, "second fresh session"));
}

#[test]
fn standalone_nex_binary_respects_cli_env_project_user_legacy_default_precedence() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    let project_nex = project.join(".nex");
    let env_nex = tmp.path().join("env-nex");
    let fake = tmp.path().join("fake-llm.txt");
    write_fake_llm(&fake, "PRECEDENCE_MARKER");

    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&project_nex).unwrap();
    fs::create_dir_all(&env_nex).unwrap();
    write_model_config(&project_nex.join("config.toml"), "project-model");
    write_model_config(&env_nex.join("config.toml"), "env-dir-model");
    write_model_config(&home.join(".nex").join("config.toml"), "user-model");
    write_model_config(&home.join(".wg").join("config.toml"), "legacy-home-model");
    write_model_config(
        &project.join(".wg").join("config.toml"),
        "legacy-project-model",
    );

    let env_config = tmp.path().join("env-config.toml");
    let cli_config = tmp.path().join("cli-config.toml");
    write_model_config(&env_config, "env-config-model");
    write_model_config(&cli_config, "cli-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("cli model wins");
            args.splice(0..0, ["--model".into(), "cli-model".into()]);
            args
        },
        &[
            ("NEX_MODEL", "env-model".into()),
            ("NEX_CONFIG", env_config.as_os_str().to_os_string()),
        ],
    );
    assert_eq!(journal_init_model(&journal), "cli-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("cli config wins");
            args.splice(
                0..0,
                ["--config".into(), cli_config.as_os_str().to_os_string()],
            );
            args
        },
        &[("NEX_CONFIG", env_config.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "cli-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("nex model wins"),
        &[
            ("NEX_MODEL", "env-model".into()),
            ("NEX_CONFIG", env_config.as_os_str().to_os_string()),
        ],
    );
    assert_eq!(journal_init_model(&journal), "env-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &env_nex,
        &fake,
        base_nex_args("nex dir wins"),
        &[("NEX_DIR", env_nex.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "env-dir-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("nex config wins"),
        &[("NEX_CONFIG", env_config.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "env-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("project config wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "project-model");

    fs::remove_file(project_nex.join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("user config wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "user-model");

    fs::remove_file(home.join(".nex").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("legacy project wg fallback wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "legacy-project-model");

    fs::remove_file(project.join(".wg").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("legacy home wg fallback wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "legacy-home-model");

    fs::remove_file(home.join(".wg").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("default model wins"),
        &[],
    );
    let default_model = Config::default()
        .resolve_model_for_role(DispatchRole::TaskAgent)
        .model;
    assert_eq!(journal_init_model(&journal), default_model);
}

#[test]
fn standalone_runtime_merges_endpoints_and_model_registry_entries_by_identity() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    fs::create_dir_all(project.join(".nex")).unwrap();
    fs::create_dir_all(project.join(".wg")).unwrap();
    fs::create_dir_all(&nested).unwrap();

    write(
        &home.join(".wg").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://legacy.invalid/v1"

[[llm_endpoints.endpoints]]
name = "legacy-only"
provider = "openai"
url = "https://legacy-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "legacy-wire"
tier = "standard"

[[model_registry]]
id = "legacy-model"
provider = "openai"
model = "legacy-only-wire"
tier = "standard"
"#,
    );
    write(
        &home.join(".nex").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://user.invalid/v1"

[[llm_endpoints.endpoints]]
name = "user-only"
provider = "openai"
url = "https://user-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "user-wire"
tier = "standard"

[[model_registry]]
id = "user-model"
provider = "openai"
model = "user-only-wire"
tier = "standard"
"#,
    );
    write(
        &project.join(".nex").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://project.invalid/v1"

[[llm_endpoints.endpoints]]
name = "project-only"
provider = "openai"
url = "https://project-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "project-wire"
tier = "standard"

[[model_registry]]
id = "project-model"
provider = "openai"
model = "project-only-wire"
tier = "standard"
"#,
    );

    let runtime = resolve_standalone(&NexRuntimeResolveInput {
        cwd: Some(nested),
        home_dir: Some(home),
        ..Default::default()
    });
    let config = load_config(&runtime).unwrap();

    assert_eq!(
        config
            .llm_endpoints
            .find_by_name("shared")
            .and_then(|ep| ep.url.as_deref()),
        Some("https://project.invalid/v1")
    );
    for name in ["legacy-only", "user-only", "project-only"] {
        assert!(
            config.llm_endpoints.find_by_name(name).is_some(),
            "endpoint {name} should be preserved"
        );
    }
    assert_eq!(
        config
            .llm_endpoints
            .endpoints
            .iter()
            .filter(|ep| ep.name == "shared")
            .count(),
        1,
        "same endpoint name should merge instead of duplicating"
    );

    assert_eq!(
        config
            .registry_lookup("shared-model")
            .map(|entry| entry.model),
        Some("project-wire".to_string())
    );
    for id in ["legacy-model", "user-model", "project-model"] {
        assert!(
            config.registry_lookup(id).is_some(),
            "model registry id {id} should be preserved"
        );
    }
}

#[test]
fn wg_nex_autonomous_binary_ignores_human_standalone_nex_routing_state() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let wg_dir = project.join(".wg");
    let fake = tmp.path().join("fake-llm.txt");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&wg_dir).unwrap();
    write_fake_llm(&fake, "WG_AUTONOMOUS_MARKER");
    write_model_config(
        &home.join(".nex").join("config.toml"),
        "human-standalone-model",
    );
    write(
        &home.join(".nex").join("config.toml"),
        r#"
[models.task_agent]
model = "nex:human-standalone-model"

[[llm_endpoints.endpoints]]
name = "human"
provider = "openai"
url = "https://human.invalid/v1"
is_default = true
"#,
    );
    write_model_config(&wg_dir.join("config.toml"), "wg-autonomous-model");

    let runtime = resolve_wg_autonomous(&wg_dir, Some(home.clone()));
    assert!(
        runtime
            .config_paths
            .iter()
            .all(|path| !path.starts_with(home.join(".nex"))),
        "autonomous wg runtime must not include human ~/.nex config paths: {:?}",
        runtime.config_paths
    );
    let config = load_config(&runtime).unwrap();
    assert_eq!(
        config.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "wg-autonomous-model"
    );
    assert!(config.llm_endpoints.find_by_name("human").is_none());

    let (journal, output) =
        run_wg_nex_autonomous(&home, &project, &wg_dir, &fake, "wg autonomous isolation");
    assert!(
        journal_contains(&journal, "WG_AUTONOMOUS_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{output}"
    );
    assert_eq!(journal_init_model(&journal), "wg-autonomous-model");
    assert!(
        standalone_journals(&home.join(".nex")).is_empty(),
        "wg nex autonomous should not create or reuse standalone ~/.nex sessions"
    );
}
