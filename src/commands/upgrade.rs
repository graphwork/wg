use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use walkdir::WalkDir;

use super::migrate::{self, ConfigMigrateTarget};
use super::secret_cmd;
use super::service::{self, ServiceState};

const STATE_NAME: &str = "upgrade-state.toml";
const DEFAULT_SOURCE_URL: &str = "https://github.com/graphwork/wg.git";
const DEFAULT_TARGET_REF: &str = "origin/main";
const DISK_WARNING_BYTES: u64 = 3 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct UpgradeArgs {
    pub dry_run: bool,
    pub yes: bool,
    pub source: Option<String>,
    pub target_ref: Option<String>,
    pub source_dir: Option<PathBuf>,
    pub clean: bool,
    pub rollback: bool,
    pub migrate_secrets: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct UpgradeState {
    created_at: String,
    install_dir: String,
    backup_dir: String,
    previous_version: String,
    new_version: Option<String>,
    source_dir: String,
    source_url: String,
    target_ref: String,
    target_commit: Option<String>,
}

#[derive(Debug, Clone)]
enum InstallSource {
    CargoInstall { path: PathBuf },
    DeveloperCheckout { path: PathBuf },
    Homebrew { path: PathBuf },
    Nix { path: PathBuf },
    SystemPackage { path: PathBuf },
    Unknown { path: PathBuf },
}

#[derive(Debug, Clone)]
struct ToolStatus {
    available: bool,
    detail: String,
}

#[derive(Debug, Clone)]
struct DaemonState {
    label: String,
    running: bool,
}

#[derive(Debug, Clone)]
struct BackupSet {
    binary_backup_dir: PathBuf,
    config_backup_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct DiskUsage {
    source_bytes: Option<u64>,
    target_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
enum CheckoutStatus {
    Missing,
    GitCheckout,
    NotGitCheckout,
}

#[derive(Debug, Clone)]
enum GraphLayoutCheck {
    LegacyWorkgraphWillCopy { from: PathBuf, to: PathBuf },
    LegacyWorkgraphPresent { path: PathBuf },
    CanonicalWgPresent { path: PathBuf },
    GraphPresent { path: PathBuf },
    GraphMissing { path: PathBuf },
    StaleActiveProfile { profile: String, expected: PathBuf },
    ActiveProfileOk { profile: String },
    NoActiveProfile,
}

pub fn run(workgraph_dir: &Path, args: UpgradeArgs, _json: bool) -> Result<()> {
    let home_wg = home_wg_dir()?;
    let current_exe = env::current_exe()
        .context("failed to locate current wg executable")?
        .canonicalize()
        .context("failed to canonicalize current wg executable")?;

    if args.rollback {
        return run_rollback(workgraph_dir, &home_wg, &current_exe, &args);
    }

    let install_source = detect_install_source(&current_exe);
    if refused_install_source(&install_source) {
        print_non_source_managed_refusal(&install_source);
        anyhow::bail!("{}", refusal_message(&install_source));
    }

    let source_url = resolve_source_url(&args);
    let target_ref = resolve_target_ref(&args);
    let source_dir = resolve_source_dir(&home_wg, &args);
    let install_dir = cargo_bin_dir()?;
    let checkout_status = checkout_status(&source_dir);
    let cargo = probe_tool("cargo");
    let git = probe_tool("git");
    let daemon = detect_daemon_state(workgraph_dir)?;
    let graph_checks = collect_graph_layout_checks(workgraph_dir, &home_wg);
    let disk_usage = collect_disk_usage(&source_dir);
    let target_version = read_checkout_package_version(&source_dir).ok().flatten();
    let target_commit = git_rev_parse_short(&source_dir, "HEAD").ok().flatten();

    print_upgrade_plan(
        &current_exe,
        &install_source,
        &source_url,
        &target_ref,
        &source_dir,
        &install_dir,
        &checkout_status,
        &cargo,
        &git,
        &daemon,
        &disk_usage,
        target_version.as_deref(),
        target_commit.as_deref(),
        args.clean,
        args.dry_run,
    );
    print_migration_preflight(workgraph_dir, &graph_checks)?;

    if args.dry_run {
        println!();
        println!("Dry run complete. No files were changed.");
        println!("Rollback after a real upgrade: wg upgrade --rollback");
        return Ok(());
    }

    ensure_tool_available("git", &git)?;
    ensure_tool_available("cargo", &cargo)?;

    confirm_or_bail(args.yes, "Proceed with source-managed wg upgrade?")?;

    sync_source_checkout(&source_dir, &source_url, &target_ref, &checkout_status)?;

    let refreshed_version = read_checkout_package_version(&source_dir).ok().flatten();
    let refreshed_commit = git_rev_parse_short(&source_dir, "HEAD").ok().flatten();
    println!();
    println!("Resolved source checkout:");
    println!("  path: {}", source_dir.display());
    println!("  ref: {}", target_ref);
    println!(
        "  commit: {}",
        refreshed_commit.as_deref().unwrap_or("unknown")
    );
    println!(
        "  package version: {}",
        refreshed_version.as_deref().unwrap_or("unknown")
    );

    let backups = create_backups(&home_wg, &install_dir, &current_exe, workgraph_dir)?;
    println!();
    println!("Backups:");
    println!("  binaries: {}", backups.binary_backup_dir.display());
    println!("  configs:  {}", backups.config_backup_dir.display());

    if args.clean {
        run_cargo_clean(&source_dir)?;
    }

    run_cargo_install(&source_dir)?;

    apply_migrations(workgraph_dir, &graph_checks, args.migrate_secrets)?;

    write_toml(
        &home_wg.join(STATE_NAME),
        &UpgradeState {
            created_at: Utc::now().to_rfc3339(),
            install_dir: install_dir.display().to_string(),
            backup_dir: backups.binary_backup_dir.display().to_string(),
            previous_version: env!("CARGO_PKG_VERSION").to_string(),
            new_version: refreshed_version.clone(),
            source_dir: source_dir.display().to_string(),
            source_url: source_url.clone(),
            target_ref: target_ref.clone(),
            target_commit: refreshed_commit.clone(),
        },
    )?;

    if daemon.running {
        println!();
        println!("Restarting daemon so it uses the upgraded wg binary.");
        service::run_restart(workgraph_dir, false)?;
    } else {
        println!();
        println!("Daemon was not running; no restart needed.");
    }

    print_validation(&install_dir, workgraph_dir);
    print_disk_usage(
        &collect_disk_usage(&source_dir),
        &source_dir,
        "Disk usage after upgrade",
    );
    println!();
    println!(
        "Upgrade complete: {} -> {}",
        env!("CARGO_PKG_VERSION"),
        refreshed_version.as_deref().unwrap_or("source checkout")
    );
    println!("Rollback command: wg upgrade --rollback");
    println!("Rollback source: {}", backups.binary_backup_dir.display());
    Ok(())
}

fn run_rollback(
    workgraph_dir: &Path,
    home_wg: &Path,
    current_exe: &Path,
    args: &UpgradeArgs,
) -> Result<()> {
    let state_path = home_wg.join(STATE_NAME);
    let state = read_toml::<UpgradeState>(&state_path).ok();
    let install_dir = state
        .as_ref()
        .map(|s| PathBuf::from(&s.install_dir))
        .or_else(|| cargo_bin_dir().ok())
        .or_else(|| current_exe.parent().map(Path::to_path_buf))
        .context("could not determine install directory for rollback")?;
    let backup_dir = if let Some(state) = state.as_ref() {
        Some(PathBuf::from(&state.backup_dir))
    } else {
        latest_backup_dir(&home_wg.join("backups").join("bin"))?
    }
    .context("no rollback backup found under ~/.wg/backups/bin")?;

    if !backup_dir.join(binary_name("wg")).exists() {
        anyhow::bail!(
            "rollback backup {} does not contain {}",
            backup_dir.display(),
            binary_name("wg")
        );
    }

    let daemon = detect_daemon_state(workgraph_dir)?;
    println!("WG rollback");
    println!("  current wg: {}", current_exe.display());
    println!("  install dir: {}", install_dir.display());
    println!("  backup dir: {}", backup_dir.display());
    println!("  daemon: {}", daemon.label);
    if let Some(state) = &state {
        println!(
            "  version: {} -> {}",
            state.new_version.as_deref().unwrap_or("source checkout"),
            state.previous_version
        );
    }

    if args.dry_run {
        println!("Dry run complete. No files were changed.");
        return Ok(());
    }

    confirm_or_bail(args.yes, "Restore the previous wg/nex binaries?")?;
    restore_binaries(&backup_dir, &install_dir)?;

    if daemon.running {
        println!("Restarting daemon with restored wg binary.");
        service::run_restart(workgraph_dir, false)?;
    }

    println!("Rollback complete.");
    println!("Restored binaries from {}", backup_dir.display());
    Ok(())
}

fn detect_install_source(current_exe: &Path) -> InstallSource {
    if looks_like_homebrew(current_exe) {
        return InstallSource::Homebrew {
            path: current_exe.to_path_buf(),
        };
    }
    if looks_like_cargo_install(current_exe) {
        return InstallSource::CargoInstall {
            path: current_exe.to_path_buf(),
        };
    }
    if current_exe.starts_with("/nix/store") {
        return InstallSource::Nix {
            path: current_exe.to_path_buf(),
        };
    }
    if looks_like_system_package(current_exe) {
        return InstallSource::SystemPackage {
            path: current_exe.to_path_buf(),
        };
    }
    if looks_like_developer_checkout(current_exe) {
        return InstallSource::DeveloperCheckout {
            path: current_exe.to_path_buf(),
        };
    }
    InstallSource::Unknown {
        path: current_exe.to_path_buf(),
    }
}

fn refused_install_source(source: &InstallSource) -> bool {
    matches!(
        source,
        InstallSource::Homebrew { .. }
            | InstallSource::Nix { .. }
            | InstallSource::SystemPackage { .. }
    )
}

fn looks_like_homebrew(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/Cellar/wg/")
        || s.contains("/Cellar/workgraph/")
        || s.starts_with("/opt/homebrew/Cellar/")
        || s.starts_with("/usr/local/Cellar/")
}

fn looks_like_cargo_install(path: &Path) -> bool {
    if let Some(cargo_home) = env::var_os("CARGO_HOME")
        && path.starts_with(PathBuf::from(cargo_home).join("bin"))
    {
        return true;
    }
    dirs::home_dir()
        .map(|home| path.starts_with(home.join(".cargo").join("bin")))
        .unwrap_or(false)
}

fn looks_like_system_package(path: &Path) -> bool {
    path.starts_with("/usr/bin") || path.starts_with("/bin") || path.starts_with("/usr/sbin")
}

fn looks_like_developer_checkout(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if !(s.contains("/target/debug/") || s.contains("/target/release/")) {
        return false;
    }
    path.ancestors().any(|ancestor| {
        ancestor.join("Cargo.toml").is_file() && ancestor.join("src/main.rs").is_file()
    })
}

fn print_non_source_managed_refusal(source: &InstallSource) {
    println!("WG upgrade");
    println!("  current version: {}", env!("CARGO_PKG_VERSION"));
    println!("  install source: {}", install_source_label(source));
    println!("  path: {}", install_source_path(source).display());
    println!();
    println!("Refusing to replace a package-manager-owned install.");
    println!("{}", owner_command(source));
}

fn refusal_message(source: &InstallSource) -> String {
    format!(
        "wg upgrade source-managed mode refuses {}; use the owning package manager",
        install_source_label(source)
    )
}

fn install_source_label(source: &InstallSource) -> &'static str {
    match source {
        InstallSource::CargoInstall { .. } => "Cargo install",
        InstallSource::DeveloperCheckout { .. } => "developer checkout",
        InstallSource::Homebrew { .. } => "Homebrew",
        InstallSource::Nix { .. } => "Nix",
        InstallSource::SystemPackage { .. } => "system package manager",
        InstallSource::Unknown { .. } => "unknown copied binary",
    }
}

fn install_source_path(source: &InstallSource) -> &Path {
    match source {
        InstallSource::CargoInstall { path }
        | InstallSource::DeveloperCheckout { path }
        | InstallSource::Homebrew { path }
        | InstallSource::Nix { path }
        | InstallSource::SystemPackage { path }
        | InstallSource::Unknown { path } => path,
    }
}

fn owner_command(source: &InstallSource) -> &'static str {
    match source {
        InstallSource::Homebrew { .. } => "Use: brew upgrade graphwork/tap/wg",
        InstallSource::Nix { .. } => "Use: nix profile upgrade wg",
        InstallSource::SystemPackage { .. } => {
            "Use your OS package manager, for example: apt upgrade wg or dnf upgrade wg"
        }
        InstallSource::CargoInstall { .. }
        | InstallSource::DeveloperCheckout { .. }
        | InstallSource::Unknown { .. } => "Use: wg upgrade",
    }
}

fn resolve_source_url(args: &UpgradeArgs) -> String {
    args.source
        .clone()
        .or_else(|| env::var("WG_UPGRADE_SOURCE_URL").ok())
        .unwrap_or_else(|| DEFAULT_SOURCE_URL.to_string())
}

fn resolve_target_ref(args: &UpgradeArgs) -> String {
    args.target_ref
        .clone()
        .or_else(|| env::var("WG_UPGRADE_REF").ok())
        .unwrap_or_else(|| DEFAULT_TARGET_REF.to_string())
}

fn resolve_source_dir(home_wg: &Path, args: &UpgradeArgs) -> PathBuf {
    args.source_dir
        .clone()
        .or_else(|| env::var_os("WG_UPGRADE_SOURCE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home_wg.join("source").join("wg"))
}

fn cargo_bin_dir() -> Result<PathBuf> {
    if let Some(cargo_home) = env::var_os("CARGO_HOME") {
        return Ok(PathBuf::from(cargo_home).join("bin"));
    }
    dirs::home_dir()
        .map(|home| home.join(".cargo").join("bin"))
        .context("could not determine Cargo bin directory; set CARGO_HOME or HOME")
}

fn checkout_status(source_dir: &Path) -> CheckoutStatus {
    if !source_dir.exists() {
        CheckoutStatus::Missing
    } else if source_dir.join(".git").exists() {
        CheckoutStatus::GitCheckout
    } else {
        CheckoutStatus::NotGitCheckout
    }
}

fn probe_tool(name: &str) -> ToolStatus {
    match Command::new(name).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let first = stdout
                .lines()
                .chain(stderr.lines())
                .find(|line| !line.trim().is_empty())
                .unwrap_or("available")
                .trim()
                .to_string();
            ToolStatus {
                available: true,
                detail: first,
            }
        }
        Ok(output) => ToolStatus {
            available: false,
            detail: format!(
                "{} --version exited with {}: {}",
                name,
                output.status,
                compact_output(&output, 20, 2048)
            ),
        },
        Err(err) => ToolStatus {
            available: false,
            detail: format!("{} not found in PATH: {}", name, err),
        },
    }
}

fn ensure_tool_available(name: &str, status: &ToolStatus) -> Result<()> {
    if status.available {
        return Ok(());
    }
    let next_step = match name {
        "cargo" => {
            "install Rust/Cargo (for example with rustup) or make cargo available in PATH, then rerun `wg upgrade`"
        }
        "git" => "install git or make git available in PATH, then rerun `wg upgrade`",
        _ => "install the missing tool and rerun `wg upgrade`",
    };
    anyhow::bail!(
        "preflight phase failed: required tool `{}` is unavailable: {}. Suggested next step: {}",
        name,
        status.detail,
        next_step
    )
}

fn print_upgrade_plan(
    current_exe: &Path,
    install_source: &InstallSource,
    source_url: &str,
    target_ref: &str,
    source_dir: &Path,
    install_dir: &Path,
    checkout_status: &CheckoutStatus,
    cargo: &ToolStatus,
    git: &ToolStatus,
    daemon: &DaemonState,
    disk_usage: &DiskUsage,
    target_version: Option<&str>,
    target_commit: Option<&str>,
    clean: bool,
    dry_run: bool,
) {
    println!("WG upgrade{}", if dry_run { " (dry run)" } else { "" });
    println!("  current wg: {}", current_exe.display());
    println!("  current version: {}", env!("CARGO_PKG_VERSION"));
    println!("  install source: {}", install_source_label(install_source));
    println!(
        "  install path: {}",
        install_source_path(install_source).display()
    );
    println!("  install target: {}", install_dir.display());
    println!("  source path: {}", source_dir.display());
    println!("  source upstream: {}", source_url);
    println!("  target ref: {}", target_ref);
    println!("  target channel: source/{}", target_ref);
    println!(
        "  target version: {}",
        target_version.unwrap_or("unknown until source checkout is cloned/fetched")
    );
    println!("  target commit: {}", target_commit.unwrap_or("unknown"));
    println!("  daemon: {}", daemon.label);
    println!("  cargo: {}", cargo.detail);
    println!("  git: {}", git.detail);
    println!();
    println!("Source checkout:");
    match checkout_status {
        CheckoutStatus::Missing => {
            println!(
                "  - missing; planned action: git clone {} {}",
                source_url,
                source_dir.display()
            );
        }
        CheckoutStatus::GitCheckout => {
            println!(
                "  - present; planned action: git fetch origin, then checkout {}",
                target_ref
            );
        }
        CheckoutStatus::NotGitCheckout => {
            println!(
                "  - invalid: path exists but is not a git checkout; real upgrade will refuse it"
            );
        }
    }
    println!();
    println!("Build/install plan:");
    if clean {
        println!("  command: cd {} && cargo clean", source_dir.display());
    } else {
        println!(
            "  clean: not requested; use `wg upgrade --clean` to reclaim target/cache space before rebuilding"
        );
    }
    println!(
        "  command: cd {} && cargo install --path . --locked",
        source_dir.display()
    );
    if daemon.running {
        println!("  daemon action: restart after successful install");
    } else {
        println!("  daemon action: none; daemon is not running");
    }
    print_disk_usage(disk_usage, source_dir, "Disk usage");
}

fn print_disk_usage(usage: &DiskUsage, source_dir: &Path, title: &str) {
    println!();
    println!("{}:", title);
    println!(
        "  managed source: {} ({})",
        source_dir.display(),
        format_optional_bytes(usage.source_bytes)
    );
    println!(
        "  cargo target/cache: {} ({})",
        source_dir.join("target").display(),
        format_optional_bytes(usage.target_bytes)
    );
    if let Some(bytes) = usage.source_bytes
        && bytes >= DISK_WARNING_BYTES
    {
        println!(
            "  hint: managed source plus build outputs are over {}; run `wg upgrade --clean` to reclaim space at the cost of a slower rebuild",
            format_bytes(DISK_WARNING_BYTES)
        );
    }
}

fn print_migration_preflight(
    workgraph_dir: &Path,
    graph_checks: &[GraphLayoutCheck],
) -> Result<()> {
    println!();
    println!("Migration plan:");
    println!("  command: wg config lint");
    if let Err(err) = crate::commands::config_cmd::lint_config(
        workgraph_dir,
        crate::commands::config_cmd::LintTarget::Merged,
        false,
    ) {
        println!("    warning: {}", err);
    }
    println!("  command: wg migrate config --dry-run (global + local)");
    migrate::run_config_migrate(workgraph_dir, ConfigMigrateTarget::All, true, false)?;
    println!("  command: wg migrate secrets --dry-run");
    secret_cmd::run_migrate_secrets(workgraph_dir, true, false, false, false)?;
    println!("  command: graph-layout/profile/default checks");
    for check in graph_checks {
        println!("    {}", graph_check_message(check));
    }
    Ok(())
}

fn apply_migrations(
    workgraph_dir: &Path,
    graph_checks: &[GraphLayoutCheck],
    migrate_secrets: bool,
) -> Result<()> {
    println!();
    println!("Applying migrations:");
    println!("  command: wg migrate config --all");
    migrate::run_config_migrate(workgraph_dir, ConfigMigrateTarget::All, false, false)?;
    if migrate_secrets {
        println!("  command: wg migrate secrets");
        secret_cmd::run_migrate_secrets(workgraph_dir, false, false, false, false)?;
    } else {
        println!(
            "  offered: wg migrate secrets (rerun with `wg upgrade --migrate-secrets` to rewrite api_key_env entries now)"
        );
    }
    apply_graph_layout_migrations(graph_checks)?;
    println!("  profile/default checks complete");
    Ok(())
}

fn sync_source_checkout(
    source_dir: &Path,
    source_url: &str,
    target_ref: &str,
    checkout_status: &CheckoutStatus,
) -> Result<()> {
    println!();
    println!("Syncing source checkout:");
    match checkout_status {
        CheckoutStatus::Missing => {
            if let Some(parent) = source_dir.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            run_phase_command(
                "git clone",
                Command::new("git")
                    .arg("clone")
                    .arg("--")
                    .arg(source_url)
                    .arg(source_dir),
            )?;
        }
        CheckoutStatus::GitCheckout => {
            if let Some(origin) = git_remote_url(source_dir, "origin").ok().flatten()
                && origin != source_url
            {
                println!(
                    "  origin currently points at {}; using existing checkout but updating origin to {}",
                    origin, source_url
                );
                run_phase_command(
                    "git remote set-url",
                    Command::new("git")
                        .current_dir(source_dir)
                        .arg("remote")
                        .arg("set-url")
                        .arg("origin")
                        .arg(source_url),
                )?;
            }
        }
        CheckoutStatus::NotGitCheckout => {
            anyhow::bail!(
                "source sync phase failed: managed source path {} exists but is not a git checkout. Move it aside or set --source-dir to a clean path.",
                source_dir.display()
            );
        }
    }

    run_phase_command(
        "git fetch",
        Command::new("git")
            .current_dir(source_dir)
            .arg("fetch")
            .arg("--prune")
            .arg("origin"),
    )?;
    run_phase_command(
        "git checkout",
        Command::new("git")
            .current_dir(source_dir)
            .arg("checkout")
            .arg("--force")
            .arg(target_ref),
    )?;
    run_phase_command(
        "git reset",
        Command::new("git")
            .current_dir(source_dir)
            .arg("reset")
            .arg("--hard")
            .arg(target_ref),
    )?;
    Ok(())
}

fn run_cargo_clean(source_dir: &Path) -> Result<()> {
    println!();
    println!("Running cargo clean before build:");
    run_phase_command(
        "cargo clean",
        Command::new("cargo").current_dir(source_dir).arg("clean"),
    )
    .map(|_| ())
    .map_err(|err| {
        anyhow::anyhow!(
            "cargo clean phase failed separately from build/install: {:#}",
            err
        )
    })
}

fn run_cargo_install(source_dir: &Path) -> Result<()> {
    println!();
    println!("Running cargo install:");
    run_phase_command(
        "cargo install",
        Command::new("cargo")
            .current_dir(source_dir)
            .arg("install")
            .arg("--path")
            .arg(".")
            .arg("--locked"),
    )
    .map(|_| ())
}

fn run_phase_command(phase: &str, command: &mut Command) -> Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("{} phase failed to start", phase))?;
    if !output.status.success() {
        anyhow::bail!(
            "{} phase failed with {}\nSuggested next step: verify prerequisites and rerun `wg upgrade`; if the command keeps failing, file a bug with this phase name and the output below.\n{}",
            phase,
            output.status,
            compact_output(&output, 80, 12 * 1024)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(8)
    {
        println!("  {}", line);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(8)
    {
        println!("  {}", line);
    }
    Ok(output)
}

fn collect_graph_layout_checks(workgraph_dir: &Path, home_wg: &Path) -> Vec<GraphLayoutCheck> {
    let mut checks = Vec::new();
    let project_root = workgraph_dir.parent().unwrap_or_else(|| Path::new("."));
    let canonical = project_root.join(".wg");
    let legacy = project_root.join(".workgraph");

    if canonical.is_dir() {
        checks.push(GraphLayoutCheck::CanonicalWgPresent { path: canonical });
    }
    if legacy.is_dir() && !project_root.join(".wg").is_dir() {
        checks.push(GraphLayoutCheck::LegacyWorkgraphWillCopy {
            from: legacy,
            to: project_root.join(".wg"),
        });
    } else if legacy.is_dir() {
        checks.push(GraphLayoutCheck::LegacyWorkgraphPresent { path: legacy });
    }

    let graph_path = workgraph_dir.join("graph.jsonl");
    if graph_path.exists() {
        checks.push(GraphLayoutCheck::GraphPresent { path: graph_path });
    } else {
        checks.push(GraphLayoutCheck::GraphMissing { path: graph_path });
    }

    let active_profile = home_wg.join("active-profile");
    if let Ok(profile) = fs::read_to_string(&active_profile) {
        let profile = profile.trim().to_string();
        let expected = home_wg.join("profiles").join(format!("{}.toml", profile));
        if expected.exists() {
            checks.push(GraphLayoutCheck::ActiveProfileOk { profile });
        } else {
            checks.push(GraphLayoutCheck::StaleActiveProfile { profile, expected });
        }
    } else {
        checks.push(GraphLayoutCheck::NoActiveProfile);
    }

    checks
}

fn graph_check_message(check: &GraphLayoutCheck) -> String {
    match check {
        GraphLayoutCheck::LegacyWorkgraphWillCopy { from, to } => format!(
            "legacy .workgraph layout found: would copy {} to {}",
            from.display(),
            to.display()
        ),
        GraphLayoutCheck::LegacyWorkgraphPresent { path } => {
            format!(
                "legacy .workgraph present alongside .wg: {}",
                path.display()
            )
        }
        GraphLayoutCheck::CanonicalWgPresent { path } => {
            format!("canonical .wg present: {}", path.display())
        }
        GraphLayoutCheck::GraphPresent { path } => {
            format!("graph layout marker OK: {}", path.display())
        }
        GraphLayoutCheck::GraphMissing { path } => {
            format!("graph.jsonl missing: {}", path.display())
        }
        GraphLayoutCheck::StaleActiveProfile { profile, expected } => format!(
            "stale active profile {:?}: missing {}",
            profile,
            expected.display()
        ),
        GraphLayoutCheck::ActiveProfileOk { profile } => {
            format!("active profile {:?} exists", profile)
        }
        GraphLayoutCheck::NoActiveProfile => "no active profile set".to_string(),
    }
}

fn apply_graph_layout_migrations(checks: &[GraphLayoutCheck]) -> Result<()> {
    for check in checks {
        if let GraphLayoutCheck::LegacyWorkgraphWillCopy { from, to } = check {
            let backup = from.with_file_name(format!(
                ".workgraph.backup.{}",
                Utc::now().format("%Y%m%dT%H%M%SZ")
            ));
            println!(
                "  graph-layout: copying {} to {}",
                from.display(),
                to.display()
            );
            copy_dir_recursive(from, to)?;
            println!(
                "  graph-layout: preserving timestamped backup at {}",
                backup.display()
            );
            copy_dir_recursive(from, &backup)?;
        }
    }
    Ok(())
}

fn create_backups(
    home_wg: &Path,
    install_dir: &Path,
    current_exe: &Path,
    workgraph_dir: &Path,
) -> Result<BackupSet> {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let binary_backup_dir = home_wg.join("backups").join("bin").join(&timestamp);
    let config_backup_dir = home_wg.join("backups").join("config").join(&timestamp);
    fs::create_dir_all(&binary_backup_dir)?;
    fs::create_dir_all(&config_backup_dir)?;

    for bin in ["wg", "nex"] {
        let name = binary_name(bin);
        let src = install_dir.join(&name);
        if src.exists() {
            copy_preserving_permissions(&src, &binary_backup_dir.join(&name))?;
        }
    }

    if !current_exe.starts_with(install_dir) && current_exe.exists() {
        copy_preserving_permissions(current_exe, &binary_backup_dir.join("current-wg"))?;
    }

    copy_if_exists(
        &home_wg.join("config.toml"),
        &config_backup_dir.join("global-config.toml"),
    )?;
    copy_if_exists(
        &workgraph_dir.join("config.toml"),
        &config_backup_dir.join("project-config.toml"),
    )?;
    copy_if_exists(
        &home_wg.join(STATE_NAME),
        &config_backup_dir.join(STATE_NAME),
    )?;

    Ok(BackupSet {
        binary_backup_dir,
        config_backup_dir,
    })
}

fn restore_binaries(backup_dir: &Path, install_dir: &Path) -> Result<()> {
    fs::create_dir_all(install_dir)?;
    for bin in ["wg", "nex"] {
        let name = binary_name(bin);
        let src = backup_dir.join(&name);
        if src.exists() {
            let dest = install_dir.join(&name);
            copy_atomic(&src, &dest)?;
            println!("Restored {}", dest.display());
        }
    }
    Ok(())
}

fn copy_atomic(src: &Path, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .with_context(|| format!("{} has no parent directory", dest.display()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.upgrade-{}",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("binary"),
        std::process::id()
    ));
    fs::copy(src, &tmp).with_context(|| {
        format!(
            "failed to copy {} to temporary path {}",
            src.display(),
            tmp.display()
        )
    })?;
    copy_permissions(src, &tmp)?;
    fs::rename(&tmp, dest).or_else(|rename_err| {
        if dest.exists() {
            let _ = fs::remove_file(dest);
            fs::rename(&tmp, dest).map_err(|second_err| {
                anyhow::anyhow!(
                    "failed to replace {}: {}; retry after removing {} failed: {}",
                    dest.display(),
                    rename_err,
                    dest.display(),
                    second_err
                )
            })
        } else {
            Err(anyhow::anyhow!(
                "failed to move {} to {}: {}",
                tmp.display(),
                dest.display(),
                rename_err
            ))
        }
    })?;
    Ok(())
}

fn print_validation(install_dir: &Path, workgraph_dir: &Path) {
    println!();
    println!("Validation:");
    print_command_output(&install_dir.join(binary_name("wg")), &["--version"]);
    print_command_output(&install_dir.join(binary_name("nex")), &["--version"]);
    println!("  wg config lint:");
    if let Err(err) = crate::commands::config_cmd::lint_config(
        workgraph_dir,
        crate::commands::config_cmd::LintTarget::Merged,
        false,
    ) {
        println!("    warning: {}", err);
    }
    println!("  wg service status:");
    match detect_daemon_state(workgraph_dir) {
        Ok(state) => println!("    {}", state.label),
        Err(err) => println!("    warning: {}", err),
    }
}

fn print_command_output(program: &Path, args: &[&str]) {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("  {} {}", program.display(), stdout.trim());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!(
                "  warning: {} {:?} exited with {}: {}",
                program.display(),
                args,
                output.status,
                stderr.trim()
            );
        }
        Err(err) => println!(
            "  warning: failed to run {} {:?}: {}",
            program.display(),
            args,
            err
        ),
    }
}

fn detect_daemon_state(workgraph_dir: &Path) -> Result<DaemonState> {
    match ServiceState::load(workgraph_dir)? {
        Some(state) => {
            if super::is_process_alive(state.pid) {
                Ok(DaemonState {
                    label: format!("running (PID {})", state.pid),
                    running: true,
                })
            } else {
                Ok(DaemonState {
                    label: format!("not running (stale PID {})", state.pid),
                    running: false,
                })
            }
        }
        None => Ok(DaemonState {
            label: "not running".to_string(),
            running: false,
        }),
    }
}

fn collect_disk_usage(source_dir: &Path) -> DiskUsage {
    DiskUsage {
        source_bytes: dir_size(source_dir).ok(),
        target_bytes: dir_size(&source_dir.join("target")).ok(),
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn format_optional_bytes(bytes: Option<u64>) -> String {
    bytes
        .map(format_bytes)
        .unwrap_or_else(|| "unavailable".to_string())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

fn read_checkout_package_version(source_dir: &Path) -> Result<Option<String>> {
    let cargo_toml = source_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&cargo_toml)
        .with_context(|| format!("failed to read {}", cargo_toml.display()))?;
    let value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", cargo_toml.display()))?;
    Ok(value
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .map(std::string::ToString::to_string))
}

fn git_rev_parse_short(source_dir: &Path, rev: &str) -> Result<Option<String>> {
    if !source_dir.join(".git").exists() {
        return Ok(None);
    }
    let output = Command::new("git")
        .current_dir(source_dir)
        .arg("rev-parse")
        .arg("--short")
        .arg(rev)
        .output()
        .with_context(|| format!("failed to run git rev-parse in {}", source_dir.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn git_remote_url(source_dir: &Path, remote: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(source_dir)
        .arg("remote")
        .arg("get-url")
        .arg(remote)
        .output()
        .with_context(|| format!("failed to inspect git remote in {}", source_dir.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn compact_output(output: &Output, max_lines: usize, max_bytes: usize) -> String {
    let mut combined = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        combined.push_str("--- stdout ---\n");
        combined.push_str(&stdout);
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
    }
    if !stderr.trim().is_empty() {
        combined.push_str("--- stderr ---\n");
        combined.push_str(&stderr);
        if !combined.ends_with('\n') {
            combined.push('\n');
        }
    }
    if combined.is_empty() {
        return "(command produced no output)".to_string();
    }

    let lines: Vec<&str> = combined.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut tail = lines[start..].join("\n");
    if start > 0 {
        tail = format!(
            "(showing last {} of {} output lines)\n{}",
            max_lines,
            lines.len(),
            tail
        );
    }
    if tail.len() > max_bytes {
        let keep_from = tail.len().saturating_sub(max_bytes);
        tail = format!(
            "(showing last {} bytes of command output)\n{}",
            max_bytes,
            &tail[keep_from..]
        );
    }
    tail
}

fn confirm_or_bail(yes: bool, prompt: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("confirmation required; rerun with --yes to proceed");
    }
    print!("{} [y/N] ", prompt);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    if matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        anyhow::bail!("upgrade cancelled")
    }
}

fn binary_name(stem: &str) -> String {
    format!("{}{}", stem, env::consts::EXE_SUFFIX)
}

fn home_wg_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".wg"))
        .context("could not determine home directory")
}

fn latest_backup_dir(root: &Path) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    Ok(entries.pop())
}

fn copy_if_exists(src: &Path, dest: &Path) -> Result<()> {
    if src.exists() {
        fs::copy(src, dest)
            .with_context(|| format!("failed to copy {} to {}", src.display(), dest.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    for entry in WalkDir::new(src) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
            copy_permissions(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_preserving_permissions(src: &Path, dest: &Path) -> Result<()> {
    fs::copy(src, dest)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dest.display()))?;
    copy_permissions(src, dest)
}

fn copy_permissions(src: &Path, dest: &Path) -> Result<()> {
    let permissions = fs::metadata(src)?.permissions();
    fs::set_permissions(dest, permissions)?;
    Ok(())
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}
