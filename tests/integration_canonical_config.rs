//! Integration tests for the canonical-config UX:
//!   `wg config init [--global|--local]`
//!   `wg migrate config`
//!   built-in defaults (no `~/.wg/config.toml` required)
//!
//! These tests exercise the real `wg` binary so they catch CLI plumbing
//! regressions (subcommand parsing, dispatch, file writes) — not just
//! the underlying Rust functions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use worksgood::graph::WorkGraph;
use worksgood::parser::save_graph;

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(path.exists(), "wg binary not found at {:?}", path);
    path
}

fn wg(wg_dir: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", home)
        .env_remove("WG_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn wg")
}

fn wg_ok(wg_dir: &Path, home: &Path, args: &[&str]) -> String {
    let out = wg(wg_dir, home, args);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "wg {:?} failed.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr,
    );
    stdout
}

fn fresh_workgraph(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph = WorkGraph::new();
    save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();
    wg_dir
}

// ---------------------------------------------------------------------------
// test_defaults_no_user_config
// ---------------------------------------------------------------------------

#[test]
fn defaults_no_user_config_are_graph_only_and_unselected() {
    // With no config, read-only inspection works but no LLM route is created.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    // Sanity — no config file written yet.
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!wg_dir.join("config.toml").exists());

    let out = wg_ok(&wg_dir, &home, &["config", "--merged"]);
    assert!(out.contains("recommended = \"Pi\""), "got:\n{out}");
    assert!(
        out.contains("INVALID"),
        "unselected roles must be visible: {out}"
    );
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!wg_dir.join("config.toml").exists());
}

// ---------------------------------------------------------------------------
// test_config_init_global_writes_minimal
// ---------------------------------------------------------------------------

#[test]
fn config_init_global_writes_minimal_canonical() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    let stdout = wg_ok(
        &wg_dir,
        &home,
        &["config", "init", "--global", "--route", "pi"],
    );
    assert!(
        stdout.contains("Wrote minimal global config"),
        "init should announce what it wrote; got:\n{}",
        stdout,
    );

    let path = home.join(".wg/config.toml");
    assert!(
        path.exists(),
        "init --global should create ~/.wg/config.toml; got nothing at {:?}",
        path,
    );
    let body = fs::read_to_string(&path).unwrap();

    // The Pi route writes the complete Pi profile snapshot.
    assert!(body.contains("[agent]"), "missing [agent]; got:\n{}", body);
    assert!(body.contains("model = \"pi:openrouter:z-ai/glm-5.2\""));
    assert!(body.contains("[tiers]"));
    assert!(body.contains("fast = \"pi:openrouter:deepseek/deepseek-chat\""));
    assert!(body.contains("fast_reasoning = \"low\""));
    assert!(body.contains("standard_reasoning = \"high\""));
    assert!(body.contains("premium_reasoning = \"xhigh\""));
    assert!(body.contains("[models.default]"));
    assert!(body.contains("[models.task_agent]"));
    assert!(body.contains("[models.evaluator]"));
    assert!(body.contains("[models.assigner]"));

    // The config must not contain deprecated keys carried over from older
    // templates — the profile snapshots are clean.
    assert!(
        !body.contains("verify_autospawn_enabled"),
        "global config should not contain deprecated keys; got:\n{}",
        body,
    );
    assert!(
        !body.contains("agent.executor") && !body.contains("\nexecutor ="),
        "global config should not contain agent.executor (deprecated); got:\n{}",
        body,
    );

    // The file must parse cleanly as a `Config` — that's the round-trip
    // guarantee the profile-as-snapshot model rests on.
    let cfg: worksgood::config::Config = toml::from_str(&body)
        .unwrap_or_else(|e| panic!("global config must round-trip through Config: {e}\n{body}"));
    assert_eq!(
        cfg.tiers.standard.as_deref(),
        Some("pi:openrouter:z-ai/glm-5.2")
    );
    cfg.validate_pi_model_plane().unwrap();
}

// ---------------------------------------------------------------------------
// Regression: --route codex-cli must produce a working codex config.
// Before the 2026-05 profile-as-snapshot pivot, this route emitted a config
// that named codex in some places but left other roles unset / claude-default
// — see the fix-codex-init task. The fix shares the codex profile template
// as the single source of truth, so `--route codex-cli` is now byte-identical
// to a `wg profile use codex` swap.
// ---------------------------------------------------------------------------

#[test]
fn config_init_route_pi_produces_complete_pi_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &home,
        &["config", "init", "--global", "--route", "pi"],
    );
    let body = fs::read_to_string(home.join(".wg/config.toml")).unwrap();

    assert!(body.contains("model = \"pi:openrouter:z-ai/glm-5.2\""));
    assert!(body.contains("model = \"pi:openrouter:deepseek/deepseek-chat\""));
    assert!(!body.contains("codex:gpt") && !body.contains("claude:opus"));

    let cfg: worksgood::config::Config = toml::from_str(&body).unwrap();
    cfg.validate_pi_model_plane().unwrap();
}

#[test]
fn config_init_route_pi_matches_pi_profile_template() {
    // The single-source-of-truth contract: `wg config init --route codex-cli`
    // must emit byte-identical content to the codex profile template that
    // `wg profile use codex` would swap in. Same source, same bytes.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &home,
        &["config", "init", "--global", "--route", "pi"],
    );
    let written = fs::read_to_string(home.join(".wg/config.toml")).unwrap();
    let template = worksgood::profile::named::STARTER_PI;
    assert_eq!(
        written, template,
        "pi route must emit the Pi profile template verbatim",
    );
}

#[test]
fn config_init_refuses_to_clobber_existing_without_force() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    // Pre-existing global config with custom value.
    fs::create_dir_all(home.join(".wg")).unwrap();
    fs::write(
        home.join(".wg/config.toml"),
        "[agent]\nmodel = \"openrouter:anthropic/claude-opus-4-7\"\n",
    )
    .unwrap();

    let out = wg(
        &wg_dir,
        &home,
        &["config", "init", "--global", "--route", "pi"],
    );
    assert!(
        !out.status.success(),
        "init --global should refuse to clobber an existing file"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists") || stderr.contains("--force"),
        "error message should mention --force; got:\n{}",
        stderr,
    );
}

#[test]
fn config_init_force_makes_backup() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::create_dir_all(home.join(".wg")).unwrap();
    fs::write(
        home.join(".wg/config.toml"),
        "# pre-existing\n[agent]\nmodel = \"custom-model\"\n",
    )
    .unwrap();

    wg_ok(
        &wg_dir,
        &home,
        &["config", "init", "--global", "--route", "pi", "--force"],
    );
    let backup = home.join(".wg/config.toml.bak");
    assert!(
        backup.exists(),
        "init --global --force should write a .bak; got nothing at {:?}",
        backup,
    );
    let backup_body = fs::read_to_string(&backup).unwrap();
    assert!(backup_body.contains("custom-model"));
}

// ---------------------------------------------------------------------------
// test_migrate_strips_deprecated
// ---------------------------------------------------------------------------

#[test]
fn migrate_strips_deprecated_agent_executor() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::write(
        wg_dir.join("config.toml"),
        r#"
[agent]
executor = "claude"
model = "claude:opus"
"#,
    )
    .unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["migrate", "config", "--local"]);
    assert!(
        stdout.contains("agent.executor"),
        "migrate should report removing agent.executor; got:\n{}",
        stdout,
    );

    let body = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert!(
        !body.contains("executor"),
        "migrated config must not contain executor; got:\n{}",
        body,
    );
    assert!(
        body.contains("model = \"claude:opus\""),
        "migrated config must keep model; got:\n{}",
        body,
    );
}

// ---------------------------------------------------------------------------
// test_migrate_stale_model
// ---------------------------------------------------------------------------

#[test]
fn migrate_rewrites_stale_openrouter_sonnet_model() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::write(
        wg_dir.join("config.toml"),
        r#"
[agent]
model = "openrouter:anthropic/claude-sonnet-4"
"#,
    )
    .unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["migrate", "config", "--local"]);
    assert!(
        stdout.contains("openrouter:anthropic/claude-sonnet-4-6"),
        "migrate should announce the rewrite; got:\n{}",
        stdout,
    );

    let body = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert!(body.contains("openrouter:anthropic/claude-sonnet-4-6"));
    assert!(
        !body.contains("\"openrouter:anthropic/claude-sonnet-4\""),
        "old stale string must be gone; got:\n{}",
        body,
    );
}

#[test]
fn migrate_dry_run_does_not_modify() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    let original = "[agent]\nexecutor = \"claude\"\n";
    fs::write(wg_dir.join("config.toml"), original).unwrap();

    wg_ok(
        &wg_dir,
        &home,
        &["migrate", "config", "--local", "--dry-run"],
    );
    let after = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert_eq!(original, after, "dry-run must not touch the config file");
}

// ---------------------------------------------------------------------------
// `wg config lint` — read-only companion to `wg migrate config`.
// ---------------------------------------------------------------------------

#[test]
fn lint_reports_deprecated_keys() {
    // A config with `agent.executor` (deprecated — handler is derived from
    // model spec now) must trigger a warning from `wg config lint`. The
    // command is read-only so the file content must be preserved exactly.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    let stale = "[agent]\nexecutor = \"claude\"\nmodel = \"claude:opus\"\n";
    fs::write(wg_dir.join("config.toml"), stale).unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["config", "lint", "--local"]);
    assert!(
        stdout.contains("agent.executor"),
        "lint should name the deprecated key; got:\n{}",
        stdout,
    );
    assert!(
        stdout.to_lowercase().contains("deprecated") || stdout.to_lowercase().contains("removed"),
        "lint should label the finding as deprecated/removable; got:\n{}",
        stdout,
    );
    assert!(
        stdout.contains("wg migrate config"),
        "lint should point at `wg migrate config` for the fix; got:\n{}",
        stdout,
    );
}

#[test]
fn lint_reports_stale_models() {
    // Stale openrouter model strings (e.g. claude-sonnet-4 with no minor)
    // must be flagged with their canonical replacement.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::write(
        wg_dir.join("config.toml"),
        "[agent]\nmodel = \"openrouter:anthropic/claude-sonnet-4\"\n",
    )
    .unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["config", "lint", "--local"]);
    assert!(
        stdout.contains("openrouter:anthropic/claude-sonnet-4-6"),
        "lint should announce the canonical replacement; got:\n{}",
        stdout,
    );
    // The stale value (without -6) must appear as well — it's the "from" side
    // of the rewrite. Match the exact-quoted form so we don't get a false
    // positive from `claude-sonnet-4-6` matching the `claude-sonnet-4` prefix.
    assert!(
        stdout.contains("\"openrouter:anthropic/claude-sonnet-4\""),
        "lint should mention the exact stale value as a quoted string; got:\n{}",
        stdout,
    );
}

#[test]
fn lint_does_not_modify_files() {
    // The lint command must be strictly read-only — the on-disk content
    // (including byte-exact whitespace) must be unchanged after lint runs,
    // and no `.bak` / `.pre-migrate.*` siblings may appear.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    let original = "\n[agent]\nexecutor = \"claude\"\nmodel = \"openrouter:anthropic/claude-sonnet-4\"\n\n[dispatcher]\nchat_agent = true\nmax_chats = 4\n";
    fs::write(wg_dir.join("config.toml"), original).unwrap();

    wg_ok(&wg_dir, &home, &["config", "lint", "--local"]);

    let after = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert_eq!(
        original, after,
        "lint must not modify the config file (byte-for-byte)",
    );

    // No backup or migration siblings created.
    let entries: Vec<String> = fs::read_dir(&wg_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    for name in &entries {
        assert!(
            !name.contains("pre-migrate") && !name.ends_with(".bak"),
            "lint must not create backup siblings; found {} in {:?}",
            name,
            entries,
        );
    }
}

#[test]
fn lint_clean_local_config_reports_clean() {
    // A canonical local config should produce a "clean" message with
    // no warning lines.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::write(
        wg_dir.join("config.toml"),
        "[agent]\nmodel = \"claude:opus\"\n",
    )
    .unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["config", "lint", "--local"]);
    assert!(
        stdout.to_lowercase().contains("clean"),
        "clean config should be announced as clean; got:\n{}",
        stdout,
    );
    assert!(
        !stdout.to_lowercase().contains("warning:"),
        "clean config should produce no warnings; got:\n{}",
        stdout,
    );
}

#[test]
fn lint_reports_renamed_keys() {
    // `chat_agent` → `coordinator_agent` and `max_chats` → `max_coordinators`
    // are predictable renames; lint must announce both with the new names.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    fs::write(
        wg_dir.join("config.toml"),
        "[dispatcher]\nchat_agent = true\nmax_chats = 4\n",
    )
    .unwrap();

    let stdout = wg_ok(&wg_dir, &home, &["config", "lint", "--local"]);
    assert!(
        stdout.contains("coordinator_agent"),
        "lint should name the canonical replacement; got:\n{}",
        stdout,
    );
    assert!(
        stdout.contains("max_coordinators"),
        "lint should name the canonical replacement; got:\n{}",
        stdout,
    );
}

// ---------------------------------------------------------------------------
// `wg quickstart` works with no global config (sanity)
// ---------------------------------------------------------------------------

#[test]
fn quickstart_with_no_global_config_does_not_error() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("fakehome");
    fs::create_dir_all(&home).unwrap();
    let wg_dir = fresh_workgraph(&tmp);

    let out = wg(&wg_dir, &home, &["quickstart"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "quickstart with no global config should succeed.\nstdout: {}\nstderr: {}",
        stdout,
        stderr,
    );
}
