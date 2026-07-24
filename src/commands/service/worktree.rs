//! Worktree lifecycle cleanup for agent isolation.
//!
//! Two-tier cleanup model:
//! - **Atomic (happy path):** Agent wrapper writes `.wg-cleanup-pending` marker
//!   at exit; the daemon's single-flight background maintenance lane reaps
//!   marked worktrees after all fail-closed safety gates pass. Coordinator ticks
//!   never touch retained paths. Missed/coalesced wakes are retried off-thread.
//! - **GC (fallback):** `wg gc --worktrees` (in [`super::super::worktree_gc`])
//!   handles worktrees orphaned by kills, crashes, or bugs. Same safety predicate
//!   plus an uncommitted-changes gate. User-invoked, dry-run by default.
//!
//! Shared constants ([`HEARTBEAT_LIVENESS_TIMEOUT_SECS`]) and removal machinery
//! ([`remove_worktree`], [`find_branch_for_worktree`]) live here and are reused
//! by both paths.

#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use worksgood::config::ResourceManagementConfig;
use worksgood::metrics::{CleanupTimer, ResourceRecoveryStats, record_recovery_branch};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The directory under the project root where agent worktrees live.
pub const WORKTREES_DIR: &str = ".wg-worktrees";

/// Marker file written inside a worktree by the agent wrapper after
/// merge-back completes, signaling the worktree is eligible for sweep.
///
/// Two-phase atomic cleanup:
/// 1. Wrapper writes this marker at agent exit (can't do `git worktree remove --force`
///    inline — see `test_wrapper_preserves_worktree` sacred invariant).
/// 2. The daemon's background cleanup lane sweeps marked worktrees whose agent
///    is not live and whose evaluation + merge evidence is affirmative.
///
/// Idempotent + crash-safe: if a crash happens between marker write and sweep,
/// a later coalesced maintenance wake retries. If the wrapper never writes the marker (e.g. kill -9),
/// the existing dead-agent reaper still sees the agent as dead and can
/// fall back to `cleanup_orphaned_worktrees()`.
pub const CLEANUP_PENDING_MARKER: &str = ".wg-cleanup-pending";

/// Heartbeat freshness timeout (seconds) for the worktree-cleanup
/// liveness check. A worktree is considered owned by a live agent only
/// if the agent's last heartbeat is within this window AND its process
/// is alive AND its status is alive. Set generously (5 minutes) to
/// accommodate agents that briefly stall during long tool calls.
///
/// See `AgentEntry::is_live` for the full invariant.
pub const HEARTBEAT_LIVENESS_TIMEOUT_SECS: u64 = 300;

/// Determine whether a task's worktree is safe to reap under the retention policy.
///
/// A worktree is **only** safe to reap when BOTH:
/// 1. The task's evaluation passed — `task.status == Done` AND an explicit
///    `.evaluate-<task_id>` task exists and is also `Done`.
/// 2. The branch has been merged into `main` (or `master`) — i.e., the branch
///    tip is reachable from the main branch, so all commits are permanently
///    captured.
///
/// Either condition alone is insufficient: eval-pass-only means the work hasn't
/// landed in main and the agent might still need to handle merge conflicts;
/// merge-only means the eval might still be failing and the work is unverified.
///
/// Returns `false` (do NOT reap) when any signal is missing — including unknown
/// task IDs, missing graph entries, unfindable branches, or unreachable git.
/// This is the safe default: keep the worktree until we can affirmatively prove
/// the work is captured. See task `worktree-retention-don` for motivation.
pub fn is_safe_to_reap(
    graph: Option<&worksgood::graph::WorkGraph>,
    task_id: Option<&str>,
    project_root: &Path,
    branch: Option<&str>,
) -> bool {
    let graph = match graph {
        Some(g) => g,
        None => return false,
    };
    let task_id = match task_id {
        Some(t) => t,
        None => return false,
    };
    let task = match graph.get_task(task_id) {
        Some(t) => t,
        None => return false,
    };
    if task.status != worksgood::graph::Status::Done {
        return false;
    }
    let eval_id = format!(".evaluate-{}", task_id);
    let Some(eval) = graph.get_task(&eval_id) else {
        // Missing evaluation evidence is not a pass. Retain the source until
        // the graph contains an affirmative completed evaluator record.
        return false;
    };
    if eval.status != worksgood::graph::Status::Done {
        return false;
    }
    let branch = match branch {
        Some(b) => b,
        None => return false,
    };
    is_branch_merged(project_root, branch)
}

/// Returns true if the named branch's tip is an ancestor of `main` (or `master`).
/// Equivalent to: "all commits on this branch are also reachable from main."
pub fn is_branch_merged(project_root: &Path, branch: &str) -> bool {
    for main in &["main", "master"] {
        let exists = Command::new("git")
            .args(["rev-parse", "--verify"])
            .arg(format!("refs/heads/{}", main))
            .current_dir(project_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !exists {
            continue;
        }
        let output = Command::new("git")
            .args(["merge-base", "--is-ancestor", branch, main])
            .current_dir(project_root)
            .output();
        if let Ok(out) = output
            && out.status.success()
        {
            return true;
        }

        // WG normally squash-merges task branches, so reachability alone says
        // "unmerged" even when every patch is already integrated. `git cherry`
        // is the exact incident recovery proof: all `-` (or no output) means
        // patch-equivalent. Refuse branches with unique merge commits because
        // cherry intentionally ignores merges and would otherwise over-claim.
        let unique_merges = Command::new("git")
            .args(["rev-list", "--count", "--merges"])
            .arg(format!("{main}..{branch}"))
            .current_dir(project_root)
            .output();
        let no_unique_merges = unique_merges
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|count| count.trim() == "0");
        if !no_unique_merges {
            continue;
        }
        let cherry = Command::new("git")
            .args(["cherry", main, branch])
            .current_dir(project_root)
            .output();
        if let Ok(cherry) = cherry
            && cherry.status.success()
            && String::from_utf8_lossy(&cherry.stdout)
                .lines()
                .all(|line| line.starts_with('-'))
        {
            return true;
        }
    }
    false
}

/// Maximum number of retry attempts for transient failures.
const MAX_RETRIES: usize = 3;

/// Initial retry delay in milliseconds.
const INITIAL_RETRY_DELAY_MS: u64 = 100;

/// Retry a fallible operation with exponential backoff.
/// Returns the result of the operation or the last error if all retries fail.
fn retry_operation<T, F>(mut operation: F, operation_name: &str) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut last_error = None;

    for attempt in 0..=MAX_RETRIES {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = Some(e);

                if attempt < MAX_RETRIES {
                    let delay_ms = INITIAL_RETRY_DELAY_MS * 2_u64.pow(attempt as u32);
                    eprintln!(
                        "[worktree] {} failed on attempt {}/{}, retrying in {}ms: {}",
                        operation_name,
                        attempt + 1,
                        MAX_RETRIES + 1,
                        delay_ms,
                        last_error.as_ref().unwrap()
                    );
                    thread::sleep(Duration::from_millis(delay_ms));
                }
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("Operation {} failed with no error details", operation_name)))
}

/// Calculate the total size of a directory in bytes for metrics tracking.
/// Returns 0 if the directory doesn't exist or can't be read.
fn calculate_directory_size(dir: &Path) -> Result<u64> {
    if !dir.exists() {
        return Ok(0);
    }

    let mut total_size = 0;

    fn visit_dir(dir: &Path, total_size: &mut u64) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                visit_dir(&path, total_size)?;
            } else if let Ok(metadata) = entry.metadata() {
                *total_size += metadata.len();
            }
        }
        Ok(())
    }

    visit_dir(dir, &mut total_size).unwrap_or_else(|_| {
        eprintln!(
            "[metrics] Warning: Failed to calculate directory size for {:?}",
            dir
        );
    });

    Ok(total_size)
}

/// Automatic source-safe removal used only after the retention/source gates.
/// The lifecycle marker is removed, then `git worktree remove` runs WITHOUT
/// `--force`; a source change racing the earlier status check makes git refuse.
/// There is deliberately no filesystem fallback.
fn remove_worktree_source_safe(
    project_root: &Path,
    worktree_path: &Path,
    branch: Option<&str>,
) -> Result<()> {
    let marker = worktree_path.join(CLEANUP_PENDING_MARKER);
    if marker.exists() {
        fs::remove_file(&marker).with_context(|| {
            format!(
                "failed to remove validated lifecycle marker {}",
                marker.display()
            )
        })?;
    }
    let output = Command::new("git")
        .args(["worktree", "remove"])
        .arg(worktree_path)
        .current_dir(project_root)
        .output()
        .context("failed to run source-safe git worktree remove")?;
    if !output.status.success() {
        // Keep the lifecycle request queued after a TOCTOU refusal. This exact
        // marker is the only path the source predicate ignores.
        let _ = fs::write(&marker, "wg-owned cleanup retry\n");
        anyhow::bail!(
            "source-safe git worktree remove refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if let Some(branch) = branch {
        let output = Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(project_root)
            .output()
            .context("failed to delete archived worktree branch")?;
        if !output.status.success() {
            anyhow::bail!(
                "worktree removed but branch cleanup failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}

/// Remove a worktree and its branch. Force-removes to discard uncommitted changes.
pub fn remove_worktree(project_root: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let timer = CleanupTimer::start(format!("remove_worktree: {}", branch));
    let mut resources = ResourceRecoveryStats::default();
    let mut cleanup_errors = Vec::new();

    // Calculate disk space before cleanup for metrics
    let initial_size = calculate_directory_size(worktree_path).unwrap_or(0);

    // Remove .wg symlink first (git worktree remove won't remove it)
    let symlink_path = worktree_path.join(".wg");
    if symlink_path.exists() {
        match fs::remove_file(&symlink_path) {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                eprintln!(
                    "[worktree] Permission denied removing .wg symlink, attempting permission fix"
                );
                if let Err(fallback_err) = fix_permissions_and_remove_file(&symlink_path) {
                    cleanup_errors.push(format!(
                        "Failed to remove .wg symlink {:?} even after permission fix: {}",
                        symlink_path, fallback_err
                    ));
                } else {
                    eprintln!("[worktree] Successfully removed .wg symlink after permission fix");
                    resources.symlinks_cleaned += 1;
                }
            }
            Err(e) => {
                cleanup_errors.push(format!(
                    "Failed to remove .wg symlink {:?}: {}",
                    symlink_path, e
                ));
            }
            Ok(()) => {
                resources.symlinks_cleaned += 1;
            }
        }
    }

    // Remove isolated cargo target directory
    let target_dir = worktree_path.join("target");
    if target_dir.exists() {
        match fs::remove_dir_all(&target_dir) {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                eprintln!(
                    "[worktree] Permission denied removing target directory, attempting permission fix"
                );
                if let Err(fallback_err) = fix_permissions_and_remove_dir(&target_dir) {
                    cleanup_errors.push(format!(
                        "Failed to remove target directory {:?} even after permission fix: {}",
                        target_dir, fallback_err
                    ));
                } else {
                    eprintln!(
                        "[worktree] Successfully removed target directory after permission fix"
                    );
                    resources.directories_removed += 1;
                }
            }
            Err(e) => {
                cleanup_errors.push(format!(
                    "Failed to remove target directory {:?}: {}",
                    target_dir, e
                ));
            }
            Ok(()) => {
                resources.directories_removed += 1;
            }
        }
    }

    // Force-remove the worktree
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(project_root)
        .output()
        .context("Failed to execute git worktree remove command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_errors.push(format!("Git worktree remove failed: {}", stderr.trim()));
    } else {
        resources.worktrees_removed += 1;
        resources.disk_space_recovered_bytes += initial_size;
    }

    // Delete the branch
    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(project_root)
        .output()
        .context("Failed to execute git branch delete command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_errors.push(format!(
            "Git branch delete failed for '{}': {}",
            branch,
            stderr.trim()
        ));
    } else {
        resources.branches_pruned += 1;
    }

    // NOTE: We intentionally do NOT run `git worktree prune` here.
    // Global prune can remove metadata for other agents' worktrees that are
    // temporarily missing during concurrent cleanup, causing data loss.

    let success = cleanup_errors.is_empty();
    timer.complete(success, resources);

    if !success {
        return Err(anyhow!(
            "Worktree removal completed with errors:\n{}",
            cleanup_errors.join("\n")
        ));
    }

    Ok(())
}

/// Verify that a worktree cleanup was successful.
/// Checks that the worktree directory and all related artifacts have been removed.
pub fn verify_worktree_cleanup(
    worktree_path: &Path,
    branch: &str,
    project_root: &Path,
) -> Result<()> {
    let mut verification_errors = Vec::new();

    // Check if the worktree directory still exists
    if worktree_path.exists() {
        verification_errors.push(format!(
            "Worktree directory still exists: {:?}",
            worktree_path
        ));

        // List remaining contents for troubleshooting
        if let Ok(entries) = fs::read_dir(worktree_path) {
            let remaining: Vec<_> = entries
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
                .collect();
            if !remaining.is_empty() {
                verification_errors.push(format!("Remaining files in worktree: {:?}", remaining));
            }
        }
    }

    // Check if the branch still exists locally
    let output = Command::new("git")
        .args(["branch", "--list", branch])
        .current_dir(project_root)
        .output()
        .context("Failed to check if branch exists")?;

    if output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty() {
        verification_errors.push(format!("Branch '{}' still exists locally", branch));
    }

    // Check for stale worktree entries in git
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project_root)
        .output()
        .context("Failed to list worktrees")?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let worktree_str = worktree_path.to_string_lossy();

        for line in text.lines() {
            if let Some(path) = line.strip_prefix("worktree ")
                && path == worktree_str.as_ref()
            {
                verification_errors.push(format!("Stale worktree entry found in git: {}", path));
                break;
            }
        }
    }

    // Check for .wg symlink
    let symlink_path = worktree_path.join(".wg");
    if symlink_path.exists() {
        verification_errors.push(format!(".wg symlink still exists: {:?}", symlink_path));
    }

    // Check for target directory
    let target_dir = worktree_path.join("target");
    if target_dir.exists() {
        verification_errors.push(format!("Target directory still exists: {:?}", target_dir));
    }

    if verification_errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Worktree cleanup verification failed:\n{}",
            verification_errors.join("\n")
        ))
    }
}

/// Remove a worktree with verification if enabled in config.
/// Enhanced version of remove_worktree that optionally verifies cleanup completion.
pub fn remove_worktree_verified(
    project_root: &Path,
    worktree_path: &Path,
    branch: &str,
    config: &ResourceManagementConfig,
) -> Result<()> {
    // First, perform the standard removal
    remove_worktree(project_root, worktree_path, branch)?;

    // If verification is enabled, verify the cleanup
    if config.cleanup_verification {
        match verify_worktree_cleanup(worktree_path, branch, project_root) {
            Ok(()) => {
                eprintln!(
                    "[worktree] Cleanup verification passed for {:?}",
                    worktree_path
                );
            }
            Err(e) => {
                eprintln!(
                    "[worktree] Cleanup verification failed for {:?}: {}",
                    worktree_path, e
                );

                // Attempt additional cleanup for any remaining artifacts
                attempt_force_cleanup(worktree_path)?;

                // Re-verify after force cleanup
                if let Err(e2) = verify_worktree_cleanup(worktree_path, branch, project_root) {
                    return Err(anyhow!("Cleanup failed even after force cleanup: {}", e2));
                }

                eprintln!("[worktree] Force cleanup succeeded for {:?}", worktree_path);
            }
        }
    }

    Ok(())
}

/// Attempt additional force cleanup of remaining worktree artifacts.
fn attempt_force_cleanup(worktree_path: &Path) -> Result<()> {
    eprintln!("[worktree] Attempting force cleanup of {:?}", worktree_path);

    // If the directory still exists, try to remove it with maximum force
    if worktree_path.exists() {
        // First, try to fix permissions and make everything writable
        if let Ok(entries) = fs::read_dir(worktree_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    match fs::remove_file(&path) {
                        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                            let _ = fix_permissions_and_remove_file(&path);
                        }
                        _ => {}
                    }
                } else if path.is_dir() {
                    match fs::remove_dir_all(&path) {
                        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                            let _ = fix_permissions_and_remove_dir(&path);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Finally, remove the directory itself with permission handling
        match fs::remove_dir_all(worktree_path) {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                eprintln!(
                    "[worktree] Permission denied during force cleanup, attempting permission fix"
                );
                fix_permissions_and_remove_dir(worktree_path).with_context(|| {
                    format!(
                        "Failed to force-remove worktree directory {:?} even after permission fix",
                        worktree_path
                    )
                })?;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "Failed to force-remove worktree directory {:?}",
                        worktree_path
                    )
                });
            }
            Ok(()) => {}
        }
    }

    Ok(())
}

/// Check for recoverable commits on a dead agent's worktree branch.
/// If commits exist, creates a recovery branch at `recover/<agent-id>/<task-id>`.
/// Returns the number of commits found.
pub fn recover_commits(project_root: &Path, branch: &str, agent_id: &str) -> usize {
    let commit_count = Command::new("git")
        .args(["log", "--oneline", &format!("HEAD..{}", branch)])
        .current_dir(project_root)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .count()
        })
        .unwrap_or(0);

    if commit_count > 0 {
        let recovery_branch = format!("recover/{}", branch.strip_prefix("wg/").unwrap_or(branch));
        eprintln!(
            "[worktree] Dead agent {} had {} commits on {}. Creating recovery branch: {}",
            agent_id, commit_count, branch, recovery_branch
        );
        let _ = Command::new("git")
            .args(["branch", &recovery_branch, branch])
            .current_dir(project_root)
            .output();
    }

    commit_count
}

/// Clean up a dead agent's worktree: recover commits, then remove worktree and branch.
/// Uses retry logic for transient failures and enhanced error reporting.
pub fn cleanup_dead_agent_worktree(
    project_root: &Path,
    worktree_path: &Path,
    branch: &str,
    agent_id: &str,
) {
    cleanup_dead_agent_worktree_with_config(project_root, worktree_path, branch, agent_id, None);
}

/// Clean up a dead agent's worktree with optional resource management configuration.
/// When config is provided, uses verified cleanup with additional checks.
pub fn cleanup_dead_agent_worktree_with_config(
    project_root: &Path,
    worktree_path: &Path,
    branch: &str,
    agent_id: &str,
    config: Option<&ResourceManagementConfig>,
) {
    use worksgood::metrics::record_dead_agent_cleanup;

    eprintln!(
        "[worktree] Cleaning up dead agent {} worktree {:?} (branch: {})",
        agent_id, worktree_path, branch
    );

    // Recover commits before removing
    let commit_count = recover_commits(project_root, branch, agent_id);
    if commit_count > 0 {
        eprintln!(
            "[worktree] Recovered {} commits from dead agent {}",
            commit_count, agent_id
        );
        // If commit recovery creates a recovery branch, track it
        record_recovery_branch();
    }

    // Remove the worktree with retry logic
    let cleanup_result = retry_operation(
        || {
            if let Some(config) = config {
                remove_worktree_verified(project_root, worktree_path, branch, config)
            } else {
                remove_worktree(project_root, worktree_path, branch)
            }
        },
        &format!("worktree cleanup for agent {}", agent_id),
    );

    match cleanup_result {
        Ok(()) => {
            eprintln!(
                "[worktree] Successfully cleaned up worktree for dead agent {}",
                agent_id
            );
            record_dead_agent_cleanup();
        }
        Err(e) => {
            eprintln!(
                "[worktree] ERROR: Failed to clean up worktree {:?} for agent {} after {} retries: {}",
                worktree_path, agent_id, MAX_RETRIES, e
            );

            // Log individual error details for troubleshooting
            eprintln!("[worktree] Full error chain: {:?}", e);

            // Try a final fallback: manual directory removal if the worktree path still exists
            if worktree_path.exists() {
                eprintln!(
                    "[worktree] Attempting fallback: force removal of directory {:?}",
                    worktree_path
                );
                if let Err(fallback_err) = fs::remove_dir_all(worktree_path) {
                    eprintln!("[worktree] Fallback also failed: {}", fallback_err);
                } else {
                    eprintln!("[worktree] Fallback succeeded: directory removed");
                    record_dead_agent_cleanup();
                }
            }
        }
    }
}

/// Parse `git worktree list --porcelain` output to find the branch for a given worktree path.
pub fn find_branch_for_worktree(project_root: &Path, worktree_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project_root)
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    let worktree_str = worktree_path.to_string_lossy();

    // Porcelain output is blocks separated by blank lines.
    // Each block has: worktree <path>\nHEAD <sha>\nbranch refs/heads/<name>\n
    let mut current_path: Option<&str> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path);
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if let Some(cp) = current_path
                && cp == worktree_str.as_ref()
            {
                // Convert refs/heads/wg/agent-X/task-Y to wg/agent-X/task-Y
                return Some(
                    branch_ref
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch_ref)
                        .to_string(),
                );
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }

    None
}

/// Clean up orphaned worktrees from a previous service run.
/// Called once on service startup. Scans `.wg-worktrees/` for directories
/// that don't correspond to alive agents.
pub fn cleanup_orphaned_worktrees(dir: &Path) -> Result<usize> {
    use worksgood::metrics::{CleanupTimer, record_orphaned_cleanup};

    let timer = CleanupTimer::start("cleanup_orphaned_worktrees");
    let project_root = dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from {:?}", dir))?;
    let worktrees_dir = project_root.join(WORKTREES_DIR);

    if !worktrees_dir.exists() {
        timer.complete(true, worksgood::metrics::ResourceRecoveryStats::default());
        return Ok(0);
    }

    let registry = worksgood::service::registry::AgentRegistry::load(dir)?;

    // Load graph so the retention policy can verify task state. If the graph
    // can't be read, fall back to retain-by-default (don't reap).
    let graph_path = dir.join("graph.jsonl");
    let graph = if graph_path.exists() {
        worksgood::parser::load_graph(&graph_path).ok()
    } else {
        None
    };

    let mut cleaned = 0;

    for entry in fs::read_dir(&worktrees_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip non-agent directories (e.g., .merge-lock file)
        if !name.starts_with("agent-") {
            continue;
        }

        // Check if this agent is alive
        let is_alive = registry
            .agents
            .get(&name)
            .map(|a| a.is_live(HEARTBEAT_LIVENESS_TIMEOUT_SECS))
            .unwrap_or(false);

        if !is_alive {
            let wt_path = entry.path();

            // Retention policy: even orphaned worktrees are preserved until
            // the task is eval-passed AND its branch is merged into main.
            // This protects WIP from crashes / kill -9 / rate-limit scenarios
            // so `wg retry` can resume in-place.
            let branch_opt = find_branch_for_worktree(project_root, &wt_path);
            let task_id_opt: Option<String> = registry
                .agents
                .get(&name)
                .map(|a| a.task_id.clone())
                .or_else(|| {
                    branch_opt
                        .as_deref()
                        .and_then(|b| b.strip_prefix(&format!("wg/{}/", name)).map(str::to_string))
                });
            if !is_safe_to_reap(
                graph.as_ref(),
                task_id_opt.as_deref(),
                project_root,
                branch_opt.as_deref(),
            ) {
                eprintln!(
                    "[worktree] Preserving orphan {} (task '{:?}' not yet eval-passed AND merged — retention policy)",
                    name, task_id_opt
                );
                continue;
            }
            eprintln!("[worktree] Cleaning orphaned worktree: {}", name);

            // Try to find the branch from git porcelain output
            let branch = branch_opt;

            if let Some(ref branch) = branch {
                // Use the enhanced cleanup function with retry logic
                cleanup_dead_agent_worktree(project_root, &wt_path, branch, &name);
            } else {
                eprintln!(
                    "[worktree] No git branch found for orphaned worktree {}, attempting manual cleanup",
                    name
                );

                // No branch found — use fallback cleanup with error reporting
                let mut cleanup_errors = Vec::new();

                // Remove .wg symlink
                let symlink_path = wt_path.join(".wg");
                if symlink_path.exists()
                    && let Err(e) = fs::remove_file(&symlink_path)
                {
                    cleanup_errors.push(format!("Failed to remove .wg symlink: {}", e));
                }

                // Remove isolated cargo target directory
                let target_dir = wt_path.join("target");
                if target_dir.exists()
                    && let Err(e) = fs::remove_dir_all(&target_dir)
                {
                    cleanup_errors.push(format!("Failed to remove target directory: {}", e));
                }

                // Try git worktree remove
                let output = Command::new("git")
                    .args(["worktree", "remove", "--force"])
                    .arg(&wt_path)
                    .current_dir(project_root)
                    .output();

                match output {
                    Ok(output) if !output.status.success() => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        cleanup_errors
                            .push(format!("Git worktree remove failed: {}", stderr.trim()));
                    }
                    Err(e) => {
                        cleanup_errors
                            .push(format!("Failed to execute git worktree remove: {}", e));
                    }
                    _ => {} // Success case
                }

                if !cleanup_errors.is_empty() {
                    eprintln!(
                        "[worktree] Warnings during manual cleanup of {}: {}",
                        name,
                        cleanup_errors.join("; ")
                    );
                }
            }

            cleaned += 1;
            record_orphaned_cleanup();
        }
    }

    // NOTE: We intentionally do NOT run `git worktree prune` here.
    // Other agents may be running concurrently; global prune can damage their
    // worktree metadata if their directory is temporarily absent.

    let resources = worksgood::metrics::ResourceRecoveryStats {
        worktrees_removed: cleaned as u64,
        ..Default::default()
    };
    timer.complete(true, resources);

    Ok(cleaned)
}

/// Remove the `target/` build-artifact directory inside a worktree.
///
/// Build artifacts (~16G/agent for this project) are not needed once the
/// agent has exited — `cargo` will rebuild them on resume if the worktree
/// is reused for `wg retry`. This is the per-worktree primitive used by both
/// the agent-exit hook and the periodic reaper.
///
/// Returns the bytes freed (best-effort estimate from
/// [`calculate_directory_size`]). Returns `Ok(0)` if `target/` does not exist.
pub fn reap_target_dir(worktree_path: &Path) -> Result<u64> {
    let target = worktree_path.join("target");
    if !target.exists() {
        return Ok(0);
    }
    let size = calculate_directory_size(&target).unwrap_or(0);
    match fs::remove_dir_all(&target) {
        Ok(()) => Ok(size),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            fix_permissions_and_remove_dir(&target)
                .map(|_| size)
                .with_context(|| format!("Failed to reap target dir at {:?}", target))
        }
        Err(e) => {
            Err(anyhow!(e)).with_context(|| format!("Failed to reap target dir at {:?}", target))
        }
    }
}

/// Reap `target/` directories from worktrees whose owning agent is NOT live.
///
/// Walks `.wg-worktrees/<agent-N>` and, for each worktree that has no
/// live occupant, removes the worktree's `target/` directory. Source
/// files, the `.git` pointer, and the worktree itself are preserved —
/// only build artifacts are reaped.
///
/// "Live occupant" means *any* registry entry whose `worktree_path`
/// matches the directory AND is [`AgentEntry::is_live`]. This protects
/// `wg retry`-in-place: agent-806 may run inside `agent-772/`, and the
/// directory-name lookup alone would (incorrectly) treat it as dead.
/// As a fallback when no agent records a worktree_path (legacy entries
/// from before this field was added), we also check the entry whose ID
/// matches the directory name.
///
/// This is the safety-net half of the target-reaper protocol (see
/// `docs/AGENT-LIFECYCLE.md`). The happy-path reaper runs inline in the
/// agent wrapper at exit; this function catches cases where the wrapper
/// crashed or was killed before it could clean up (e.g. `kill -9`).
///
/// Returns `(worktrees_reaped, bytes_freed)`. Errors on individual worktrees
/// are logged but do not abort the sweep.
pub fn reap_dead_target_dirs(dir: &Path) -> Result<(usize, u64)> {
    let project_root = dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from {:?}", dir))?;
    let worktrees_dir = project_root.join(WORKTREES_DIR);

    if !worktrees_dir.exists() {
        return Ok((0, 0));
    }

    let registry = worksgood::service::registry::AgentRegistry::load(dir)?;
    let mut count = 0usize;
    let mut bytes_freed = 0u64;

    for entry in fs::read_dir(&worktrees_dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[reap-targets] read_dir entry error: {}", e);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("agent-") {
            continue;
        }

        let wt_path = entry.path();

        // Primary: live occupant per registered worktree_path. Catches
        // `wg retry`-in-place where the new agent's ID differs from the
        // directory name.
        if registry.is_worktree_occupied(&wt_path, HEARTBEAT_LIVENESS_TIMEOUT_SECS) {
            continue;
        }

        // Fallback: legacy registry entries (pre-worktree_path) — use the
        // directory-name → agent-id correspondence. Skip when that agent
        // is live to preserve backwards compatibility for in-flight
        // agents that registered before the upgrade.
        let legacy_alive = registry
            .agents
            .get(&name)
            .map(|a| a.is_live(HEARTBEAT_LIVENESS_TIMEOUT_SECS))
            .unwrap_or(false);
        if legacy_alive {
            continue;
        }

        if !wt_path.join("target").exists() {
            continue;
        }

        match reap_target_dir(&wt_path) {
            Ok(0) => {}
            Ok(freed) => {
                eprintln!(
                    "[reap-targets] Removed target/ in {} ({} bytes freed)",
                    name, freed
                );
                count += 1;
                bytes_freed += freed;
            }
            Err(e) => {
                eprintln!("[reap-targets] Failed for {}: {}", name, e);
            }
        }
    }

    Ok((count, bytes_freed))
}

/// Maximum retained entries examined by one maintenance-lane batch.
/// A continuation is queued when more remain; the dispatcher never waits for it.
pub const CLEANUP_SWEEP_BATCH_SIZE: usize = 32;

/// Ordinary wakeups are folded into at most one retained sweep per interval.
/// Explicit continuations for a bounded batch bypass this delay.
pub const CLEANUP_SWEEP_MIN_INTERVAL: Duration = Duration::from_secs(5);

const CLEANUP_LANE_STATUS_FILE: &str = "worktree-cleanup.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CleanupSweepReport {
    pub candidates: usize,
    pub examined: usize,
    pub cache_hits: usize,
    pub invalidated: usize,
    pub merge_checks: usize,
    pub source_checks: usize,
    pub retained: usize,
    pub removed: usize,
    pub errors: usize,
    pub deferred: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupLanePhase {
    #[default]
    Idle,
    Due,
    Running,
    Deferred,
    Completed,
    Error,
}

/// Small persisted diagnostic read by `wg service status`. All filesystem I/O
/// that updates this record is performed by the maintenance worker, never by a
/// dispatch-critical coordinator tick.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CleanupLaneSnapshot {
    pub phase: CleanupLanePhase,
    pub requests: u64,
    pub coalesced: u64,
    pub batches_completed: u64,
    pub last_trigger: Option<String>,
    pub last_completed_at: Option<String>,
    pub last_duration_ms: u64,
    pub last_report: CleanupSweepReport,
    pub last_error: Option<String>,
}

pub fn load_cleanup_lane_snapshot(dir: &Path) -> Option<CleanupLaneSnapshot> {
    let bytes = fs::read(dir.join("service").join(CLEANUP_LANE_STATUS_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn run_background_disk_maintenance(dir: &Path, log: &CleanupLog) {
    let config = worksgood::config::Config::load_or_default(dir);
    let resources = &config.coordinator.resource_management;
    let was_due = worksgood::disk_sentinel::load_snapshot(dir)
        .ok()
        .flatten()
        .and_then(|snapshot| chrono::DateTime::parse_from_rfc3339(&snapshot.generated_at).ok())
        .map(|generated| {
            (chrono::Utc::now() - generated.with_timezone(&chrono::Utc)).num_seconds()
                >= resources.disk_scan_interval_seconds as i64
        })
        .unwrap_or(true);
    match worksgood::disk_sentinel::refresh_if_due(dir, resources) {
        Ok(Some(snapshot)) if snapshot.level != worksgood::disk_sentinel::DiskLevel::Healthy => {
            log(
                true,
                &format!("Disk sentinel {:?}: {}", snapshot.level, snapshot.reason),
            );
        }
        Ok(_) => {}
        Err(error) => log(true, &format!("Disk sentinel warning: {error:#}")),
    }
    if was_due {
        match worksgood::disk_sentinel::cleanup_owned(dir, resources, true) {
            Ok(report)
                if report.reaped > 0
                    || report.compressed_files > 0
                    || report.deduplicated_files > 0 =>
            {
                log(
                    false,
                    &format!(
                        "Disk cleanup: reaped {} owned target(s), freed {} bytes; compressed {} stream(s), saved {} bytes; deduplicated {}, saved {} bytes",
                        report.reaped,
                        report.bytes_freed,
                        report.compressed_files,
                        report.compression_bytes_saved,
                        report.deduplicated_files,
                        report.deduplication_bytes_saved
                    ),
                );
            }
            Ok(_) => {}
            Err(error) => log(true, &format!("Disk cleanup warning: {error:#}")),
        }
    }
}

fn save_cleanup_lane_snapshot(dir: &Path, snapshot: &CleanupLaneSnapshot) {
    let service = dir.join("service");
    if fs::create_dir_all(&service).is_err() {
        return;
    }
    let path = service.join(CLEANUP_LANE_STATUS_FILE);
    let tmp = service.join(format!(".{CLEANUP_LANE_STATUS_FILE}.tmp"));
    let Ok(bytes) = serde_json::to_vec_pretty(snapshot) else {
        return;
    };
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetainedDecisionKey {
    marker: Vec<u8>,
    ownership: String,
    task_id: Option<String>,
    task_status: Option<String>,
    eval_status: Option<String>,
    branch: Option<String>,
    branch_head: Option<String>,
    main_heads: String,
}

#[derive(Default)]
struct RetainedSweepCache {
    decisions: HashMap<PathBuf, RetainedDecisionKey>,
    cursor: usize,
}

fn read_repo_git_dir(project_root: &Path) -> Option<PathBuf> {
    let dot_git = project_root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = fs::read_to_string(dot_git).ok()?;
    let raw = pointer.trim().strip_prefix("gitdir: ")?;
    let path = PathBuf::from(raw);
    Some(if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    })
}

fn read_ref_oid(project_root: &Path, branch: &str) -> Option<String> {
    let git_dir = read_repo_git_dir(project_root)?;
    let reference = format!("refs/heads/{branch}");
    let loose = git_dir.join(&reference);
    if let Ok(value) = fs::read_to_string(loose) {
        return Some(value.trim().to_string());
    }
    let packed = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (oid, name) = line.split_once(' ')?;
        (name == reference).then(|| oid.to_string())
    })
}

/// Read branch identity without spawning git. The worktree `.git` pointer and
/// HEAD/ref files are enough to key a retained negative decision. If any piece
/// is unavailable we return `None`, which is fail-closed.
fn branch_from_worktree_metadata(worktree_path: &Path) -> Option<String> {
    let pointer = fs::read_to_string(worktree_path.join(".git")).ok()?;
    let raw = pointer.trim().strip_prefix("gitdir: ")?;
    let git_dir = {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            worktree_path.join(path)
        }
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

fn main_head_key(project_root: &Path) -> String {
    ["main", "master"]
        .into_iter()
        .map(|name| {
            format!(
                "{name}={}",
                read_ref_oid(project_root, name).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn inferred_task_id(name: &str, branch: Option<&str>) -> Option<String> {
    branch
        .and_then(|branch| branch.strip_prefix(&format!("wg/{name}/")))
        .map(str::to_string)
}

fn recorded_worktree_path_matches(recorded: &str, worktree_path: &Path) -> bool {
    let recorded = Path::new(recorded);
    if recorded == worktree_path {
        return true;
    }
    matches!(
        (recorded.canonicalize(), worktree_path.canonicalize()),
        (Ok(recorded), Ok(actual)) if recorded == actual
    )
}

/// Return a stable ownership/liveness key, a preferred task id, and whether
/// ownership currently makes deletion ineligible. A live occupant always
/// retains. Conflicting task ownership also retains fail-closed.
fn ownership_state(
    registry: &worksgood::service::registry::AgentRegistry,
    name: &str,
    worktree_path: &Path,
) -> (String, Option<String>, bool) {
    let path_text = worktree_path.to_string_lossy();
    let mut matching = registry
        .agents
        .values()
        .filter(|agent| {
            agent.id == name
                || agent.worktree_path.as_deref().is_some_and(|path| {
                    path == path_text || recorded_worktree_path_matches(path, worktree_path)
                })
        })
        .collect::<Vec<_>>();
    matching.sort_by(|a, b| a.id.cmp(&b.id));

    let mut tasks = matching
        .iter()
        .map(|agent| agent.task_id.clone())
        .collect::<Vec<_>>();
    tasks.sort();
    tasks.dedup();
    let live = matching
        .iter()
        .any(|agent| agent.is_live(HEARTBEAT_LIVENESS_TIMEOUT_SECS));
    let conflict = tasks.len() > 1;
    let preferred = registry
        .agents
        .get(name)
        .map(|agent| agent.task_id.clone())
        .or_else(|| (tasks.len() == 1).then(|| tasks[0].clone()));
    let signature = matching
        .iter()
        .map(|agent| {
            format!(
                "{}:{}:{:?}:{}:{}:{}",
                agent.id,
                agent.task_id,
                agent.status,
                agent.pid,
                agent.last_heartbeat,
                agent.worktree_path.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    (
        format!("{signature};live={live};conflict={conflict}"),
        preferred,
        live || conflict,
    )
}

fn task_state_key(
    graph: &worksgood::graph::WorkGraph,
    task_id: Option<&str>,
) -> (Option<String>, Option<String>, bool) {
    let Some(task_id) = task_id else {
        return (None, None, false);
    };
    let task_status = graph
        .get_task(task_id)
        .map(|task| format!("{:?}", task.status));
    let eval_status = graph
        .get_task(&format!(".evaluate-{task_id}"))
        .map(|task| format!("{:?}", task.status));
    let passed = graph
        .get_task(task_id)
        .is_some_and(|task| task.status == worksgood::graph::Status::Done)
        && graph
            .get_task(&format!(".evaluate-{task_id}"))
            .is_some_and(|task| task.status == worksgood::graph::Status::Done);
    (task_status, eval_status, passed)
}

fn retain_cached(
    cache: &mut RetainedSweepCache,
    path: &Path,
    key: RetainedDecisionKey,
    report: &mut CleanupSweepReport,
) {
    if cache.decisions.insert(path.to_path_buf(), key).is_some() {
        report.invalidated += 1;
    }
    report.retained += 1;
}

/// Re-check every destructive precondition immediately before removal. Cached
/// decisions are negative-only and are never consulted here.
fn freshly_safe_to_remove(
    dir: &Path,
    project_root: &Path,
    name: &str,
    worktree_path: &Path,
) -> Result<Option<String>> {
    let marker = worktree_path.join(CLEANUP_PENDING_MARKER);
    if !marker.is_file() {
        return Ok(None);
    }
    let registry = worksgood::service::registry::AgentRegistry::load(dir)?;
    let (_ownership, owner_task, blocked) = ownership_state(&registry, name, worktree_path);
    if blocked {
        return Ok(None);
    }
    let branch = find_branch_for_worktree(project_root, worktree_path);
    let task_id = owner_task.or_else(|| inferred_task_id(name, branch.as_deref()));
    let graph_path = dir.join("graph.jsonl");
    let graph = worksgood::parser::load_graph(&graph_path)
        .context("Failed to reload graph for final worktree cleanup gate")?;
    if !is_safe_to_reap(
        Some(&graph),
        task_id.as_deref(),
        project_root,
        branch.as_deref(),
    ) {
        return Ok(None);
    }
    if worksgood::disk_sentinel::worktree_has_user_source_changes(worktree_path) {
        return Ok(None);
    }
    Ok(branch)
}

fn sweep_cleanup_pending_worktrees_batch(
    dir: &Path,
    cache: &mut RetainedSweepCache,
    batch_size: usize,
) -> Result<CleanupSweepReport> {
    let project_root = dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from {:?}", dir))?;
    let worktrees_dir = project_root.join(WORKTREES_DIR);
    let mut report = CleanupSweepReport::default();
    if !worktrees_dir.exists() {
        cache.cursor = 0;
        return Ok(report);
    }

    // This entire discovery/load path runs only on the background maintenance
    // worker. Nothing in coordinator_tick touches the retained filesystem.
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&worktrees_dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.errors += 1;
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("agent-") && entry.path().join(CLEANUP_PENDING_MARKER).exists() {
            candidates.push((name, entry.path()));
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    report.candidates = candidates.len();
    if candidates.is_empty() {
        cache.cursor = 0;
        cache.decisions.clear();
        return Ok(report);
    }

    if cache.cursor >= candidates.len() {
        cache.cursor = 0;
    }
    let start = cache.cursor;
    let end = (start + batch_size.max(1)).min(candidates.len());
    report.deferred = end < candidates.len();
    cache.cursor = if report.deferred { end } else { 0 };

    let registry = worksgood::service::registry::AgentRegistry::load(dir)?;
    let graph_path = dir.join("graph.jsonl");
    let graph = worksgood::parser::load_graph(&graph_path)
        .context("Failed to load graph for retained worktree sweep")?;

    for (name, worktree_path) in &candidates[start..end] {
        report.examined += 1;
        let marker = match fs::read(worktree_path.join(CLEANUP_PENDING_MARKER)) {
            Ok(marker) => marker,
            Err(_) => {
                report.errors += 1;
                report.retained += 1;
                continue;
            }
        };
        let (ownership, owner_task, ownership_blocked) =
            ownership_state(&registry, name, worktree_path);
        let branch = branch_from_worktree_metadata(worktree_path);
        let task_id = owner_task.or_else(|| inferred_task_id(name, branch.as_deref()));
        let (task_status, eval_status, eval_passed) = task_state_key(&graph, task_id.as_deref());

        // Cheap/live/evaluation negatives intentionally omit merge state. An
        // unrelated main-branch change must not invalidate every retained
        // missing-evaluation entry.
        let early_key = RetainedDecisionKey {
            marker: marker.clone(),
            ownership: ownership.clone(),
            task_id: task_id.clone(),
            task_status: task_status.clone(),
            eval_status: eval_status.clone(),
            branch: None,
            branch_head: None,
            main_heads: String::new(),
        };
        if ownership_blocked || !eval_passed {
            if cache.decisions.get(worktree_path) == Some(&early_key) {
                report.cache_hits += 1;
                report.retained += 1;
            } else {
                retain_cached(cache, worktree_path, early_key, &mut report);
            }
            continue;
        }

        let branch_head = branch
            .as_deref()
            .and_then(|branch| read_ref_oid(project_root, branch));
        let merge_key = RetainedDecisionKey {
            marker,
            ownership,
            task_id: task_id.clone(),
            task_status,
            eval_status,
            branch: branch.clone(),
            branch_head,
            main_heads: main_head_key(project_root),
        };
        if cache.decisions.get(worktree_path) == Some(&merge_key) {
            report.cache_hits += 1;
            report.retained += 1;
            continue;
        }
        if cache.decisions.contains_key(worktree_path) {
            report.invalidated += 1;
        }

        let Some(branch_name) = branch.as_deref() else {
            cache.decisions.insert(worktree_path.clone(), merge_key);
            report.retained += 1;
            continue;
        };
        report.merge_checks += 1;
        if !is_branch_merged(project_root, branch_name) {
            cache.decisions.insert(worktree_path.clone(), merge_key);
            report.retained += 1;
            continue;
        }

        // Never cache a positive source decision. Dirty/source-error outcomes
        // remain retained and a later bounded sweep checks them again. A fresh
        // status check is also repeated in freshly_safe_to_remove.
        report.source_checks += 1;
        if worksgood::disk_sentinel::worktree_has_user_source_changes(worktree_path) {
            cache.decisions.remove(worktree_path);
            report.retained += 1;
            continue;
        }

        match freshly_safe_to_remove(dir, project_root, name, worktree_path)? {
            Some(fresh_branch) => {
                match remove_worktree_source_safe(project_root, worktree_path, Some(&fresh_branch))
                {
                    Ok(()) => {
                        cache.decisions.remove(worktree_path);
                        report.removed += 1;
                    }
                    Err(_) => {
                        cache.decisions.remove(worktree_path);
                        report.errors += 1;
                        report.retained += 1;
                    }
                }
            }
            None => {
                cache.decisions.remove(worktree_path);
                report.retained += 1;
            }
        }
    }

    Ok(report)
}

/// Synchronous compatibility entry point for explicit commands and focused
/// safety tests. The daemon does not call this from coordinator_tick; it owns a
/// [`RetainedWorktreeCleanupLane`] instead.
pub fn sweep_cleanup_pending_worktrees(dir: &Path) -> Result<usize> {
    let mut cache = RetainedSweepCache::default();
    let mut removed = 0;
    loop {
        let report = sweep_cleanup_pending_worktrees_batch(dir, &mut cache, usize::MAX)?;
        removed += report.removed;
        if !report.deferred {
            return Ok(removed);
        }
    }
}

type CleanupRunner = Box<dyn FnMut() -> Result<CleanupSweepReport> + Send + 'static>;
pub type CleanupLog = Arc<dyn Fn(bool, &str) + Send + Sync + 'static>;

/// Single-flight retained-worktree maintenance lane. Requests use a capacity-1
/// channel: one running sweep plus at most one deferred wake. Timer/fs-watch
/// bursts therefore cannot create concurrent destructive sweeps or an
/// unbounded queue.
pub struct RetainedWorktreeCleanupLane {
    request_tx: SyncSender<()>,
    snapshot: Arc<Mutex<CleanupLaneSnapshot>>,
    shutdown: Arc<AtomicBool>,
}

impl RetainedWorktreeCleanupLane {
    pub fn start(dir: PathBuf, log: CleanupLog) -> Result<Self> {
        let runner_dir = dir.clone();
        let runner_log = Arc::clone(&log);
        let mut cache = RetainedSweepCache::default();
        let runner: CleanupRunner = Box::new(move || {
            // Disk accounting includes a bounded `.wg-worktrees` size walk;
            // it belongs on the same slow-filesystem lane, never in a tick.
            run_background_disk_maintenance(&runner_dir, &runner_log);
            sweep_cleanup_pending_worktrees_batch(&runner_dir, &mut cache, CLEANUP_SWEEP_BATCH_SIZE)
        });
        Self::start_with_runner(dir, log, CLEANUP_SWEEP_MIN_INTERVAL, runner)
    }

    fn start_with_runner(
        dir: PathBuf,
        log: CleanupLog,
        min_interval: Duration,
        mut runner: CleanupRunner,
    ) -> Result<Self> {
        let (request_tx, request_rx) = sync_channel::<()>(1);
        let snapshot = Arc::new(Mutex::new(CleanupLaneSnapshot::default()));
        let worker_snapshot = Arc::clone(&snapshot);
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        thread::Builder::new()
            .name("wg-worktree-cleanup".to_string())
            .spawn(move || {
                let mut last_started: Option<Instant> = None;
                while request_rx.recv().is_ok() {
                    if worker_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(last) = last_started {
                        let delay = min_interval.saturating_sub(last.elapsed());
                        if !delay.is_zero() {
                            if let Ok(mut state) = worker_snapshot.lock() {
                                if state.phase != CleanupLanePhase::Deferred {
                                    state.phase = CleanupLanePhase::Due;
                                }
                                save_cleanup_lane_snapshot(&dir, &state);
                            }
                            thread::sleep(delay);
                        }
                    }
                    if worker_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    last_started = Some(Instant::now());

                    // A bounded batch may request an immediate continuation.
                    loop {
                        let started = Instant::now();
                        if let Ok(mut state) = worker_snapshot.lock() {
                            state.phase = CleanupLanePhase::Running;
                            state.last_error = None;
                            save_cleanup_lane_snapshot(&dir, &state);
                        }
                        match runner() {
                            Ok(report) => {
                                let duration = started.elapsed();
                                let deferred = report.deferred;
                                if let Ok(mut state) = worker_snapshot.lock() {
                                    state.phase = if deferred {
                                        CleanupLanePhase::Deferred
                                    } else {
                                        CleanupLanePhase::Completed
                                    };
                                    state.batches_completed =
                                        state.batches_completed.saturating_add(1);
                                    state.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
                                    state.last_duration_ms =
                                        duration.as_millis().min(u64::MAX as u128) as u64;
                                    state.last_report = report.clone();
                                    save_cleanup_lane_snapshot(&dir, &state);
                                }
                                // An unchanged all-cache-hit pass is visible in
                                // the status snapshot but does not repeat a log
                                // line. Log only first/change/removal/error work.
                                if report.errors > 0
                                    || report.removed > 0
                                    || report.invalidated > 0
                                    || (report.candidates > 0
                                        && report.cache_hits != report.examined)
                                {
                                    log(
                                        false,
                                        &format!(
                                            "Retained-worktree sweep {}: duration={}ms candidates={} examined={} cached={} invalidated={} merge_checks={} source_checks={} retained={} removed={} errors={}",
                                            if deferred { "deferred" } else { "completed" },
                                            duration.as_millis(),
                                            report.candidates,
                                            report.examined,
                                            report.cache_hits,
                                            report.invalidated,
                                            report.merge_checks,
                                            report.source_checks,
                                            report.retained,
                                            report.removed,
                                            report.errors,
                                        ),
                                    );
                                }
                                if !deferred {
                                    break;
                                }
                                thread::yield_now();
                            }
                            Err(error) => {
                                let duration = started.elapsed();
                                if let Ok(mut state) = worker_snapshot.lock() {
                                    state.phase = CleanupLanePhase::Error;
                                    state.batches_completed =
                                        state.batches_completed.saturating_add(1);
                                    state.last_completed_at = Some(chrono::Utc::now().to_rfc3339());
                                    state.last_duration_ms =
                                        duration.as_millis().min(u64::MAX as u128) as u64;
                                    state.last_error = Some(format!("{error:#}"));
                                    save_cleanup_lane_snapshot(&dir, &state);
                                }
                                log(
                                    true,
                                    &format!(
                                        "Retained-worktree sweep failed closed after {}ms: {error:#}",
                                        duration.as_millis()
                                    ),
                                );
                                break;
                            }
                        }
                        if worker_shutdown.load(Ordering::Acquire) {
                            return;
                        }
                    }
                }
            })
            .context("Failed to start retained-worktree cleanup lane")?;

        Ok(Self {
            request_tx,
            snapshot,
            shutdown,
        })
    }

    /// Non-blocking and filesystem-free. Safe to call on every timer/fs wake.
    pub fn request(&self, trigger: &str) {
        if let Ok(mut state) = self.snapshot.lock() {
            state.requests = state.requests.saturating_add(1);
            state.last_trigger = Some(trigger.to_string());
            match self.request_tx.try_send(()) {
                Ok(()) => {
                    if state.phase != CleanupLanePhase::Running {
                        state.phase = CleanupLanePhase::Due;
                    }
                }
                Err(TrySendError::Full(())) => {
                    state.coalesced = state.coalesced.saturating_add(1);
                    if state.phase == CleanupLanePhase::Running {
                        state.phase = CleanupLanePhase::Deferred;
                    }
                }
                Err(TrySendError::Disconnected(())) => {
                    state.phase = CleanupLanePhase::Error;
                    state.last_error = Some("cleanup worker disconnected".to_string());
                }
            }
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> CleanupLaneSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
}

impl Drop for RetainedWorktreeCleanupLane {
    fn drop(&mut self) {
        // Deliberately do not join: a network filesystem syscall can remain
        // blocked indefinitely. Process shutdown must not wait on maintenance.
        self.shutdown.store(true, Ordering::Release);
        let _ = self.request_tx.try_send(());
    }
}

/// Prune worktrees that are older than `max_age_secs`.
/// Called periodically from the triage loop. Only removes worktrees
/// whose agents are no longer alive.
#[allow(dead_code)]
pub fn prune_stale_worktrees(dir: &Path, max_age_secs: u64) -> Result<usize> {
    let project_root = dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from {:?}", dir))?;
    let worktrees_dir = project_root.join(WORKTREES_DIR);

    if !worktrees_dir.exists() {
        return Ok(0);
    }

    let registry = worksgood::service::registry::AgentRegistry::load(dir)?;
    let mut pruned = 0;

    for entry in fs::read_dir(&worktrees_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        if !name.starts_with("agent-") {
            continue;
        }

        // Skip alive agents
        let is_alive = registry
            .agents
            .get(&name)
            .map(|a| a.is_live(HEARTBEAT_LIVENESS_TIMEOUT_SECS))
            .unwrap_or(false);

        if is_alive {
            continue;
        }

        // Check age
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = match modified.elapsed() {
            Ok(d) => d,
            Err(_) => continue,
        };

        if age.as_secs() > max_age_secs {
            let wt_path = entry.path();
            eprintln!(
                "[worktree] Pruning stale worktree {} (age: {}s > {}s)",
                name,
                age.as_secs(),
                max_age_secs
            );

            let branch = find_branch_for_worktree(project_root, &wt_path);
            if let Some(ref branch) = branch {
                // Use the enhanced cleanup function with retry logic
                cleanup_dead_agent_worktree(project_root, &wt_path, branch, &name);
            } else {
                eprintln!(
                    "[worktree] No git branch found for stale worktree {}, attempting manual cleanup",
                    name
                );

                // Use fallback cleanup with error reporting (same as orphaned cleanup)
                let mut cleanup_errors = Vec::new();

                let symlink_path = wt_path.join(".wg");
                if symlink_path.exists()
                    && let Err(e) = fs::remove_file(&symlink_path)
                {
                    cleanup_errors.push(format!("Failed to remove .wg symlink: {}", e));
                }

                let target_dir = wt_path.join("target");
                if target_dir.exists()
                    && let Err(e) = fs::remove_dir_all(&target_dir)
                {
                    cleanup_errors.push(format!("Failed to remove target directory: {}", e));
                }

                let output = Command::new("git")
                    .args(["worktree", "remove", "--force"])
                    .arg(&wt_path)
                    .current_dir(project_root)
                    .output();

                match output {
                    Ok(output) if !output.status.success() => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        cleanup_errors
                            .push(format!("Git worktree remove failed: {}", stderr.trim()));
                    }
                    Err(e) => {
                        cleanup_errors
                            .push(format!("Failed to execute git worktree remove: {}", e));
                    }
                    _ => {} // Success case
                }

                if !cleanup_errors.is_empty() {
                    eprintln!(
                        "[worktree] Warnings during manual cleanup of stale {}: {}",
                        name,
                        cleanup_errors.join("; ")
                    );
                }
            }

            pruned += 1;
        }
    }

    // NOTE: No global `git worktree prune` — concurrent agents may be running.

    Ok(pruned)
}

/// Get all recovery branches sorted by age (oldest first).
/// Returns a list of (branch_name, last_commit_timestamp) tuples.
#[allow(dead_code)]
fn get_recovery_branches(project_root: &Path) -> Result<Vec<(String, u64)>> {
    let output = Command::new("git")
        .args([
            "branch",
            "-r",
            "--format=%(refname:short) %(committerdate:unix)",
        ])
        .current_dir(project_root)
        .output()
        .context("Failed to list remote branches")?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut recovery_branches = Vec::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let branch = parts[0];
            if let Some(branch_name) = branch.strip_prefix("origin/recover/")
                && let Ok(timestamp) = parts[1].parse::<u64>()
            {
                recovery_branches.push((format!("recover/{}", branch_name), timestamp));
            }
        }
    }

    // Also check local recovery branches
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short) %(committerdate:unix)",
            "refs/heads/recover/**",
        ])
        .current_dir(project_root)
        .output()
        .context("Failed to list local recovery branches")?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let branch = parts[0];
                if branch.starts_with("recover/")
                    && let Ok(timestamp) = parts[1].parse::<u64>()
                {
                    // Avoid duplicates - only add if not already present
                    if !recovery_branches.iter().any(|(b, _)| b == branch) {
                        recovery_branches.push((branch.to_string(), timestamp));
                    }
                }
            }
        }
    }

    // Sort by timestamp (oldest first)
    recovery_branches.sort_by_key(|(_, timestamp)| *timestamp);
    Ok(recovery_branches)
}

/// Prune recovery branches based on age and count limits.
/// Returns the number of branches pruned.
#[allow(dead_code)]
fn prune_recovery_branches(
    project_root: &Path,
    config: &ResourceManagementConfig,
) -> Result<usize> {
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("Failed to get current time: {}", e))?
        .as_secs();

    let recovery_branches = get_recovery_branches(project_root)?;
    let mut pruned_count = 0;

    // Age-based pruning
    if config.recovery_branch_max_age > 0 {
        for (branch, timestamp) in &recovery_branches {
            let age = current_time.saturating_sub(*timestamp);
            if age > config.recovery_branch_max_age {
                eprintln!(
                    "[recovery] Pruning aged recovery branch {} (age: {}s > {}s)",
                    branch, age, config.recovery_branch_max_age
                );

                if let Err(e) = delete_recovery_branch(project_root, branch) {
                    eprintln!(
                        "[recovery] Failed to delete aged recovery branch {}: {}",
                        branch, e
                    );
                } else {
                    pruned_count += 1;
                }
            }
        }
    }

    // Count-based pruning
    if config.recovery_branch_max_count > 0 {
        // Get fresh list after age-based pruning
        let remaining_branches = get_recovery_branches(project_root)?;
        let excess_count = remaining_branches
            .len()
            .saturating_sub(config.recovery_branch_max_count as usize);

        if excess_count > 0 {
            eprintln!(
                "[recovery] Pruning {} excess recovery branches (limit: {})",
                excess_count, config.recovery_branch_max_count
            );

            // Prune oldest branches first
            for (branch, _) in remaining_branches.iter().take(excess_count) {
                if let Err(e) = delete_recovery_branch(project_root, branch) {
                    eprintln!(
                        "[recovery] Failed to delete excess recovery branch {}: {}",
                        branch, e
                    );
                } else {
                    pruned_count += 1;
                }
            }
        }
    }

    if pruned_count > 0 {
        eprintln!("[recovery] Pruned {} recovery branches", pruned_count);
    }

    Ok(pruned_count)
}

/// Delete a recovery branch both locally and remotely (if present).
#[allow(dead_code)]
fn delete_recovery_branch(project_root: &Path, branch: &str) -> Result<()> {
    // Delete local branch
    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(project_root)
        .output()
        .context("Failed to execute git branch delete command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only log as warning if branch doesn't exist locally
        if !stderr.contains("not found") {
            eprintln!(
                "[recovery] Warning: Failed to delete local recovery branch {}: {}",
                branch,
                stderr.trim()
            );
        }
    }

    // Delete remote branch if it exists
    let output = Command::new("git")
        .args(["push", "origin", "--delete", branch])
        .current_dir(project_root)
        .output();

    if let Ok(output) = output
        && !output.status.success()
    {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only log as warning for actual errors, not "branch not found"
        if !stderr.contains("not found") && !stderr.contains("does not exist") {
            eprintln!(
                "[recovery] Warning: Failed to delete remote recovery branch {}: {}",
                branch,
                stderr.trim()
            );
        }
    }

    Ok(())
}

/// Run recovery branch pruning if enough time has passed since last prune.
/// This is typically called from the coordinator's triage loop.
#[allow(dead_code)]
pub fn maybe_prune_recovery_branches(
    project_root: &Path,
    config: &ResourceManagementConfig,
    last_prune_time: &mut SystemTime,
) -> Result<usize> {
    if config.recovery_prune_interval == 0 {
        return Ok(0); // Pruning disabled
    }

    let current_time = SystemTime::now();
    let elapsed = current_time
        .duration_since(*last_prune_time)
        .unwrap_or(Duration::from_secs(u64::MAX));

    if elapsed.as_secs() >= config.recovery_prune_interval {
        *last_prune_time = current_time;
        prune_recovery_branches(project_root, config)
    } else {
        Ok(0)
    }
}

/// A cleanup job to be processed by the cleanup queue.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CleanupJob {
    pub job_type: CleanupJobType,
    pub priority: CleanupPriority,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum CleanupJobType {
    DeadAgent {
        project_root: PathBuf,
        worktree_path: PathBuf,
        branch: String,
        agent_id: String,
    },
    OrphanedWorktree {
        project_root: PathBuf,
        worktree_path: PathBuf,
        agent_id: String,
    },
    RecoveryBranchPrune {
        project_root: PathBuf,
    },
}

impl std::fmt::Display for CleanupJobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CleanupJobType::DeadAgent {
                project_root,
                agent_id,
                ..
            } => {
                write!(
                    f,
                    "DeadAgent(project: {}, agent: {})",
                    project_root.display(),
                    agent_id
                )
            }
            CleanupJobType::OrphanedWorktree {
                project_root,
                agent_id,
                ..
            } => {
                write!(
                    f,
                    "OrphanedWorktree(project: {}, agent: {})",
                    project_root.display(),
                    agent_id
                )
            }
            CleanupJobType::RecoveryBranchPrune { project_root } => {
                write!(
                    f,
                    "RecoveryBranchPrune(project: {})",
                    project_root.display()
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum CleanupPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// A thread-safe cleanup job queue for coordinating worktree cleanup operations.
/// Prevents resource contention during high-frequency cleanup scenarios.
#[allow(dead_code)]
pub struct CleanupQueue {
    inner: Arc<Mutex<CleanupQueueInner>>,
    not_empty: Arc<Condvar>,
    not_full: Arc<Condvar>,
}

#[allow(dead_code)]
struct CleanupQueueInner {
    queue: VecDeque<CleanupJob>,
    max_size: usize,
    shutdown: bool,
}

#[allow(dead_code)]
impl CleanupQueue {
    /// Create a new cleanup queue with the specified maximum size.
    pub fn new(max_size: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CleanupQueueInner {
                queue: VecDeque::new(),
                max_size,
                shutdown: false,
            })),
            not_empty: Arc::new(Condvar::new()),
            not_full: Arc::new(Condvar::new()),
        }
    }

    /// Add a cleanup job to the queue. Blocks if the queue is full.
    pub fn enqueue(&self, job: CleanupJob) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();

        // Wait for space if queue is full
        while inner.queue.len() >= inner.max_size && !inner.shutdown {
            inner = self.not_full.wait(inner).unwrap();
        }

        if inner.shutdown {
            return Err(anyhow!("Cleanup queue is shutting down"));
        }

        // Insert job in priority order (higher priority first)
        let insert_pos = inner
            .queue
            .iter()
            .position(|existing| existing.priority < job.priority)
            .unwrap_or(inner.queue.len());

        inner.queue.insert(insert_pos, job);
        self.not_empty.notify_one();

        Ok(())
    }

    /// Remove and return the next job from the queue. Blocks if the queue is empty.
    pub fn dequeue(&self) -> Option<CleanupJob> {
        let mut inner = self.inner.lock().unwrap();

        // Wait for a job if queue is empty
        while inner.queue.is_empty() && !inner.shutdown {
            inner = self.not_empty.wait(inner).unwrap();
        }

        if inner.shutdown && inner.queue.is_empty() {
            return None;
        }

        let job = inner.queue.pop_front();
        if job.is_some() {
            self.not_full.notify_one();
        }

        job
    }

    /// Try to remove a job without blocking. Returns None if queue is empty.
    pub fn try_dequeue(&self) -> Option<CleanupJob> {
        let mut inner = self.inner.lock().unwrap();
        let job = inner.queue.pop_front();
        if job.is_some() {
            self.not_full.notify_one();
        }
        job
    }

    /// Signal the queue to shutdown and wake all waiting threads.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.shutdown = true;
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }

    /// Get the current queue size.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().queue.is_empty()
    }
}

/// A cleanup worker that processes jobs from the cleanup queue.
#[allow(dead_code)]
pub struct CleanupWorker {
    queue: Arc<CleanupQueue>,
    config: ResourceManagementConfig,
}

#[allow(dead_code)]
impl CleanupWorker {
    /// Create a new cleanup worker with the given queue and configuration.
    pub fn new(queue: Arc<CleanupQueue>, config: ResourceManagementConfig) -> Self {
        Self { queue, config }
    }

    /// Start the cleanup worker in a separate thread.
    /// Returns a join handle that can be used to wait for the worker to finish.
    pub fn start(self) -> std::thread::JoinHandle<()> {
        thread::spawn(move || {
            eprintln!("[cleanup] Cleanup worker started");

            while let Some(job) = self.queue.dequeue() {
                self.process_job(job);
            }

            eprintln!("[cleanup] Cleanup worker finished");
        })
    }

    /// Process a single cleanup job.
    fn process_job(&self, job: CleanupJob) {
        match job.job_type {
            CleanupJobType::DeadAgent {
                ref project_root,
                ref worktree_path,
                ref branch,
                ref agent_id,
            } => {
                eprintln!("[cleanup] Processing dead agent cleanup: {}", agent_id);
                cleanup_dead_agent_worktree_with_config(
                    project_root,
                    worktree_path,
                    branch,
                    agent_id,
                    Some(&self.config),
                );
            }
            CleanupJobType::OrphanedWorktree {
                ref project_root,
                ref worktree_path,
                ref agent_id,
            } => {
                eprintln!(
                    "[cleanup] Processing orphaned worktree cleanup: {}",
                    agent_id
                );

                // Try to find the branch for this worktree
                if let Some(branch) = find_branch_for_worktree(project_root, worktree_path) {
                    cleanup_dead_agent_worktree_with_config(
                        project_root,
                        worktree_path,
                        &branch,
                        agent_id,
                        Some(&self.config),
                    );
                } else {
                    // Fallback to manual cleanup
                    eprintln!(
                        "[cleanup] No branch found for orphaned worktree {}, using manual cleanup",
                        agent_id
                    );
                    if let Err(e) = attempt_force_cleanup(worktree_path) {
                        eprintln!("[cleanup] Manual cleanup failed for {}: {}", agent_id, e);
                    }
                }
            }
            CleanupJobType::RecoveryBranchPrune { ref project_root } => {
                eprintln!("[cleanup] Processing recovery branch pruning");
                if let Err(e) = prune_recovery_branches(project_root, &self.config) {
                    eprintln!("[cleanup] Recovery branch pruning failed: {}", e);
                }
            }
        }
    }
}

/// Enqueue a dead agent cleanup job.
#[allow(dead_code)]
pub fn enqueue_dead_agent_cleanup(
    queue: &CleanupQueue,
    project_root: PathBuf,
    worktree_path: PathBuf,
    branch: String,
    agent_id: String,
    priority: CleanupPriority,
) -> Result<()> {
    let job = CleanupJob {
        job_type: CleanupJobType::DeadAgent {
            project_root,
            worktree_path,
            branch,
            agent_id,
        },
        priority,
    };
    queue.enqueue(job)
}

/// Enqueue an orphaned worktree cleanup job.
#[allow(dead_code)]
pub fn enqueue_orphaned_cleanup(
    queue: &CleanupQueue,
    project_root: PathBuf,
    worktree_path: PathBuf,
    agent_id: String,
    priority: CleanupPriority,
) -> Result<()> {
    let job = CleanupJob {
        job_type: CleanupJobType::OrphanedWorktree {
            project_root,
            worktree_path,
            agent_id,
        },
        priority,
    };
    queue.enqueue(job)
}

/// Enqueue a recovery branch pruning job.
#[allow(dead_code)]
pub fn enqueue_recovery_prune(
    queue: &CleanupQueue,
    project_root: PathBuf,
    priority: CleanupPriority,
) -> Result<()> {
    let job = CleanupJob {
        job_type: CleanupJobType::RecoveryBranchPrune { project_root },
        priority,
    };
    queue.enqueue(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_git_repo(path: &Path) {
        Command::new("git")
            .args(["init", "-b", "main"])
            .arg(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        // Some old git versions ignore -b on init — force the branch name.
        Command::new("git")
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .current_dir(path)
            .output()
            .unwrap();
        fs::write(path.join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
    }

    /// Merge a branch into main so the retention policy sees it as merged.
    fn merge_branch_into_main(project: &Path, branch: &str) {
        Command::new("git")
            .args(["merge", "--no-ff", "--no-edit", branch])
            .current_dir(project)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
    }

    /// Write a graph file with one task at the given status. Used by sweep tests.
    fn write_graph_with_task_and_eval(
        wg_dir: &Path,
        task_id: &str,
        status: worksgood::graph::Status,
        eval_status: Option<worksgood::graph::Status>,
    ) {
        use worksgood::graph::{Node, Task, WorkGraph};
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: task_id.to_string(),
            title: "test".to_string(),
            status,
            ..Task::default()
        }));
        if let Some(es) = eval_status {
            graph.add_node(Node::Task(Task {
                id: format!(".evaluate-{}", task_id),
                title: format!("eval {}", task_id),
                status: es,
                ..Task::default()
            }));
        }
        let graph_path = wg_dir.join("graph.jsonl");
        worksgood::parser::save_graph(&graph, &graph_path).unwrap();
    }

    fn create_test_worktree(
        project: &Path,
        agent_id: &str,
        task_id: &str,
    ) -> (std::path::PathBuf, String) {
        let branch = format!("wg/{}/{}", agent_id, task_id);
        let wt_dir = project.join(WORKTREES_DIR).join(agent_id);
        fs::create_dir_all(project.join(WORKTREES_DIR)).unwrap();

        Command::new("git")
            .args(["worktree", "add"])
            .arg(&wt_dir)
            .args(["-b", &branch, "HEAD"])
            .current_dir(project)
            .output()
            .unwrap();

        (wt_dir, branch)
    }

    #[test]
    fn test_remove_worktree() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-1", "task-foo");
        assert!(wt_path.exists());

        remove_worktree(&project, &wt_path, &branch).unwrap();
        assert!(!wt_path.exists());

        // Branch should be deleted
        let output = Command::new("git")
            .args(["branch", "--list", &branch])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
    }

    #[test]
    fn test_recover_commits_no_commits() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (_wt_path, branch) = create_test_worktree(&project, "agent-2", "task-bar");
        let count = recover_commits(&project, &branch, "agent-2");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_recover_commits_with_commits() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-3", "task-baz");

        // Make a commit in the worktree
        fs::write(wt_path.join("new_file.txt"), "agent work").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "agent work"])
            .current_dir(&wt_path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();

        let count = recover_commits(&project, &branch, "agent-3");
        assert_eq!(count, 1);

        // Recovery branch should exist
        let recovery_branch = format!("recover/agent-3/task-baz");
        let output = Command::new("git")
            .args(["branch", "--list", &recovery_branch])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
            "Recovery branch should exist"
        );

        // Clean up
        remove_worktree(&project, &wt_path, &branch).unwrap();
    }

    #[test]
    fn test_cleanup_dead_agent_worktree() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-4", "task-qux");
        assert!(wt_path.exists());

        cleanup_dead_agent_worktree(&project, &wt_path, &branch, "agent-4");
        assert!(!wt_path.exists());
    }

    #[test]
    fn test_find_branch_for_worktree() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-5", "task-find");
        let found = find_branch_for_worktree(&project, &wt_path);
        assert_eq!(found, Some(branch.clone()));

        // Clean up
        remove_worktree(&project, &wt_path, &branch).unwrap();
    }

    #[test]
    fn test_remove_worktree_nonexistent_reports_errors() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        // Removing a nonexistent worktree should now report errors with enhanced error handling
        let fake_path = project.join(WORKTREES_DIR).join("agent-999");
        let result = remove_worktree(&project, &fake_path, "wg/agent-999/fake");
        // Should return an error now that we have enhanced error reporting
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Worktree removal completed with errors"));
    }

    #[test]
    fn test_retry_operation_success_first_try() {
        let mut call_count = 0;
        let result = retry_operation(
            || {
                call_count += 1;
                Ok("success")
            },
            "test operation",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(call_count, 1);
    }

    #[test]
    fn test_retry_operation_success_after_retries() {
        let mut call_count = 0;
        let result = retry_operation(
            || {
                call_count += 1;
                if call_count < 3 {
                    Err(anyhow::anyhow!("temporary failure"))
                } else {
                    Ok("success")
                }
            },
            "test operation",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(call_count, 3);
    }

    #[test]
    fn test_retry_operation_max_retries_exceeded() {
        let mut call_count = 0;
        let result: anyhow::Result<&str> = retry_operation(
            || {
                call_count += 1;
                Err(anyhow::anyhow!("persistent failure"))
            },
            "test operation",
        );
        assert!(result.is_err());
        assert_eq!(call_count, MAX_RETRIES + 1);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("persistent failure")
        );
    }

    #[test]
    fn test_enhanced_cleanup_with_corrupted_git() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-test", "task-test");
        assert!(wt_path.exists());

        // Simulate a corrupted state by creating an invalid .git file
        // This should trigger the enhanced error handling and fallback mechanisms
        let git_file = wt_path.join(".git");
        if git_file.exists() {
            // Overwrite .git with invalid content to simulate corruption
            let _ = fs::write(&git_file, "corrupted git content");
        }

        // The cleanup should still work due to enhanced error handling
        cleanup_dead_agent_worktree(&project, &wt_path, &branch, "agent-test");

        // Verify that the directory is removed or at least attempted
        // (The exact behavior may depend on the filesystem and permissions)
    }

    #[test]
    fn test_resource_management_config_defaults() {
        let config = ResourceManagementConfig::default();
        assert!(config.cleanup_verification);
        assert_eq!(config.recovery_branch_max_age, 604800); // 7 days
        assert_eq!(config.recovery_branch_max_count, 10);
        assert!(config.cleanup_job_queue);
        assert_eq!(config.cleanup_queue_size, 50);
        assert_eq!(config.recovery_prune_interval, 3600); // 1 hour
    }

    #[test]
    fn test_verify_worktree_cleanup_success() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        // Non-existent worktree should pass verification
        let fake_path = project.join("nonexistent");
        let result = verify_worktree_cleanup(&fake_path, "fake-branch", &project);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_worktree_cleanup_failure() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-verify", "task-verify");

        // Verification should fail because worktree still exists
        let result = verify_worktree_cleanup(&wt_path, &branch, &project);
        assert!(result.is_err());

        // Clean up for next test
        remove_worktree(&project, &wt_path, &branch).unwrap();
    }

    #[test]
    fn test_remove_worktree_verified() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) = create_test_worktree(&project, "agent-verified", "task-verified");

        let config = ResourceManagementConfig {
            cleanup_verification: true,
            ..Default::default()
        };

        // Verified removal should succeed and pass verification
        let result = remove_worktree_verified(&project, &wt_path, &branch, &config);
        assert!(result.is_ok());
        assert!(!wt_path.exists());
    }

    #[test]
    fn test_remove_worktree_verified_disabled() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let (wt_path, branch) =
            create_test_worktree(&project, "agent-unverified", "task-unverified");

        let config = ResourceManagementConfig {
            cleanup_verification: false,
            ..Default::default()
        };

        // Should work without verification
        let result = remove_worktree_verified(&project, &wt_path, &branch, &config);
        assert!(result.is_ok());
        assert!(!wt_path.exists());
    }

    #[test]
    fn test_cleanup_queue_basic() {
        let queue = CleanupQueue::new(5);
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());

        let job = CleanupJob {
            job_type: CleanupJobType::RecoveryBranchPrune {
                project_root: PathBuf::from("/tmp"),
            },
            priority: CleanupPriority::Normal,
        };

        queue.enqueue(job).unwrap();
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());

        let dequeued = queue.try_dequeue();
        assert!(dequeued.is_some());
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_cleanup_queue_priority_ordering() {
        let queue = CleanupQueue::new(10);

        // Add jobs with different priorities
        let low_job = CleanupJob {
            job_type: CleanupJobType::RecoveryBranchPrune {
                project_root: PathBuf::from("/tmp1"),
            },
            priority: CleanupPriority::Low,
        };

        let high_job = CleanupJob {
            job_type: CleanupJobType::RecoveryBranchPrune {
                project_root: PathBuf::from("/tmp2"),
            },
            priority: CleanupPriority::High,
        };

        let normal_job = CleanupJob {
            job_type: CleanupJobType::RecoveryBranchPrune {
                project_root: PathBuf::from("/tmp3"),
            },
            priority: CleanupPriority::Normal,
        };

        // Add in non-priority order
        queue.enqueue(low_job).unwrap();
        queue.enqueue(high_job).unwrap();
        queue.enqueue(normal_job).unwrap();

        // Should dequeue in priority order: High, Normal, Low
        let first = queue.try_dequeue().unwrap();
        assert_eq!(first.priority, CleanupPriority::High);

        let second = queue.try_dequeue().unwrap();
        assert_eq!(second.priority, CleanupPriority::Normal);

        let third = queue.try_dequeue().unwrap();
        assert_eq!(third.priority, CleanupPriority::Low);
    }

    #[test]
    fn test_enqueue_functions() {
        let queue = CleanupQueue::new(10);

        // Test enqueue_dead_agent_cleanup
        let result = enqueue_dead_agent_cleanup(
            &queue,
            PathBuf::from("/project"),
            PathBuf::from("/worktree"),
            "branch".to_string(),
            "agent-1".to_string(),
            CleanupPriority::High,
        );
        assert!(result.is_ok());
        assert_eq!(queue.len(), 1);

        // Test enqueue_orphaned_cleanup
        let result = enqueue_orphaned_cleanup(
            &queue,
            PathBuf::from("/project"),
            PathBuf::from("/orphaned"),
            "agent-2".to_string(),
            CleanupPriority::Normal,
        );
        assert!(result.is_ok());
        assert_eq!(queue.len(), 2);

        // Test enqueue_recovery_prune
        let result =
            enqueue_recovery_prune(&queue, PathBuf::from("/project"), CleanupPriority::Low);
        assert!(result.is_ok());
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn test_get_recovery_branches_empty_repo() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let branches = get_recovery_branches(&project).unwrap();
        assert!(branches.is_empty());
    }

    #[test]
    fn test_prune_recovery_branches_no_branches() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let config = ResourceManagementConfig {
            recovery_branch_max_age: 86400,
            recovery_branch_max_count: 5,
            ..Default::default()
        };

        let pruned = prune_recovery_branches(&project, &config).unwrap();
        assert_eq!(pruned, 0);
    }

    #[test]
    fn test_cleanup_orphaned_worktrees_skips_live_agents() {
        use worksgood::service::registry::{AgentEntry, AgentRegistry, AgentStatus};

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Create worktrees for two agents: one live, one dead
        let (live_wt, _live_branch) = create_test_worktree(&project, "agent-100", "task-live");
        let (dead_wt, _dead_branch) = create_test_worktree(&project, "agent-200", "task-dead");

        assert!(live_wt.exists());
        assert!(dead_wt.exists());

        // Build a registry where agent-100 is alive (use our own PID) and
        // agent-200 is dead (use a non-existent PID).
        let our_pid = std::process::id();
        let now = chrono::Utc::now().to_rfc3339();
        let mut registry = AgentRegistry::default();
        registry.agents.insert(
            "agent-100".to_string(),
            AgentEntry {
                id: "agent-100".to_string(),
                pid: our_pid,
                task_id: "task-live".to_string(),
                executor: "test".to_string(),
                started_at: now.clone(),
                last_heartbeat: now.clone(),
                status: AgentStatus::Working,
                output_file: String::new(),
                model: None,
                completed_at: None,
                worktree_path: None,
            },
        );
        registry.agents.insert(
            "agent-200".to_string(),
            AgentEntry {
                id: "agent-200".to_string(),
                pid: 999_999_999, // non-existent PID
                task_id: "task-dead".to_string(),
                executor: "test".to_string(),
                started_at: now.clone(),
                last_heartbeat: now.clone(),
                status: AgentStatus::Dead,
                output_file: String::new(),
                model: None,
                completed_at: None,
                worktree_path: None,
            },
        );
        registry.save(&wg_dir).unwrap();

        // Set up the dead-agent task as Done + eval-pass + merged so the
        // retention policy permits cleanup. The live-agent task is left Open.
        write_graph_with_task_and_eval(
            &wg_dir,
            "task-dead",
            worksgood::graph::Status::Done,
            Some(worksgood::graph::Status::Done),
        );
        merge_branch_into_main(&project, "wg/agent-200/task-dead");

        // Run orphan cleanup
        let cleaned = cleanup_orphaned_worktrees(&wg_dir).unwrap();

        // Dead agent's worktree should be cleaned
        assert_eq!(cleaned, 1, "should clean exactly 1 orphaned worktree");
        assert!(!dead_wt.exists(), "dead agent worktree should be removed");

        // Live agent's worktree MUST survive
        assert!(live_wt.exists(), "live agent worktree must NOT be removed");
    }

    // ---------- Two-phase atomic cleanup tests ----------

    fn write_graph_with_task(wg_dir: &Path, task_id: &str, status: worksgood::graph::Status) {
        use worksgood::graph::{Node, Task, WorkGraph};
        let mut graph = WorkGraph::new();
        let task = Task {
            id: task_id.to_string(),
            title: "test".to_string(),
            status,
            ..Task::default()
        };
        graph.add_node(Node::Task(task));
        let graph_path = wg_dir.join("graph.jsonl");
        worksgood::parser::save_graph(&graph, &graph_path).unwrap();
    }

    fn register_agent(
        wg_dir: &Path,
        agent_id: &str,
        task_id: &str,
        pid: u32,
        status: worksgood::service::registry::AgentStatus,
    ) {
        register_agent_with_worktree(wg_dir, agent_id, task_id, pid, status, None);
    }

    fn register_agent_with_worktree(
        wg_dir: &Path,
        agent_id: &str,
        task_id: &str,
        pid: u32,
        status: worksgood::service::registry::AgentStatus,
        worktree_path: Option<&Path>,
    ) {
        use worksgood::service::registry::{AgentEntry, AgentRegistry};
        let now = chrono::Utc::now().to_rfc3339();
        let mut registry = AgentRegistry::load(wg_dir).unwrap_or_default();
        registry.agents.insert(
            agent_id.to_string(),
            AgentEntry {
                id: agent_id.to_string(),
                pid,
                task_id: task_id.to_string(),
                executor: "test".to_string(),
                started_at: now.clone(),
                last_heartbeat: now.clone(),
                status,
                output_file: String::new(),
                model: None,
                completed_at: None,
                worktree_path: worktree_path.map(|p| p.to_string_lossy().to_string()),
            },
        );
        registry.save(wg_dir).unwrap();
    }

    #[test]
    fn atomic_worktree_cleanup_on_success() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Agent completed successfully AND eval passed AND branch merged into main.
        // Only this combination is safe to reap under the retention policy.
        let (wt_path, branch) = create_test_worktree(&project, "agent-ok", "task-ok");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-ok", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir,
            "agent-ok",
            "task-ok",
            999_999_999,
            AgentStatus::Done,
        );
        merge_branch_into_main(&project, &branch);

        assert!(wt_path.exists(), "precondition: worktree exists");

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 1, "should remove exactly one worktree");
        assert!(
            !wt_path.exists(),
            "worktree must be removed when eval-passed AND merged"
        );
    }

    /// New retention policy (worktree-retention-don):
    /// Failed tasks must NOT have their worktree reaped, regardless of marker
    /// or agent state. The WIP needs to survive for `wg retry`-in-place.
    #[test]
    fn test_worktree_not_reaped_on_agent_failure() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Agent failed: task=Failed, agent=Failed, marker present.
        // Under new policy: must NOT be reaped (preserves WIP for retry).
        let (wt_path, _branch) = create_test_worktree(&project, "agent-fail", "task-fail");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-fail", Status::Failed, None);
        register_agent(
            &wg_dir,
            "agent-fail",
            "task-fail",
            999_999_998,
            AgentStatus::Failed,
        );

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 0, "MUST NOT reap on failure — WIP must survive");
        assert!(
            wt_path.exists(),
            "worktree must survive agent failure for retry-in-place"
        );
    }

    #[test]
    fn atomic_worktree_cleanup_skips_no_marker() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Agent may have died before writing marker: don't touch it from sweep.
        // (The dead-agent reaper path is responsible for that case.)
        let (wt_path, _branch) = create_test_worktree(&project, "agent-nomark", "task-nomark");
        write_graph_with_task(&wg_dir, "task-nomark", Status::Done);
        register_agent(
            &wg_dir,
            "agent-nomark",
            "task-nomark",
            999_999_997,
            AgentStatus::Dead,
        );

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 0);
        assert!(
            wt_path.exists(),
            "worktree without marker must NOT be swept — sacred-worktree invariant"
        );
    }

    #[test]
    fn atomic_worktree_cleanup_skips_live_agent() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Marker present but agent is still live (stuck wrapper + race):
        // MUST refuse to remove.
        let (wt_path, _branch) = create_test_worktree(&project, "agent-live", "task-live");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task(&wg_dir, "task-live", Status::InProgress);
        register_agent(
            &wg_dir,
            "agent-live",
            "task-live",
            std::process::id(), // our own PID — definitely alive
            AgentStatus::Working,
        );

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 0);
        assert!(wt_path.exists(), "must not remove worktree of live agent");
    }

    #[test]
    fn atomic_worktree_cleanup_skips_in_progress_task() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Marker present, agent dead, but task still in-progress (triage will
        // unclaim it). We must not yank the worktree before the task transitions.
        let (wt_path, _branch) = create_test_worktree(&project, "agent-ip", "task-ip");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task(&wg_dir, "task-ip", Status::InProgress);
        register_agent(
            &wg_dir,
            "agent-ip",
            "task-ip",
            999_999_996,
            AgentStatus::Dead,
        );

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 0);
        assert!(
            wt_path.exists(),
            "must not remove worktree when task is still in-progress"
        );
    }

    #[test]
    fn atomic_worktree_cleanup_orphan_agent_checks_task_via_branch() {
        use worksgood::graph::Status;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Worktree exists with marker, but agent has NO registry entry
        // (registry wiped, or manual drop). Task is still Open — non-terminal.
        // The sweep must infer task_id from the branch name and refuse to remove.
        let (wt_path, _branch) = create_test_worktree(&project, "agent-orphan", "task-orphan");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task(&wg_dir, "task-orphan", Status::Open);
        // Ensure an empty registry file exists so load() doesn't fail.
        worksgood::service::registry::AgentRegistry::default()
            .save(&wg_dir)
            .unwrap();

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(
            removed, 0,
            "orphan agent (no registry entry) with Open task must NOT be swept"
        );
        assert!(wt_path.exists());
    }

    #[test]
    fn atomic_worktree_cleanup_orphan_agent_terminal_task_is_swept() {
        use worksgood::graph::Status;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Same as above but task is Done with eval pass + merged branch —
        // now it's safe to remove.
        let (wt_path, branch) = create_test_worktree(&project, "agent-orph2", "task-orph2");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-orph2", Status::Done, Some(Status::Done));
        worksgood::service::registry::AgentRegistry::default()
            .save(&wg_dir)
            .unwrap();
        merge_branch_into_main(&project, &branch);

        let removed = sweep_cleanup_pending_worktrees(&wg_dir).unwrap();
        assert_eq!(removed, 1);
        assert!(!wt_path.exists());
    }

    #[test]
    fn atomic_worktree_cleanup_idempotent() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        let (wt_path, branch) = create_test_worktree(&project, "agent-idem", "task-idem");
        fs::write(wt_path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-idem", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir,
            "agent-idem",
            "task-idem",
            999_999_995,
            AgentStatus::Done,
        );
        merge_branch_into_main(&project, &branch);

        // First sweep removes it
        assert_eq!(sweep_cleanup_pending_worktrees(&wg_dir).unwrap(), 1);
        assert!(!wt_path.exists());

        // Second sweep is a no-op (worktree already gone) — must not error
        assert_eq!(sweep_cleanup_pending_worktrees(&wg_dir).unwrap(), 0);
    }

    #[test]
    fn atomic_cleanup_ignores_only_marker_and_preserves_real_dirty_source() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        let (wt, branch) = create_test_worktree(&project, "agent-dirty", "task-dirty");
        fs::write(wt.join(CLEANUP_PENDING_MARKER), "").unwrap();
        fs::write(wt.join("valuable-untracked.rs"), "preserve me").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-dirty", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir,
            "agent-dirty",
            "task-dirty",
            999_999_994,
            AgentStatus::Done,
        );
        merge_branch_into_main(&project, &branch);

        assert_eq!(sweep_cleanup_pending_worktrees(&wg_dir).unwrap(), 0);
        assert_eq!(
            fs::read_to_string(wt.join("valuable-untracked.rs")).unwrap(),
            "preserve me"
        );
        assert!(wt.join(CLEANUP_PENDING_MARKER).exists());
    }

    /// New retention policy (worktree-retention-don):
    /// Both eval-pass AND merge-to-main are required. Either alone keeps the
    /// worktree alive.
    #[test]
    fn test_worktree_reaped_only_after_eval_pass_and_merge() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        // ---- Scenario A: Done but eval pending → KEEP ----
        let temp_a = TempDir::new().unwrap();
        let project_a = temp_a.path().join("project");
        fs::create_dir_all(&project_a).unwrap();
        init_git_repo(&project_a);
        let wg_dir_a = project_a.join(".wg");
        fs::create_dir_all(wg_dir_a.join("service")).unwrap();

        let (wt_a, branch_a) = create_test_worktree(&project_a, "agent-a", "task-a");
        fs::write(wt_a.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir_a, "task-a", Status::Done, Some(Status::Open));
        register_agent(
            &wg_dir_a,
            "agent-a",
            "task-a",
            999_999_991,
            AgentStatus::Done,
        );
        merge_branch_into_main(&project_a, &branch_a);

        assert_eq!(
            sweep_cleanup_pending_worktrees(&wg_dir_a).unwrap(),
            0,
            "Done + merged but eval not yet Done → MUST keep worktree"
        );
        assert!(wt_a.exists());

        // ---- Scenario B: Done + eval-pass but NOT merged → KEEP ----
        let temp_b = TempDir::new().unwrap();
        let project_b = temp_b.path().join("project");
        fs::create_dir_all(&project_b).unwrap();
        init_git_repo(&project_b);
        let wg_dir_b = project_b.join(".wg");
        fs::create_dir_all(wg_dir_b.join("service")).unwrap();

        let (wt_b, _branch_b) = create_test_worktree(&project_b, "agent-b", "task-b");
        // Add a commit on the branch so it's distinguishable from main.
        // Without this, the new branch shares HEAD with main and the
        // is-ancestor check trivially succeeds.
        fs::write(wt_b.join("delta.txt"), "branch-only").unwrap();
        Command::new("git")
            .args(["add", "delta.txt"])
            .current_dir(&wt_b)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "branch work"])
            .current_dir(&wt_b)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        fs::write(wt_b.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir_b, "task-b", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir_b,
            "agent-b",
            "task-b",
            999_999_990,
            AgentStatus::Done,
        );
        // Branch has its own commit, NOT merged into main.

        assert_eq!(
            sweep_cleanup_pending_worktrees(&wg_dir_b).unwrap(),
            0,
            "Done + eval-pass but not merged → MUST keep worktree"
        );
        assert!(wt_b.exists());

        // ---- Scenario C: Done + eval-pass + merged → REAP ----
        let temp_c = TempDir::new().unwrap();
        let project_c = temp_c.path().join("project");
        fs::create_dir_all(&project_c).unwrap();
        init_git_repo(&project_c);
        let wg_dir_c = project_c.join(".wg");
        fs::create_dir_all(wg_dir_c.join("service")).unwrap();

        let (wt_c, branch_c) = create_test_worktree(&project_c, "agent-c", "task-c");
        fs::write(wt_c.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir_c, "task-c", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir_c,
            "agent-c",
            "task-c",
            999_999_989,
            AgentStatus::Done,
        );
        merge_branch_into_main(&project_c, &branch_c);

        assert_eq!(
            sweep_cleanup_pending_worktrees(&wg_dir_c).unwrap(),
            1,
            "Done + eval-pass + merged → reap"
        );
        assert!(!wt_c.exists());
    }

    #[test]
    fn patch_equivalent_squash_or_cherry_pick_is_safe_to_archive() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        let (wt, branch) = create_test_worktree(&project, "agent-patch", "task-patch");
        fs::write(wt.join("patch.txt"), "same patch on main\n").unwrap();
        Command::new("git")
            .args(["add", "patch.txt"])
            .current_dir(&wt)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "branch patch"])
            .current_dir(&wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
            .status()
            .unwrap();
        let branch_tip = Command::new("git")
            .args(["rev-parse", &branch])
            .current_dir(&project)
            .output()
            .unwrap();
        let branch_tip = String::from_utf8(branch_tip.stdout).unwrap();
        Command::new("git")
            .args(["cherry-pick", branch_tip.trim()])
            .current_dir(&project)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_DATE", "2021-01-01T00:00:00Z")
            .status()
            .unwrap();
        assert!(is_branch_merged(&project, &branch));

        fs::write(wt.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-patch", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir,
            "agent-patch",
            "task-patch",
            999_999_988,
            AgentStatus::Done,
        );
        assert_eq!(sweep_cleanup_pending_worktrees(&wg_dir).unwrap(), 1);
        assert!(!wt.exists());
    }

    /// Crash without marker: under new policy, orphan cleanup should ALSO
    /// preserve the worktree until eval+merge — so `wg retry` can resume.
    #[test]
    fn test_orphan_cleanup_preserves_unfinished_work() {
        use worksgood::graph::Status;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Worktree exists, no agent registered (crash scenario), no marker.
        // Task is Failed (crashed). Under new policy: KEEP.
        let (wt_path, _branch) = create_test_worktree(&project, "agent-crash", "task-crash");
        write_graph_with_task_and_eval(&wg_dir, "task-crash", Status::Failed, None);
        worksgood::service::registry::AgentRegistry::default()
            .save(&wg_dir)
            .unwrap();

        let cleaned = cleanup_orphaned_worktrees(&wg_dir).unwrap();
        assert_eq!(cleaned, 0, "MUST NOT reap orphan with unfinished work");
        assert!(wt_path.exists(), "WIP must survive for retry");
    }

    // ---------- Target-dir reaper tests (worktree-target-dirs) ----------

    /// Helper: create a fake `target/` dir with some byte content so we can
    /// observe size accounting and removal.
    fn populate_fake_target(worktree_path: &Path) -> u64 {
        let target = worktree_path.join("target");
        fs::create_dir_all(target.join("debug/build")).unwrap();
        let payload = b"x".repeat(4096);
        fs::write(target.join("debug/build/artifact.o"), &payload).unwrap();
        fs::write(target.join("debug/.fingerprint"), &payload).unwrap();
        payload.len() as u64 * 2
    }

    #[test]
    fn reap_target_dir_removes_dir_and_reports_size() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let (wt_path, _branch) = create_test_worktree(&project, "agent-A", "task-A");

        let written = populate_fake_target(&wt_path);
        assert!(wt_path.join("target").exists());

        let freed = reap_target_dir(&wt_path).unwrap();
        assert!(
            freed >= written,
            "reported freed bytes {} should be >= written {}",
            freed,
            written
        );
        assert!(
            !wt_path.join("target").exists(),
            "target/ must be removed after reap"
        );
        // Source files (the worktree itself) must remain.
        assert!(
            wt_path.join("file.txt").exists(),
            "source files must survive target reap"
        );
        assert!(
            wt_path.join(".git").exists(),
            ".git pointer must survive target reap"
        );
    }

    #[test]
    fn reap_target_dir_no_target_returns_zero() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let (wt_path, _branch) = create_test_worktree(&project, "agent-B", "task-B");

        // No target/ to begin with.
        assert!(!wt_path.join("target").exists());
        let freed = reap_target_dir(&wt_path).unwrap();
        assert_eq!(freed, 0);
    }

    #[test]
    fn reap_dead_target_dirs_skips_live_agents() {
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Live agent: registered with our own PID + fresh heartbeat.
        let (live_wt, _) = create_test_worktree(&project, "agent-live", "task-live");
        populate_fake_target(&live_wt);
        register_agent(
            &wg_dir,
            "agent-live",
            "task-live",
            std::process::id(),
            AgentStatus::Working,
        );

        // Dead agent: registered with a non-existent PID.
        let (dead_wt, _) = create_test_worktree(&project, "agent-dead", "task-dead");
        populate_fake_target(&dead_wt);
        register_agent(
            &wg_dir,
            "agent-dead",
            "task-dead",
            999_999_999,
            AgentStatus::Dead,
        );

        let (reaped, freed) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(reaped, 1, "exactly one target/ should be reaped");
        assert!(freed > 0, "should report bytes freed");

        assert!(
            live_wt.join("target").exists(),
            "live agent's target/ MUST NOT be touched"
        );
        assert!(
            !dead_wt.join("target").exists(),
            "dead agent's target/ must be reaped"
        );

        // The worktrees themselves must survive.
        assert!(live_wt.exists());
        assert!(dead_wt.exists());
    }

    #[test]
    fn reap_dead_target_dirs_handles_orphan_with_no_registry_entry() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        // Save an empty registry so load() succeeds.
        worksgood::service::registry::AgentRegistry::default()
            .save(&wg_dir)
            .unwrap();

        // Worktree with target/ but agent not registered (crashed before reg).
        let (wt_path, _) = create_test_worktree(&project, "agent-orphan", "task-orphan");
        populate_fake_target(&wt_path);

        let (reaped, _freed) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(reaped, 1, "orphan (no registry entry) is not live → reap");
        assert!(!wt_path.join("target").exists());
        assert!(wt_path.exists(), "worktree itself must remain");
    }

    #[test]
    fn reap_dead_target_dirs_idempotent() {
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        let (wt_path, _) = create_test_worktree(&project, "agent-dead", "task-dead");
        populate_fake_target(&wt_path);
        register_agent(
            &wg_dir,
            "agent-dead",
            "task-dead",
            999_999_999,
            AgentStatus::Dead,
        );

        let (first, _) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(first, 1);

        // Running again is a no-op (no target/ left to reap).
        let (second, second_bytes) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(second, 0);
        assert_eq!(second_bytes, 0);
    }

    /// Regression — reaper-edge-case:
    ///
    /// `wg retry`-in-place reuses agent-A's worktree for the new agent-B.
    /// Registry now has TWO entries that point at the same worktree path:
    /// agent-A (Dead, the original owner that gave the dir its name) and
    /// agent-B (Working, the live retry agent). The reaper must NOT touch
    /// `target/` while agent-B is alive, even though the directory-name
    /// lookup (agent-A) reports dead.
    #[test]
    fn reap_dead_target_dirs_protects_retry_in_place_worktree() {
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        // Worktree directory was created for agent-A; the dir is named "agent-A".
        let (wt_path, _branch) = create_test_worktree(&project, "agent-A", "task-shared");
        populate_fake_target(&wt_path);

        // agent-A: original owner, now dead (failed task → wg retry).
        register_agent_with_worktree(
            &wg_dir,
            "agent-A",
            "task-shared",
            999_999_999,
            AgentStatus::Failed,
            Some(&wt_path),
        );

        // agent-B: the live `wg retry` agent occupying agent-A's worktree.
        // Same worktree_path, but its ID does NOT match the directory name.
        register_agent_with_worktree(
            &wg_dir,
            "agent-B",
            "task-shared",
            std::process::id(), // our own PID — definitely alive
            AgentStatus::Working,
            Some(&wt_path),
        );

        let (reaped, freed) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(
            reaped, 0,
            "MUST NOT reap target/ when ANY live agent occupies the worktree (wg retry-in-place)"
        );
        assert_eq!(freed, 0);
        assert!(
            wt_path.join("target").exists(),
            "live retry agent's target/ must survive — would force slow rebuild on resume"
        );
    }

    #[test]
    fn retained_sweep_is_bounded_and_deduplicates_negative_entries() {
        use worksgood::graph::{Node, Status, Task, WorkGraph};
        use worksgood::service::registry::{AgentEntry, AgentRegistry, AgentStatus};

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let wg_dir = project.join(".wg");
        let worktrees = project.join(WORKTREES_DIR);
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        worksgood::parser::save_graph(&WorkGraph::new(), wg_dir.join("graph.jsonl")).unwrap();
        AgentRegistry::default().save(&wg_dir).unwrap();

        for index in 0..80 {
            let path = worktrees.join(format!("agent-{index:03}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(CLEANUP_PENDING_MARKER), "").unwrap();
        }

        let mut cache = RetainedSweepCache::default();
        let first = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(first.candidates, 80);
        assert_eq!(first.examined, 7, "one batch must have a hard entry bound");
        assert!(first.deferred);

        let mut first_examined = first.examined;
        while cache.cursor != 0 {
            first_examined += sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7)
                .unwrap()
                .examined;
        }
        assert_eq!(first_examined, 80);

        let mut second_examined = 0;
        let mut second_hits = 0;
        loop {
            let report = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
            second_examined += report.examined;
            second_hits += report.cache_hits;
            if !report.deferred {
                break;
            }
        }
        assert_eq!(second_examined, 80);
        assert_eq!(
            second_hits, 80,
            "unchanged retained entries use negative cache"
        );

        // Marker content is keyed directly (not by an eventually-consistent
        // mtime), so a relevant marker change invalidates immediately.
        fs::write(
            worktrees.join("agent-000").join(CLEANUP_PENDING_MARKER),
            "changed",
        )
        .unwrap();
        let marker_changed = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(marker_changed.invalidated, 1);
        assert_eq!(marker_changed.cache_hits, 6);

        // Ownership/liveness and task/evaluation state each participate in the
        // key. Use our live PID first, then a dead owner and changed graph.
        let now = chrono::Utc::now().to_rfc3339();
        let mut registry = AgentRegistry::default();
        registry.agents.insert(
            "agent-000".to_string(),
            AgentEntry {
                id: "agent-000".to_string(),
                pid: std::process::id(),
                task_id: "task-000".to_string(),
                executor: "test".to_string(),
                started_at: now.clone(),
                last_heartbeat: now.clone(),
                status: AgentStatus::Working,
                output_file: String::new(),
                model: None,
                completed_at: None,
                worktree_path: Some(worktrees.join("agent-000").to_string_lossy().to_string()),
            },
        );
        registry.save(&wg_dir).unwrap();
        cache.cursor = 0;
        let live_changed = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(live_changed.invalidated, 1);

        registry.agents.get_mut("agent-000").unwrap().pid = 999_999_999;
        registry.agents.get_mut("agent-000").unwrap().status = AgentStatus::Done;
        registry.save(&wg_dir).unwrap();
        cache.cursor = 0;
        let liveness_changed =
            sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(liveness_changed.invalidated, 1);

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "task-000".to_string(),
            title: "task".to_string(),
            status: Status::Done,
            ..Task::default()
        }));
        graph.add_node(Node::Task(Task {
            id: ".evaluate-task-000".to_string(),
            title: "eval".to_string(),
            status: Status::Open,
            ..Task::default()
        }));
        worksgood::parser::save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();
        cache.cursor = 0;
        let graph_changed = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(graph_changed.invalidated, 1);

        graph.get_task_mut(".evaluate-task-000").unwrap().status = Status::Done;
        worksgood::parser::save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();
        cache.cursor = 0;
        let eval_changed = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 7).unwrap();
        assert_eq!(eval_changed.invalidated, 1);
    }

    #[test]
    fn retained_merge_and_source_changes_invalidate_without_cached_deletion() {
        use worksgood::graph::Status;
        use worksgood::service::registry::AgentStatus;

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        let (worktree, branch) = create_test_worktree(&project, "agent-key", "task-key");
        fs::write(worktree.join("branch.txt"), "branch-only").unwrap();
        Command::new("git")
            .args(["add", "branch.txt"])
            .current_dir(&worktree)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "branch change"])
            .current_dir(&worktree)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
        fs::write(worktree.join(CLEANUP_PENDING_MARKER), "").unwrap();
        write_graph_with_task_and_eval(&wg_dir, "task-key", Status::Done, Some(Status::Done));
        register_agent(
            &wg_dir,
            "agent-key",
            "task-key",
            999_999_999,
            AgentStatus::Done,
        );

        let mut cache = RetainedSweepCache::default();
        let first = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 8).unwrap();
        assert_eq!(first.merge_checks, 1);
        assert_eq!(first.removed, 0);
        let unchanged = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 8).unwrap();
        assert_eq!(unchanged.cache_hits, 1);
        assert_eq!(
            unchanged.merge_checks, 0,
            "unchanged merge proof is not rerun"
        );

        merge_branch_into_main(&project, &branch);
        fs::write(worktree.join("valuable-untracked.rs"), "keep").unwrap();
        let dirty = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 8).unwrap();
        assert_eq!(
            dirty.invalidated, 1,
            "main-head change invalidates retained proof"
        );
        assert_eq!(dirty.source_checks, 1);
        assert_eq!(dirty.removed, 0);
        assert!(
            worktree.exists(),
            "fresh dirty-source gate must preserve source"
        );

        fs::remove_file(worktree.join("valuable-untracked.rs")).unwrap();
        let cleaned = sweep_cleanup_pending_worktrees_batch(&wg_dir, &mut cache, 8).unwrap();
        assert_eq!(
            cleaned.source_checks, 1,
            "dirty decisions are never positive-cached"
        );
        assert_eq!(cleaned.removed, 1);
        assert!(!worktree.exists());
    }

    #[test]
    fn blocked_cleanup_lane_coalesces_and_stays_single_flight() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();
        worksgood::parser::save_graph(
            &worksgood::graph::WorkGraph::new(),
            wg_dir.join("graph.jsonl"),
        )
        .unwrap();
        worksgood::service::registry::AgentRegistry::default()
            .save(&wg_dir)
            .unwrap();

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let runs = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let runner: CleanupRunner = {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let runs = Arc::clone(&runs);
            let release_rx = Arc::clone(&release_rx);
            Box::new(move || {
                let now = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                max_active.fetch_max(now, AtomicOrdering::SeqCst);
                runs.fetch_add(1, AtomicOrdering::SeqCst);
                started_tx.send(()).unwrap();
                let released = release_rx.lock().unwrap().recv().is_ok();
                active.fetch_sub(1, AtomicOrdering::SeqCst);
                if !released {
                    return Ok(CleanupSweepReport::default());
                }
                Ok(CleanupSweepReport::default())
            })
        };
        let log: CleanupLog = Arc::new(|_, _| {});
        let lane = RetainedWorktreeCleanupLane::start_with_runner(
            wg_dir.clone(),
            log,
            Duration::ZERO,
            runner,
        )
        .unwrap();

        lane.request("fs-watch");
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let request_started = Instant::now();
        for _ in 0..100 {
            lane.request("timer");
        }
        assert!(
            request_started.elapsed() < Duration::from_millis(100),
            "wake coalescing must be non-blocking and filesystem-free"
        );

        // The injected filesystem sweep remains blocked while ordinary callers
        // continue immediately. The owned live-daemon smoke exercises a real
        // coordinator tick and shell spawn under this same blocked condition.
        assert_eq!(active.load(AtomicOrdering::SeqCst), 1);

        release_tx.send(()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        release_tx.send(()).unwrap();
        for _ in 0..100 {
            if lane.snapshot().batches_completed >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            runs.load(AtomicOrdering::SeqCst),
            2,
            "100 overlapping wakes coalesce to one deferred run"
        );
        assert_eq!(
            max_active.load(AtomicOrdering::SeqCst),
            1,
            "destructive sweeps are single-flight"
        );
        assert!(lane.snapshot().coalesced >= 99);
    }

    #[test]
    fn reap_dead_target_dirs_no_worktrees_dir() {
        // If `.wg-worktrees` doesn't exist, reaper should return Ok((0, 0)).
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let wg_dir = project.join(".wg");
        fs::create_dir_all(wg_dir.join("service")).unwrap();

        let (reaped, freed) = reap_dead_target_dirs(&wg_dir).unwrap();
        assert_eq!(reaped, 0);
        assert_eq!(freed, 0);
    }
}

/// Fix permissions on a file and attempt removal
/// This provides a fallback strategy for permission-denied errors
#[cfg(unix)]
fn fix_permissions_and_remove_file(file_path: &Path) -> Result<()> {
    // Try to make the file writable
    if let Ok(metadata) = fs::metadata(file_path) {
        let mut perms = metadata.permissions();
        perms.set_mode(0o644); // Read/write for owner, read for others

        fs::set_permissions(file_path, perms)
            .with_context(|| format!("Failed to fix file permissions for {:?}", file_path))?;

        // Retry removal after permission fix
        fs::remove_file(file_path).with_context(|| {
            format!("Failed to remove file {:?} after permission fix", file_path)
        })?;
    }

    Ok(())
}

/// Fix permissions on a directory and its contents, then attempt removal
/// This provides a fallback strategy for permission-denied errors
#[cfg(unix)]
fn fix_permissions_and_remove_dir(dir_path: &Path) -> Result<()> {
    if !dir_path.exists() {
        return Ok(());
    }

    // Recursively fix permissions
    fn fix_permissions_recursive(path: &Path) -> Result<()> {
        if path.is_dir() {
            // Make directory executable/readable
            if let Ok(metadata) = fs::metadata(path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755); // rwxr-xr-x
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
                perms.set_mode(0o644); // rw-r--r--
                let _ = fs::set_permissions(path, perms);
            }
        }
        Ok(())
    }

    fix_permissions_recursive(dir_path)
        .with_context(|| format!("Failed to fix directory permissions for {:?}", dir_path))?;

    // Retry removal after permission fix
    fs::remove_dir_all(dir_path).with_context(|| {
        format!(
            "Failed to remove directory {:?} after permission fix",
            dir_path
        )
    })?;

    Ok(())
}

/// Fallback implementations for non-Unix systems
#[cfg(not(unix))]
fn fix_permissions_and_remove_file(file_path: &Path) -> Result<()> {
    fs::remove_file(file_path).with_context(|| {
        format!(
            "Failed to remove file {:?} (permission fix not available on this platform)",
            file_path
        )
    })
}

#[cfg(not(unix))]
fn fix_permissions_and_remove_dir(dir_path: &Path) -> Result<()> {
    fs::remove_dir_all(dir_path).with_context(|| {
        format!(
            "Failed to remove directory {:?} (permission fix not available on this platform)",
            dir_path
        )
    })
}
