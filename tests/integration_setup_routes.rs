//! Integration tests for the 5-route `wg setup` / `wg init` flow and the
//! `wg config reset` command. Validation criteria from
//! `wg-setup-5-smooth-2`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use worksgood::config::{Config, DispatchRole};
use worksgood::config_defaults::{RouteParams, SetupRoute, config_for_route};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn run_wg_in_isolation(fake_home: &Path, args: &[&str]) -> std::process::Output {
    run_wg_in_isolation_with_env(fake_home, args, &[])
}

fn run_wg_in_isolation_with_env(
    fake_home: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    cmd.args(args);
    if let Some(index) = args.iter().position(|arg| *arg == "--dir")
        && let Some(dir) = args.get(index + 1)
        && let Some(root) = Path::new(dir).parent()
    {
        cmd.current_dir(root);
    }
    cmd.env("HOME", fake_home);
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENROUTER_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("WG_DIR");
    cmd.env_remove("WG_PROJECT_ROOT");
    cmd.env_remove("WG_TASK_ID");
    cmd.env_remove("WG_AGENT_ID");
    cmd.env_remove("WG_WORKER_CAPABILITY");
    cmd.env_remove("WG_WORKER_CONTROL_PROTOCOL");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.output()
        .unwrap_or_else(|e| panic!("Failed to run wg: {}", e))
}

fn load_local_config(project_root: &Path) -> Config {
    let path = project_root.join("worksgood.toml");
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read project config at {:?}: {}", path, e));
    toml::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse worksgood.toml:\n{}\nError: {}", content, e))
}

fn assert_legacy_setup_route_rejected(route: &str, extra_args: &[&str]) {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&graph).unwrap();
    worksgood::parser::save_graph(
        &worksgood::graph::WorkGraph::new(),
        &graph.join("graph.jsonl"),
    )
    .unwrap();

    let mut args = vec!["--dir", graph.to_str().unwrap(), "setup", "--route", route];
    args.extend_from_slice(extra_args);
    args.push("--yes");
    let output = run_wg_in_isolation(&fake_home, &args);
    assert!(
        !output.status.success(),
        "legacy route unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("supported route is: pi"),
        "unexpected diagnostic for {route}: {stderr}"
    );
    assert!(!project.join("worksgood.toml").exists());
    assert!(!fake_home.join(".wg/config.toml").exists());
}

fn install_fake_pi(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = root.join("fake-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let pi = bin_dir.join("pi");
    fs::write(&pi, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&pi).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&pi, permissions).unwrap();
    bin_dir
}

// ---------------------------------------------------------------------------
// Per-route config completeness — pure-Rust tests of config_for_route.
// (Same names as the validation checklist — also covered in lib unit tests.)
// ---------------------------------------------------------------------------

#[test]
fn test_route_openrouter_complete_config() {
    let cfg = config_for_route(SetupRoute::Openrouter, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert_eq!(cfg.agent.executor, "native");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(
        cfg.tiers.standard.as_deref(),
        Some("openrouter:anthropic/claude-sonnet-4-6")
    );
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "anthropic/claude-opus-4-7"
    );
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "openrouter");
    // Round-trip
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_claude_cli_complete_config() {
    let cfg = config_for_route(SetupRoute::ClaudeCli, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("claude"));
    assert_eq!(cfg.agent.executor, "claude");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.tiers.standard.as_deref(), Some("claude:opus"));
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "opus"
    );
    // Claude CLI doesn't need an endpoint.
    assert!(cfg.llm_endpoints.endpoints.is_empty());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_codex_cli_complete_config() {
    let cfg = config_for_route(SetupRoute::CodexCli, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("codex"));
    assert_eq!(cfg.agent.executor, "codex");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_local_complete_config() {
    let cfg = config_for_route(
        SetupRoute::Local,
        RouteParams {
            url: Some("http://localhost:11434/v1".to_string()),
            model: Some("qwen3:4b".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "local");
    assert!(cfg.llm_endpoints.endpoints[0].api_key_env.is_none());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_nex_custom_complete_config() {
    let cfg = config_for_route(
        SetupRoute::NexCustom,
        RouteParams {
            url: Some("https://example.com/v1".to_string()),
            api_key_env: Some("MY_KEY".to_string()),
            model: Some("foo".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "oai-compat");
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

// ---------------------------------------------------------------------------
// CLI flow: wg setup --route <name> --yes writes complete configs.
// ---------------------------------------------------------------------------

#[test]
fn test_setup_non_interactive_route_activates_profile_and_exact_model() {
    // Pre-change regression: setup wrote this config but left active-profile
    // absent, making `wg profile use pi` appear to be a required second step.
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();
    let fake_bin = install_fake_pi(tmp.path());
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    fs::create_dir_all(&graph).unwrap();
    worksgood::parser::save_graph(
        &worksgood::graph::WorkGraph::new(),
        &graph.join("graph.jsonl"),
    )
    .unwrap();
    let model = "pi:openrouter:test/setup-model";

    let output = run_wg_in_isolation_with_env(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "setup",
            "--route",
            "pi",
            "--model",
            model,
            "--yes",
        ],
        &[("PATH", fake_bin.to_str().unwrap())],
    );
    assert!(
        output.status.success(),
        "wg setup --route pi --yes failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_local_config(&project);
    assert_eq!(
        cfg.models
            .default
            .as_ref()
            .and_then(|role| role.model.as_deref()),
        Some(model)
    );
    assert!(cfg.tiers.standard.is_none());
    assert!(cfg.tiers.fast.is_none());
    assert_eq!(cfg.strong_tier_spec().as_deref(), Some(model));
    assert_eq!(cfg.weak_tier_spec().as_deref(), Some(model));
    assert!(!fake_home.join(".wg/active-profile").exists());
    assert!(!fake_home.join(".wg/config.toml").exists());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Profile: project-local route is effective"),
        "{stdout}"
    );
    assert!(stdout.contains("Pi handler: AVAILABLE"), "{stdout}");
    assert!(stdout.contains("Pi auth/model: NOT VERIFIED"), "{stdout}");
    assert!(stdout.contains("run `pi`, use `/login`"), "{stdout}");
    assert!(stdout.contains("no cross-provider fallback"), "{stdout}");
}

#[test]
fn test_setup_reports_unavailable_pi_without_claiming_auth_or_model_access() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let empty_path = tmp.path().join("empty-path");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&empty_path).unwrap();

    let graph = tmp.path().join("project/.wg");
    fs::create_dir_all(&graph).unwrap();
    worksgood::parser::save_graph(
        &worksgood::graph::WorkGraph::new(),
        &graph.join("graph.jsonl"),
    )
    .unwrap();
    let output = run_wg_in_isolation_with_env(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "setup",
            "--route",
            "pi",
            "--yes",
        ],
        &[("PATH", empty_path.to_str().unwrap())],
    );
    assert!(
        output.status.success(),
        "bounded readiness limitation must not invent another route: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fake_home.join(".wg/active-profile").exists());
    assert!(graph.parent().unwrap().join("worksgood.toml").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Pi handler: UNAVAILABLE on PATH"),
        "{stdout}"
    );
    assert!(stdout.contains("Pi auth/model: NOT VERIFIED"), "{stdout}");
    assert!(
        stdout.contains("install Pi, then rerun `wg setup`"),
        "{stdout}"
    );
    assert!(stdout.contains("no fallback was chosen"), "{stdout}");
}

#[test]
fn test_setup_legacy_codex_route_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected("codex-cli", &[]);
}

#[test]
fn test_setup_legacy_openrouter_endpoint_route_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected(
        "openrouter",
        &[
            "--url",
            "http://127.0.0.1:9/api/v1",
            "--api-key-env",
            "OPENROUTER_API_KEY",
        ],
    );
}

#[test]
fn test_setup_legacy_openrouter_route_without_key_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected("openrouter", &[]);
}

#[test]
fn test_setup_legacy_local_route_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected(
        "local",
        &["--url", "http://localhost:11434/v1", "--model", "qwen3:4b"],
    );
}

#[test]
fn test_setup_legacy_nex_custom_route_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected("nex-custom", &[]);
}

#[test]
fn test_setup_dry_run_does_not_write() {
    // --dry-run prints the would-be config but doesn't touch the filesystem.
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &["setup", "--route", "pi", "--dry-run", "--yes"],
    );
    assert!(output.status.success());

    // No global config should have been created — under either the modern
    // `.wg` or legacy `.wg` global dir.
    let modern = fake_home.join(".wg/config.toml");
    let legacy = fake_home.join(".wg/config.toml");
    assert!(
        !modern.exists() && !legacy.exists(),
        "dry-run must not create global config (neither {} nor {} should exist)",
        modern.display(),
        legacy.display(),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run") || stdout.contains("dispatcher") || stdout.contains("agent"),
        "dry-run output should include the would-be config, got: {}",
        stdout,
    );
}

// ---------------------------------------------------------------------------
// wg init --dry-run: no write
// ---------------------------------------------------------------------------

#[test]
fn test_init_dry_run_no_write() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let wg_dir = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["init", "--route", "pi", "--dry-run"])
        .env("HOME", &fake_home)
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_CONTROL_PROTOCOL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init --dry-run failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The .wg directory should NOT have been created.
    assert!(
        !wg_dir.exists(),
        ".wg directory should not exist after --dry-run"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run") || stdout.contains("[dispatcher]") || stdout.contains("[agent]"),
        "stdout should show the would-be config, got: {}",
        stdout,
    );
}

// ---------------------------------------------------------------------------
// wg init -x claude → populated [tiers] (the bug)
// ---------------------------------------------------------------------------

#[test]
fn test_init_legacy_executor_is_rejected_without_initializing_graph() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "init",
            "-x",
            "claude",
            "--no-agency",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("legacy executor") && stderr.contains("Pi is the sole LLM handler"),
        "{stderr}"
    );
    assert!(!graph.exists());
    assert!(!project.join("worksgood.toml").exists());
}

#[test]
fn test_init_legacy_openrouter_route_is_rejected_without_initializing_graph() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "init",
            "--route",
            "openrouter",
            "--no-agency",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("supported route is: pi"), "{stderr}");
    assert!(!graph.exists());
    assert!(!project.join("worksgood.toml").exists());
}

// ---------------------------------------------------------------------------
// wg config reset: backup + --keep-keys
// ---------------------------------------------------------------------------

#[test]
fn test_config_reset_legacy_route_is_rejected_without_mutating_project_authority() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&graph).unwrap();
    let path = project.join("worksgood.toml");
    let pre = r#"schema_version = 1

[models.default]
model = "claude:opus"
"#;
    fs::write(&path, pre).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "config",
            "reset",
            "--route",
            "claude-cli",
            "--keep-keys",
            "--yes",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("supported reset route is: pi"), "{stderr}");
    assert_eq!(fs::read_to_string(path).unwrap(), pre);
}

#[test]
fn test_config_reset_creates_backup() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    fs::create_dir_all(&graph).unwrap();

    let pre = r#"
schema_version = 1

[models.default]
model = "claude:opus"
reasoning = "high"
"#;
    fs::write(project.join("worksgood.toml"), pre).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "config",
            "reset",
            "--route",
            "pi",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "config reset failed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let backups: Vec<_> = fs::read_dir(&project)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("worksgood.toml.bak-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    let backup_content = fs::read_to_string(backups[0].path()).unwrap();
    assert!(backup_content.contains("claude"));
    assert!(backup_content.contains("opus"));
}

#[test]
fn test_config_reset_dry_run_does_not_write() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let graph = project.join(".wg");
    fs::create_dir_all(&graph).unwrap();

    let pre =
        "schema_version = 1\n\n[models.default]\nmodel = \"claude:sonnet\"\nreasoning = \"high\"\n";
    let path = project.join("worksgood.toml");
    fs::write(&path, pre).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "--dir",
            graph.to_str().unwrap(),
            "config",
            "reset",
            "--route",
            "pi",
            "--dry-run",
        ],
    );
    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), pre);
    assert!(
        !fs::read_dir(&project)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("worksgood.toml.bak-")
            })
    );
}

#[test]
fn test_setup_legacy_openrouter_stdin_secret_route_is_rejected_without_writing() {
    assert_legacy_setup_route_rejected(
        "openrouter",
        &["--scope", "local", "--from-stdin", "--backend", "keystore"],
    );
}

#[test]
fn test_setup_legacy_openrouter_cannot_reactivate_global_login() {
    assert_legacy_setup_route_rejected("openrouter", &["--scope", "local"]);
}

#[test]
fn test_setup_legacy_claude_route_is_rejected_without_prompting_for_credentials() {
    assert_legacy_setup_route_rejected("claude-cli", &[]);
}

#[test]
fn test_setup_route_pi_local_ignores_legacy_global_endpoint_authority() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    fs::create_dir_all(fake_home.join(".wg")).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(
        fake_home.join(".wg/config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "env:OPENROUTER_API_KEY"
is_default = true
"#,
    )
    .unwrap();

    let output = Command::new(wg_binary())
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("OPENROUTER_API_KEY", "sk-or-global-reuse")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .env_remove("WG_WORKER_CAPABILITY")
        .env_remove("WG_WORKER_CONTROL_PROTOCOL")
        .args(["setup", "--route", "pi", "--scope", "local", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wg setup pi local reuse failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_local_config(&project);
    assert!(!cfg.llm_endpoints.inherit_global);
    assert!(cfg.llm_endpoints.endpoints.is_empty());
    cfg.validate_pi_model_plane().unwrap();
    assert!(
        !fake_home.join(".wg/active-profile").exists(),
        "--scope local must not mutate the global active-profile pointer"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("global active-profile intentionally unchanged by --scope local")
    );
}

#[test]
fn test_setup_help_mentions_pi_and_hides_provider_credentials() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("pi:<provider>:<model>"));
    assert!(!stdout.contains("--from-stdin"));
    assert!(!stdout.contains("Secret backend"));
}
