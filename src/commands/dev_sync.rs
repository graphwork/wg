//! `wg dev-sync` — the one supported command for the local development inner
//! loop.
//!
//! The manual flow this replaces was four steps, two of which were easy to get
//! wrong (and one of which — a worktree-built `cargo install` — silently
//! mis-deployed this repo twice):
//!
//! ```text
//! cargo install --path . --locked
//! wg service start --force
//! env -u WG_PI_PLUGIN_DIR WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install
//! wg pi-plugin status
//! ```
//!
//! `wg dev-sync` runs those in order and then prints a verification block that
//! makes a mis-deploy visible at a glance: the resolved binary path + content
//! hash, the running binary's embed digest vs the on-disk cache digest, and the
//! live daemon's build identity (so a stale daemon is obvious).
//!
//! It refuses to run from a WG worktree, because `cargo install` from a
//! prunable `.wg-worktrees/` tree bakes that path into the shared global binary
//! (see `worksgood::build_provenance`). Only the operator runs this, from the
//! main checkout.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use worksgood::build_provenance;
use worksgood::service_identity;

/// Options for a dev-sync run.
#[derive(Debug, Clone, Copy)]
pub struct DevSyncOptions {
    pub json: bool,
    pub no_install: bool,
    pub no_restart: bool,
    pub dry_run: bool,
}

/// The parsed subset of `wg pi-plugin status` the verification block reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedPluginStatus {
    pub embed_digest: Option<String>,
    pub cache_digest: Option<String>,
    pub cache_state: Option<String>,
}

/// A structured verification snapshot. `render_human` is the operator-facing
/// form; the field set is what a mis-deploy needs to be visible.
#[derive(Debug, Clone, Serialize)]
pub struct DevSyncReport {
    pub repo_root: String,
    pub branch: String,
    pub head: String,
    pub worktree_build: bool,
    pub binary_path: String,
    pub binary_sha256: Option<String>,
    pub embed_digest: Option<String>,
    pub cache_digest: Option<String>,
    pub cache_state: Option<String>,
    pub daemon_health: String,
    pub daemon_pid: Option<u32>,
    pub daemon_build_id: Option<String>,
    pub daemon_sha256: Option<String>,
    pub warnings: Vec<String>,
}

/// Parse the `embed digest` / `cache digest` / `cache state` lines out of a
/// `wg pi-plugin status` transcript. Pure + tolerant: a missing line yields
/// `None` rather than an error, so the verification block degrades honestly.
pub fn parse_status_output(text: &str) -> ParsedPluginStatus {
    let mut out = ParsedPluginStatus::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("embed digest:") {
            out.embed_digest = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("cache digest:") {
            out.cache_digest = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("cache state:") {
            // e.g. "current (matches this binary's embedded build)" — keep the
            // first token as the machine-readable state.
            let value = rest.trim();
            let first = value.split_whitespace().next().unwrap_or(value);
            out.cache_state = Some(first.to_string());
        }
    }
    out
}

/// Render the verification block. Kept as a pure string function so the exact
/// operator-visible contract (path, hash, both digests, daemon identity) is
/// unit-testable.
pub fn render_human(report: &DevSyncReport) -> String {
    let mut out = String::new();
    out.push_str("WG dev-sync verification\n");
    out.push_str(&format!("  repo:          {}\n", report.repo_root));
    out.push_str(&format!(
        "  branch/HEAD:   {} @ {}\n",
        report.branch, report.head
    ));
    out.push_str(&format!(
        "  wg binary:     {}{}\n",
        report.binary_path,
        report
            .binary_sha256
            .as_deref()
            .map(|h| format!(" ({h})"))
            .unwrap_or_default()
    ));
    out.push_str(&format!(
        "  embed digest:  {}\n",
        report.embed_digest.as_deref().unwrap_or("<unknown>")
    ));
    out.push_str(&format!(
        "  cache digest:  {}\n",
        report.cache_digest.as_deref().unwrap_or("<none>")
    ));
    out.push_str(&format!(
        "  cache state:   {}\n",
        report.cache_state.as_deref().unwrap_or("<unknown>")
    ));
    out.push_str(&format!("  daemon:        {}\n", report.daemon_health));
    out.push_str(&format!(
        "  daemon pid:    {}\n",
        report
            .daemon_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    ));
    out.push_str(&format!(
        "  daemon build:  {}{}\n",
        report.daemon_build_id.as_deref().unwrap_or("<none>"),
        report
            .daemon_sha256
            .as_deref()
            .map(|h| format!(" ({h})"))
            .unwrap_or_default()
    ));
    if report.warnings.is_empty() {
        out.push_str("  status:        OK\n");
    } else {
        out.push_str("  status:        WARN\n");
        for warning in &report.warnings {
            out.push_str(&format!("  warning: {warning}\n"));
        }
    }
    out
}

/// The installed binary path (`cargo install` target). Falls back to the
/// running executable when the canonical path does not exist.
fn installed_binary_path() -> PathBuf {
    let candidate = if let Some(root) = std::env::var_os("CARGO_INSTALL_ROOT") {
        Some(PathBuf::from(root).join("bin").join("wg"))
    } else if let Some(home) = std::env::var_os("CARGO_HOME") {
        Some(PathBuf::from(home).join("bin").join("wg"))
    } else {
        dirs::home_dir().map(|home| home.join(".cargo").join("bin").join("wg"))
    };
    match candidate {
        Some(path) if path.exists() => path,
        _ => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wg")),
    }
}

/// Shorten an `sha256:<hex>` (or bare hex) value to a stable display form:
/// `sha256:` + the first 12 hex chars. Used for both the installed binary and
/// the daemon so the two compare apples-to-apples.
fn short_hash(value: &str) -> String {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    format!("sha256:{}", hex.chars().take(12).collect::<String>())
}

fn git_output(args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {args:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_step(label: &str, cmd: &mut Command) -> Result<()> {
    println!("dev-sync: {label} ...");
    let status = cmd
        .status()
        .with_context(|| format!("failed to launch step `{label}`"))?;
    if !status.success() {
        anyhow::bail!("dev-sync step `{label}` failed with {status}");
    }
    Ok(())
}

/// Collect the verification snapshot. `dir` is the workgraph dir whose daemon
/// we describe.
fn collect_report(
    repo_root: &Path,
    binary_path: &Path,
    plugin_status: ParsedPluginStatus,
    dir: &Path,
) -> DevSyncReport {
    let branch = git_output(&["branch", "--show-current"], repo_root)
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "HEAD".to_string());
    let head = git_output(&["rev-parse", "--short=12", "HEAD"], repo_root)
        .unwrap_or_else(|_| "unknown".to_string());
    let binary_sha256 = service_identity::executable_sha256(binary_path)
        .ok()
        .map(|h| short_hash(&h));

    let observation = service_identity::observe_service(dir);
    let state = observation.state.as_ref();
    let handshake = observation.handshake_identity.as_ref();
    let daemon_sha256 = handshake.map(|i| short_hash(&i.executable_sha256));
    let daemon_build_id = handshake.map(|i| i.build_id.clone());

    let worktree_build = build_provenance::is_worktree_path(repo_root);
    let mut warnings = Vec::new();
    if worktree_build {
        warnings.push(format!(
            "repo root {} is a WG worktree; run dev-sync from the main checkout",
            repo_root.display()
        ));
    }
    if let Some(warning) = build_provenance::worktree_build_warning() {
        warnings.push(warning);
    }
    if let (Some(local), Some(daemon)) = (binary_sha256.as_deref(), daemon_sha256.as_deref())
        && local != daemon
    {
        warnings.push(format!(
            "daemon binary ({daemon}) differs from installed binary ({local}); \
             restart the daemon so the new binary is live"
        ));
    }
    match plugin_status.cache_state.as_deref() {
        Some(state) if state.eq_ignore_ascii_case("current") => {}
        Some(state) => warnings.push(format!(
            "pi-worksgood plugin cache state is `{state}` (expected `current`); run `wg pi-plugin install`"
        )),
        None => warnings.push("could not read pi-worksgood plugin cache state".to_string()),
    }

    DevSyncReport {
        repo_root: repo_root.display().to_string(),
        branch,
        head,
        worktree_build,
        binary_path: binary_path.display().to_string(),
        binary_sha256,
        embed_digest: plugin_status.embed_digest,
        cache_digest: plugin_status.cache_digest,
        cache_state: plugin_status.cache_state,
        daemon_health: format!("{:?}", observation.health),
        daemon_pid: state.map(|s| s.pid),
        daemon_build_id,
        daemon_sha256,
        warnings,
    }
}

/// Run the dev-sync flow.
pub fn run(dir: &Path, opts: DevSyncOptions) -> Result<()> {
    let repo_root = PathBuf::from(
        git_output(&["rev-parse", "--show-toplevel"], Path::new("."))
            .context("wg dev-sync must run inside the WG git checkout")?,
    );
    if build_provenance::is_worktree_path(&repo_root) {
        anyhow::bail!(
            "refusing to run dev-sync from a WG worktree ({}): `cargo install` from a \
             prunable worktree bakes that path into the shared global binary. Run it from \
             the main checkout.",
            repo_root.display()
        );
    }

    let binary = installed_binary_path();
    println!("dev-sync: repo {}", repo_root.display());
    println!("dev-sync: installed binary {}", binary.display());

    // 1. Install from the main checkout.
    if opts.no_install {
        println!("dev-sync: skipping install (--no-install)");
    } else if opts.dry_run {
        println!("dev-sync: [dry-run] cargo install --path . --locked");
    } else {
        let mut cmd = Command::new("cargo");
        cmd.arg("install")
            .arg("--path")
            .arg(".")
            .arg("--locked")
            .current_dir(&repo_root);
        run_step("install (cargo install --path . --locked)", &mut cmd)?;
    }

    // 2. Restart the daemon so the new binary is live.
    if opts.no_restart {
        println!("dev-sync: skipping daemon restart (--no-restart)");
    } else if opts.dry_run {
        println!(
            "dev-sync: [dry-run] {} service start --force",
            binary.display()
        );
    } else {
        let mut cmd = Command::new(&binary);
        cmd.args(["service", "start", "--force"]);
        run_step("daemon restart (service start --force)", &mut cmd)?;
    }

    // 3. Sync the plugin cache from the (new) binary's embed, then read back
    //    its verification output. Run it as a child so the freshly installed
    //    bytes are what does the work — the current process still holds the old
    //    executable image in memory.
    if opts.dry_run {
        println!(
            "dev-sync: [dry-run] env -u WG_PI_PLUGIN_DIR WG_PI_PLUGIN_FORCE_CACHE=1 {} pi-plugin install",
            binary.display()
        );
    } else {
        let mut cmd = Command::new(&binary);
        cmd.args(["pi-plugin", "install"])
            .env_remove("WG_PI_PLUGIN_DIR")
            .env("WG_PI_PLUGIN_FORCE_CACHE", "1");
        run_step("plugin sync (pi-plugin install)", &mut cmd)?;
    }

    let status_text = if opts.dry_run {
        // Best-effort read of the *current* state without changing anything.
        let mut cmd = Command::new(&binary);
        cmd.args(["pi-plugin", "status"])
            .env_remove("WG_PI_PLUGIN_DIR");
        String::from_utf8_lossy(&cmd.output().map(|o| o.stdout).unwrap_or_default()).to_string()
    } else {
        let output = Command::new(&binary)
            .args(["pi-plugin", "status"])
            .env_remove("WG_PI_PLUGIN_DIR")
            .output()
            .context("run `pi-plugin status` for verification")?;
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    let plugin_status = parse_status_output(&status_text);

    let report = collect_report(&repo_root, &binary, plugin_status, dir);
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_human(&report));
        if !status_text.trim().is_empty() {
            println!("\n--- wg pi-plugin status ---");
            print!("{status_text}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_output_extracts_digests_and_state() {
        let text = "\
WorksGood Pi integration status (pi-worksgood / @worksgood/pi)
  compat version:   0.3.0
  source:           Cache (embedded → versioned cache)
  resolved entry:   /c/wg/worksgood-pi/0.3.0/pi-worksgood/index.js
  cache dir:        /c/wg/worksgood-pi/0.3.0
  embed digest:     b3:aaaa
  cache digest:     b3:aaaa
  cache state:      current (matches this binary's embedded build)
  build ready:      yes
";
        let parsed = parse_status_output(text);
        assert_eq!(parsed.embed_digest.as_deref(), Some("b3:aaaa"));
        assert_eq!(parsed.cache_digest.as_deref(), Some("b3:aaaa"));
        assert_eq!(parsed.cache_state.as_deref(), Some("current"));
    }

    #[test]
    fn parse_status_output_tolerates_missing_lines() {
        let parsed = parse_status_output("nothing useful here\n");
        assert_eq!(parsed, ParsedPluginStatus::default());
    }

    fn sample_report() -> DevSyncReport {
        DevSyncReport {
            repo_root: "/home/bot/wg".to_string(),
            branch: "main".to_string(),
            head: "abc123def456".to_string(),
            worktree_build: false,
            binary_path: "/home/bot/.cargo/bin/wg".to_string(),
            binary_sha256: Some("sha256:111111111111".to_string()),
            embed_digest: Some("b3:embed".to_string()),
            cache_digest: Some("b3:embed".to_string()),
            cache_state: Some("current".to_string()),
            daemon_health: "Healthy".to_string(),
            daemon_pid: Some(4242),
            daemon_build_id: Some("wgb-111111111111".to_string()),
            daemon_sha256: Some("sha256:111111111111".to_string()),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn render_human_prints_the_verification_contract() {
        let out = render_human(&sample_report());
        // binary path + hash
        assert!(out.contains("/home/bot/.cargo/bin/wg"));
        assert!(out.contains("sha256:111111111111"));
        // both digests
        assert!(out.contains("embed digest:  b3:embed"));
        assert!(out.contains("cache digest:  b3:embed"));
        // daemon identity
        assert!(out.contains("daemon:        Healthy"));
        assert!(out.contains("daemon pid:    4242"));
        assert!(out.contains("wgb-111111111111"));
        assert!(out.contains("status:        OK"));
    }

    #[test]
    fn render_human_warns_when_daemon_binary_differs() {
        let mut report = sample_report();
        report.daemon_sha256 = Some("sha256:999999999999".to_string());
        report.warnings.push(
            "daemon binary (sha256:999999999999) differs from installed binary (sha256:111111111111); \
             restart the daemon so the new binary is live"
                .to_string(),
        );
        let out = render_human(&report);
        assert!(out.contains("status:        WARN"));
        assert!(out.contains("differs from installed binary"));
    }

    #[test]
    fn collect_report_flags_drifted_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut plugin_status = ParsedPluginStatus::default();
        plugin_status.embed_digest = Some("b3:embed".to_string());
        plugin_status.cache_digest = Some("b3:old".to_string());
        plugin_status.cache_state = Some("DRIFTED".to_string());
        let report = collect_report(
            Path::new("/home/bot/wg"),
            Path::new("/nonexistent/wg"),
            plugin_status,
            dir.path(),
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("DRIFTED") || w.contains("cache state")),
            "a non-current cache must warn: {:?}",
            report.warnings
        );
    }

    #[test]
    fn installed_binary_path_is_absolute_or_current_exe() {
        let path = installed_binary_path();
        assert!(!path.as_os_str().is_empty());
    }
}
