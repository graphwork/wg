//! Filesystem-isolated integration tests for `wg migrate project-local-pi
//! --cleanup-global-routing` (design-project-local-pi-config.md §9.2).
//!
//! These tests prove the cleanup removes *only* stale machine-global
//! model-routing and active-profile state while preserving every reusable
//! profile definition, every identity/federation/secret datum, and every
//! other non-routing byte. They cover dry-run, backup/rollback evidence,
//! second-run idempotence, malformed-config fail-closed, and exercise the
//! migration through the real `wg migrate project-local-pi` CLI command.

use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use worksgood::config::Config;

struct EnvRestore {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        unsafe {
            if let Some(p) = self.previous.take() {
                std::env::set_var(self.key, p);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn blake3_hex(bytes: &[u8]) -> String {
    format!("b3:{}", hex::encode(blake3::hash(bytes).as_bytes()))
}

fn digest(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| blake3_hex(&b))
}

/// Build a sentinel-filled `~/.wg`: routing state to remove + protected
/// data that must survive byte-for-byte.
fn sentinel_global(global: &Path) {
    fs::create_dir_all(global).unwrap();
    // Stale routing in the global config.
    fs::write(
        global.join("config.toml"),
        r#"profile = "pi"

[agent]
model = "pi:global-provider:stale-model"
executor = "claude"

[dispatcher]
model = "pi:global-provider:stale-model"
provider = "global"
max_agents = 7

[models.task_agent]
model = "pi:global-provider:stale-model"
reasoning = "high"

[tiers]
fast = "pi:global-provider:fast"

[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50

[[execution.fallbacks]]
primary = "pi:global-provider:stale-model"
models = ["pi:global-provider:alt"]

[secrets]
allow_plaintext = true

[auth]
claude_code_oauth_token = "must-not-be-touched"

[native_executor]
preserved_field = "preserved"
"#,
    )
    .unwrap();
    fs::write(global.join("active-profile"), "pi\n").unwrap();

    // Protected roots — must survive byte-identical.
    fs::create_dir_all(global.join("profiles")).unwrap();
    fs::write(
        global.join("profiles/pi.toml"),
        "# reusable profile definition\n[agent]\nmodel = \"pi:provider:model\"\n",
    )
    .unwrap();
    fs::create_dir_all(global.join("secrets")).unwrap();
    fs::write(global.join("secrets/alpha"), b"secret-bytes-alpha\n").unwrap();
    fs::create_dir_all(global.join("keystore")).unwrap();
    fs::write(global.join("keystore/root.key"), b"root-key-bytes\n").unwrap();
    fs::write(
        global.join("profile-usage.jsonl"),
        "{\"name\":\"pi\",\"ts\":\"now\"}\n",
    )
    .unwrap();
}

/// Snapshot every protected path's digest before cleanup.
struct ProtectedSnapshot {
    profiles_pi: Option<String>,
    secrets_alpha: Option<String>,
    keystore_root: Option<String>,
    profile_usage: Option<String>,
    auth_in_config: Option<String>, // digest of whole config to check auth section survives
}

impl ProtectedSnapshot {
    fn snapshot(global: &Path) -> Self {
        Self {
            profiles_pi: digest(&global.join("profiles/pi.toml")),
            secrets_alpha: digest(&global.join("secrets/alpha")),
            keystore_root: digest(&global.join("keystore/root.key")),
            profile_usage: digest(&global.join("profile-usage.jsonl")),
            auth_in_config: digest(&global.join("config.toml")),
        }
    }

    fn assert_unchanged(&self, global: &Path) {
        assert_eq!(
            digest(&global.join("profiles/pi.toml")),
            self.profiles_pi,
            "reusable profile definition must be byte-identical"
        );
        assert_eq!(
            digest(&global.join("secrets/alpha")),
            self.secrets_alpha,
            "secret material must be byte-identical"
        );
        assert_eq!(
            digest(&global.join("keystore/root.key")),
            self.keystore_root,
            "keystore private key must be byte-identical"
        );
        assert_eq!(
            digest(&global.join("profile-usage.jsonl")),
            self.profile_usage,
            "profile usage history must be byte-identical"
        );
    }
}

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps
    path.pop(); // debug or release
    if path.file_name().and_then(|s| s.to_str()) == Some("debug") {
        // integration tests run from <target>/debug/deps; the wg binary is
        // at <target>/debug/wg
    }
    path.join("wg")
}

fn run_wg(global: &Path, home: &Path, args: &[&str]) -> (std::process::Output, String) {
    let mut cmd = Command::new(wg_binary());
    cmd.env_clear();
    cmd.env("WG_GLOBAL_DIR", global);
    cmd.env("HOME", home);
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
    cmd.args(args);
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("stdout:\n{stdout}\nstderr:\n{stderr}\n");
    (output, combined)
}

#[test]
#[serial]
fn lib_apply_removes_only_routing_and_preserves_sentinels() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let _r = EnvRestore::set("WG_GLOBAL_DIR", &global);

    let snap = ProtectedSnapshot::snapshot(&global);

    let args = worksgood::migrate_project_local_pi::ProjectLocalPiMigrateArgs {
        dry_run: false,
        yes: true,
        cleanup_global_routing: true,
        rollback: None,
    };
    worksgood::migrate_project_local_pi::run_project_local_pi_migrate(
        Path::new("/nonexistent-graph"),
        args,
        false,
    )
    .unwrap();

    // Routing gone.
    let after = fs::read_to_string(global.join("config.toml")).unwrap();
    assert!(
        !after.contains("pi:global-provider"),
        "routing leaked: {after}"
    );
    assert!(!after.contains("fallback_model"));
    assert!(!after.contains("[models]"));
    assert!(!after.contains("[tiers]"));
    assert!(!after.contains("profile ="));
    assert!(!after.contains("executor ="));
    // Active-profile pointer removed.
    assert!(!global.join("active-profile").exists());

    // Preserved non-routing bytes survive.
    assert!(after.contains("max_agents = 7"));
    assert!(after.contains("allow_plaintext = true"));
    assert!(after.contains("claude_code_oauth_token"));
    assert!(after.contains("must-not-be-touched"));
    assert!(after.contains("[native_executor]"));
    assert!(after.contains("monthly_budget_usd = 50"));

    snap.assert_unchanged(&global);

    // Backup + receipt written.
    let migrations = global.join("migrations/project-local-pi");
    let entries: Vec<_> = fs::read_dir(&migrations)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1, "exactly one receipt dir expected");
    let receipt_dir = entries[0].path();
    assert!(receipt_dir.join("config.toml.pre").exists());
    assert!(receipt_dir.join("active-profile.pre").exists());
    assert!(receipt_dir.join("receipt.json").exists());

    // Backup contains the original routing bytes.
    let backup = fs::read_to_string(receipt_dir.join("config.toml.pre")).unwrap();
    assert!(backup.contains("pi:global-provider:stale-model"));
}

#[test]
#[serial]
fn cli_dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let before_config = digest(&global.join("config.toml")).unwrap();
    let before_active = digest(&global.join("active-profile")).unwrap();
    let snap = ProtectedSnapshot::snapshot(&global);

    let (output, combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--dry-run",
            "--yes",
        ],
    );
    assert!(output.status.success(), "dry-run failed: {combined}");

    // Nothing changed.
    assert_eq!(digest(&global.join("config.toml")).unwrap(), before_config);
    assert_eq!(
        digest(&global.join("active-profile")).unwrap(),
        before_active
    );
    assert!(!global.join("migrations").exists());
    snap.assert_unchanged(&global);
}

#[test]
#[serial]
fn cli_apply_via_real_wg_binary_then_idempotent_second_run() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let snap = ProtectedSnapshot::snapshot(&global);

    // First apply.
    let (out, combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
        ],
    );
    assert!(out.status.success(), "first apply failed: {combined}");
    assert!(!global.join("active-profile").exists());

    let after_first = fs::read_to_string(global.join("config.toml")).unwrap();
    assert!(!after_first.contains("pi:global-provider"));
    snap.assert_unchanged(&global);

    let migrations = global.join("migrations/project-local-pi");
    let count_after_first = fs::read_dir(&migrations)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .len();
    assert_eq!(count_after_first, 1);

    // Second run is a no-op: no new receipt, no backup, no mtime change.
    let config_mtime_before = fs::metadata(global.join("config.toml"))
        .unwrap()
        .modified()
        .unwrap();
    let (out2, combined2) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
        ],
    );
    assert!(out2.status.success(), "second run failed: {combined2}");
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(
        stdout.contains("nothing to clean") || stdout.contains("no stale global routing"),
        "second run should report no-op: {stdout}"
    );

    let count_after_second = fs::read_dir(&migrations)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .len();
    assert_eq!(
        count_after_second, count_after_first,
        "no new receipt on no-op"
    );

    let config_mtime_after = fs::metadata(global.join("config.toml"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        config_mtime_before, config_mtime_after,
        "config mtime must not change on no-op"
    );
}

#[test]
#[serial]
fn cli_malformed_config_fail_closed() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    fs::create_dir_all(&global).unwrap();
    fs::write(
        global.join("config.toml"),
        "this is = = not valid toml {{{\n",
    )
    .unwrap();
    fs::write(global.join("active-profile"), "pi\n").unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let before_config = digest(&global.join("config.toml")).unwrap();
    let before_active = digest(&global.join("active-profile")).unwrap();

    let (out, _combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
        ],
    );
    assert!(!out.status.success(), "malformed config must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not valid TOML"),
        "expected TOML error: {stderr}"
    );

    // Nothing changed.
    assert_eq!(digest(&global.join("config.toml")).unwrap(), before_config);
    assert_eq!(
        digest(&global.join("active-profile")).unwrap(),
        before_active
    );
    assert!(!global.join("migrations").exists());
}

#[test]
#[serial]
fn cli_rollback_restores_exact_bytes() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let original_config = digest(&global.join("config.toml")).unwrap();
    let original_active = digest(&global.join("active-profile")).unwrap();

    // Apply.
    let (out, combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
            "--json",
        ],
    );
    assert!(out.status.success(), "apply failed: {combined}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let receipt_id = payload
        .get("receipt_id")
        .and_then(|v| v.as_str())
        .expect("receipt_id in JSON output")
        .to_string();

    // Routing gone.
    assert!(!global.join("active-profile").exists());
    assert!(
        !fs::read_to_string(global.join("config.toml"))
            .unwrap()
            .contains("pi:global-provider")
    );

    // Rollback.
    let (rb, combined) = run_wg(
        &global,
        &home,
        &["migrate", "project-local-pi", "--rollback", &receipt_id],
    );
    assert!(rb.status.success(), "rollback failed: {combined}");

    // Exact bytes restored.
    assert_eq!(
        digest(&global.join("config.toml")).unwrap(),
        original_config
    );
    assert_eq!(
        digest(&global.join("active-profile")).unwrap(),
        original_active
    );
}

#[test]
#[serial]
fn cli_rollback_refuses_after_user_edit() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let (out, combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
            "--json",
        ],
    );
    assert!(out.status.success(), "apply failed: {combined}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let receipt_id = payload
        .get("receipt_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // User edits the cleaned config.
    fs::write(
        global.join("config.toml"),
        "# user hand-edited\n[dispatcher]\nmax_agents = 9\n",
    )
    .unwrap();

    let (rb, _combined) = run_wg(
        &global,
        &home,
        &["migrate", "project-local-pi", "--rollback", &receipt_id],
    );
    assert!(!rb.status.success(), "rollback must refuse after user edit");
    let stderr = String::from_utf8_lossy(&rb.stderr);
    assert!(
        stderr.contains("refus") || stderr.contains("differ"),
        "expected refusal: {stderr}"
    );

    // User edit preserved.
    assert!(
        fs::read_to_string(global.join("config.toml"))
            .unwrap()
            .contains("user hand-edited")
    );
}

#[test]
#[serial]
fn cli_informational_mode_without_cleanup_flag_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let before_config = digest(&global.join("config.toml")).unwrap();
    let before_active = digest(&global.join("active-profile")).unwrap();

    // Without --cleanup-global-routing, the command is informational only.
    let (out, combined) = run_wg(&global, &home, &["migrate", "project-local-pi"]);
    assert!(
        out.status.success(),
        "informational mode failed: {combined}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed routing selectors") || stdout.contains("global routing cleanup"),
        "informational mode should report the plan: {stdout}"
    );

    // Nothing changed.
    assert_eq!(digest(&global.join("config.toml")).unwrap(), before_config);
    assert_eq!(
        digest(&global.join("active-profile")).unwrap(),
        before_active
    );
    assert!(!global.join("migrations").exists());
}

#[test]
#[serial]
fn cli_config_lint_reports_stale_global_routing_before_migrating() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    // A project graph so `wg config lint` has a working dir.
    let project = tmp.path().join("project");
    fs::create_dir_all(project.join(".wg")).unwrap();

    let _r = EnvRestore::set("WG_GLOBAL_DIR", &global);

    let mut cmd = Command::new(wg_binary());
    cmd.env_clear();
    cmd.env("WG_GLOBAL_DIR", &global);
    cmd.env("HOME", &home);
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
    cmd.current_dir(&project);
    cmd.args([
        "--dir",
        project.join(".wg").to_str().unwrap(),
        "config",
        "lint",
        "--global",
    ]);
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "lint failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("stale machine-global routing") || stdout.contains("stale_global_routing"),
        "lint should report stale global routing: {stdout}"
    );
    assert!(
        stdout.contains("agent.model"),
        "lint should name agent.model: {stdout}"
    );
    assert!(
        stdout.contains("active-profile"),
        "lint should name active-profile: {stdout}"
    );
    assert!(
        stdout.contains("wg migrate project-local-pi --cleanup-global-routing"),
        "lint should name the remediation command: {stdout}"
    );

    // Secret values must NOT appear in lint output.
    assert!(!stdout.contains("must-not-be-touched"));
}

#[test]
#[serial]
fn cli_winning_source_after_migration_is_not_global() {
    let tmp = TempDir::new().unwrap();
    let global = tmp.path().join(".wg");
    sentinel_global(&global);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(project.join(".wg")).unwrap();

    let _r = EnvRestore::set("WG_GLOBAL_DIR", &global);

    // Before migration: global routing is inactive but present.
    let (config, sources) = Config::load_with_sources(&project.join(".wg")).unwrap();
    assert!(
        config.agent.model.is_empty(),
        "global route must not be effective"
    );
    assert!(
        sources
            .values()
            .all(|s| *s != worksgood::config::ConfigSource::Global),
        "no effective source may be Global"
    );

    // Run the cleanup.
    let (out, combined) = run_wg(
        &global,
        &home,
        &[
            "migrate",
            "project-local-pi",
            "--cleanup-global-routing",
            "--yes",
        ],
    );
    assert!(out.status.success(), "cleanup failed: {combined}");

    // After migration: still route-less (no worksgood.toml), but the global
    // routing selectors are gone. The winning source for every leaf remains
    // builtin-default, never Global.
    let (config2, sources2) = Config::load_with_sources(&project.join(".wg")).unwrap();
    assert!(config2.agent.model.is_empty());
    assert!(
        sources2
            .values()
            .all(|s| *s != worksgood::config::ConfigSource::Global),
        "no effective source may be Global after migration"
    );
    // The global config no longer carries routing.
    let after_global = fs::read_to_string(global.join("config.toml")).unwrap();
    assert!(!after_global.contains("pi:global-provider"));
}
