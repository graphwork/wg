use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
struct DevCheck {
    repo_root: PathBuf,
    branch: String,
    current_head: String,
    main_head: String,
    main_head_time: Option<SystemTime>,
    binary_path: PathBuf,
    binary_mtime: Option<SystemTime>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DevCheckJson {
    repo_root: String,
    branch: String,
    current_head: String,
    main_head: String,
    main_head_time: Option<String>,
    binary_path: String,
    binary_mtime: Option<String>,
    ok: bool,
    warnings: Vec<String>,
}

pub fn run(json: bool) -> Result<()> {
    let repo_root = git_output(&["rev-parse", "--show-toplevel"], None)
        .context("not inside a git repository")?;
    let repo_root = PathBuf::from(repo_root);
    let check = collect_dev_check(&repo_root, std::env::current_exe().ok())?;

    if json {
        print_json(&check)?;
    } else {
        print_human(&check);
    }

    Ok(())
}

fn collect_dev_check(repo_root: &Path, binary_path: Option<PathBuf>) -> Result<DevCheck> {
    let branch = git_output(&["branch", "--show-current"], Some(repo_root))
        .unwrap_or_else(|_| "HEAD".to_string());
    let branch = if branch.is_empty() {
        "HEAD".to_string()
    } else {
        branch
    };
    let current_head = git_output(&["rev-parse", "--short=12", "HEAD"], Some(repo_root))
        .unwrap_or_else(|_| "unknown".to_string());
    let main_head = git_output(&["rev-parse", "--short=12", "main"], Some(repo_root))
        .unwrap_or_else(|_| "unknown".to_string());
    let main_head_time = git_output(&["log", "-1", "--format=%ct", "main"], Some(repo_root))
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));

    let binary_path = binary_path.unwrap_or_else(|| PathBuf::from("unknown"));
    let binary_mtime = std::fs::metadata(&binary_path)
        .ok()
        .and_then(|meta| meta.modified().ok());

    let warnings = evaluate_warnings(&branch, main_head_time, binary_mtime);

    Ok(DevCheck {
        repo_root: repo_root.to_path_buf(),
        branch,
        current_head,
        main_head,
        main_head_time,
        binary_path,
        binary_mtime,
        warnings,
    })
}

fn evaluate_warnings(
    branch: &str,
    main_head_time: Option<SystemTime>,
    binary_mtime: Option<SystemTime>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if branch != "main" {
        warnings.push(format!(
            "current branch is '{branch}', not 'main'; `cargo install --path .` would build from this checkout"
        ));
    }

    match (binary_mtime, main_head_time) {
        (Some(binary_mtime), Some(main_head_time)) if binary_mtime < main_head_time => {
            warnings.push(
                "current wg binary is older than local main HEAD; run `cargo install --path .` from main"
                    .to_string(),
            );
        }
        (None, _) => warnings.push("could not read current wg binary mtime".to_string()),
        (_, None) => warnings.push("could not read local main HEAD timestamp".to_string()),
        _ => {}
    }

    warnings
}

fn print_human(check: &DevCheck) {
    println!("WG dev check");
    println!("  repo: {}", check.repo_root.display());
    println!("  branch: {}", check.branch);
    println!("  current HEAD: {}", check.current_head);
    println!(
        "  main HEAD: {} ({})",
        check.main_head,
        format_system_time(check.main_head_time)
    );
    println!(
        "  wg binary: {} ({})",
        check.binary_path.display(),
        format_system_time(check.binary_mtime)
    );

    if check.warnings.is_empty() {
        println!("  status: OK");
    } else {
        println!("  status: WARN");
        for warning in &check.warnings {
            println!("  warning: {warning}");
        }
    }
}

fn print_json(check: &DevCheck) -> Result<()> {
    let payload = DevCheckJson {
        repo_root: check.repo_root.display().to_string(),
        branch: check.branch.clone(),
        current_head: check.current_head.clone(),
        main_head: check.main_head.clone(),
        main_head_time: check.main_head_time.map(format_system_time_value),
        binary_path: check.binary_path.display().to_string(),
        binary_mtime: check.binary_mtime.map(format_system_time_value),
        ok: check.warnings.is_empty(),
        warnings: check.warnings.clone(),
    };
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn format_system_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time_value)
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_system_time_value(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn git_output(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_on_non_main_branch() {
        let warnings = evaluate_warnings("wg/agent-1398/fix-tui-perf", None, None);
        assert!(warnings.iter().any(|w| w.contains("not 'main'")));
    }

    #[test]
    fn warns_when_binary_is_older_than_main_head() {
        let main_head_time = UNIX_EPOCH + Duration::from_secs(200);
        let binary_mtime = UNIX_EPOCH + Duration::from_secs(100);
        let warnings = evaluate_warnings("main", Some(main_head_time), Some(binary_mtime));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("older than local main HEAD"))
        );
    }

    #[test]
    fn green_on_main_with_current_binary() {
        let main_head_time = UNIX_EPOCH + Duration::from_secs(100);
        let binary_mtime = UNIX_EPOCH + Duration::from_secs(200);
        let warnings = evaluate_warnings("main", Some(main_head_time), Some(binary_mtime));
        assert!(warnings.is_empty());
    }
}
