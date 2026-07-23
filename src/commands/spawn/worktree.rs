//! Git worktree isolation for spawned agents.
//!
//! When worktree isolation is enabled, each agent gets its own git worktree
//! at `.wg-worktrees/<agent-id>/`, branched from HEAD. The `.wg/`
//! directory is symlinked into the worktree so the `wg` CLI works normally.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// The Windows `\\?\` verbatim-prefix stripping lives in one place — the
// shared `strip_verbatim_prefix` in this module's parent (`spawn/mod.rs`),
// consolidating njt's #24/#25/#28. `git worktree add` and `git -C` both bail
// on verbatim paths, so the call sites below strip first.
use super::strip_verbatim_prefix;

const OWNER_SCHEMA: u32 = 1;
const OWNER_FILE: &str = "wg-spawn-owner.json";

#[cfg(test)]
thread_local! {
    static CREATE_FAULT_BOUNDARY: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

fn creation_fault(boundary: &str) -> Result<()> {
    #[cfg(test)]
    {
        let injected = CREATE_FAULT_BOUNDARY.with(|fault| *fault.borrow() == Some(boundary));
        if injected {
            anyhow::bail!("injected worktree creation failure at {boundary}");
        }
    }
    let _ = boundary;
    Ok(())
}

/// Worktree paths and metadata for an isolated agent workspace.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Branch name: `wg/<agent-id>/<task-id>`.
    pub branch: String,
    /// Absolute path to the main project root.
    pub project_root: PathBuf,
    /// Shared WG graph linked into the worktree.
    pub workgraph_dir: PathBuf,
    /// Agent and task ownership expected by the spawn transaction.
    pub agent_id: String,
    pub task_id: String,
    /// Source revision used to create this worktree. Rollback requires HEAD to
    /// remain exactly here, so even clean committed work from a setup hook or
    /// concurrent actor is preserved rather than force-deleted.
    base_oid: Option<String>,
    /// Present for worktrees created by this version of WG. Legacy worktrees
    /// remain reusable only after the same path/branch/gitdir checks pass.
    owner_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorktreeOwner {
    schema: u32,
    token: String,
    agent_id: String,
    task_id: String,
    branch: String,
    path: String,
    #[serde(default)]
    base_oid: Option<String>,
}

/// A collision is recoverable by allocating a different monotonically
/// increasing agent ID. It is deliberately distinct from an isolation setup
/// error, which must abort the spawn rather than degrade to the shared checkout.
#[derive(Debug)]
pub struct WorktreeCollision {
    message: String,
}

impl std::fmt::Display for WorktreeCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WorktreeCollision {}

pub fn is_collision(error: &anyhow::Error) -> bool {
    error.downcast_ref::<WorktreeCollision>().is_some()
}

fn collision(message: impl Into<String>) -> anyhow::Error {
    WorktreeCollision {
        message: message.into(),
    }
    .into()
}

/// Create a worktree for an agent.
///
/// 1. Error out if a worktree/branch with the same name already exists — worktrees
///    are sacred and must only be removed by explicit user action (`wg worktree archive`)
/// 2. `git worktree add .wg-worktrees/<agent-id> -b wg/<agent-id>/<task-id> HEAD`
/// 3. Symlink `.wg` into the worktree
/// 4. Run `worktree-setup.sh` if it exists (best-effort)
pub fn create_worktree(
    project_root: &Path,
    workgraph_dir: &Path,
    agent_id: &str,
    task_id: &str,
) -> Result<WorktreeInfo> {
    let branch = format!("wg/{}/{}", agent_id, task_id);
    let worktrees_dir = project_root.join(".wg-worktrees");
    let worktree_dir = worktrees_dir.join(agent_id);

    fs::create_dir_all(&worktrees_dir).with_context(|| {
        format!(
            "failed to create isolated-worktree parent {}",
            worktrees_dir.display()
        )
    })?;

    // Reserve the filesystem name atomically. `git worktree add` accepts an
    // existing *empty* directory. This closes the check-then-create race while
    // ensuring an unknown or dirty directory is never removed or overwritten.
    match fs::create_dir(&worktree_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(collision(format!(
                "isolated-worktree path collision: a path already exists at {} for {}; preserve it and allocate a new agent ID (inspect/remove explicitly with: wg worktree archive {} --remove)",
                worktree_dir.display(),
                agent_id,
                agent_id
            )));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to reserve isolated-worktree path {} for {}",
                    worktree_dir.display(),
                    agent_id
                )
            });
        }
    }

    if let Err(error) = creation_fault("path-reserved") {
        let _ = fs::remove_dir(&worktree_dir);
        return Err(error);
    }

    match agent_branch_exists(project_root, agent_id) {
        Ok(true) => {
            let _ = fs::remove_dir(&worktree_dir); // ours and still empty
            return Err(collision(format!(
                "isolated-worktree branch namespace collision at 'wg/{}/' for {}; preserve the existing branch(es) and allocate a new agent ID",
                agent_id, agent_id
            )));
        }
        Ok(false) => {}
        Err(error) => {
            let _ = fs::remove_dir(&worktree_dir); // roll back our empty reservation
            return Err(error);
        }
    }

    // Pin the source revision so a concurrent checkout movement cannot make
    // ownership cleanup ambiguous.
    let base_oid = match git_stdout(project_root, &["rev-parse", "HEAD"])
        .context("failed to resolve HEAD before isolated-worktree creation")
    {
        Ok(oid) => oid,
        Err(error) => {
            let _ = fs::remove_dir(&worktree_dir);
            return Err(error);
        }
    };
    let output = match Command::new("git")
        .args(["worktree", "add"])
        .arg(strip_verbatim_prefix(&worktree_dir))
        .args(["-b", &branch, &base_oid])
        .current_dir(strip_verbatim_prefix(project_root))
        .output()
        .context("Failed to run git worktree add")
    {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_dir(&worktree_dir);
            return Err(error);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let partial = WorktreeInfo {
            path: worktree_dir.clone(),
            branch: branch.clone(),
            project_root: project_root.to_path_buf(),
            workgraph_dir: workgraph_dir.to_path_buf(),
            agent_id: agent_id.to_string(),
            task_id: task_id.to_string(),
            base_oid: Some(base_oid.clone()),
            owner_token: None,
        };
        let exact_registration = git_worktree_entries(project_root)
            .unwrap_or_default()
            .into_iter()
            .any(|entry| {
                entry.branch.as_deref() == Some(branch.as_str())
                    && same_canonical_path(&entry.path, &worktree_dir)
            });
        if exact_registration {
            return match cleanup_unlaunched_worktree(&partial) {
                Ok(()) => Err(anyhow::anyhow!(
                    "git worktree add reported failure for {} at {} after registering it: {}. The strictly WG-owned partial worktree and branch were rolled back; retry is safe",
                    agent_id,
                    worktree_dir.display(),
                    stderr.trim()
                )),
                Err(cleanup) => Err(anyhow::anyhow!(
                    "git worktree add reported failure for {} at {} and rollback could not prove/complete ownership: {} (creation error: {}). Source was preserved; inspect `git worktree list`, repair with `git worktree repair`, then retry",
                    agent_id,
                    worktree_dir.display(),
                    cleanup,
                    stderr.trim()
                )),
            };
        }

        // Only remove the atomic reservation when it is still empty. If Git or
        // another actor left bytes behind, preserve them for explicit recovery.
        let reservation_removed = fs::remove_dir(&worktree_dir).is_ok();
        let collision_now =
            branch_exists(project_root, &branch).unwrap_or(false) || !reservation_removed;
        let message = format!(
            "git worktree add failed for {} at {} (branch '{}'): {}. {}",
            agent_id,
            worktree_dir.display(),
            branch,
            stderr.trim(),
            if reservation_removed {
                "the empty WG reservation was rolled back"
            } else {
                "the non-empty/unverified path was preserved; inspect it and run `git worktree repair` or archive it explicitly"
            }
        );
        if collision_now {
            return Err(collision(message));
        }
        anyhow::bail!(message);
    }

    let partial = WorktreeInfo {
        path: worktree_dir.clone(),
        branch: branch.clone(),
        project_root: project_root.to_path_buf(),
        workgraph_dir: workgraph_dir.to_path_buf(),
        agent_id: agent_id.to_string(),
        task_id: task_id.to_string(),
        base_oid: Some(base_oid.clone()),
        owner_token: None,
    };
    if let Err(error) = creation_fault("git-registered") {
        let cleanup = cleanup_unlaunched_worktree(&partial);
        return Err(error.context(format!(
            "worktree Git-registration boundary failed; cleanup={}",
            cleanup
                .err()
                .map(|e| format!("failed ({e:#}); source preserved"))
                .unwrap_or_else(|| "complete".to_string())
        )));
    }

    let token = uuid::Uuid::new_v4().to_string();
    let mut info = WorktreeInfo {
        path: worktree_dir,
        branch,
        project_root: project_root.to_path_buf(),
        workgraph_dir: workgraph_dir.to_path_buf(),
        agent_id: agent_id.to_string(),
        task_id: task_id.to_string(),
        base_oid: Some(base_oid),
        owner_token: Some(token.clone()),
    };

    // Record ownership in Git's private administrative directory, not in the
    // checked-out source. The token lets rollback prove it is deleting exactly
    // the worktree this transaction created.
    if let Err(error) = write_owner_record(&info, &token) {
        let cleanup = cleanup_unlaunched_worktree(&info);
        return Err(error.context(format!(
            "failed to record ownership for isolated worktree {}; cleanup: {}",
            info.path.display(),
            cleanup
                .err()
                .map(|e| format!("failed ({e:#}); source preserved"))
                .unwrap_or_else(|| "complete".to_string())
        )));
    }
    if let Err(error) = creation_fault("owner-recorded") {
        let cleanup = cleanup_unlaunched_worktree(&info);
        return Err(error.context(format!(
            "worktree owner-record boundary failed; cleanup={}",
            cleanup
                .err()
                .map(|e| format!("failed ({e:#}); source preserved"))
                .unwrap_or_else(|| "complete".to_string())
        )));
    }

    let symlink_target = match workgraph_dir.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            let cleanup = rollback_created_worktree(&info);
            return Err(error).with_context(|| {
                format!(
                    "failed to canonicalize WG graph for {}; rollback={:?}",
                    info.path.display(),
                    cleanup
                )
            });
        }
    };
    let symlink_path = info.path.join(".wg");
    if let Err(error) = create_workgraph_link(&symlink_target, &symlink_path) {
        let cleanup = rollback_created_worktree(&info);
        return Err(error).with_context(|| {
            format!(
                "failed to link WG graph into {}; rollback={:?}",
                info.path.display(),
                cleanup
            )
        });
    }
    if let Err(error) = creation_fault("graph-linked") {
        let cleanup = rollback_created_worktree(&info);
        return Err(error.context(format!(
            "worktree graph-link boundary failed; cleanup={cleanup:?}"
        )));
    }

    // Verification is mandatory both now and again immediately before launch.
    info = match verify_worktree_info(&info) {
        Ok(verified) => verified,
        Err(error) => {
            let cleanup = rollback_created_worktree(&info);
            return Err(error).with_context(|| {
                format!(
                    "new isolated worktree failed ownership verification; rollback={:?}",
                    cleanup
                )
            });
        }
    };
    if let Err(error) = creation_fault("initial-verified") {
        let cleanup = rollback_created_worktree(&info);
        return Err(error.context(format!(
            "worktree initial-verification boundary failed; cleanup={cleanup:?}"
        )));
    }

    // Run worktree-setup.sh if it exists. Same `\\?\` stripping as the
    // main bash wrapper spawn — without it, bash can't parse the script path.
    let setup_script = workgraph_dir.join("worktree-setup.sh");
    if setup_script.exists()
        && let Ok(bash_path) = worksgood::platform_bash::bash_exe_path(None)
    {
        let _ = Command::new(&bash_path)
            .arg(strip_verbatim_prefix(&setup_script))
            .arg(strip_verbatim_prefix(&info.path))
            .arg(strip_verbatim_prefix(project_root))
            .current_dir(strip_verbatim_prefix(&info.path))
            .output(); // Best-effort; launch verification still runs afterward.
    }
    if let Err(error) = creation_fault("setup-complete") {
        let cleanup = rollback_created_worktree(&info);
        return Err(error.context(format!(
            "worktree setup boundary failed; cleanup={cleanup:?}"
        )));
    }

    match verify_worktree_info(&info) {
        Ok(verified) => Ok(verified),
        Err(error) => {
            let cleanup = rollback_created_worktree(&info);
            Err(error).with_context(|| {
                format!(
                    "isolated worktree changed during setup; rollback={:?}",
                    cleanup
                )
            })
        }
    }
}

fn git_stdout(project_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(strip_verbatim_prefix(project_root))
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn branch_exists(project_root: &Path, branch: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(strip_verbatim_prefix(project_root))
        .output()
        .context("failed to inspect isolated-worktree branch")?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "failed to inspect branch '{}' in {}: {}",
            branch,
            project_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// Return whether any historical/current WG branch already claims this agent
/// ID. An exact task branch check is insufficient: `wg/agent-1/old-task` must
/// prevent reusing `agent-1` for `new-task`, even when its worktree directory
/// and registry row are stale or missing.
pub fn agent_branch_exists(project_root: &Path, agent_id: &str) -> Result<bool> {
    let prefix = format!("refs/heads/wg/{agent_id}/");
    let output = Command::new("git")
        .args(["for-each-ref", "--format=%(refname)"])
        .arg(&prefix)
        .current_dir(strip_verbatim_prefix(project_root))
        .output()
        .context("failed to inspect isolated-worktree branch namespace")?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to inspect branch namespace '{}' in {}: {}",
            prefix,
            project_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|branch| branch.starts_with(&prefix)))
}

fn gitdir_from_pointer(path: &Path) -> Result<PathBuf> {
    let pointer = path.join(".git");
    let metadata = fs::symlink_metadata(&pointer)
        .with_context(|| format!("missing Git worktree indirection at {}", pointer.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!(
            "Git worktree indirection at {} is not a regular file",
            pointer.display()
        );
    }
    let content = fs::read_to_string(&pointer)
        .with_context(|| format!("failed to read Git indirection at {}", pointer.display()))?;
    let raw = content
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(|| anyhow::anyhow!("corrupt Git indirection at {}", pointer.display()))?;
    let candidate = PathBuf::from(raw);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        path.join(candidate)
    };
    candidate.canonicalize().with_context(|| {
        format!(
            "Git administrative directory is missing: {}",
            candidate.display()
        )
    })
}

fn owner_path(path: &Path) -> Result<PathBuf> {
    Ok(gitdir_from_pointer(path)?.join(OWNER_FILE))
}

fn write_owner_record(info: &WorktreeInfo, token: &str) -> Result<()> {
    let canonical_path = info.path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize new worktree {}",
            info.path.display()
        )
    })?;
    let record = WorktreeOwner {
        schema: OWNER_SCHEMA,
        token: token.to_string(),
        agent_id: info.agent_id.clone(),
        task_id: info.task_id.clone(),
        branch: info.branch.clone(),
        path: canonical_path.to_string_lossy().to_string(),
        base_oid: info.base_oid.clone(),
    };
    let path = owner_path(&info.path)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to create worktree owner record {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(&record)?)?;
    file.sync_all()?;
    Ok(())
}

#[derive(Debug)]
struct GitWorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
}

fn git_worktree_entries(project_root: &Path) -> Result<Vec<GitWorktreeEntry>> {
    let text = git_stdout(project_root, &["worktree", "list", "--porcelain"])?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    for line in text.lines().chain(std::iter::once("")) {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(previous) = current_path.take() {
                entries.push(GitWorktreeEntry {
                    path: previous,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(PathBuf::from(path));
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string(),
            );
        } else if line.is_empty()
            && let Some(path) = current_path.take()
        {
            entries.push(GitWorktreeEntry {
                path,
                branch: current_branch.take(),
            });
        }
    }
    Ok(entries)
}

fn same_canonical_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Verify filesystem identity, Git's administrative indirection and porcelain
/// registration, branch ownership, and the shared `.wg` link. A caller must run
/// this immediately before process launch; mere directory existence is never
/// sufficient evidence of isolation.
pub fn verify_worktree_info(info: &WorktreeInfo) -> Result<WorktreeInfo> {
    let expected_path = info.project_root.join(".wg-worktrees").join(&info.agent_id);
    if !same_canonical_path(&info.path, &expected_path) {
        anyhow::bail!(
            "worktree path ownership mismatch: attempted {} but {} owns {}",
            info.path.display(),
            info.agent_id,
            expected_path.display()
        );
    }
    if !info.path.is_dir() {
        anyhow::bail!(
            "isolated worktree is not a directory: {}",
            info.path.display()
        );
    }

    let gitdir = gitdir_from_pointer(&info.path)?;
    let common_raw = git_stdout(&info.project_root, &["rev-parse", "--git-common-dir"])?;
    let common = {
        let path = PathBuf::from(common_raw);
        let path = if path.is_absolute() {
            path
        } else {
            info.project_root.join(path)
        };
        path.canonicalize()
            .context("failed to resolve common Git directory")?
    };
    let admin_parent = common.join("worktrees");
    if !gitdir.starts_with(&admin_parent) {
        anyhow::bail!(
            "Git indirection for {} escapes this repository's worktree metadata: {}",
            info.path.display(),
            gitdir.display()
        );
    }

    let expected_branch = format!("wg/{}/{}", info.agent_id, info.task_id);
    if info.branch != expected_branch {
        anyhow::bail!(
            "worktree branch ownership mismatch: expected '{}' for {}/{}, found '{}'",
            expected_branch,
            info.agent_id,
            info.task_id,
            info.branch
        );
    }
    let actual_branch = git_stdout(&info.path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .context("isolated worktree has detached or corrupt HEAD")?;
    if actual_branch != expected_branch {
        anyhow::bail!(
            "worktree branch mismatch at {}: expected '{}', found '{}'",
            info.path.display(),
            expected_branch,
            actual_branch
        );
    }
    let top = PathBuf::from(git_stdout(&info.path, &["rev-parse", "--show-toplevel"])?);
    if !same_canonical_path(&top, &info.path) {
        anyhow::bail!(
            "Git top-level mismatch for isolated worktree {}: {}",
            info.path.display(),
            top.display()
        );
    }

    let registered = git_worktree_entries(&info.project_root)?
        .into_iter()
        .any(|entry| {
            entry.branch.as_deref() == Some(expected_branch.as_str())
                && same_canonical_path(&entry.path, &info.path)
        });
    if !registered {
        anyhow::bail!(
            "Git worktree metadata does not register {} on branch '{}'",
            info.path.display(),
            expected_branch
        );
    }

    let graph_link = info.path.join(".wg");
    let graph_target = graph_link
        .canonicalize()
        .with_context(|| format!("missing/corrupt WG graph link at {}", graph_link.display()))?;
    let expected_graph = info
        .workgraph_dir
        .canonicalize()
        .context("failed to canonicalize expected WG graph")?;
    if graph_target != expected_graph {
        anyhow::bail!(
            "WG graph link mismatch at {}: expected {}, found {}",
            graph_link.display(),
            expected_graph.display(),
            graph_target.display()
        );
    }

    let ownership_path = gitdir.join(OWNER_FILE);
    if ownership_path.exists() {
        let owner: WorktreeOwner = serde_json::from_slice(
            &fs::read(&ownership_path)
                .with_context(|| format!("failed to read {}", ownership_path.display()))?,
        )
        .with_context(|| format!("corrupt worktree owner record {}", ownership_path.display()))?;
        let canonical = info.path.canonicalize()?;
        if owner.schema != OWNER_SCHEMA
            || owner.agent_id != info.agent_id
            || owner.task_id != info.task_id
            || owner.branch != expected_branch
            || Path::new(&owner.path) != canonical
            || info
                .base_oid
                .as_ref()
                .is_some_and(|base_oid| owner.base_oid.as_ref() != Some(base_oid))
            || info
                .owner_token
                .as_ref()
                .is_some_and(|token| token != &owner.token)
        {
            anyhow::bail!(
                "worktree owner record mismatch at {} (expected agent={} task={} branch={})",
                ownership_path.display(),
                info.agent_id,
                info.task_id,
                expected_branch
            );
        }
    } else if info.owner_token.is_some() {
        anyhow::bail!(
            "worktree owner record missing at {} for newly-created {}",
            ownership_path.display(),
            info.path.display()
        );
    }

    Ok(info.clone())
}

fn status_has_unknown_changes(info: &WorktreeInfo) -> Result<bool> {
    let output = git_stdout(
        &info.path,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    Ok(output.lines().any(|line| {
        let path = line.get(3..).unwrap_or(line).trim_matches('"');
        path != ".wg" && path != crate::commands::service::worktree::CLEANUP_PENDING_MARKER
    }))
}

/// Roll back only a worktree carrying the exact private ownership token minted
/// by this process. Dirty source is preserved untouched and reported loudly.
pub fn rollback_created_worktree(info: &WorktreeInfo) -> Result<()> {
    if info.owner_token.is_none() {
        anyhow::bail!(
            "refusing rollback of legacy/unowned worktree {}",
            info.path.display()
        );
    }
    verify_worktree_info(info).context("refusing rollback: ownership verification failed")?;
    let base_oid = info
        .base_oid
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("refusing rollback without a recorded base revision"))?;
    let head_oid = git_stdout(&info.path, &["rev-parse", "HEAD"])?;
    if head_oid != base_oid {
        anyhow::bail!(
            "refusing rollback of isolated worktree {} because HEAD moved from {} to {}; committed source preserved",
            info.path.display(),
            base_oid,
            head_oid
        );
    }
    if status_has_unknown_changes(info)? {
        anyhow::bail!(
            "refusing rollback of dirty isolated worktree {}; source preserved (inspect it, then use: wg worktree archive {} --remove)",
            info.path.display(),
            info.agent_id
        );
    }
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(strip_verbatim_prefix(&info.path))
        .current_dir(strip_verbatim_prefix(&info.project_root))
        .output()
        .context("failed to run git worktree remove during rollback")?;
    if !output.status.success() {
        anyhow::bail!(
            "git worktree rollback failed for {}: {}",
            info.path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let output = Command::new("git")
        .args(["branch", "-D", &info.branch])
        .current_dir(strip_verbatim_prefix(&info.project_root))
        .output()
        .context("failed to remove isolated-worktree branch during rollback")?;
    if !output.status.success() {
        anyhow::bail!(
            "worktree removed but branch rollback failed for '{}': {}",
            info.branch,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn cleanup_unlaunched_worktree(info: &WorktreeInfo) -> Result<()> {
    // Used only between successful `git worktree add` and owner-record write.
    // The target path was atomically reserved by this call and no hook or child
    // has run, so exact Git path+branch registration is sufficient ownership.
    let exact = git_worktree_entries(&info.project_root)?
        .into_iter()
        .any(|entry| {
            entry.branch.as_deref() == Some(info.branch.as_str())
                && same_canonical_path(&entry.path, &info.path)
        });
    if !exact {
        anyhow::bail!(
            "could not prove partial worktree ownership at {}; preserved",
            info.path.display()
        );
    }
    if let Some(base_oid) = info.base_oid.as_deref() {
        let head_oid = git_stdout(&info.path, &["rev-parse", "HEAD"])?;
        if head_oid != base_oid {
            anyhow::bail!(
                "refusing cleanup of partial worktree {} because HEAD moved from {} to {}; committed source preserved",
                info.path.display(),
                base_oid,
                head_oid
            );
        }
    }
    if status_has_unknown_changes(info)? {
        anyhow::bail!(
            "refusing cleanup of dirty partial worktree {}; source preserved",
            info.path.display()
        );
    }
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(strip_verbatim_prefix(&info.path))
        .current_dir(strip_verbatim_prefix(&info.project_root))
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "partial worktree removal failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let output = Command::new("git")
        .args(["branch", "-D", &info.branch])
        .current_dir(strip_verbatim_prefix(&info.project_root))
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "partial worktree was removed but branch '{}' remains: {}",
            info.branch,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn create_workgraph_link(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).context("Failed to symlink .wg into worktree")
}

#[cfg(windows)]
fn create_workgraph_link(target: &Path, link: &Path) -> Result<()> {
    // Prefer a directory junction over a symlink so the link works without
    // Developer Mode or admin privileges (see `create_windows_link`).
    create_windows_link(target, link).with_context(|| {
        format!(
            "Failed to link .wg into worktree from {} to {}",
            link.display(),
            target.display()
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn create_workgraph_link(target: &Path, link: &Path) -> Result<()> {
    let _ = (target, link);
    anyhow::bail!("worktree .wg linking is not supported on this platform")
}

/// Find an existing worktree for a given task by scanning `.wg-worktrees/`
/// for branches named `wg/<agent-id>/<task-id>`. Returns the worktree path
/// and branch name when one is found.
///
/// Used by the retry-in-place path: if a previous attempt left a worktree
/// behind, the next agent reuses it (preserving uncommitted WIP and prior
/// commits) rather than allocating a fresh worktree off `HEAD`.
pub fn find_verified_worktree_for_task(
    project_root: &Path,
    workgraph_dir: &Path,
    task_id: &str,
) -> Result<Option<WorktreeInfo>> {
    let mut found = Vec::new();
    for entry in git_worktree_entries(project_root)? {
        let Some(branch) = entry.branch else {
            continue;
        };
        let Some(rest) = branch.strip_prefix("wg/") else {
            continue;
        };
        let Some((agent_id, branch_task)) = rest.split_once('/') else {
            continue;
        };
        if branch_task != task_id || !agent_id.starts_with("agent-") {
            continue;
        }
        let agent_id = agent_id.to_string();
        let info = WorktreeInfo {
            path: entry.path,
            branch,
            project_root: project_root.to_path_buf(),
            workgraph_dir: workgraph_dir.to_path_buf(),
            agent_id,
            task_id: task_id.to_string(),
            base_oid: None,
            owner_token: None,
        };
        found.push(verify_worktree_info(&info).with_context(|| {
            format!(
                "retry worktree for task '{}' failed closed ownership verification",
                task_id
            )
        })?);
    }
    if found.len() > 1 {
        anyhow::bail!(
            "multiple isolated worktrees claim task '{}': {}",
            task_id,
            found
                .iter()
                .map(|info| info.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(found.pop())
}

/// Backward-compatible best-effort lookup used by read-only status and retry
/// cleanup surfaces. Historical/manual worktrees may not contain WG's `.wg`
/// link or private owner record yet; those surfaces must still be able to show
/// or explicitly remove them. Spawn itself uses
/// `find_verified_worktree_for_task` and propagates every verification error
/// fail-closed before launch.
pub fn find_worktree_for_task(project_root: &Path, task_id: &str) -> Option<(PathBuf, String)> {
    for entry in git_worktree_entries(project_root).ok()? {
        let Some(branch) = entry.branch else {
            continue;
        };
        let Some(rest) = branch.strip_prefix("wg/") else {
            continue;
        };
        let Some((agent_id, branch_task)) = rest.split_once('/') else {
            continue;
        };
        if branch_task != task_id || !agent_id.starts_with("agent-") {
            continue;
        }
        let expected = project_root.join(".wg-worktrees").join(agent_id);
        if same_canonical_path(&entry.path, &expected) {
            return Some((entry.path, branch));
        }
    }
    None
}

/// Remove a worktree and its branch. Force-removes to discard uncommitted changes.
pub fn remove_worktree(project_root: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    // Remove the symlink first (git worktree remove won't remove it)
    let symlink_path = worktree_path.join(".wg");
    if symlink_path.exists() {
        let _ = std::fs::remove_file(&symlink_path);
    }

    // Remove isolated cargo target directory
    let target_dir = worktree_path.join("target");
    if target_dir.exists() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }

    // Force-remove the worktree. Strip the `\\?\` prefix to match the
    // create-path — git rejects it on both add and remove.
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(strip_verbatim_prefix(worktree_path))
        .current_dir(strip_verbatim_prefix(project_root))
        .output();

    // Delete the branch
    let _ = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(strip_verbatim_prefix(project_root))
        .output();

    // NOTE: We intentionally do NOT run `git worktree prune` here.
    // Global prune can remove metadata for other agents' worktrees that are
    // temporarily missing during concurrent cleanup, causing data loss.

    Ok(())
}

/// Create a directory-link at `link_path` pointing to `target` on Windows.
///
/// Prefers a junction (`mklink /J`) over a symlink because junctions work for
/// every user without Developer Mode or admin privileges. A junction only
/// supports absolute local paths and only links directories, both of which
/// are true for `.workgraph`. If `mklink` isn't available we fall back to
/// `symlink_dir`, which will fail helpfully if the user doesn't have the
/// required privileges.
#[cfg(windows)]
fn create_windows_link(target: &Path, link_path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    // `mklink` is a cmd.exe builtin, not an .exe; must invoke through cmd.
    // `/J` = directory junction. CREATE_NO_WINDOW = 0x08000000 suppresses the
    // flashing console window when running from a GUI context.
    let status = Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link_path.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .creation_flags(0x0800_0000)
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => std::os::windows::fs::symlink_dir(target, link_path).context(
            "mklink /J failed and symlink_dir fallback also failed; \
             junctions normally work without admin — is cmd.exe on PATH?",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_git_repo(path: &Path) {
        Command::new("git")
            .args(["init"])
            .arg(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .unwrap();
        std::fs::write(path.join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .unwrap();
    }

    // `strip_verbatim_prefix` unit tests live with the consolidated helper in
    // `spawn/mod.rs` (it moved there from this file as part of the `\\?\`
    // consolidation of njt's #24/#25/#28).

    #[test]
    fn test_create_worktree() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let info = create_worktree(&project, &wg_dir, "agent-1", "task-foo").unwrap();
        assert!(info.path.exists());
        assert_eq!(info.branch, "wg/agent-1/task-foo");
        assert!(info.path.join(".wg").exists()); // symlink
        assert!(info.path.join("file.txt").exists()); // source checked out

        // Cleanup
        remove_worktree(&project, &info.path, &info.branch).unwrap();
        assert!(!info.path.exists());
    }

    #[test]
    fn test_create_worktree_behavior_without_local_git_repo() {
        // Note: In the test environment, Git can find parent repositories even in temp directories.
        // This test verifies the function behavior when there's no local .git directory
        // but Git might still find a parent repository (which is acceptable behavior).

        let temp = TempDir::new().unwrap();

        // Verify temp directory itself doesn't have .git
        assert!(
            !temp.path().join(".git").exists(),
            "Temp directory should not have .git"
        );

        // Test worktree creation - this may succeed or fail depending on whether
        // Git finds a parent repository in the test environment
        let result = create_worktree(temp.path(), temp.path(), "agent-1", "task-foo");

        // The exact behavior depends on test environment, but the function should not crash
        match result {
            Ok(info) => {
                // If it succeeds, Git found a parent repo - this is valid Git behavior.
                println!("Worktree creation succeeded - Git found parent repository");
                rollback_created_worktree(&info).unwrap();
            }
            Err(_e) => {
                // If it fails, no accessible Git repo was found - also valid
                println!("Worktree creation failed - no accessible Git repository");
            }
        }

        // The key test is that the function handles both cases gracefully without panicking
        // and never leaks its atomic reservation on failure.
        assert!(
            !temp.path().join(".wg-worktrees/agent-1").exists(),
            "failed creation must roll back the reserved target path"
        );
    }

    #[test]
    fn test_create_worktree_refuses_to_overwrite_existing() {
        // Sacred-worktree invariant: if a worktree already exists at the target
        // path, create_worktree must refuse rather than silently nuke it.
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let info = create_worktree(&project, &wg_dir, "agent-collide", "task-one").unwrap();
        assert!(info.path.exists());

        // Second creation with the same agent-id must fail, preserving the first.
        let err = create_worktree(&project, &wg_dir, "agent-collide", "task-two").unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("already exists"),
            "expected 'already exists' in error, got: {}",
            msg
        );
        assert!(
            info.path.exists(),
            "original worktree must be preserved on collision"
        );

        remove_worktree(&project, &info.path, &info.branch).unwrap();
    }

    #[test]
    fn stale_branch_for_other_task_reserves_the_agent_id_namespace() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        let output = Command::new("git")
            .args(["branch", "wg/agent-stale/old-task"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(output.status.success());

        let error = create_worktree(&project, &wg_dir, "agent-stale", "new-task").unwrap_err();
        assert!(is_collision(&error));
        assert!(format!("{error:#}").contains("branch namespace collision"));
        assert!(agent_branch_exists(&project, "agent-stale").unwrap());
        assert!(!project.join(".wg-worktrees/agent-stale").exists());
    }

    #[test]
    fn test_remove_worktree_idempotent() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();

        let info = create_worktree(&project, &wg_dir, "agent-1", "task-foo").unwrap();
        remove_worktree(&project, &info.path, &info.branch).unwrap();
        // Second remove should not fail
        remove_worktree(&project, &info.path, &info.branch).unwrap();
    }

    #[test]
    fn test_worktree_symlink_points_to_workgraph() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        // Write a marker file so we can verify the symlink target
        std::fs::write(wg_dir.join("marker"), "test").unwrap();

        let info = create_worktree(&project, &wg_dir, "agent-2", "task-bar").unwrap();
        let symlink = info.path.join(".wg");
        // On Unix this is a symlink; on Windows it's a directory junction
        // (a reparse point that isn't classified as a symlink by the stdlib
        // but resolves identically for I/O).
        #[cfg(unix)]
        assert!(symlink.is_symlink());
        #[cfg(windows)]
        assert!(symlink.exists(), "link should be readable");
        // The marker file should be readable through the link
        assert_eq!(
            std::fs::read_to_string(symlink.join("marker")).unwrap(),
            "test"
        );

        remove_worktree(&project, &info.path, &info.branch).unwrap();
    }

    #[test]
    fn stale_directory_without_git_metadata_is_preserved_as_collision() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();

        let stale = project.join(".wg-worktrees/agent-9");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("unknown-dirty-source.txt"), "do not delete").unwrap();

        let error = create_worktree(&project, &wg_dir, "agent-9", "new-task").unwrap_err();
        assert!(is_collision(&error));
        assert_eq!(
            fs::read_to_string(stale.join("unknown-dirty-source.txt")).unwrap(),
            "do not delete"
        );
        assert!(!stale.join(".git").exists());
    }

    #[test]
    fn interrupted_worktree_add_with_corrupt_git_pointer_fails_closed_untouched() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();

        let interrupted = project.join(".wg-worktrees/agent-10");
        fs::create_dir_all(&interrupted).unwrap();
        fs::write(
            interrupted.join(".git"),
            "gitdir: /missing/interrupted/admin\n",
        )
        .unwrap();
        fs::write(interrupted.join("partial.patch"), "valuable partial work").unwrap();

        let error = create_worktree(&project, &wg_dir, "agent-10", "task-x").unwrap_err();
        assert!(is_collision(&error));
        assert_eq!(
            fs::read_to_string(interrupted.join("partial.patch")).unwrap(),
            "valuable partial work"
        );
        assert_eq!(
            fs::read_to_string(interrupted.join(".git")).unwrap(),
            "gitdir: /missing/interrupted/admin\n"
        );
    }

    #[test]
    fn verification_rejects_corrupt_git_indirection_and_mismatched_branch() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        let info = create_worktree(&project, &wg_dir, "agent-11", "task-y").unwrap();

        let mut mismatched = info.clone();
        mismatched.branch = "wg/agent-other/task-y".to_string();
        let error = verify_worktree_info(&mismatched).unwrap_err();
        assert!(format!("{error:#}").contains("branch ownership mismatch"));

        let pointer_path = info.path.join(".git");
        let pointer = fs::read_to_string(&pointer_path).unwrap();
        fs::write(&pointer_path, "not-a-gitdir\n").unwrap();
        let error = verify_worktree_info(&info).unwrap_err();
        assert!(format!("{error:#}").contains("corrupt Git indirection"));
        fs::write(&pointer_path, pointer).unwrap();
        rollback_created_worktree(&info).unwrap();
    }

    #[test]
    fn verification_rejects_mismatched_private_owner_record() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        let info = create_worktree(&project, &wg_dir, "agent-12", "task-z").unwrap();

        let record_path = owner_path(&info.path).unwrap();
        let original = fs::read(&record_path).unwrap();
        let mut record: WorktreeOwner = serde_json::from_slice(&original).unwrap();
        record.agent_id = "agent-someone-else".to_string();
        fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
        let error = verify_worktree_info(&info).unwrap_err();
        assert!(format!("{error:#}").contains("owner record mismatch"));

        fs::write(&record_path, original).unwrap();
        rollback_created_worktree(&info).unwrap();
    }

    #[test]
    fn every_creation_boundary_rolls_back_clean_owned_git_state() {
        for boundary in [
            "path-reserved",
            "git-registered",
            "owner-recorded",
            "graph-linked",
            "initial-verified",
            "setup-complete",
        ] {
            let temp = TempDir::new().unwrap();
            let project = temp.path().join("project");
            fs::create_dir_all(&project).unwrap();
            init_git_repo(&project);
            let wg_dir = project.join(".wg");
            fs::create_dir_all(&wg_dir).unwrap();

            CREATE_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = Some(boundary));
            let result = create_worktree(&project, &wg_dir, "agent-fault", "task-fault");
            CREATE_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = None);
            assert!(result.is_err(), "boundary={boundary}");
            assert!(
                !project.join(".wg-worktrees/agent-fault").exists(),
                "boundary={boundary}: filesystem worktree leaked"
            );
            assert!(
                !branch_exists(&project, "wg/agent-fault/task-fault").unwrap(),
                "boundary={boundary}: branch leaked"
            );
            let entries = git_worktree_entries(&project).unwrap();
            assert_eq!(
                entries.len(),
                1,
                "boundary={boundary}: Git worktree registration leaked"
            );
        }
    }

    #[test]
    fn transactional_rollback_preserves_clean_committed_source() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        let info = create_worktree(&project, &wg_dir, "agent-commit", "task-commit").unwrap();
        fs::write(info.path.join("valuable-commit.txt"), "keep committed work").unwrap();
        let output = Command::new("git")
            .args(["add", "valuable-commit.txt"])
            .current_dir(&info.path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["commit", "-qm", "setup hook work"])
            .current_dir(&info.path)
            .output()
            .unwrap();
        assert!(output.status.success());

        let error = rollback_created_worktree(&info).unwrap_err();
        assert!(format!("{error:#}").contains("HEAD moved"));
        assert_eq!(
            fs::read_to_string(info.path.join("valuable-commit.txt")).unwrap(),
            "keep committed work"
        );
        assert!(verify_worktree_info(&info).is_ok());

        remove_worktree(&project, &info.path, &info.branch).unwrap();
    }

    #[test]
    fn transactional_rollback_preserves_dirty_source() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);
        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        let info = create_worktree(&project, &wg_dir, "agent-13", "task-dirty").unwrap();
        fs::write(info.path.join("valuable-untracked.txt"), "keep me").unwrap();

        let error = rollback_created_worktree(&info).unwrap_err();
        assert!(format!("{error:#}").contains("dirty isolated worktree"));
        assert_eq!(
            fs::read_to_string(info.path.join("valuable-untracked.txt")).unwrap(),
            "keep me"
        );
        assert!(verify_worktree_info(&info).is_ok());

        remove_worktree(&project, &info.path, &info.branch).unwrap();
    }
}
