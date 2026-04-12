//! Manual cleanup commands for edge case recovery.
//!
//! Provides commands to manually clean up orphaned worktrees, recovery branches,
//! and other edge cases that may not be handled by automatic cleanup operations.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::{load_workgraph};
use crate::commands::service::worktree::{WORKTREES_DIR, remove_worktree, verify_worktree_cleanup};

/// Manual cleanup commands for edge case recovery
#[derive(Parser, Debug)]
pub struct CleanupArgs {
    #[clap(subcommand)]
    pub subcmd: CleanupSubcommand,
}

#[derive(Parser, Debug)]
pub enum CleanupSubcommand {
    /// Clean up orphaned worktrees that have no corresponding agent metadata
    Orphaned(OrphanedArgs),
    /// Clean up old recovery branches
    RecoveryBranches(RecoveryBranchesArgs),
}

#[derive(Parser, Debug)]
pub struct OrphanedArgs {
    /// Actually perform the cleanup (dry-run by default)
    #[clap(long)]
    pub execute: bool,

    /// Force cleanup even if errors occur
    #[clap(long)]
    pub force: bool,

    /// Directory to search for orphaned worktrees (defaults to current directory)
    #[clap(long)]
    pub dir: Option<String>,
}

#[derive(Parser, Debug)]
pub struct RecoveryBranchesArgs {
    /// Maximum age of recovery branches to keep (in days)
    #[clap(long, default_value = "30")]
    pub max_age_days: u32,

    /// Actually perform the cleanup (dry-run by default)
    #[clap(long)]
    pub execute: bool,

    /// Force cleanup even if errors occur
    #[clap(long)]
    pub force: bool,

    /// Directory containing the git repository (defaults to current directory)
    #[clap(long)]
    pub dir: Option<String>,
}

pub fn run(args: CleanupArgs) -> Result<()> {
    match args.subcmd {
        CleanupSubcommand::Orphaned(orphaned_args) => run_orphaned_cleanup(orphaned_args),
        CleanupSubcommand::RecoveryBranches(recovery_args) => run_recovery_branches_cleanup(recovery_args),
    }
}

/// Clean up orphaned worktrees that have no corresponding agent metadata
fn run_orphaned_cleanup(args: OrphanedArgs) -> Result<()> {
    let project_root = if let Some(dir) = args.dir {
        std::path::PathBuf::from(dir)
    } else {
        std::env::current_dir().context("Failed to get current directory")?
    };

    println!("Scanning for orphaned worktrees in: {}", project_root.display());

    // Load workgraph to verify project structure
    let (_graph, _graph_path) = load_workgraph(&project_root)?;

    let worktrees_dir = project_root.join(WORKTREES_DIR);
    if !worktrees_dir.exists() {
        println!("No worktrees directory found at: {}", worktrees_dir.display());
        return Ok(());
    }

    let agents_dir = project_root.join(".workgraph").join("agents");

    // Get list of active agents from metadata
    let mut active_agents = HashSet::new();
    if agents_dir.exists() {
        for entry in fs::read_dir(&agents_dir).context("Failed to read agents directory")? {
            let entry = entry.context("Failed to read agent directory entry")?;
            if entry.path().is_dir() {
                let agent_id = entry.file_name().to_string_lossy().to_string();

                // Check if agent has valid metadata
                let metadata_path = entry.path().join("metadata.json");
                if metadata_path.exists() {
                    if let Ok(metadata_content) = fs::read_to_string(&metadata_path) {
                        if serde_json::from_str::<serde_json::Value>(&metadata_content).is_ok() {
                            active_agents.insert(agent_id);
                        }
                    }
                }
            }
        }
    }

    println!("Found {} active agents with valid metadata", active_agents.len());

    // Scan worktrees directory for orphaned entries
    let mut orphaned_worktrees = Vec::new();
    for entry in fs::read_dir(&worktrees_dir).context("Failed to read worktrees directory")? {
        let entry = entry.context("Failed to read worktree directory entry")?;
        if entry.path().is_dir() {
            let worktree_name = entry.file_name().to_string_lossy().to_string();

            // Check if this worktree has a corresponding active agent
            if !active_agents.contains(&worktree_name) {
                orphaned_worktrees.push((worktree_name.clone(), entry.path()));
                println!("Found orphaned worktree: {} -> {}", worktree_name, entry.path().display());
            }
        }
    }

    if orphaned_worktrees.is_empty() {
        println!("No orphaned worktrees found.");
        return Ok(());
    }

    println!("Found {} orphaned worktree(s)", orphaned_worktrees.len());

    if !args.execute {
        println!("\nDry-run mode. Use --execute to actually perform cleanup.");
        println!("Use --force to continue cleanup even if individual operations fail.");
        return Ok(());
    }

    // Perform cleanup
    let mut cleanup_errors = Vec::new();
    let mut cleanup_successes = 0;

    for (agent_id, worktree_path) in orphaned_worktrees {
        println!("Cleaning up orphaned worktree: {}", agent_id);

        // Try to determine the branch name
        let branch = format!("wg/{}/task", agent_id);

        match cleanup_orphaned_worktree(&project_root, &worktree_path, &branch) {
            Ok(()) => {
                cleanup_successes += 1;
                println!("✓ Successfully cleaned up orphaned worktree: {}", agent_id);
            }
            Err(e) => {
                let error_msg = format!("Failed to clean up orphaned worktree {}: {}", agent_id, e);
                cleanup_errors.push(error_msg.clone());

                if args.force {
                    eprintln!("⚠ {}", error_msg);
                    eprintln!("  Continuing due to --force flag...");
                } else {
                    return Err(anyhow!(error_msg));
                }
            }
        }
    }

    println!("\nCleanup complete:");
    println!("  Successes: {}", cleanup_successes);
    println!("  Errors: {}", cleanup_errors.len());

    if !cleanup_errors.is_empty() && args.force {
        println!("\nErrors encountered (ignored due to --force):");
        for error in cleanup_errors {
            println!("  - {}", error);
        }
    }

    Ok(())
}

/// Clean up old recovery branches
fn run_recovery_branches_cleanup(args: RecoveryBranchesArgs) -> Result<()> {
    let project_root = if let Some(dir) = args.dir {
        std::path::PathBuf::from(dir)
    } else {
        std::env::current_dir().context("Failed to get current directory")?
    };

    println!("Scanning for old recovery branches in: {}", project_root.display());

    // Verify this is a git repository
    if !project_root.join(".git").exists() {
        return Err(anyhow!("Not a git repository: {}", project_root.display()));
    }

    // Get list of recovery branches
    let output = Command::new("git")
        .args(["branch", "-a"])
        .current_dir(&project_root)
        .output()
        .context("Failed to list git branches")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Git branch listing failed: {}", stderr));
    }

    let branches_output = String::from_utf8_lossy(&output.stdout);
    let recovery_branches: Vec<&str> = branches_output
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("recover/"))
        .map(|line| line.trim_start_matches("* "))
        .collect();

    if recovery_branches.is_empty() {
        println!("No recovery branches found.");
        return Ok(());
    }

    println!("Found {} recovery branch(es)", recovery_branches.len());

    // Check age of recovery branches
    let mut old_branches = Vec::new();
    let max_age_seconds = args.max_age_days as i64 * 24 * 3600;

    for branch in recovery_branches {
        // Get branch creation time (last commit time on the branch)
        let output = Command::new("git")
            .args(["log", "-1", "--format=%ct", branch])
            .current_dir(&project_root)
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let timestamp_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Ok(timestamp) = timestamp_str.parse::<i64>() {
                    let age_seconds = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64 - timestamp;

                    if age_seconds > max_age_seconds {
                        let age_days = age_seconds / (24 * 3600);
                        old_branches.push((branch.to_string(), age_days));
                        println!("Found old recovery branch: {} (age: {} days)", branch, age_days);
                    }
                }
            }
        }
    }

    if old_branches.is_empty() {
        println!("No recovery branches older than {} days found.", args.max_age_days);
        return Ok(());
    }

    println!("Found {} recovery branch(es) older than {} days", old_branches.len(), args.max_age_days);

    if !args.execute {
        println!("\nDry-run mode. Use --execute to actually perform cleanup.");
        println!("Use --force to continue cleanup even if individual operations fail.");
        return Ok(());
    }

    // Perform cleanup
    let mut cleanup_errors = Vec::new();
    let mut cleanup_successes = 0;

    for (branch, age_days) in old_branches {
        println!("Deleting recovery branch: {} (age: {} days)", branch, age_days);

        let output = Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(&project_root)
            .output()
            .context("Failed to execute git branch delete command")?;

        if output.status.success() {
            cleanup_successes += 1;
            println!("✓ Successfully deleted recovery branch: {}", branch);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let error_msg = format!("Failed to delete recovery branch {}: {}", branch, stderr.trim());
            cleanup_errors.push(error_msg.clone());

            if args.force {
                eprintln!("⚠ {}", error_msg);
                eprintln!("  Continuing due to --force flag...");
            } else {
                return Err(anyhow!(error_msg));
            }
        }
    }

    println!("\nCleanup complete:");
    println!("  Successes: {}", cleanup_successes);
    println!("  Errors: {}", cleanup_errors.len());

    if !cleanup_errors.is_empty() && args.force {
        println!("\nErrors encountered (ignored due to --force):");
        for error in cleanup_errors {
            println!("  - {}", error);
        }
    }

    Ok(())
}

/// Clean up a specific orphaned worktree with enhanced error handling
fn cleanup_orphaned_worktree(project_root: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    // Try standard cleanup first
    match remove_worktree(project_root, worktree_path, branch) {
        Ok(()) => {
            // Verify cleanup was successful
            verify_worktree_cleanup(worktree_path, branch, project_root)?;
            return Ok(());
        }
        Err(e) => {
            eprintln!("[cleanup] Standard removal failed for {:?}: {}", worktree_path, e);
            eprintln!("[cleanup] Attempting fallback cleanup...");
        }
    }

    // Fallback: manual cleanup with enhanced error handling
    attempt_manual_worktree_cleanup(project_root, worktree_path, branch)
}

/// Attempt manual cleanup of a worktree with permission-aware error handling
fn attempt_manual_worktree_cleanup(project_root: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let mut cleanup_errors = Vec::new();

    // Step 1: Clean up .workgraph symlink with permission handling
    let wg_symlink = worktree_path.join(".workgraph");
    if wg_symlink.exists() {
        match fs::remove_file(&wg_symlink) {
            Ok(()) => {
                eprintln!("[cleanup] Successfully removed .workgraph symlink");
            }
            Err(e) => {
                let error_msg = format!("Failed to remove .workgraph symlink: {}", e);
                cleanup_errors.push(error_msg.clone());
                eprintln!("[cleanup] {}", error_msg);

                // Try to fix permissions and retry
                if let Err(perm_err) = fix_permissions_and_retry_removal(&wg_symlink) {
                    cleanup_errors.push(format!("Permission fix also failed: {}", perm_err));
                } else {
                    eprintln!("[cleanup] Successfully removed .workgraph symlink after permission fix");
                }
            }
        }
    }

    // Step 2: Clean up target directory with permission handling
    let target_dir = worktree_path.join("target");
    if target_dir.exists() {
        match fs::remove_dir_all(&target_dir) {
            Ok(()) => {
                eprintln!("[cleanup] Successfully removed target directory");
            }
            Err(e) => {
                let error_msg = format!("Failed to remove target directory: {}", e);
                cleanup_errors.push(error_msg.clone());
                eprintln!("[cleanup] {}", error_msg);

                // Try to fix permissions and retry
                if let Err(perm_err) = fix_directory_permissions_and_retry(&target_dir) {
                    cleanup_errors.push(format!("Target directory permission fix failed: {}", perm_err));
                } else {
                    eprintln!("[cleanup] Successfully removed target directory after permission fix");
                }
            }
        }
    }

    // Step 3: Try git worktree remove
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(project_root)
        .output()
        .context("Failed to execute git worktree remove command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_errors.push(format!("Git worktree remove failed: {}", stderr.trim()));
    }

    // Step 4: Try to remove the branch
    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(project_root)
        .output()
        .context("Failed to execute git branch delete command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_errors.push(format!("Git branch delete failed: {}", stderr.trim()));
    }

    // Step 5: Final directory cleanup
    if worktree_path.exists() {
        match fs::remove_dir_all(worktree_path) {
            Ok(()) => {
                eprintln!("[cleanup] Successfully removed worktree directory");
            }
            Err(e) => {
                let error_msg = format!("Failed to remove worktree directory: {}", e);
                cleanup_errors.push(error_msg.clone());
                eprintln!("[cleanup] {}", error_msg);

                // Final attempt with permission fixes
                if let Err(perm_err) = fix_directory_permissions_and_retry(worktree_path) {
                    cleanup_errors.push(format!("Final directory cleanup failed: {}", perm_err));
                } else {
                    eprintln!("[cleanup] Successfully removed worktree directory after permission fix");
                }
            }
        }
    }

    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("Manual cleanup completed with errors:\n{}", cleanup_errors.join("\n")))
    }
}

/// Fix permissions on a file and retry removal
fn fix_permissions_and_retry_removal(file_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Try to make the file writable
    if let Ok(metadata) = fs::metadata(file_path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o644); // Read/write for owner, read for others

        if let Err(e) = fs::set_permissions(file_path, perms) {
            return Err(anyhow!("Failed to fix file permissions: {}", e));
        }

        // Retry removal
        fs::remove_file(file_path).context("Failed to remove file after permission fix")?;
    }

    Ok(())
}

/// Fix permissions on a directory and its contents, then retry removal
fn fix_directory_permissions_and_retry(dir_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if !dir_path.exists() {
        return Ok(());
    }

    // Recursively fix permissions
    fn fix_permissions_recursive(path: &Path) -> Result<()> {
        if path.is_dir() {
            // Make directory executable/readable
            if let Ok(metadata) = fs::metadata(path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(path, perms);
            }

            // Fix permissions for all entries
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    fix_permissions_recursive(&entry.path())?;
                }
            }
        } else {
            // Make file writable
            if let Ok(metadata) = fs::metadata(path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o644);
                let _ = fs::set_permissions(path, perms);
            }
        }
        Ok(())
    }

    fix_permissions_recursive(dir_path).context("Failed to fix directory permissions")?;

    // Retry removal
    fs::remove_dir_all(dir_path).context("Failed to remove directory after permission fix")?;

    Ok(())
}