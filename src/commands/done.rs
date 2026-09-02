use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use std::process::Command;
use worksgood::agency::capture_task_output;
use worksgood::config::{Config, CoordinatorConfig};
use worksgood::graph::{
    FailureClass, LogEntry, Node, Status, create_user_board_task, evaluate_cycle_iteration,
    parse_token_usage, parse_wg_tokens, user_board_handle, user_board_seq,
};
use worksgood::graph::{Task, parse_delay};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use worksgood::parser::modify_graph;
use worksgood::query;
use worksgood::service::registry::AgentRegistry;
use worksgood::smoke::{self, Manifest as SmokeManifest, ScenarioOutcome};

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use worksgood::parser::load_graph;

/// Enhanced timeout resolution with priority order
fn resolve_verify_timeout(
    task: &Task,
    coordinator_config: &CoordinatorConfig,
) -> std::time::Duration {
    // 1. Task-specific timeout (highest priority)
    if let Some(task_timeout) = &task.verify_timeout
        && let Some(secs) = parse_delay(task_timeout)
    {
        return std::time::Duration::from_secs(secs);
    }

    // 2. Global environment variable
    if let Ok(env_timeout) = std::env::var("WG_VERIFY_TIMEOUT")
        && let Ok(secs) = env_timeout.parse::<u64>()
    {
        return std::time::Duration::from_secs(secs);
    }

    // 3. Coordinator configuration default
    coordinator_config
        .verify_default_timeout
        .as_ref()
        .and_then(|s| parse_delay(s))
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(900)) // New default: 900s instead of 300s
}

/// Result of running a verify command.
struct VerifyOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: String,
}

/// Progress monitoring for verify commands
#[derive(Debug)]
struct ProgressMonitor {
    last_stdout_activity: std::time::Instant,
    last_stderr_activity: std::time::Instant,
}

impl ProgressMonitor {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            last_stdout_activity: now,
            last_stderr_activity: now,
        }
    }

    fn last_activity(&self) -> std::time::Instant {
        self.last_stdout_activity.max(self.last_stderr_activity)
    }

    fn has_recent_activity(&self, threshold: std::time::Duration) -> bool {
        self.last_activity().elapsed() < threshold
    }
}

/// Triage result for timeout processes
#[derive(Debug, PartialEq)]
enum TriageResult {
    GenuineHang { reason: String },
    WaitingOnLocks { detected_locks: Vec<String> },
    UnknownButActive { activity_type: String },
}

#[cfg(test)]
#[derive(Debug, PartialEq)]
enum PushOutcome {
    /// No `origin` remote configured — skipped silently.
    NoRemote,
    /// `git push origin main` succeeded AND the agent branch was deleted on origin.
    PushedAndDeleted,
    /// `git push origin main` succeeded but the branch-delete push failed.
    PushedNotDeleted { delete_error: String },
    /// `git push origin main` failed (network, permissions, non-FF unresolvable).
    LocalOnly { push_error: String },
}

#[cfg(test)]
#[derive(Debug)]
enum WorktreeMergeResult {
    NotInWorktree,
    NoCommits,
    /// Worktree branch has 0 commits ahead of main, but the working tree has
    /// staged or modified tracked files — the agent prepared work but never
    /// committed it. Treating this as NoCommits silently drops the work.
    UncommittedChanges {
        files: Vec<String>,
    },
    Merged {
        commit_sha: String,
        push_outcome: PushOutcome,
    },
    Conflict {
        conflicting_files: Vec<String>,
    },
}

#[cfg(test)]
struct MergeLockGuard {
    file: Option<std::fs::File>,
    #[cfg(not(unix))]
    path: std::path::PathBuf,
}

#[cfg(test)]
impl MergeLockGuard {
    fn acquire(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(path)
                .context("Failed to open merge lock file")?;
            let fd = file.as_raw_fd();
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if ret != 0 {
                anyhow::bail!(
                    "Failed to acquire merge lock: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(Self { file: Some(file) })
        }

        #[cfg(not(unix))]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .context("Failed to create merge lock file")?;
            Ok(Self {
                file: Some(file),
                path: path.to_path_buf(),
            })
        }
    }
}

#[cfg(test)]
impl Drop for MergeLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if let Some(file) = &self.file {
                unsafe {
                    libc::flock(file.as_raw_fd(), libc::LOCK_UN);
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = self.file.take();
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Best-effort: push local `main` to `origin/main` (FF-only, with a single
/// fetch+ff-merge+retry on non-FF), then delete the agent branch on origin.
///
/// Never fails — `wg done` must remain successful even if the remote is
/// unavailable. Returns a `PushOutcome` describing what happened so the caller
/// can surface it in the `[merge]` log line.
///
/// Branch deletion is **only** attempted after the main push succeeds (the
/// audit doc requires the squash commit be reachable from `origin/main`
/// before we drop the only ref to the agent branch tip on origin).
#[cfg(test)]
fn push_main_and_delete_branch(project_root: &str, branch: &str) -> PushOutcome {
    use std::process::Command;

    // 0. If there's no `origin` remote, skip silently — common in tests and
    //    detached/local-only repos. Without this we'd report a misleading
    //    `push failed: ... 'origin' does not appear to be a git repository`.
    let remote_check = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_root)
        .output();
    let has_origin = matches!(remote_check, Ok(o) if o.status.success());
    if !has_origin {
        return PushOutcome::NoRemote;
    }

    // 1. First push attempt.
    let push = Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(project_root)
        .output();

    let initial_push_ok = matches!(&push, Ok(o) if o.status.success());
    let mut push_err_msg = String::new();
    if !initial_push_ok {
        push_err_msg = match &push {
            Ok(o) => one_line_error(&o.stderr),
            Err(e) => e.to_string(),
        };

        // Try to recover from a non-FF rejection: fetch origin main, fast-forward
        // local main to origin/main, retry the push. We do NOT attempt a
        // non-fast-forward merge — local main has the squash commit on top
        // and origin/main has its own work; a real reconciliation needs human
        // judgment, not best-effort merges.
        let fetch_ok = Command::new("git")
            .args(["fetch", "origin", "main"])
            .current_dir(project_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let ff_ok = if fetch_ok {
            Command::new("git")
                .args(["merge", "--ff-only", "origin/main"])
                .current_dir(project_root)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        } else {
            false
        };

        if ff_ok {
            let retry = Command::new("git")
                .args(["push", "origin", "main"])
                .current_dir(project_root)
                .output();
            let retry_ok = matches!(&retry, Ok(o) if o.status.success());
            if !retry_ok {
                if let Ok(o) = retry {
                    push_err_msg = one_line_error(&o.stderr);
                }
                return PushOutcome::LocalOnly {
                    push_error: push_err_msg,
                };
            }
            // fall through — push succeeded on retry
        } else {
            return PushOutcome::LocalOnly {
                push_error: push_err_msg,
            };
        }
    }

    // 2. Push succeeded — squash commit is now on origin/main. Safe to delete
    //    the agent branch on origin.
    let delete = Command::new("git")
        .args(["push", "origin", &format!(":refs/heads/{}", branch)])
        .current_dir(project_root)
        .output();

    match delete {
        Ok(o) if o.status.success() => PushOutcome::PushedAndDeleted,
        Ok(o) => PushOutcome::PushedNotDeleted {
            delete_error: one_line_error(&o.stderr),
        },
        Err(e) => PushOutcome::PushedNotDeleted {
            delete_error: e.to_string(),
        },
    }
}

#[cfg(test)]
fn one_line_error(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}

#[derive(Clone, Debug)]
struct WorktreeInfo {
    worktree_path: String,
    branch: String,
    project_root: String,
    agent_id: Option<String>,
    task_id: Option<String>,
}

thread_local! {
    /// The daemon executes capability-brokered worker operations on its own
    /// thread, where process-wide worker environment variables are
    /// intentionally absent. Preserve the exact authenticated worktree
    /// context thread-locally so `wg done` cannot silently fall back to the
    /// graph-root/human completion path.
    static BROKERED_WORKTREE: std::cell::RefCell<Option<WorktreeInfo>> = const {
        std::cell::RefCell::new(None)
    };
}

pub(crate) fn run_from_worker_control(
    dir: &Path,
    id: &str,
    converged: bool,
    full_smoke: bool,
    worktree_path: &Path,
    agent_id: &str,
) -> Result<()> {
    let branch_output = std::process::Command::new("git")
        .args([
            "-C",
            &worktree_path.to_string_lossy(),
            "branch",
            "--show-current",
        ])
        .output()
        .context("failed to inspect brokered worker branch")?;
    if !branch_output.status.success() {
        anyhow::bail!("failed to inspect brokered worker branch");
    }
    let branch =
        String::from_utf8(branch_output.stdout).context("brokered worker branch is not UTF-8")?;
    let project_root = dir
        .parent()
        .context("worker control graph directory has no project root")?
        .to_string_lossy()
        .to_string();
    let context = WorktreeInfo {
        worktree_path: worktree_path.to_string_lossy().to_string(),
        branch: branch.trim().to_string(),
        project_root,
        agent_id: Some(agent_id.to_string()),
        task_id: Some(id.to_string()),
    };
    BROKERED_WORKTREE.with(|slot| {
        let previous = slot.replace(Some(context));
        let result = run_inner(dir, id, converged, false, false, true, full_smoke, false);
        slot.replace(previous);
        result
    })
}

/// Detect git's "no changes staged" messages from a failed `git commit`.
///
/// Git emits different phrasings depending on working-tree state:
///   - "nothing to commit, working tree clean" — clean tree, nothing staged
///   - "nothing added to commit but untracked files present" — clean tree + untracked
///   - "no changes added to commit" — modified tracked files but none staged
///
/// All three mean "the squash-merge produced no new content," which after a prior
/// successful merge to main is the expected retry state, not a failure.
#[cfg(test)]
fn is_no_changes_to_commit(stdout: &str, stderr: &str) -> bool {
    let needles = [
        "nothing to commit",
        "nothing added to commit",
        "no changes added to commit",
    ];
    needles
        .iter()
        .any(|n| stdout.contains(n) || stderr.contains(n))
}

/// Detect and authenticate a task-owned completion worktree.
///
/// A complete absence of task-owned worktree context is the ordinary
/// human/root path and resolves deliverables against the graph project root.
/// Once brokered context (or environment context naming this exact task) is
/// active, every supplied project/task/agent/git binding must agree.
/// Inconsistent or partial active context is an error, never permission to fall
/// back to the graph root (where stale deliverables may exist).
fn detect_worktree(
    wg_dir: &Path,
    task_id: &str,
    assigned_agent: Option<&str>,
) -> Result<Option<WorktreeInfo>> {
    let brokered = BROKERED_WORKTREE.with(|slot| slot.borrow().clone());
    let context = if let Some(context) = brokered {
        context
    } else {
        let agent_id = std::env::var("WG_AGENT_ID").ok();
        let context_task_id = std::env::var("WG_TASK_ID").ok();
        // Worker variables inherited by a human/test subcommand for another
        // task are not active completion authority. This preserves the
        // ordinary root path. Once WG_TASK_ID names this exact task, the
        // context is active and must validate completely below.
        let task_owned_active = context_task_id.as_deref() == Some(task_id);
        if !task_owned_active {
            return Ok(None);
        }
        let wt_path = std::env::var("WG_WORKTREE_PATH").ok();
        let branch = std::env::var("WG_BRANCH").ok();
        let project_root = std::env::var("WG_PROJECT_ROOT").ok();
        let (Some(wt_path), Some(branch), Some(project_root)) = (wt_path, branch, project_root)
        else {
            anyhow::bail!(
                "done.worktree_context_incomplete: WG_WORKTREE_PATH, WG_BRANCH, and WG_PROJECT_ROOT must be supplied together"
            );
        };
        WorktreeInfo {
            worktree_path: wt_path,
            branch,
            project_root,
            agent_id,
            task_id: context_task_id,
        }
    };

    validate_worktree_context(wg_dir, task_id, assigned_agent, &context)?;
    Ok(Some(context))
}

fn validate_worktree_context(
    wg_dir: &Path,
    task_id: &str,
    assigned_agent: Option<&str>,
    context: &WorktreeInfo,
) -> Result<()> {
    let expected_root = wg_dir
        .parent()
        .context("done.worktree_context_mismatch: graph directory has no project root")?;
    let canonical_expected = expected_root
        .canonicalize()
        .context("done.worktree_context_mismatch: graph project root is unavailable")?;
    let canonical_declared = Path::new(&context.project_root)
        .canonicalize()
        .context("done.worktree_context_mismatch: declared project root is unavailable")?;
    if canonical_declared != canonical_expected {
        anyhow::bail!(
            "done.worktree_context_mismatch: declared project root does not own this graph"
        );
    }
    if let Some(context_task) = context.task_id.as_deref()
        && context_task != task_id
    {
        anyhow::bail!(
            "done.worktree_context_mismatch: worktree task '{}' does not match completion task '{}'",
            context_task,
            task_id
        );
    }
    if let Some(context_agent) = context.agent_id.as_deref()
        && assigned_agent != Some(context_agent)
    {
        anyhow::bail!(
            "done.worktree_context_mismatch: worktree agent '{}' is not the assigned owner of '{}'",
            context_agent,
            task_id
        );
    }

    let worktree = Path::new(&context.worktree_path);
    let canonical_worktree = worktree
        .canonicalize()
        .context("done.worktree_context_mismatch: retained worktree is unavailable")?;
    let top = git_output(worktree, &["rev-parse", "--show-toplevel"])
        .context("done.worktree_context_mismatch: retained path is not a git worktree")?;
    let canonical_top = Path::new(&top)
        .canonicalize()
        .context("done.worktree_context_mismatch: retained git root is unavailable")?;
    if canonical_top != canonical_worktree {
        anyhow::bail!(
            "done.worktree_context_mismatch: retained path is not the exact git worktree root"
        );
    }
    let actual_branch = git_output(worktree, &["branch", "--show-current"])
        .context("done.worktree_context_mismatch: cannot inspect retained branch")?;
    if actual_branch != context.branch {
        anyhow::bail!(
            "done.worktree_context_mismatch: retained branch '{}' does not match authenticated branch '{}'",
            actual_branch,
            context.branch
        );
    }
    let worktree_common = git_output(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let project_common = git_output(
        &canonical_expected,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let canonical_worktree_common = Path::new(&worktree_common)
        .canonicalize()
        .context("done.worktree_context_mismatch: retained git common directory is unavailable")?;
    let canonical_project_common = Path::new(&project_common)
        .canonicalize()
        .context("done.worktree_context_mismatch: project git common directory is unavailable")?;
    if canonical_worktree_common != canonical_project_common {
        anyhow::bail!(
            "done.worktree_context_mismatch: retained worktree belongs to a different project"
        );
    }
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, root.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed in {}: {}",
            args,
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct GitIdentity {
    name: String,
    email: String,
}

#[cfg(test)]
impl GitIdentity {
    fn from_author_fields(name: &str, email: &str) -> Option<Self> {
        let name = name.trim();
        let email = email.trim();
        if name.is_empty()
            || email.is_empty()
            || name.chars().any(char::is_control)
            || email.chars().any(char::is_control)
        {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            email: email.to_string(),
        })
    }

    fn from_trailer_value(value: &str) -> Option<Self> {
        let value = value.trim();
        let open = value.rfind('<')?;
        if !value.ends_with('>') {
            return None;
        }
        Self::from_author_fields(&value[..open], &value[open + 1..value.len() - 1])
    }

    fn render(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }

    fn dedup_key(&self) -> String {
        self.email.to_lowercase()
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
struct SquashAttribution {
    /// The oldest source commit's author remains the squash commit author.
    author: GitIdentity,
    /// Every other source author and every valid source Co-authored-by trailer.
    coauthors: Vec<GitIdentity>,
}

/// Read attribution from every source commit that is not already reachable
/// from the target `HEAD`. A squash discards those commit objects from main's
/// ancestry, so the resulting commit must carry their identities explicitly.
///
/// NUL separators keep fields unambiguous because Git commit objects cannot
/// contain NUL bytes. We recognize only a complete
/// `Co-authored-by: Name <email>` line (case-insensitive key). Reading the body
/// rather than only Git's final trailer block is intentional: older WG
/// integration commits separated multiple co-author lines with blank lines, so
/// `%(trailers:...)` returned only the final identity and caused the loss this
/// path is responsible for repairing.
#[cfg(test)]
fn collect_squash_attribution(
    project_root: &str,
    branch: &str,
) -> Result<Option<SquashAttribution>> {
    use std::collections::HashSet;
    use std::process::Command;

    let output = Command::new("git")
        .args(["log", "--reverse", "-z", "--format=%an%x00%ae%x00%B"])
        .arg(format!("HEAD..{branch}"))
        .current_dir(project_root)
        .output()
        .context("Failed to read source attribution for squash merge")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed while reading squash attribution: {}",
            one_line_error(&output.stderr)
        );
    }

    let fields = output
        .stdout
        .split(|byte| *byte == b'\0')
        .collect::<Vec<_>>();
    let mut commits = Vec::new();
    for fields in fields.chunks_exact(3) {
        let name = String::from_utf8_lossy(fields[0]);
        let email = String::from_utf8_lossy(fields[1]);
        let body = String::from_utf8_lossy(fields[2]);
        let author = GitIdentity::from_author_fields(&name, &email).ok_or_else(|| {
            anyhow::anyhow!("source commit has an invalid author identity: {name:?} <{email:?}>")
        })?;
        let coauthors = body
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("co-authored-by")
                    .then(|| GitIdentity::from_trailer_value(value))
                    .flatten()
            })
            .collect::<Vec<_>>();
        commits.push((author, coauthors));
    }

    let Some((author, _)) = commits.first() else {
        return Ok(None);
    };
    let author = author.clone();
    let mut seen = HashSet::from([author.dedup_key()]);
    let mut coauthors = Vec::new();
    for (commit_author, commit_coauthors) in commits {
        for identity in std::iter::once(commit_author).chain(commit_coauthors) {
            if seen.insert(identity.dedup_key()) {
                coauthors.push(identity);
            }
        }
    }

    Ok(Some(SquashAttribution { author, coauthors }))
}

#[cfg(test)]
fn attempt_worktree_merge(wt: &WorktreeInfo, task_id: &str) -> Result<WorktreeMergeResult> {
    use std::process::Command;

    let wt_git = Path::new(&wt.worktree_path).join(".git");
    if !wt_git.exists() {
        eprintln!(
            "Warning: Worktree .git pointer missing at {} — skipping merge",
            wt.worktree_path
        );
        return Ok(WorktreeMergeResult::NotInWorktree);
    }

    // Serialize the source-range read together with the squash and commit.
    // Otherwise another landing could advance main after attribution was read,
    // making us credit commits that the actual squash no longer contains.
    let merge_lock_path = Path::new(&wt.project_root)
        .join(".wg-worktrees")
        .join(".merge-lock");
    if let Some(parent) = merge_lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _merge_lock = MergeLockGuard::acquire(&merge_lock_path)?;

    let attribution = collect_squash_attribution(&wt.project_root, &wt.branch)?;

    if attribution.is_none() {
        // Before declaring NoCommits, make sure the agent didn't stage work and
        // forget to commit. `git status --porcelain` runs in the worktree
        // directory because the staging area is per-working-tree. Any entry
        // that isn't `??` (untracked) represents work that would silently
        // disappear when we mark the task done and clean up the worktree.
        let porcelain = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&wt.worktree_path)
            .output()
            .context("Failed to check worktree status for uncommitted changes")?;

        if porcelain.status.success() {
            let dirty: Vec<String> = String::from_utf8_lossy(&porcelain.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .filter(|l| !l.starts_with("??"))
                .map(|l| {
                    // Porcelain format is "XY path" (X staged, Y unstaged).
                    // Skip the 2-char status field and the separating space.
                    l.get(3..).unwrap_or(l).to_string()
                })
                .collect();

            if !dirty.is_empty() {
                return Ok(WorktreeMergeResult::UncommittedChanges { files: dirty });
            }
        }

        return Ok(WorktreeMergeResult::NoCommits);
    }

    let merge_result = Command::new("git")
        .args(["merge", "--squash", &wt.branch])
        .current_dir(&wt.project_root)
        .output()
        .context("Failed to run git merge --squash")?;

    let result = if !merge_result.status.success() {
        let diff_output = Command::new("git")
            .args(["diff", "--name-only", "--diff-filter=U"])
            .current_dir(&wt.project_root)
            .output();

        let conflicting_files = diff_output
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let conflicting_files = if conflicting_files.is_empty() {
            String::from_utf8_lossy(&merge_result.stderr)
                .lines()
                .filter(|l| l.contains("CONFLICT"))
                .filter_map(|l| l.rsplit_once(' ').map(|(_, f)| f.to_string()))
                .collect()
        } else {
            conflicting_files
        };

        let _ = Command::new("git")
            .args(["reset", "--hard", "HEAD"])
            .current_dir(&wt.project_root)
            .output();

        WorktreeMergeResult::Conflict { conflicting_files }
    } else {
        let attribution = attribution.expect("source commits were checked before squash merge");
        let agent_label = wt.agent_id.as_deref().unwrap_or("unknown");
        let mut commit_msg = format!(
            "feat: {} ({})\n\nSquash-merged from worktree branch {}",
            task_id, agent_label, wt.branch
        );
        if !attribution.coauthors.is_empty() {
            commit_msg.push_str("\n\n");
        }
        for (index, coauthor) in attribution.coauthors.iter().enumerate() {
            if index > 0 {
                commit_msg.push('\n');
            }
            commit_msg.push_str("Co-authored-by: ");
            commit_msg.push_str(&coauthor.render());
        }
        let author = attribution.author.render();
        let commit_output = Command::new("git")
            .args(["commit", "--author", &author, "-m", &commit_msg])
            .current_dir(&wt.project_root)
            .output()
            .context("Failed to commit squash merge")?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            let stdout = String::from_utf8_lossy(&commit_output.stdout);
            if is_no_changes_to_commit(&stdout, &stderr) {
                return Ok(WorktreeMergeResult::NoCommits);
            }
            anyhow::bail!("git commit failed: {}", stderr);
        }

        let sha_output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&wt.project_root)
            .output()
            .context("Failed to get commit SHA")?;

        let commit_sha = String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string();

        // Best-effort push of main + delete of the agent branch on origin.
        // This closes the gap documented in
        // docs/audit-unmerged-branches-2026-04-26.md: prior to this, every
        // clean `wg done` left the squash commit on local main only and the
        // agent branch lingering on origin.
        let push_outcome = push_main_and_delete_branch(&wt.project_root, &wt.branch);

        WorktreeMergeResult::Merged {
            commit_sha,
            push_outcome,
        }
    };

    Ok(result)
}

#[cfg(test)]
fn create_deferred_merge_task(
    path: &Path,
    task_id: &str,
    wt: &WorktreeInfo,
    conflicting_files: &[String],
) -> Result<String> {
    let merge_task_id = format!(".merge-{}", task_id);
    let files_list = conflicting_files
        .iter()
        .map(|f| format!("- `{}`", f))
        .collect::<Vec<_>>()
        .join("\n");
    let description = format!(
        "## Deferred Merge: {}\n\n\
         The agent's worktree branch could not be cleanly merged at done-time.\n\n\
         **Source branch:** `{}`\n\
         **Target branch:** main\n\
         **Conflicting files:**\n{}\n\n\
         Resolve the conflicts and squash-merge the branch to main.",
        task_id, wt.branch, files_list,
    );
    let merge_task_id_clone = merge_task_id.clone();
    let task_id_owned = task_id.to_string();
    let branch = wt.branch.clone();
    let agent_id = wt.agent_id.clone().unwrap_or_default();
    let conflicting_owned: Vec<String> = conflicting_files.to_vec();
    modify_graph(path, |graph| {
        if graph.get_task(&merge_task_id_clone).is_some() {
            return false;
        }
        let merge_task = Task {
            id: merge_task_id_clone.clone(),
            title: format!("Merge: {}", task_id_owned),
            description: Some(description.clone()),
            status: Status::Incomplete,
            after: vec![task_id_owned.clone()],
            tags: vec!["merge".to_string(), "deferred".to_string()],
            exec_mode: Some("full".to_string()),
            created_at: Some(Utc::now().to_rfc3339()),
            log: vec![LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("merge-defer".to_string()),
                user: None,
                message: format!(
                    "Deferred merge created: branch={}, conflicts={:?}, agent={}",
                    branch, conflicting_owned, agent_id,
                ),
            }],
            ..Default::default()
        };
        if let Some(source_task) = graph.get_task_mut(&task_id_owned) {
            if !source_task.before.contains(&merge_task_id_clone) {
                source_task.before.push(merge_task_id_clone.clone());
            }
        }
        graph.add_node(Node::Task(merge_task));
        true
    })
    .context("Failed to create deferred merge task")?;
    Ok(merge_task_id)
}

fn mark_worktree_for_cleanup(wt: &WorktreeInfo) {
    let marker = Path::new(&wt.worktree_path).join(".wg-cleanup-pending");
    let _ = std::fs::write(&marker, "");
}

/// Get the list of modified files in the current worktree using git diff.
/// Returns relative paths from the project root.
fn get_modified_files(project_root: &Path) -> Result<Vec<String>> {
    use std::process::Command;

    let output = Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg("HEAD")
        .current_dir(project_root)
        .output()
        .context("Failed to run git diff to detect modified files")?;

    if !output.status.success() {
        return Ok(Vec::new()); // No git repo or no changes
    }

    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect();

    Ok(files)
}

/// Detect common lock files that might indicate waiting processes
fn detect_cargo_locks() -> Result<Vec<String>> {
    detect_cargo_locks_with_stderr("")
}

/// Detect lock contention from both lock files and stderr patterns
fn detect_cargo_locks_with_stderr(stderr_content: &str) -> Result<Vec<String>> {
    let mut locks = Vec::new();

    // Common cargo lock files
    let lock_patterns = [
        "target/.rustc_info.json.lock",
        "target/debug/.cargo-lock",
        "Cargo.lock",
    ];

    for pattern in &lock_patterns {
        if std::path::Path::new(pattern).exists() {
            locks.push(pattern.to_string());
        }
    }

    // Check stderr for cargo lock contention messages
    let lock_messages = [
        "Blocking waiting for file lock on artifact directory",
        "Blocking waiting for file lock on package cache",
        "Blocking waiting for file lock on",
        "waiting for file lock on build directory",
        "waiting for file lock on the registry cache",
    ];

    for message in &lock_messages {
        if stderr_content.contains(message) {
            locks.push(format!("stderr_pattern: {}", message));
        }
    }

    Ok(locks)
}

/// Basic triage implementation for timeout processes
fn triage_timeout_process(
    monitor: &ProgressMonitor,
    _progress_timeout: std::time::Duration,
) -> Result<TriageResult> {
    // 1. Check for recent output activity
    if monitor.has_recent_activity(std::time::Duration::from_secs(60)) {
        return Ok(TriageResult::UnknownButActive {
            activity_type: "recent_output".to_string(),
        });
    }

    // 2. Check for cargo lock files (common contention point)
    let lock_files = detect_cargo_locks()?;
    if !lock_files.is_empty() {
        return Ok(TriageResult::WaitingOnLocks {
            detected_locks: lock_files,
        });
    }

    // 3. Default to genuine hang if no other indicators
    Ok(TriageResult::GenuineHang {
        reason: format!(
            "no_activity_{}s_no_locks",
            monitor.last_activity().elapsed().as_secs()
        ),
    })
}

/// Check if an error output indicates file lock contention
fn is_lock_contention_error(stderr: &str) -> bool {
    let lock_patterns = [
        "Blocking waiting for file lock on artifact directory",
        "Blocking waiting for file lock on package cache",
        "Blocking waiting for file lock on",
        "waiting for file lock on build directory",
        "waiting for file lock on the registry cache",
    ];

    lock_patterns.iter().any(|pattern| stderr.contains(pattern))
}

/// Run a verify command with retry logic for file lock contention
fn run_verify_command_with_retry(
    verify_cmd: &str,
    project_root: &Path,
    task: &Task,
    coordinator_config: &CoordinatorConfig,
) -> std::result::Result<VerifyOutput, VerifyOutput> {
    const MAX_RETRIES: u32 = 3;
    const BASE_DELAY_SECS: u64 = 5;

    let mut last_error: Option<VerifyOutput> = None;

    for attempt in 1..=MAX_RETRIES {
        match run_verify_command(verify_cmd, project_root, task, coordinator_config) {
            Ok(output) => return Ok(output),
            Err(error) => {
                // Check if this is a lock contention issue
                if is_lock_contention_error(&error.stderr) {
                    eprintln!(
                        "Verify attempt {}/{} failed due to file lock contention: {}",
                        attempt,
                        MAX_RETRIES,
                        error.stderr.lines().next().unwrap_or("")
                    );

                    if attempt < MAX_RETRIES {
                        let delay_secs = BASE_DELAY_SECS * (2_u64.pow(attempt - 1)); // Exponential backoff
                        eprintln!("Retrying in {} seconds...", delay_secs);
                        std::thread::sleep(std::time::Duration::from_secs(delay_secs));
                        last_error = Some(error);
                        continue;
                    }
                } else if error.exit_code == "timeout" {
                    // For timeouts, check if stderr suggests lock contention
                    if is_lock_contention_error(&error.stderr) {
                        eprintln!("Verify timeout appears to be due to file lock contention");
                        if attempt < MAX_RETRIES {
                            let delay_secs = BASE_DELAY_SECS * (2_u64.pow(attempt - 1));
                            eprintln!(
                                "Retrying in {} seconds with extended timeout...",
                                delay_secs
                            );
                            std::thread::sleep(std::time::Duration::from_secs(delay_secs));
                            last_error = Some(error);
                            continue;
                        }
                    }
                }

                // Not a retryable error or max retries reached
                return Err(error);
            }
        }
    }

    // Return the last error if all retries failed
    Err(last_error.unwrap())
}

/// Map modified files to relevant test modules/files.
/// Returns a list of test-specific cargo commands to run.
fn map_files_to_tests(modified_files: &[String]) -> Option<Vec<String>> {
    let mut test_commands = Vec::new();

    for file in modified_files {
        // Check for core files that should trigger full test suite
        if is_core_file(file) {
            return None; // Fall back to full test suite
        }

        // Map source files to test modules
        if let Some(test_cmd) = map_file_to_test_command(file)
            && !test_commands.contains(&test_cmd)
        {
            test_commands.push(test_cmd);
        }
    }

    if test_commands.is_empty() {
        None
    } else {
        Some(test_commands)
    }
}

/// Check if a file is considered "core" and should trigger full test suite.
fn is_core_file(file: &str) -> bool {
    matches!(
        file,
        "src/lib.rs"
            | "src/main.rs"
            | "Cargo.toml"
            | "Cargo.lock"
            | "build.rs"
            | ".gitignore"
            | "README.md"
    ) || file.starts_with("src/lib/")
        || file.contains("/mod.rs")
        || file.ends_with("/lib.rs")
}

/// Map a single file to its relevant test command.
fn map_file_to_test_command(file: &str) -> Option<String> {
    if file.starts_with("tests/") {
        // Direct test file - run the specific test
        if let Some(test_name) = file
            .strip_prefix("tests/")
            .and_then(|f| f.strip_suffix(".rs"))
        {
            return Some(format!("cargo test --test {}", test_name));
        }
    } else if file.starts_with("src/") {
        // Source file - map to relevant test module
        if let Some(module_path) = file
            .strip_prefix("src/")
            .and_then(|f| f.strip_suffix(".rs"))
        {
            // Convert path to module name (e.g., "commands/add.rs" -> "add", "commands/viz/mod.rs" -> "viz")
            let module_name = if module_path.ends_with("/mod") {
                module_path.strip_suffix("/mod").unwrap_or(module_path)
            } else {
                module_path
            };

            // Extract the final component for testing
            let test_module = module_name.split('/').next_back().unwrap_or(module_name);

            return Some(format!("cargo test {}", test_module));
        }
    }

    None
}

/// Generate a scoped verify command if conditions are met.
/// Returns the scoped command or None to fall back to original.
fn generate_scoped_verify_command(
    verify_cmd: &str,
    project_root: &Path,
    coordinator_config: &CoordinatorConfig,
) -> Option<String> {
    // Only scope "cargo test" commands
    if verify_cmd.trim() != "cargo test" || !coordinator_config.scoped_verify_enabled {
        return None;
    }

    // Get modified files
    let modified_files = match get_modified_files(project_root) {
        Ok(files) => files,
        Err(_) => return None, // Fall back on error
    };

    if modified_files.is_empty() {
        return None; // No changes, use original command
    }

    // Map to test commands
    if let Some(test_commands) = map_files_to_tests(&modified_files) {
        if test_commands.len() == 1 {
            // Single scoped command
            Some(test_commands.into_iter().next().unwrap())
        } else if test_commands.len() > 1 {
            // Multiple test commands - combine them
            Some(test_commands.join(" && "))
        } else {
            None
        }
    } else {
        None // Fall back to full test suite
    }
}

/// Detect if a verify command is likely free-text rather than an executable command.
///
/// Uses multiple heuristics to identify natural language descriptions:
/// 1. First word not found in PATH and not a shell builtin
/// 2. No shell metacharacters (|, &&, ;, >, <) and multiple English words
/// 3. Contains common descriptive patterns
fn is_free_text_verify_command(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }

    let first_word = cmd.split_whitespace().next().unwrap_or("");

    // Quick check: if it looks like a valid command prefix, it's probably not free-text
    let known_commands = [
        "cargo", "npm", "npx", "yarn", "pnpm", "make", "cmake", "go", "python", "python3",
        "pytest", "ruby", "rake", "bundle", "mvn", "gradle", "ant", "dotnet", "zig", "rustc",
        "gcc", "g++", "clang", "clang++", "javac", "java", "test", "[", "true", "false", "exit",
        "echo", "printf", "cat", "grep", "find", "ls", "diff", "cmp", "wc", "head", "tail", "sort",
        "uniq", "cut", "tr", "bash", "sh", "zsh", "env", "timeout",
    ];

    if known_commands.contains(&first_word) {
        return false;
    }

    // Check for shell metacharacters - commands with these are likely executable
    let shell_chars = ['|', '&', ';', '>', '<', '(', ')', '{', '}', '$', '`'];
    if cmd.chars().any(|c| shell_chars.contains(&c)) {
        return false;
    }

    // If multiple words and no shell metacharacters, likely free-text
    let word_count = cmd.split_whitespace().count();
    if word_count > 1 {
        // Check for common descriptive patterns
        let lower = cmd.to_lowercase();
        let descriptive_patterns = [
            "exists",
            "is valid",
            "are valid",
            "passes",
            "succeeds",
            "works",
            "complete",
            "documentation",
            "tests pass",
            "builds successfully",
            "no errors",
            "no warnings",
            "has been",
            "have been",
            "should be",
            "must be",
            "ensure",
            "verify that",
        ];

        if descriptive_patterns
            .iter()
            .any(|pattern| lower.contains(pattern))
        {
            return true;
        }

        // If it's multiple words without shell chars and doesn't look like a command, likely free-text
        return true;
    }

    false
}

/// Run a verify command in a shell.
/// Returns Ok(VerifyOutput) with captured output on success,
/// or Err(VerifyOutput) with captured output on failure.
fn run_verify_command(
    verify_cmd: &str,
    project_root: &Path,
    task: &Task,
    coordinator_config: &CoordinatorConfig,
) -> std::result::Result<VerifyOutput, VerifyOutput> {
    use std::process::Command;
    use std::time::{Duration, Instant};

    // Try to generate a scoped command first
    let effective_cmd =
        generate_scoped_verify_command(verify_cmd, project_root, coordinator_config)
            .unwrap_or_else(|| verify_cmd.to_string());

    // Log scoping decision
    if effective_cmd != verify_cmd {
        eprintln!("[scoped-verify] Using scoped command: {}", effective_cmd);
        eprintln!("[scoped-verify] Original command: {}", verify_cmd);
    }

    // A free-text `verify` field was once routed into a synthetic evaluator.
    // That authority is retired: compatibility input fails deliberately rather
    // than creating review work or interpreting an evaluator as completion.
    if is_free_text_verify_command(&effective_cmd) {
        return Err(VerifyOutput {
            stdout: String::new(),
            stderr: "free-text verify compatibility is retired; use an executable deterministic verify command or ## Validation criteria".to_string(),
            exit_code: "retired-free-text-verify".to_string(),
        });
    }

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&effective_cmd)
        .current_dir(project_root)
        .env("TERM", "dumb") // Set TERM=dumb to avoid terminal-related failures
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Err(VerifyOutput {
                stdout: String::new(),
                stderr: format!("Failed to spawn verify command: {}", e),
                exit_code: "spawn-error".to_string(),
            });
        }
    };

    // Read stdout and stderr in background threads to prevent pipe buffer deadlock.
    // Without this, a child producing >64KB of output blocks on write and never exits.
    let stdout_handle = child.stdout.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::BufReader::new(s), &mut buf).ok();
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::BufReader::new(s), &mut buf).ok();
            buf
        })
    });

    let timeout = resolve_verify_timeout(task, coordinator_config);
    let start = Instant::now();
    let monitor = ProgressMonitor::new();

    // Poll with short sleeps to implement timeout without external crate
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    // Check if triage is enabled
                    if coordinator_config.verify_triage_enabled {
                        // Perform triage to determine if this is a genuine hang or waiting
                        let progress_timeout = coordinator_config
                            .verify_progress_timeout
                            .as_ref()
                            .and_then(|s| parse_delay(s))
                            .map(std::time::Duration::from_secs)
                            .unwrap_or(std::time::Duration::from_secs(300));

                        match triage_timeout_process(&monitor, progress_timeout) {
                            Ok(TriageResult::WaitingOnLocks { detected_locks }) => {
                                eprintln!(
                                    "Verify timeout triage: detected lock contention on {:?}, extending timeout by 300s",
                                    detected_locks
                                );
                                // Extend timeout and continue
                                // Note: This is a simple implementation - in production we might want retry limits
                                std::thread::sleep(std::time::Duration::from_secs(5));
                                continue;
                            }
                            Ok(TriageResult::UnknownButActive { activity_type }) => {
                                eprintln!(
                                    "Verify timeout triage: process active ({}), extending timeout by 300s",
                                    activity_type
                                );
                                // Extend timeout and continue
                                std::thread::sleep(std::time::Duration::from_secs(5));
                                continue;
                            }
                            Ok(TriageResult::GenuineHang { reason }) => {
                                eprintln!(
                                    "Verify timeout triage: genuine hang detected ({}), failing",
                                    reason
                                );
                                // Proceed with normal timeout failure
                            }
                            _ => {
                                eprintln!(
                                    "Verify timeout triage: unknown condition, failing with timeout"
                                );
                                // Proceed with normal timeout failure
                            }
                        }
                    }

                    // Standard timeout failure (either no triage or triage determined genuine hang)
                    let _ = child.kill();
                    let _ = child.wait();
                    let stdout = stdout_handle
                        .map(|h| h.join().unwrap_or_default())
                        .unwrap_or_default();
                    let stderr = stderr_handle
                        .map(|h| h.join().unwrap_or_default())
                        .unwrap_or_default();
                    return Err(VerifyOutput {
                        stdout,
                        stderr: format!(
                            "Verify command timed out after {}s\n{}",
                            timeout.as_secs(),
                            stderr
                        ),
                        exit_code: "timeout".to_string(),
                    });
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(VerifyOutput {
                    stdout: String::new(),
                    stderr: format!("Failed to wait on verify command: {}", e),
                    exit_code: "wait-error".to_string(),
                });
            }
        }
    };

    let stdout = stdout_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    let stderr = stderr_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    let exit_code = status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());

    if status.success() {
        Ok(VerifyOutput {
            stdout,
            stderr,
            exit_code,
        })
    } else {
        Err(VerifyOutput {
            stdout,
            stderr,
            exit_code,
        })
    }
}

/// Returns `true` if the path emitted by `git status --porcelain` refers to a
/// WG-internal directory that should never trigger a hygiene warning.
///
/// Matches `.wg/`, `.wg-worktrees/`, legacy `.workgraph/`, and numbered
/// worktree dirs like `.workgraph.1/` and `.wg.1/`. These are WG's
/// own data — agents should never commit them, but they are *expected* to
/// sit untracked in the working tree (see project memory: "stale
/// `.workgraph.N/` dirs + agency YAMLs sit untracked"), so flagging them
/// in `wg done` only produces noise.
fn is_hygiene_ignored_path(path: &str) -> bool {
    let p = path.trim_start_matches("./");
    let head = p.split('/').next().unwrap_or(p);
    head == ".wg"
        || head == ".wg-worktrees"
        || head == ".workgraph"
        || head.starts_with(".wg.")
        || head.starts_with(".workgraph.")
}

/// Parse a single line of `git status --porcelain` and return the path.
///
/// Format: `XY <path>` where XY is the two-char status code and the path is
/// the remainder. Renames (`R `) carry `<old> -> <new>`; we use the new path
/// (after the arrow) as the relevant working-tree entry. Quoted paths
/// (containing whitespace or special chars) are unquoted minimally.
fn porcelain_path(line: &str) -> Option<&str> {
    if line.len() < 4 {
        return None;
    }
    let rest = &line[3..];
    if let Some((_old, new)) = rest.split_once(" -> ") {
        Some(new.trim_matches('"'))
    } else {
        Some(rest.trim_matches('"'))
    }
}

/// Filter `git status --porcelain` output, dropping lines whose path is a
/// WG-internal directory we don't want to warn about.
fn filter_hygiene_porcelain(status: &str) -> Vec<&str> {
    status
        .lines()
        .filter(|line| match porcelain_path(line) {
            Some(p) => !is_hygiene_ignored_path(p),
            None => true,
        })
        .collect()
}

/// Check git hygiene when an agent marks a task as done.
/// Emits warnings for uncommitted changes and stash growth.
///
/// Skipped entirely for chat-loop tasks — a chat agent is a conversation
/// endpoint, not a code agent, and should never be lectured about
/// uncommitted state (see chat-agent-loops bug B). WG-internal
/// paths (`.wg/`, `.wg.*/`, etc.) are filtered from the warning
/// even when the check does run.
fn check_agent_git_hygiene(dir: &Path, task_id: &str, tags: &[String]) {
    if tags.iter().any(|t| worksgood::chat_id::is_chat_loop_tag(t)) {
        return;
    }
    use std::process::Command;
    let project_root = dir.parent().unwrap_or(dir);
    if let Ok(output) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
    {
        let status = String::from_utf8_lossy(&output.stdout);
        let filtered = filter_hygiene_porcelain(&status);
        if !filtered.is_empty() {
            let changed: Vec<&str> = filtered.into_iter().take(10).collect();
            eprintln!(
                "Warning: git hygiene for '{}': uncommitted changes:\n{}",
                task_id,
                changed.join("\n")
            );
        }
    }
    if let Ok(output) = Command::new("git")
        .args(["stash", "list"])
        .current_dir(project_root)
        .output()
    {
        let count = String::from_utf8_lossy(&output.stdout).lines().count();
        if count > 0 {
            eprintln!(
                "Warning: git hygiene for '{}': {} stash(es) exist. Agents should never stash.",
                task_id, count
            );
        }
    }
}

/// Run the smoke gate for `wg done`.
///
/// If a smoke manifest exists and the task owns scenarios in it, those
/// scenarios run live. Any FAIL or ERROR outcome blocks `wg done` with a
/// message naming the broken scenarios. SKIP outcomes are surfaced loudly
/// but never block.
///
/// `--skip-smoke` skips the gate entirely; agents are refused the escape
/// hatch unless `WG_SMOKE_AGENT_OVERRIDE=1` is also set in the environment.
/// `--full-smoke` runs every scenario in the manifest, ignoring ownership.
fn run_smoke_gate(
    dir: &Path,
    id: &str,
    full_smoke: bool,
    skip_smoke: bool,
    is_agent: bool,
) -> Result<()> {
    if skip_smoke {
        if is_agent && std::env::var("WG_SMOKE_AGENT_OVERRIDE").ok().as_deref() != Some("1") {
            anyhow::bail!(
                "Agents cannot use --skip-smoke. The smoke gate is the regression contract; \
                 fix the broken scenario or escalate to a human. \
                 If a human is intentionally bypassing for this task, export \
                 WG_SMOKE_AGENT_OVERRIDE=1 in this shell."
            );
        }
        eprintln!(
            "WARNING: --skip-smoke bypassed the smoke gate for '{}'. Any scenario regression will not be caught.",
            id
        );
        return Ok(());
    }

    let manifest_path = SmokeManifest::resolve_path(dir);
    let manifest = SmokeManifest::load_from(&manifest_path)
        .with_context(|| format!("loading smoke manifest from {}", manifest_path.display()))?;

    let scenarios: Vec<_> = if full_smoke {
        manifest.scenarios.iter().collect()
    } else {
        manifest.scenarios_for_task(id)
    };

    if scenarios.is_empty() {
        // No scenarios owned by this task and no --full-smoke. Quiet no-op.
        return Ok(());
    }

    let manifest_dir = manifest_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dir.to_path_buf());

    eprintln!(
        "Smoke gate: running {} scenario(s) for '{}' (manifest: {})",
        scenarios.len(),
        id,
        manifest_path.display()
    );

    let report = smoke::run_scenarios(&scenarios, &manifest_dir);
    eprint!("{}", report.render());

    if report.blocks_done() {
        let mut broken: Vec<String> = report
            .failures()
            .iter()
            .map(|r| match &r.outcome {
                ScenarioOutcome::Fail { exit_code, .. } => {
                    format!("{} (exit {})", r.name, exit_code)
                }
                _ => r.name.clone(),
            })
            .collect();
        broken.extend(report.errors().iter().map(|r| match &r.outcome {
            ScenarioOutcome::Error { message } => format!("{} ({})", r.name, message),
            _ => r.name.clone(),
        }));
        anyhow::bail!(
            "Smoke gate refused 'wg done {}': {} scenario(s) broken — {}\n\
             Fix the regression or, if you understand why this is non-blocking, \
             rerun with --skip-smoke (humans only).",
            id,
            broken.len(),
            broken.join(", ")
        );
    }

    if !report.skips().is_empty() {
        eprintln!(
            "Smoke gate: {} scenario(s) emitted SKIP — verify the missing endpoint/credential before claiming done.",
            report.skips().len()
        );
    }

    Ok(())
}

/// Snapshot the gate meaning that the user will see after `wg done`.
/// New policy is eligible only after the authoritative running-attempt proof;
/// historical persisted gate snapshots remain readable and drain unchanged.
/// Every source that enters `PendingEval` therefore has a real hard gate with
/// attempt-pinned thresholds rather than an eager satellite inferred by name.
fn completion_gate_policy(
    graph: &worksgood::graph::WorkGraph,
    id: &str,
    config: &Config,
) -> Option<worksgood::eval_lifecycle::EvaluationGatePolicy> {
    if id.starts_with('.') {
        return None;
    }
    let source = graph.get_task(id)?;
    if let Some(policy) = source
        .evaluation_lifecycle
        .as_ref()
        .and_then(|lifecycle| lifecycle.gate_policy.clone())
    {
        return Some(policy);
    }
    if !worksgood::evaluation::has_authenticated_running_attempt(source) {
        return None;
    }
    worksgood::evaluation::LazyEvaluationSelection::resolve(source, config)
        .ok()?
        .gate_policy()
}

fn pick_done_target_status(
    id: &str,
    policy: Option<&worksgood::eval_lifecycle::EvaluationGatePolicy>,
) -> Status {
    if id.starts_with('.') {
        return Status::Done;
    }
    if policy.is_some_and(|policy| {
        policy.applicability == worksgood::eval_lifecycle::EvaluationGateApplicability::Required
    }) {
        Status::PendingEval
    } else {
        Status::Done
    }
}

fn create_user_board_successor_after_done(dir: &Path, id: &str) {
    let Ok(graph) = worksgood::parser::load_graph(super::graph_path(dir)) else {
        return;
    };
    if graph
        .get_task(id)
        .is_none_or(|task| !task.tags.iter().any(|tag| tag == "user-board"))
    {
        return;
    }
    let Some(handle) = user_board_handle(id) else {
        return;
    };
    let successor = create_user_board_task(handle, user_board_seq(id).unwrap_or(0) + 1);
    let successor_id = successor.id.clone();
    drop(graph);
    if let Err(error) = modify_graph(super::graph_path(dir), |graph| {
        let mut changed = false;
        if let Some(task) = graph.get_task_mut(id)
            && !task.tags.iter().any(|tag| tag == "archived")
        {
            task.tags.push("archived".into());
            changed = true;
        }
        if graph.get_task(&successor_id).is_none() {
            graph.add_node(Node::Task(successor.clone()));
            changed = true;
        }
        changed
    }) {
        eprintln!("Warning: failed to create successor board: {error}");
    } else {
        println!("Created successor board '{successor_id}'");
        super::notify_graph_changed(dir);
    }
}

fn post_graphsave_done_compat(
    dir: &Path,
    id: &str,
    actor: Option<&str>,
    converged_requested: bool,
    converged_accepted: bool,
) -> Result<()> {
    let path = super::graph_path(dir);
    let message = if converged_accepted {
        "Task marked as done (converged)"
    } else if converged_requested {
        "Task marked as done (--converged ignored, cycle is forced)"
    } else {
        "Task marked as done"
    };
    modify_graph(&path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        if converged_accepted && !task.tags.iter().any(|tag| tag == "converged") {
            task.tags.push("converged".into());
        }
        if matches!(
            task.failure_class,
            Some(FailureClass::DeliverableMissing) | Some(FailureClass::NoOperationalOutput)
        ) {
            task.failure_class = None;
            task.failure_reason = None;
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: actor.map(String::from),
            user: Some(worksgood::current_user()),
            message: message.into(),
        });
        true
    })?;
    if let Ok(mut locked_registry) = AgentRegistry::load_locked(dir) {
        if let Some(agent) = locked_registry.get_agent_by_task_mut(id) {
            agent.status = worksgood::service::registry::AgentStatus::Done;
            if agent.completed_at.is_none() {
                agent.completed_at = Some(Utc::now().to_rfc3339());
            }
        }
        let _ = locked_registry.save_ref();
    }
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "done",
        Some(id),
        actor,
        serde_json::json!({ "completion": "graph-save" }),
        config.log.rotation_threshold,
    );
    create_user_board_successor_after_done(dir, id);
    Ok(())
}

pub fn run(
    dir: &Path,
    id: &str,
    converged: bool,
    skip_verify: bool,
    ignore_unmerged_worktree: bool,
    full_smoke: bool,
    skip_smoke: bool,
) -> Result<()> {
    let is_agent = std::env::var("WG_AGENT_ID").is_ok();
    let result = run_inner(
        dir,
        id,
        converged,
        skip_verify,
        ignore_unmerged_worktree,
        is_agent,
        full_smoke,
        skip_smoke,
    );

    // Provider health consumes this typed provenance, not this command's
    // human-facing stderr. The write is best-effort so an observability
    // failure can never change `wg done` semantics; triage later validates
    // agent/task/run identity against spawn metadata before trusting it.
    if is_agent {
        let outcome = match &result {
            Ok(()) => worksgood::service::ExecutionOutcome::CompletionAccepted,
            Err(error) => worksgood::service::ExecutionOutcome::CompletionRefused {
                code: worksgood::service::completion_refusal_code(&error.to_string()),
            },
        };
        if let Err(error) = worksgood::service::record_done_outcome(dir, id, outcome) {
            eprintln!("Warning: failed to record wg done outcome: {error}");
        }
    }

    result
}

fn run_inner(
    dir: &Path,
    id: &str,
    converged: bool,
    skip_verify: bool,
    ignore_unmerged_worktree: bool,
    is_agent: bool,
    full_smoke: bool,
    skip_smoke: bool,
) -> Result<()> {
    let (mut graph, path) = super::load_workgraph_mut(dir)?;

    let task = graph.get_task_mut_or_err(id)?;

    if task.status == Status::Done {
        println!("Task '{}' is already done", id);
        return Ok(());
    }
    if task.status == Status::PendingEval {
        println!(
            "Task '{}' candidate is already complete and awaiting evaluation evidence",
            id
        );
        return Ok(());
    }

    // Resolve task-owned completion context once and reuse that exact root for
    // preflight and finalization. A context mismatch must fail before any gate
    // can consult the graph root and accidentally accept stale bytes there.
    let assigned_agent = task.assigned.clone();
    let completion_worktree = detect_worktree(dir, id, assigned_agent.as_deref())?;

    // Check for unresolved blockers (cycle-aware: only exempt back-edge blockers,
    // not all same-cycle blockers).
    //
    // Any blocker that is in the same cycle (SCC) as the task being completed
    // is exempted — both header and non-header members.  The mutual dependency
    // between cycle members is a structural back-edge; blocking on it would
    // deadlock the cycle.
    let cycle_analysis = graph.compute_cycle_analysis();
    let effective_blockers: Vec<String> = graph
        .get_task(id)
        .into_iter()
        .flat_map(|task| task.after.iter())
        .filter_map(|blocker_id| {
            let disposition = query::dependency_disposition(blocker_id, id, &graph, Some(dir));
            if disposition.is_satisfied() {
                return None;
            }
            // A structural cycle back-edge may break liveness only while its
            // peer remains a viable cycle participant. Failed/abandoned peers
            // and missing/archive boundaries never become success by cycling.
            let in_same_cycle = cycle_analysis
                .task_to_cycle
                .get(blocker_id)
                .is_some_and(|cycle| cycle_analysis.task_to_cycle.get(id) == Some(cycle));
            let viable_cycle_peer = in_same_cycle
                && graph.get_task(blocker_id).is_some_and(|blocker| {
                    !matches!(blocker.status, Status::Failed | Status::Abandoned)
                });
            if viable_cycle_peer {
                return None;
            }
            let reason = match disposition {
                query::DependencyDisposition::Blocked { reason } => reason,
                query::DependencyDisposition::EvalSystemBypass { .. }
                | query::DependencyDisposition::AdvisoryQualityBypass { .. }
                | query::DependencyDisposition::Satisfied => return None,
            };
            let status = graph
                .get_task(blocker_id)
                .map(|blocker| blocker.status.to_string())
                .or_else(|| {
                    graph
                        .get_archived_boundary(blocker_id)
                        .map(|boundary| format!("archived {}", boundary.status))
                })
                .unwrap_or_else(|| "missing".to_string());
            Some(format!("  - {blocker_id} ({status}): {reason}"))
        })
        .collect();
    if !effective_blockers.is_empty() {
        anyhow::bail!(
            "Cannot mark '{}' as done: blocked by {} unresolved required-success prerequisite(s):\n{}\nRepair by retrying/reopening the prerequisite, relinking to a completed replacement, or explicitly removing the edge with `wg rm-dep <task> <prerequisite>`.",
            id,
            effective_blockers.len(),
            effective_blockers.join("\n")
        );
    }

    // Git hygiene check for agents: warn about uncommitted changes.
    // Skipped entirely for chat-loop tasks (chat-agent-loops bug B) — see
    // `check_agent_git_hygiene`. We pass the task's tags through so the
    // skip decision is local to the hygiene helper.
    if is_agent {
        let tags = graph
            .get_task(id)
            .map(|t| t.tags.clone())
            .unwrap_or_default();
        check_agent_git_hygiene(dir, id, &tags);
    }

    // Deliverable preflight (guardrail G1): before the smoke gate, parse a
    // `## Deliverables` block (path-like `## Validation` lines as fallback)
    // from the task description. If any named filesystem deliverable is
    // absent/empty or any `registry:<file>:<id>` deliverable is missing,
    // refuse `wg done` with a machine-readable `deliverable-missing` failure
    // class instead of promoting a no-deliverable run to Done. Tasks with no
    // parsed deliverables are unaffected (no regression for research/review).
    let project_root = dir.parent().unwrap_or(dir);
    let deliverable_root = completion_worktree
        .as_ref()
        .map(|worktree| Path::new(&worktree.worktree_path))
        .unwrap_or(project_root);
    let deliverables = graph
        .get_task(id)
        .and_then(|t| t.description.clone())
        .map(|d| super::deliverables::parse_deliverables(&d))
        .unwrap_or_default();
    if !deliverables.is_empty() {
        let report = super::deliverables::preflight(&deliverables, deliverable_root);
        // No environment override exists on purpose. An env var such as
        // `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1` would be inherited by every
        // spawned agent and become a copy-pasteable way to mark missing
        // deliverables complete — defeating the gate for exactly the cases it
        // is meant to stop (see PR #54, Erik CHANGES_REQUESTED). If preflight
        // fires on a genuine false positive, fix the `## Deliverables` block so
        // it names only files this worktree actually produces (path lines are
        // parsed; discard/external-worktree files should not be listed).
        if !report.is_clean() {
            let id_owned = id.to_string();
            let reason = format!("missing deliverables:\n{}", report.missing_summary());
            let reason_for_log = reason.clone();
            modify_graph(&path, |g| {
                if let Some(t) = g.get_task_mut(&id_owned) {
                    t.failure_class = Some(FailureClass::DeliverableMissing);
                    t.failure_reason = Some(reason_for_log.clone());
                    t.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some("deliverable-preflight".to_string()),
                        user: Some(worksgood::current_user()),
                        message: format!("wg done refused: {}", reason_for_log),
                    });
                }
                true
            })
            .context("Failed to save deliverable-preflight refusal")?;
            super::notify_graph_changed(dir);
            anyhow::bail!(
                "Cannot mark '{}' as done: deliverable preflight refused — \
                 required deliverables were not produced. `wg done` will keep \
                 refusing until these exist and are non-empty:\n{}\n\n\
                 If this is a false positive (e.g. a file the task tells you to \
                 discard, or one that belongs to a different worktree/repo), \
                 correct the task's `## Deliverables` block so it lists only \
                 files this worktree actually produces — there is no \
                 environment bypass.",
                id,
                report.missing_summary()
            );
        }
    }

    // Smoke gate: a task cannot be marked done while a regression-protecting
    // smoke scenario it owns is failing. Refuse the agent escape hatch unless
    // a separate override is set, so an agent can't smother a real regression
    // by adding `--skip-smoke` to its `wg done`.
    if let Err(e) = run_smoke_gate(dir, id, full_smoke, skip_smoke, is_agent) {
        return Err(e);
    }

    // Auto-defer verify when the task has been decomposed into subtasks.
    // When an agent decomposes a parent task into children (via `wg add --after parent`),
    // the parent's --verify command creates a deadlock: verify fails because children
    // haven't run, but children are blocked on the parent completing. We resolve this
    // by detecting children and deferring verify to a synthetic aggregate task that
    // runs after all children complete.
    //
    // Gated on coordinator.verify_autospawn_enabled (default false as of 2026-04-17).
    // The shadow-task pattern is deprecated in favor of single-leaf evaluate +
    // wg rescue proxy-insert on FAIL.
    if Config::load_or_default(dir)
        .coordinator
        .verify_autospawn_enabled
        && let Some(task) = graph.get_task(id)
        && task.verify.is_some()
    {
        // Find non-system children: tasks that list this task in their `after` field
        // and were created by the agent (not system scaffolding like .assign-*, .flip-*, etc.)
        let children: Vec<String> = task
            .before
            .iter()
            .filter(|child_id| {
                !worksgood::graph::is_system_task(child_id)
                    && graph
                        .get_task(child_id)
                        .is_some_and(|ct| !ct.status.is_terminal())
            })
            .cloned()
            .collect();

        if !children.is_empty() {
            let verify_cmd = task.verify.clone().unwrap();
            let verify_timeout = task.verify_timeout.clone();
            let deferred_id = format!(".verify-deferred-{}", id);

            // Only create the deferred task if it doesn't already exist
            if graph.get_task(&deferred_id).is_none() {
                let id_for_defer = id.to_string();
                let children_clone = children.clone();
                let deferred_id_clone = deferred_id.clone();
                let verify_cmd_clone = verify_cmd.clone();
                let verify_timeout_clone = verify_timeout.clone();

                modify_graph(&path, |g| {
                    // Clear verify from the parent so it can complete
                    if let Some(parent) = g.get_task_mut(&id_for_defer) {
                        parent.verify = None;
                        parent.log.push(LogEntry {
                            timestamp: Utc::now().to_rfc3339(),
                            actor: Some("verify-defer".to_string()),
                            user: None,
                            message: format!(
                                "Verify deferred to {} — {} subtask(s) detected: {}",
                                deferred_id_clone,
                                children_clone.len(),
                                children_clone.join(", "),
                            ),
                        });
                    }

                    // Create the deferred verification task that depends on all children
                    let deferred_task = Task {
                        id: deferred_id_clone.clone(),
                        title: format!("Deferred verify: {}", id_for_defer),
                        description: Some(format!(
                            "## Deferred Verification\n\n\
                             The parent task `{}` was decomposed into subtasks. \
                             This task runs the parent's verify command after all subtasks complete.\n\n\
                             **Verify command:** `{}`\n\n\
                             **Subtasks:**\n{}\n",
                            id_for_defer,
                            verify_cmd_clone,
                            children_clone
                                .iter()
                                .map(|c| format!("- `{}`", c))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )),
                        status: Status::Open,
                        after: children_clone.clone(),
                        verify: Some(verify_cmd_clone),
                        verify_timeout: verify_timeout_clone,
                        created_at: Some(Utc::now().to_rfc3339()),
                        tags: vec!["verify-deferred".to_string()],
                        ..Default::default()
                    };

                    g.add_node(Node::Task(deferred_task));

                    // Maintain bidirectional edges: each child's `before` should point to the deferred task
                    for child_id in &children_clone {
                        if let Some(child) = g.get_task_mut(child_id)
                            && !child.before.contains(&deferred_id_clone)
                        {
                            child.before.push(deferred_id_clone.clone());
                        }
                    }

                    true
                })
                .context("Failed to create deferred verify task")?;

                eprintln!(
                    "Verify deferred to '{}' — {} subtask(s) must complete first",
                    deferred_id,
                    children.len(),
                );

                // Reload graph after mutation
                let (new_graph, _) = super::load_workgraph_mut(dir)?;
                graph = new_graph;
            }
        }
    }

    // Run verify command gate (if task has a verify field)
    if let Some(verify_cmd) = graph.get_task(id).and_then(|t| t.verify.clone()) {
        if skip_verify {
            // Block agents from using --skip-verify
            if is_agent {
                anyhow::bail!(
                    "Agents cannot use --skip-verify. The verify command must pass:\n  {}",
                    verify_cmd,
                );
            }
            eprintln!("Warning: skipping verify command: {}", verify_cmd);
        } else {
            let project_root = dir.parent().unwrap_or(dir);
            eprintln!("Running verify command: {}", verify_cmd);

            // Get task and coordinator config for enhanced timeout resolution
            let task = graph
                .get_task(id)
                .ok_or_else(|| anyhow::anyhow!("Task {} not found", id))?;
            let config = Config::load_or_default(dir);

            match run_verify_command_with_retry(
                &verify_cmd,
                project_root,
                task,
                &config.coordinator,
            ) {
                Ok(output) => {
                    // Log verify success with captured output
                    let id_for_log = id.to_string();
                    let stdout_preview: String = output.stdout.chars().take(200).collect();
                    let stderr_preview: String = output.stderr.chars().take(200).collect();
                    if !stdout_preview.is_empty() || !stderr_preview.is_empty() {
                        let mut log_msg = "Verify passed.".to_string();
                        if !stdout_preview.is_empty() {
                            log_msg.push_str(&format!(" stdout: {}", stdout_preview));
                        }
                        if !stderr_preview.is_empty() {
                            log_msg.push_str(&format!(" stderr: {}", stderr_preview));
                        }
                        let _ = modify_graph(&path, |g| {
                            if let Some(t) = g.get_task_mut(&id_for_log) {
                                // Reset verify failures on success
                                t.verify_failures = 0;
                                t.log.push(LogEntry {
                                    timestamp: Utc::now().to_rfc3339(),
                                    actor: Some("verify".to_string()),
                                    user: None,
                                    message: log_msg.clone(),
                                });
                                true
                            } else {
                                false
                            }
                        });
                        // Reload graph after mutation
                        let (new_graph, _) = super::load_workgraph_mut(dir)?;
                        graph = new_graph;
                    } else {
                        // Reset verify failures on success even without output
                        let _ = modify_graph(&path, |g| {
                            if let Some(t) = g.get_task_mut(&id_for_log) {
                                t.verify_failures = 0;
                                true
                            } else {
                                false
                            }
                        });
                        let (new_graph, _) = super::load_workgraph_mut(dir)?;
                        graph = new_graph;
                    }
                    eprintln!("Verify command passed");
                }
                Err(output) => {
                    // Check if this is a malformed verify command that can be auto-corrected
                    if let Some(corrected_cmd) =
                        worksgood::verify_lint::auto_correct_verify_command(&verify_cmd)
                    {
                        eprintln!(
                            "Verify command appears malformed, auto-correcting: {} → {}",
                            verify_cmd, corrected_cmd
                        );

                        // Update the task's verify command in the graph and reset failure count
                        let id_for_update = id.to_string();
                        let corrected_cmd_clone = corrected_cmd.clone();
                        modify_graph(&path, |g| {
                            if let Some(t) = g.get_task_mut(&id_for_update) {
                                t.verify = Some(corrected_cmd_clone.clone());
                                t.verify_failures = 0; // Reset failure count for auto-corrected command
                                t.log.push(LogEntry {
                                    timestamp: Utc::now().to_rfc3339(),
                                    actor: Some("verify-autocorrect".to_string()),
                                    user: None,
                                    message: format!(
                                        "Auto-corrected malformed verify command: '{}' → '{}'",
                                        verify_cmd, corrected_cmd_clone
                                    ),
                                });
                                true
                            } else {
                                false
                            }
                        })
                        .context("Failed to save auto-corrected verify command")?;

                        // Retry with the corrected command
                        eprintln!("Retrying with corrected command: {}", corrected_cmd);

                        // Reload graph to get updated task
                        let (new_graph, _) = super::load_workgraph_mut(dir)?;
                        let updated_task = new_graph
                            .get_task(id)
                            .ok_or_else(|| anyhow::anyhow!("Task {} not found after update", id))?;

                        match run_verify_command_with_retry(
                            &corrected_cmd,
                            project_root,
                            updated_task,
                            &config.coordinator,
                        ) {
                            Ok(output) => {
                                // Auto-correction worked! Log success
                                let id_for_log = id.to_string();
                                let stdout_preview: String =
                                    output.stdout.chars().take(200).collect();
                                let stderr_preview: String =
                                    output.stderr.chars().take(200).collect();
                                let mut log_msg =
                                    "Verify passed (after auto-correction).".to_string();
                                if !stdout_preview.is_empty() {
                                    log_msg.push_str(&format!(" stdout: {}", stdout_preview));
                                }
                                if !stderr_preview.is_empty() {
                                    log_msg.push_str(&format!(" stderr: {}", stderr_preview));
                                }
                                let _ = modify_graph(&path, |g| {
                                    if let Some(t) = g.get_task_mut(&id_for_log) {
                                        t.log.push(LogEntry {
                                            timestamp: Utc::now().to_rfc3339(),
                                            actor: Some("verify".to_string()),
                                            user: None,
                                            message: log_msg,
                                        });
                                        true
                                    } else {
                                        false
                                    }
                                });
                                eprintln!("Auto-corrected verify command passed");
                                return Ok(()); // Success after auto-correction
                            }
                            Err(_) => {
                                // Auto-corrected command also failed, proceed with normal failure handling
                                eprintln!(
                                    "Auto-corrected verify command also failed, treating as normal verify failure"
                                );
                                // Fall through to normal failure handling with the original command
                            }
                        }
                    }

                    // Normal verify failure handling (original command failed and either
                    // no auto-correction was possible, or auto-correction also failed)
                    let id_for_circuit = id.to_string();
                    let verify_cmd_clone = verify_cmd.clone();
                    let stdout_preview: String = output.stdout.chars().take(500).collect();
                    let stderr_preview: String = output.stderr.chars().take(500).collect();
                    let exit_code = output.exit_code.clone();

                    let config = Config::load_or_default(dir);
                    let max_verify_failures = config.coordinator.max_verify_failures;

                    modify_graph(&path, |g| {
                        let task = match g.get_task_mut(&id_for_circuit) {
                            Some(t) => t,
                            None => return false,
                        };
                        task.verify_failures += 1;
                        let failures = task.verify_failures;

                        // Log the verify failure with output
                        let mut log_msg = format!(
                            "Verify FAILED (exit code {}, attempt {}/{}). Command: {}",
                            exit_code,
                            failures,
                            if max_verify_failures > 0 {
                                max_verify_failures.to_string()
                            } else {
                                "unlimited".to_string()
                            },
                            verify_cmd_clone,
                        );
                        if !stdout_preview.is_empty() {
                            log_msg.push_str(&format!("\nstdout: {}", stdout_preview));
                        }
                        if !stderr_preview.is_empty() {
                            log_msg.push_str(&format!("\nstderr: {}", stderr_preview));
                        }
                        task.log.push(LogEntry {
                            timestamp: Utc::now().to_rfc3339(),
                            actor: Some("verify".to_string()),
                            user: None,
                            message: log_msg,
                        });

                        true
                    })
                    .context("Failed to save verify failure state")?;

                    // Not yet at threshold — propagate error so agent retries
                    let mut error_msg = format!(
                        "Verify command failed (exit code {}): {}",
                        exit_code, verify_cmd,
                    );
                    if !stderr_preview.is_empty() {
                        error_msg.push_str(&format!("\nstderr: {}", stderr_preview));
                    }
                    if !stdout_preview.is_empty() {
                        error_msg.push_str(&format!("\nstdout: {}", stdout_preview));
                    }
                    anyhow::bail!(error_msg);
                }
            }
        }
    }

    // Determine validation mode for this task.
    // Resolution: task.validation > "none" (default, backward compatible).
    let validation_mode = graph
        .get_task(id)
        .and_then(|t| t.validation.clone())
        .unwrap_or_else(|| "none".to_string());

    // Integrated validation: enforce log check + run validation_commands
    if validation_mode == "integrated" {
        let task_ref = graph.get_task(id).unwrap();
        let has_validation_log = task_ref
            .log
            .iter()
            .any(|entry| entry.message.to_lowercase().contains("validat"));
        if !has_validation_log {
            anyhow::bail!(
                "Cannot mark '{}' as done: integrated validation requires a validation log entry.\n\
                 Add one with: wg log {} \"Validated: <what you checked>\"",
                id,
                id
            );
        }
        let commands = task_ref.validation_commands.clone();
        if !commands.is_empty() {
            let project_root = dir.parent().unwrap_or(dir);
            let config = Config::load_or_default(dir);
            for cmd in &commands {
                eprintln!("Running validation command: {}", cmd);
                match run_verify_command_with_retry(
                    cmd,
                    project_root,
                    task_ref,
                    &config.coordinator,
                ) {
                    Ok(_) => {}
                    Err(output) => {
                        let stderr: String = output.stderr.chars().take(500).collect();
                        let stdout: String = output.stdout.chars().take(500).collect();
                        let mut msg = format!(
                            "Integrated validation failed for '{}': command failed (exit code {}): {}",
                            id, output.exit_code, cmd,
                        );
                        if !stderr.is_empty() {
                            msg.push_str(&format!("\nstderr: {}", stderr));
                        }
                        if !stdout.is_empty() {
                            msg.push_str(&format!("\nstdout: {}", stdout));
                        }
                        anyhow::bail!(msg);
                    }
                }
            }
            eprintln!("All validation commands passed");
        }
    }

    if matches!(validation_mode.as_str(), "llm" | "external") {
        anyhow::bail!(
            "validation mode '{}' used a retired reviewer-task authority; use completion manifest review/receipts instead",
            validation_mode
        );
    }

    // When --converged is passed, determine whether the task's cycle has a
    // non-trivial guard or no_converge flag. If so, ignore the converged flag.
    // This prevents workers from bypassing external validation by
    // self-declaring convergence, and enforces forced cycles.
    //
    // We do this check with immutable access before mutating the task.
    let converged_accepted = if converged {
        // Check 1: the task itself has no_converge or a guarded cycle_config
        let own_no_converge = graph
            .get_task(id)
            .and_then(|t| t.cycle_config.as_ref())
            .map(|c| c.no_converge)
            .unwrap_or(false);

        let own_guard = graph
            .get_task(id)
            .and_then(|t| t.cycle_config.as_ref())
            .and_then(|c| c.guard.as_ref())
            .map(|g| !matches!(g, worksgood::graph::LoopGuard::Always))
            .unwrap_or(false);

        // Check 2: the task is a non-header member of a cycle whose header
        // has a non-trivial guard or no_converge. This covers workers trying
        // to converge a cycle they don't own.
        let (cycle_guard, cycle_no_converge) = if !own_guard && !own_no_converge {
            let ca = graph.compute_cycle_analysis();
            ca.task_to_cycle
                .get(id)
                .map(|&idx| {
                    let cycle = &ca.cycles[idx];
                    let guard = cycle.members.iter().any(|mid| {
                        graph
                            .get_task(mid)
                            .and_then(|t| t.cycle_config.as_ref())
                            .and_then(|c| c.guard.as_ref())
                            .map(|g| !matches!(g, worksgood::graph::LoopGuard::Always))
                            .unwrap_or(false)
                    });
                    let no_conv = cycle.members.iter().any(|mid| {
                        graph
                            .get_task(mid)
                            .and_then(|t| t.cycle_config.as_ref())
                            .map(|c| c.no_converge)
                            .unwrap_or(false)
                    });
                    (guard, no_conv)
                })
                .unwrap_or((false, false))
        } else {
            (false, false)
        };

        let has_guard = own_guard || cycle_guard;
        let has_no_converge = own_no_converge || cycle_no_converge;

        if has_no_converge {
            eprintln!(
                "Warning: --converged ignored for '{}' because the cycle is configured with --no-converge.\n         \
                 All iterations must run.",
                id
            );
            false
        } else if has_guard {
            eprintln!(
                "Warning: --converged ignored for '{}' because a cycle guard is set.\n         \
                 Only the guard condition determines convergence.",
                id
            );
            false
        } else {
            true
        }
    } else {
        false
    };

    // --- Task-owned finish transaction ---
    // New worktree-backed source tasks retain their original owner through
    // leased integration, exact-candidate evaluation, protected promotion and
    // wrapper-owned cleanup. Graph-less/operator compatibility below remains
    // only for historical transactions without a managed source worktree.
    if let Some(worktree) = completion_worktree.as_ref()
        && crate::commands::finalize::task_owned_done(
            dir,
            id,
            Some(Path::new(&worktree.worktree_path)),
        )?
    {
        return Ok(());
    }

    // A terminal command without a managed source worktree still needs an
    // explicit clean/no-worktree WorkSave and GraphSave.  Never fall through
    // to the historical raw AttemptSucceeded/Status::Done compatibility path.
    if completion_worktree.is_none() {
        crate::commands::finalize::commit_terminal_success(
            dir,
            id,
            assigned_agent.as_deref(),
            "wg_done_terminal_adapter",
        )?;
        post_graphsave_done_compat(
            dir,
            id,
            assigned_agent.as_deref(),
            converged,
            converged_accepted,
        )?;
        super::notify_graph_changed(dir);
        println!("Marked '{}' as done (GraphSave committed)", id);
        return Ok(());
    }

    // --- Historical crash-safe candidate finalization compatibility ---
    // A worker push is neither required nor invoked. The wrapper/watchdog must
    // first prove the exact handler is quiescent; then WG snapshots dirty,
    // untracked and deleted source through a private Git index, validates that
    // immutable descriptor, and mechanically merges only those exact bytes.
    let mut finalization_evidence: Vec<String> = Vec::new();
    let mut finalized_candidate: Option<(worksgood::finalization::CandidateDescriptor, String)> =
        None;
    let defer_candidate_merge_for_flip;
    if let Some(wt) = completion_worktree.as_ref() {
        let context = match crate::commands::finalize::context_from_current(
            dir,
            id,
            Some(std::path::PathBuf::from(&wt.worktree_path)),
            None,
            false,
        ) {
            Ok(mut context) => {
                let gate = completion_gate_policy(&graph, id, &Config::load_or_default(dir));
                defer_candidate_merge_for_flip = gate.as_ref().is_some_and(|policy| {
                    policy.applicability
                        == worksgood::eval_lifecycle::EvaluationGateApplicability::Required
                        && policy.flip_policy
                            == worksgood::eval_lifecycle::FlipVerdictPolicy::Required
                });
                context.evaluation_policy = if defer_candidate_merge_for_flip {
                    "required-deep-readonly-flip-before-merge".to_string()
                } else {
                    gate.as_ref()
                        .map(|policy| format!("{:?}", policy.applicability).to_lowercase())
                        .unwrap_or_else(|| "none".to_string())
                };
                context
            }
            Err(error) if error.to_string().contains("finalize.writer_still_current") => {
                if std::env::var("WG_EXECUTOR_TYPE").as_deref() == Ok("pi") {
                    let tool_call = format!(
                        "wg-done:{}",
                        std::env::var("WG_SPAWN_RUN_ID").unwrap_or_else(|_| id.to_string())
                    );
                    crate::commands::pi_watchdog::reserve_worker_terminal(
                        dir,
                        id,
                        worksgood::pi_watchdog::TerminalDisposition::SuccessIntent,
                        &tool_call,
                    )?;
                }
                eprintln!(
                    "[finalize] terminal intent reserved; exact writer is still current. The wrapper/watchdog will reconcile after reap."
                );
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let store = worksgood::finalization::FinalizationStore::open(dir)?;
        let checkpoint = worksgood::finalization::checkpoint_candidate(&store, &context)?;
        let candidate = checkpoint
            .candidate
            .as_ref()
            .context("candidate checkpoint missing descriptor")?;
        finalization_evidence.extend([
            candidate.candidate_id.clone(),
            candidate.candidate_commit_oid.clone(),
            candidate.candidate_tree_oid.clone(),
            candidate.content_manifest_cid.clone(),
            checkpoint
                .validation
                .as_ref()
                .context("candidate validation receipt missing")?
                .result_id
                .clone(),
        ]);
        finalized_candidate = Some((
            candidate.clone(),
            checkpoint
                .validation
                .as_ref()
                .context("candidate validation receipt missing")?
                .result_id
                .clone(),
        ));
        if defer_candidate_merge_for_flip {
            eprintln!(
                "[finalize] candidate={} commit={} tree={} manifest={} retained immutable; waiting on required deep-readonly FLIP before merge",
                candidate.candidate_id,
                candidate.candidate_commit_oid,
                candidate.candidate_tree_oid,
                candidate.content_manifest_cid,
            );
        } else {
            let merged = worksgood::finalization::merge_candidate(&store, candidate)?;
            if let Some(conflict) = merged.merge_conflict.as_ref() {
                anyhow::bail!(
                    "merge conflict retained as RepairNeeded ({}): candidate={} tree={} manifest={}. Safe next command: {}",
                    conflict.reason_code,
                    conflict.binding.candidate_id,
                    conflict.binding.tree_oid,
                    conflict.binding.manifest_cid,
                    merged.safe_next_command
                );
            }
            let receipt = merged
                .merge_receipt
                .as_ref()
                .context("merge.receipt_missing: replay with `wg finalize reconcile`")?;
            finalization_evidence.push(receipt.receipt_id.clone());
            eprintln!(
                "[finalize] candidate={} commit={} tree={} manifest={} merge-receipt={} (no push required)",
                candidate.candidate_id,
                candidate.candidate_commit_oid,
                candidate.candidate_tree_oid,
                candidate.content_manifest_cid,
                receipt.receipt_id,
            );
            let _ = ignore_unmerged_worktree; // conflicts are always retained, never bypassed
            mark_worktree_for_cleanup(&wt);
        }
    }

    // Atomically load the freshest graph, apply the mutation, and save.
    // Using modify_graph prevents the "lost update" race where a concurrent
    // spawn_eval_inline (or any other graph writer) saves between our read
    // and write, and our write clobbers its changes — or vice-versa.
    //
    // The pre-checks above (blockers, verify, validation) used a stale graph
    // snapshot, but they are idempotent gates: if they passed on the stale
    // version, they would also pass on the fresh version (task status can only
    // move forward, blockers can only resolve, not un-resolve).
    let mut cycle_reactivated = Vec::new();
    let mut already_done = false;
    let mut cycle_info: Option<(u32, u32)> = None; // (loop_iteration, max_iterations)
    let completion_config = Config::load_or_default(dir);
    completion_config.validate_model_format()?;

    // Resolve token usage outside the lock (registry read + file I/O).
    let token_usage = AgentRegistry::load(dir).ok().and_then(|registry| {
        let agent = registry.get_agent_by_task(id)?;
        let output_path = std::path::Path::new(&agent.output_file);
        let abs_path = if output_path.is_absolute() {
            output_path.to_path_buf()
        } else {
            dir.parent().unwrap_or(dir).join(output_path)
        };
        parse_token_usage(&abs_path).or_else(|| parse_wg_tokens(&abs_path))
    });

    let id_owned = id.to_string();
    let mut transitioned_to_pending_eval = false;
    let mut completed_with_advisory_eval = false;
    let mut gate_snapshot_error: Option<String> = None;
    let graph = modify_graph(&path, |graph| {
        // Lazy eligibility is derived from the exact candidate + launch proof,
        // never status alone. Resolve both products from one immutable source
        // snapshot before taking the mutable completion borrow.
        let source_snapshot = match graph.get_task(&id_owned) {
            Some(task) => task.clone(),
            None => return false,
        };
        let selection = match worksgood::evaluation::LazyEvaluationSelection::resolve(
            &source_snapshot,
            &completion_config,
        ) {
            Ok(selection) => selection,
            Err(error) => {
                gate_snapshot_error = Some(format!("{error:#}"));
                return false;
            }
        };
        let source_candidate = finalized_candidate.as_ref().and_then(|(candidate, validation)| {
            worksgood::evaluation::has_authenticated_running_attempt(&source_snapshot).then(|| {
                worksgood::evaluation::SourceCandidateRef {
                    task_id: id_owned.clone(),
                    generation: candidate.generation,
                    source_attempt_id: candidate.attempt_id.clone(),
                    source_fence: candidate.attempt_fence,
                    finalization_round: candidate.candidate_version,
                    candidate_digest: candidate.candidate_id.clone(),
                    candidate_manifest_digest: candidate.content_manifest_cid.clone(),
                    dependency_revision_digest:
                        worksgood::evaluation::dependency_revision_digest(
                            graph,
                            &source_snapshot,
                        )
                        .unwrap_or_else(|_| "b3:dependency-digest-unavailable".to_string()),
                    validation_result_id: validation.clone(),
                }
            })
        });
        // Any evidence-free, unclaimed eager rows are historical scaffolding,
        // not work that may outlive this completion transaction. Rows carrying
        // claims/verdicts remain untouched and readable on the legacy path.
        crate::commands::legacy_eval_compat::retire_safe_synthetic_rows(
            graph,
            &id_owned,
            true,
        );
        let gate_policy = if source_candidate.is_some() {
            selection.gate_policy()
        } else {
            // Compatibility only: a previously persisted gate snapshot is
            // authoritative. Mere `.evaluate-*`/`.flip-*` row names are not.
            source_snapshot
                .evaluation_lifecycle
                .as_ref()
                .and_then(|lifecycle| lifecycle.gate_policy.clone())
        };
        let target_status = pick_done_target_status(&id_owned, gate_policy.as_ref());
        let advisory_evaluation = gate_policy.as_ref().is_some_and(|policy| {
            policy.applicability
                == worksgood::eval_lifecycle::EvaluationGateApplicability::Advisory
        });
        let task = match graph.get_task_mut(&id_owned) {
            Some(t) => t,
            None => return false,
        };

        // Re-check: another writer may have marked it Done already.
        if task.status == Status::Done {
            already_done = true;
            return false;
        }

        if let Some(source) = source_candidate.as_ref() {
            let mut request = TransitionRequest::new(
                TransitionKind::CandidateCheckpointed {
                    candidate_id: source.candidate_digest.clone(),
                    manifest_cid: source.candidate_manifest_digest.clone(),
                    validation_result_id: source.validation_result_id.clone(),
                    finalization_round: source.finalization_round,
                },
                LifecycleActor {
                    kind: worksgood::lifecycle::ActorKind::Finalizer,
                    id: "candidate-finalizer".to_string(),
                },
                "candidate_checkpointed",
                format!(
                    "candidate-checkpointed:{}:{}:{}",
                    id_owned, source.source_attempt_id, source.candidate_digest
                ),
            )
            .expecting(FenceExpectation::current(task));
            request.evidence_refs.extend([
                source.candidate_digest.clone(),
                source.candidate_manifest_digest.clone(),
                source.validation_result_id.clone(),
            ]);
            if let Err(error) = apply_transition(task, request) {
                gate_snapshot_error = Some(error.to_string());
                return false;
            }
        }

        if let Some(policy) = gate_policy
            && let Err(error) = worksgood::eval_lifecycle::snapshot_source_gate(
                task,
                policy,
                if target_status == Status::PendingEval {
                    worksgood::eval_lifecycle::EvaluationGateOutcome::AwaitingEvidence
                } else {
                    worksgood::eval_lifecycle::EvaluationGateOutcome::AdvisoryCompleted
                },
            )
        {
            gate_snapshot_error = Some(format!("{error:#}"));
            return false;
        }

        if let Some(source) = source_candidate.as_ref()
            && let Err(error) = worksgood::evaluation::mint_for_candidate(
                task,
                source,
                &selection,
                &completion_config,
            )
        {
            gate_snapshot_error = Some(format!("{error:#}"));
            return false;
        }

        // All supported terminal paths returned through task_owned_done or
        // commit_terminal_success above.  Reaching this compatibility body
        // must hold rather than resurrecting the raw AttemptSucceeded writer.
        gate_snapshot_error = Some(format!(
            "legacy terminal projection disabled for {}; replay through SaveTransaction",
            id_owned
        ));
        if gate_snapshot_error.is_some() {
            return false;
        }
        task.completed_at = Some(Utc::now().to_rfc3339());
        if target_status == Status::PendingEval {
            transitioned_to_pending_eval = true;
        } else if advisory_evaluation {
            completed_with_advisory_eval = true;
        }

        // Clear any prior deliverable-preflight / no-operational-output
        // refusal marker now that the run has produced its deliverables and
        // is being promoted out of InProgress (guardrail G1/G4 cleanup).
        if matches!(
            task.failure_class,
            Some(FailureClass::DeliverableMissing) | Some(FailureClass::NoOperationalOutput)
        ) {
            task.failure_class = None;
            task.failure_reason = None;
        }

        if converged_accepted && !task.tags.contains(&"converged".to_string()) {
            task.tags.push("converged".to_string());
        }

        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: task.assigned.clone(),
            user: Some(worksgood::current_user()),
            message: match target_status {
                Status::PendingEval => {
                    "Task pending required evaluation gate (agent reported done; awaiting exact attempt-bound hidden evaluation evidence)"
                        .to_string()
                }
                _ if advisory_evaluation => {
                    "Task marked as done; hidden bounded evaluation record is advisory evidence only (execution is not a quality pass)".to_string()
                }
                _ if converged_accepted => "Task marked as done (converged)".to_string(),
                _ if converged => {
                    "Task marked as done (--converged ignored, cycle is forced)".to_string()
                }
                _ => "Task marked as done".to_string(),
            },
        });

        // Apply pre-resolved token usage
        if task.token_usage.is_none()
            && let Some(ref usage) = token_usage
        {
            task.token_usage = Some(usage.clone());
        }

        // Capture cycle info for the output message before evaluating iteration
        let cycle_analysis = graph.compute_cycle_analysis();
        if cycle_analysis.task_to_cycle.contains_key(&id_owned) {
            // Task is in a structural cycle — find the config owner for iteration info
            if let Some(&idx) = cycle_analysis.task_to_cycle.get(&id_owned) {
                let cycle = &cycle_analysis.cycles[idx];
                for member_id in &cycle.members {
                    if let Some(t) = graph.get_task(member_id)
                        && let Some(ref cc) = t.cycle_config
                    {
                        cycle_info = Some((t.loop_iteration, cc.max_iterations));
                        break;
                    }
                }
            }
        } else if let Some(t) = graph.get_task(&id_owned)
            && let Some(ref cc) = t.cycle_config
        {
            // Implicit cycle (task has cycle_config but no SCC back-edge)
            cycle_info = Some((t.loop_iteration, cc.max_iterations));
        }

        // Evaluate structural cycle iteration
        cycle_reactivated = evaluate_cycle_iteration(graph, &id_owned, &cycle_analysis);

        true
    })
    .context("Failed to save graph")?;

    if let Some(error) = gate_snapshot_error {
        anyhow::bail!(
            "Cannot mark '{}' done: invalid evaluation gate policy (fail-closed): {}",
            id,
            error
        );
    }

    if already_done {
        println!("Task '{}' is already done", id);
        return Ok(());
    }

    super::notify_graph_changed(dir);

    // Update agent registry to reflect task completion.
    // Without this, the registry entry stays at Working until the daemon's
    // periodic triage detects the dead process — creating a window where the
    // agent appears alive and consumes an agent slot.
    if let Ok(mut locked_registry) = AgentRegistry::load_locked(dir) {
        if let Some(agent) = locked_registry.get_agent_by_task_mut(id) {
            agent.status = worksgood::service::registry::AgentStatus::Done;
            if agent.completed_at.is_none() {
                agent.completed_at = Some(Utc::now().to_rfc3339());
            }
        }
        let _ = locked_registry.save_ref();
    }
    let lease_owner = worksgood::disk_sentinel::caller_agent_for_task(id);
    if let Err(error) =
        worksgood::disk_sentinel::release_owned_cache_leases(dir, id, lease_owner.as_deref())
    {
        eprintln!("Warning: failed to release build-cache lease: {error:#}");
    }

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "done",
        Some(id),
        None,
        serde_json::Value::Null,
        config.log.rotation_threshold,
    );

    if let Some((iter, max)) = cycle_info {
        if converged_accepted {
            println!(
                "Marked '{}' as done (cycle iter {}/{}, converged — cycle halted).",
                id, iter, max
            );
        } else {
            println!(
                "Marked '{}' as done (cycle iter {}/{}). To halt the cycle here, use 'wg done {} --converged'.",
                id, iter, max, id
            );
        }
    } else if transitioned_to_pending_eval {
        println!(
            "Marked '{}' as pending-eval — required gate awaiting exact source-attempt hidden evaluation evidence before downstream tasks unblock",
            id
        );
    } else if completed_with_advisory_eval {
        println!(
            "Marked '{}' as done — bounded evaluation evidence is advisory only; inspect it in `wg show {}` or the TUI Detail pane",
            id, id
        );
    } else {
        println!("Marked '{}' as done", id);
    }

    // User board auto-increment is shared with the GraphSave fast path.
    create_user_board_successor_after_done(dir, id);

    for task_id in &cycle_reactivated {
        println!("  Cycle: re-activated '{}'", task_id);
    }

    // Archive agent conversation (prompt + output) for provenance
    if let Some(task) = graph.get_task(id)
        && let Some(ref agent_id) = task.assigned
    {
        match super::log::archive_agent(dir, id, agent_id) {
            Ok(archive_dir) => {
                eprintln!("Agent archived to {}", archive_dir.display());
            }
            Err(e) => {
                eprintln!("Warning: agent archive failed: {}", e);
            }
        }
    }

    // Capture task output (git diff, artifacts, log) as compatibility evidence.
    // New evaluation records bind immutable finalization objects instead of
    // scheduling an ordinary graph task.
    if let Some(task) = graph.get_task(id) {
        match capture_task_output(dir, task) {
            Ok(output_dir) => {
                eprintln!("Output captured to {}", output_dir.display());
            }
            Err(e) => {
                eprintln!("Warning: output capture failed: {}", e);
            }
        }
    }

    // Soft validation nudge: if no log entry mentions validation, print a tip.
    if let Some(task) = graph.get_task(id) {
        let has_validation = task
            .log
            .iter()
            .any(|entry| entry.message.to_lowercase().contains("validat"));
        if !has_validation {
            eprintln!(
                "Tip: Log validation steps before wg done (e.g., wg log {} \"Validated: tests pass\")",
                id
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;
    use worksgood::test_helpers::{make_task_with_status as make_task, setup_workgraph};

    #[test]
    fn test_done_open_task_transitions_to_done() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_in_progress_task_transitions_to_done() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::InProgress)],
        );

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_already_done_returns_ok() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Done)]);

        // Should return Ok (idempotent) rather than error
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_done_with_unresolved_blockers_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let blocker = make_task("blocker", "Blocker task", Status::Open);
        let mut blocked = make_task("blocked", "Blocked task", Status::Open);
        blocked.after = vec!["blocker".to_string()];

        setup_workgraph(dir_path, vec![blocker, blocked]);

        let result = run(dir_path, "blocked", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("blocked by"));
        assert!(err.to_string().contains("unresolved"));
    }

    #[test]
    fn test_done_with_resolved_blockers_succeeds() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let blocker = make_task("blocker", "Blocker task", Status::Done);
        let mut blocked = make_task("blocked", "Blocked task", Status::Open);
        blocked.after = vec!["blocker".to_string()];

        setup_workgraph(dir_path, vec![blocker, blocked]);

        let result = run(dir_path, "blocked", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("blocked").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_with_failed_blocker_is_rejected() {
        // Manual/worker completion is not a dependency waiver.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let blocker = make_task("blocker", "Failed blocker", Status::Failed);
        let mut blocked = make_task("blocked", "Blocked task", Status::Open);
        blocked.after = vec!["blocker".to_string()];

        setup_workgraph(dir_path, vec![blocker, blocked]);

        let result = run(dir_path, "blocked", false, false, false, false, false);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("dependency status is failed")
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("blocked").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[test]
    fn test_done_with_abandoned_blocker_is_rejected() {
        // Abandoned is terminal for retention, never successful completion.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let blocker = make_task("blocker", "Abandoned blocker", Status::Abandoned);
        let mut blocked = make_task("blocked", "Blocked task", Status::Open);
        blocked.after = vec!["blocker".to_string()];

        setup_workgraph(dir_path, vec![blocker, blocked]);

        let result = run(dir_path, "blocked", false, false, false, false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("was abandoned"));

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("blocked").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[test]
    fn test_rescue_failed_pending_eval_satellites_do_not_deadlock() {
        // Regression: the three-way satellite deadlock (live 2026-07-14).
        //
        // When an agent crashes mid-task, its parent lands in
        // `FailedPendingEval` (soft-failed, awaiting an eval verdict). The
        // rescue/eval pipeline is the satellite scaffold:
        //     X --> .flip-X --> .evaluate-X
        // Both `.flip-X` and `.evaluate-X` depend on `X`, but they ARE the
        // mechanism that resolves `X`. Before the fix, `wg done .flip-X` was
        // refused ("blocked by X: FailedPendingEval") because the system-
        // dependent bypass only exempted `PendingEval`, not
        // `FailedPendingEval`. That deadlocked three ways:
        //   1. `.flip-X` can't done   (blocked by X)
        //   2. `.evaluate-X` can't done (blocked by `.flip-X` still Open)
        //   3. X can't resolve         (waiting on `.evaluate-X`'s verdict)
        // Only manual coordinator surgery (abandon flips, respawn evals)
        // cleared it. This test asserts the rescue completes with no surgery.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let parent = make_task("X", "Crashed task", Status::FailedPendingEval);
        let mut flip = make_task(".flip-X", "FLIP: X", Status::Open);
        flip.after = vec!["X".to_string()];
        let mut eval = make_task(".evaluate-X", "Evaluate: X", Status::Open);
        eval.after = vec![".flip-X".to_string()];

        setup_workgraph(dir_path, vec![parent, flip, eval]);

        // Step 1: `.flip-X` must be able to complete even though its parent is
        // FailedPendingEval — the rescue edge must never gate on the thing
        // being rescued.
        let flip_res = run(dir_path, ".flip-X", false, false, false, false, false);
        assert!(
            flip_res.is_ok(),
            "`.flip-X` must complete over a FailedPendingEval parent (rescue \
             path), got: {:?}",
            flip_res.err()
        );
        {
            let graph = load_graph(&graph_path(dir_path)).unwrap();
            assert_eq!(graph.get_task(".flip-X").unwrap().status, Status::Done);
        }

        // Step 2: with `.flip-X` Done, `.evaluate-X` is no longer blocked and
        // can produce the verdict that resolves `X`.
        let eval_res = run(dir_path, ".evaluate-X", false, false, false, false, false);
        assert!(
            eval_res.is_ok(),
            "`.evaluate-X` must complete once `.flip-X` is Done, got: {:?}",
            eval_res.err()
        );
        {
            let graph = load_graph(&graph_path(dir_path)).unwrap();
            assert_eq!(graph.get_task(".evaluate-X").unwrap().status, Status::Done);
        }
    }

    #[test]
    fn test_rescue_bypass_does_not_leak_to_regular_dependents() {
        // The FailedPendingEval bypass is ONLY for system dependents (the
        // rescue/eval pipeline). Normal-path scoring must stay intact: a
        // regular downstream task must still be blocked by a FailedPendingEval
        // parent — proceeding would run real work against a crashed/broken
        // upstream artifact.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let parent = make_task("X", "Crashed task", Status::FailedPendingEval);
        let mut child = make_task("regular-child", "Regular downstream", Status::Open);
        child.after = vec!["X".to_string()];

        setup_workgraph(dir_path, vec![parent, child]);

        let res = run(dir_path, "regular-child", false, false, false, false, false);
        assert!(
            res.is_err(),
            "a regular (non-system) dependent must stay blocked by a \
             FailedPendingEval parent"
        );
        let err = res.unwrap_err().to_string();
        assert!(err.contains("blocked by"), "unexpected error: {}", err);
    }

    #[test]
    fn test_done_verified_task_succeeds() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Verified task", Status::InProgress);
        task.verify = Some("true".to_string());

        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_sets_completed_at_timestamp() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let before = Utc::now();
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(task.completed_at.is_some());

        // Parse the timestamp and verify it's recent
        let completed_at: chrono::DateTime<Utc> =
            task.completed_at.as_ref().unwrap().parse().unwrap();
        assert!(completed_at >= before);
    }

    #[test]
    fn test_done_creates_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();

        assert!(!task.log.is_empty());
        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.message, "Task marked as done");
        assert_eq!(last_log.actor, Some("agent-1".to_string()));
    }

    #[test]
    fn test_done_nonexistent_task_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![]);

        let result = run(dir_path, "nonexistent", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_done_uninitialized_workgraph_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // Don't initialize WG

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn test_done_log_entry_without_assigned_has_none_actor() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();

        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.actor, None);
    }

    #[test]
    fn test_done_converged_log_message() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();

        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.message, "Task marked as done (converged)");
    }

    #[test]
    fn test_done_converged_ignored_when_cycle_guard_set_on_self() {
        // When the task itself has a cycle guard, --converged should be ignored.
        // The guard is authoritative — the agent cannot self-converge.
        use worksgood::graph::{CycleConfig, LoopGuard};

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut header = make_task("header", "Cycle header", Status::Open);
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: Some(LoopGuard::TaskStatus {
                task: "validator".to_string(),
                status: Status::Failed,
            }),
            delay: None,
            no_converge: false,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        setup_workgraph(dir_path, vec![header]);

        let result = run(dir_path, "header", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("header").unwrap();

        // Converged tag should NOT be present
        assert!(
            !task.tags.contains(&"converged".to_string()),
            "converged tag should not be added when cycle guard is set"
        );

        // Log should reflect that --converged was ignored
        let last_log = task.log.last().unwrap();
        assert_eq!(
            last_log.message,
            "Task marked as done (--converged ignored, cycle is forced)"
        );
    }

    #[test]
    fn test_done_converged_ignored_for_non_header_in_guarded_cycle() {
        // When a task is a non-header member of a cycle whose header has a guard,
        // --converged should also be ignored.
        use worksgood::graph::{CycleConfig, LoopGuard};

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // Create cycle: header ↔ worker (both depend on each other)
        let mut header = make_task("header", "Cycle header", Status::Done);
        header.after = vec!["worker".to_string()];
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: Some(LoopGuard::TaskStatus {
                task: "validator".to_string(),
                status: Status::Failed,
            }),
            delay: None,
            no_converge: false,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        let mut worker = make_task("worker", "Worker in cycle", Status::Open);
        worker.after = vec!["header".to_string()];

        setup_workgraph(dir_path, vec![header, worker]);

        let result = run(dir_path, "worker", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("worker").unwrap();

        // Converged tag should NOT be present
        assert!(
            !task.tags.contains(&"converged".to_string()),
            "converged tag should not be added for non-header in guarded cycle"
        );

        // Log should reflect that --converged was ignored
        let last_log = task.log.last().unwrap();
        assert_eq!(
            last_log.message,
            "Task marked as done (--converged ignored, cycle is forced)"
        );
    }

    #[test]
    fn test_done_converged_accepted_when_guard_is_always() {
        // When cycle_config has guard = Always (trivial), --converged should work.
        use worksgood::graph::{CycleConfig, LoopGuard};

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut header = make_task("header", "Cycle header", Status::Open);
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: Some(LoopGuard::Always),
            delay: None,
            no_converge: false,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        setup_workgraph(dir_path, vec![header]);

        let result = run(dir_path, "header", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("header").unwrap();

        // Converged tag SHOULD be present (Always guard is trivial)
        assert!(
            task.tags.contains(&"converged".to_string()),
            "converged tag should be added when guard is Always"
        );

        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.message, "Task marked as done (converged)");
    }

    #[test]
    fn test_done_converged_accepted_when_no_guard() {
        // When cycle_config has no guard, --converged should work.
        use worksgood::graph::CycleConfig;

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut header = make_task("header", "Cycle header", Status::Open);
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: None,
            delay: None,
            no_converge: false,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        setup_workgraph(dir_path, vec![header]);

        let result = run(dir_path, "header", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("header").unwrap();

        // Converged tag SHOULD be present
        assert!(
            task.tags.contains(&"converged".to_string()),
            "converged tag should be added when no guard is set"
        );
    }

    #[test]
    fn test_done_without_validation_log_still_succeeds() {
        // The soft validation tip should never block completion.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);

        // No log entry contains "validat" — the tip would fire, but must not block
        let has_validation = task
            .log
            .iter()
            .any(|e| e.message.to_lowercase().contains("validat"));
        assert!(!has_validation);
    }

    #[test]
    fn test_done_with_validation_log_suppresses_tip() {
        // When a log entry contains a validation mention, no tip should fire.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Test task", Status::Open);
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: None,
            message: "Validated: all tests pass".to_string(),
        });
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);

        // Log contains "Validated" — tip should be suppressed
        let has_validation = task
            .log
            .iter()
            .any(|e| e.message.to_lowercase().contains("validat"));
        assert!(has_validation);
    }

    #[test]
    fn test_done_converged_ignored_when_no_converge_set_on_self() {
        // When the task itself has no_converge, --converged should be ignored.
        use worksgood::graph::CycleConfig;

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut header = make_task("header", "Forced cycle header", Status::Open);
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: None,
            delay: None,
            no_converge: true,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        setup_workgraph(dir_path, vec![header]);

        let result = run(dir_path, "header", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("header").unwrap();

        // Converged tag should NOT be present
        assert!(
            !task.tags.contains(&"converged".to_string()),
            "converged tag should not be added when no_converge is set"
        );

        // Log should contain the forced-ignore message (may not be last due to reactivation)
        let has_forced_msg = task
            .log
            .iter()
            .any(|e| e.message == "Task marked as done (--converged ignored, cycle is forced)");
        assert!(
            has_forced_msg,
            "Log should contain forced-ignore message, got: {:?}",
            task.log.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_done_converged_ignored_for_non_header_in_no_converge_cycle() {
        // When a task is a non-header member of a cycle with no_converge,
        // --converged should also be ignored.
        use worksgood::graph::CycleConfig;

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut header = make_task("header", "Forced cycle header", Status::Done);
        header.after = vec!["worker".to_string()];
        header.cycle_config = Some(CycleConfig {
            max_iterations: 5,
            guard: None,
            delay: None,
            no_converge: true,
            restart_on_failure: true,
            max_failure_restarts: None,
        });

        let mut worker = make_task("worker", "Worker in forced cycle", Status::Open);
        worker.after = vec!["header".to_string()];

        setup_workgraph(dir_path, vec![header, worker]);

        let result = run(dir_path, "worker", true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("worker").unwrap();

        // Converged tag should NOT be present
        assert!(
            !task.tags.contains(&"converged".to_string()),
            "converged tag should not be added for non-header in no-converge cycle"
        );

        // Log should contain the forced-ignore message (may not be last due to reactivation)
        let has_forced_msg = task
            .log
            .iter()
            .any(|e| e.message == "Task marked as done (--converged ignored, cycle is forced)");
        assert!(
            has_forced_msg,
            "Log should contain forced-ignore message, got: {:?}",
            task.log.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_done_verify_passing_allows_transition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with passing verify", Status::InProgress);
        task.verify = Some("exit 0".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_verify_failing_blocks_transition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Verify command failed"), "got: {}", err);
        assert!(err.contains("exit 1"), "got: {}", err);

        // Task should still be in-progress
        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
    }

    #[test]
    fn test_done_verify_failing_includes_output() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("echo 'test failed: expected 42 got 0' >&2; exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("test failed: expected 42 got 0"),
            "error should include command output, got: {}",
            err
        );
    }

    #[test]
    fn test_done_skip_verify_bypasses_gate() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Use run_inner with is_agent=false to simulate human usage
        let result = super::run_inner(dir_path, "t1", false, true, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_skip_verify_blocked_for_agents() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Use run_inner with is_agent=true to simulate agent context
        let result = super::run_inner(dir_path, "t1", false, true, false, true, false, false);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Agents cannot use --skip-verify"),
            "got: {}",
            err
        );

        // Task should not have transitioned
        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
    }

    #[test]
    fn test_done_no_verify_field_works_as_before() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let task = make_task("t1", "Task without verify", Status::InProgress);
        assert!(task.verify.is_none());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_converged_also_runs_verify() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", true, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Verify command failed"), "got: {}", err);

        // Task should still be in-progress
        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
    }

    #[test]
    fn test_done_external_validation_satellite_is_retired() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "External validation task", Status::InProgress);
        task.validation = Some("external".to_string());
        setup_workgraph(dir_path, vec![task]);

        let error = run(dir_path, "t1", false, false, false, false, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("retired reviewer-task authority")
        );

        let graph = load_graph(&graph_path(dir_path)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task.completed_at.is_none());
    }

    #[test]
    fn test_done_llm_validation_satellite_is_retired() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "LLM gate task", Status::InProgress);
        task.validation = Some("llm".to_string());
        setup_workgraph(dir_path, vec![task]);

        let error = run(dir_path, "t1", false, false, false, false, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("retired reviewer-task authority")
        );

        let graph = load_graph(&graph_path(dir_path)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task.completed_at.is_none());
    }

    #[test]
    fn test_done_integrated_validation_requires_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Integrated validation task", Status::InProgress);
        task.validation = Some("integrated".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Should fail: no validation log entry
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("validation log entry"));
    }

    #[test]
    fn test_done_integrated_validation_with_log_succeeds() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Integrated validation task", Status::InProgress);
        task.validation = Some("integrated".to_string());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: None,
            message: "Validated: all tests pass".to_string(),
        });
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_integrated_validation_runs_commands() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Integrated with commands", Status::InProgress);
        task.validation = Some("integrated".to_string());
        task.validation_commands = vec!["exit 1".to_string()]; // will fail
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: None,
            message: "Validated: ready".to_string(),
        });
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("validation failed"));
    }

    #[test]
    fn test_done_none_validation_is_default() {
        // validation=None (default) should behave like "none" — direct to Done
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let task = make_task("t1", "Default task", Status::InProgress);
        assert!(task.validation.is_none());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_updates_agent_registry() {
        // When a task is marked done, the agent registry entry should also
        // transition to Done so the agent slot is freed immediately.
        use worksgood::service::registry::{AgentRegistry, AgentStatus};

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Set up a registry with an agent working on this task
        let mut registry = AgentRegistry::new();
        registry.register_agent(99999, "t1", "claude", "/tmp/output.log");
        registry.save(dir_path).unwrap();

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        // Verify registry was updated
        let registry = AgentRegistry::load(dir_path).unwrap();
        let agent = registry.get_agent("agent-1").unwrap();
        assert_eq!(
            agent.status,
            AgentStatus::Done,
            "Agent registry should be updated to Done when task completes"
        );
        assert!(
            agent.completed_at.is_some(),
            "Agent should have a completed_at timestamp"
        );
    }

    #[test]
    fn test_done_verify_pipe_syntax() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with pipe verify", Status::InProgress);
        task.verify = Some("echo hello | grep hello".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(
            result.is_ok(),
            "Pipe in verify command should work: {:?}",
            result.err()
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_verify_pipe_failure_propagates() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing pipe verify", Status::InProgress);
        task.verify = Some("echo hello | grep nonexistent".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err(), "Failing pipe should propagate error");

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
    }

    #[test]
    fn test_verify_circuit_breaker_increments_failures() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("echo 'bad output' >&2; exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // First failure: should increment verify_failures and bail
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.verify_failures, 1);
        assert_eq!(task.status, Status::InProgress);
        // Check that verify failure was logged
        assert!(
            task.log
                .iter()
                .any(|e| e.message.contains("Verify FAILED")
                    && e.actor == Some("verify".to_string())),
            "Expected verify failure log entry, got: {:?}",
            task.log
        );
    }

    #[test]
    fn test_verify_failures_never_become_terminal_authority() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("echo 'FAIL: test not found' >&2; exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        for i in 0..3 {
            let result = run(dir_path, "t1", false, false, false, false, false);
            assert!(result.is_err(), "attempt {} must remain non-terminal", i);
        }

        let graph = load_graph(&graph_path(dir_path)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.verify_failures, 3);
        assert!(task.failure_reason.is_none());
        assert!(
            !task
                .log
                .iter()
                .any(|e| e.actor == Some("verify-circuit-breaker".to_string()))
        );
    }

    #[test]
    fn test_verify_success_resets_failure_count() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // Start with a task that already has some verify failures
        let mut task = make_task("t1", "Task with verify", Status::InProgress);
        task.verify = Some("exit 0".to_string());
        task.verify_failures = 2; // previous failures
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(
            task.verify_failures, 0,
            "verify_failures should be reset on success"
        );
    }

    #[test]
    fn test_verify_failure_logs_stdout_and_stderr() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with verbose verify", Status::InProgress);
        task.verify = Some("echo 'stdout line' && echo 'stderr line' >&2 && exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();

        // Verify log entry includes both stdout and stderr
        let verify_log = task
            .log
            .iter()
            .find(|e| e.message.contains("Verify FAILED"))
            .expect("should have verify failure log");
        assert!(
            verify_log.message.contains("stdout line"),
            "log should contain stdout, got: {}",
            verify_log.message
        );
        assert!(
            verify_log.message.contains("stderr line"),
            "log should contain stderr, got: {}",
            verify_log.message
        );
    }

    #[test]
    fn test_verify_circuit_breaker_distinguishes_from_agent_failures() {
        // Verify failures use the "verify" actor, circuit breaker uses "verify-circuit-breaker"
        // Regular triage/agent failures use "triage" actor
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let _ = run(dir_path, "t1", false, false, false, false, false);

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();

        // All verify-related logs should use "verify" actor
        let verify_logs: Vec<_> = task
            .log
            .iter()
            .filter(|e| e.message.contains("Verify"))
            .collect();
        assert!(!verify_logs.is_empty());
        for log in &verify_logs {
            assert_eq!(
                log.actor,
                Some("verify".to_string()),
                "Verify failure logs should use 'verify' actor, not agent/triage actor"
            );
        }
    }

    #[test]
    fn test_verify_failure_threshold_is_observational_only() {
        // Test that the config controls the threshold.
        // We write a config with max_verify_failures = 2.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with failing verify", Status::InProgress);
        task.verify = Some("exit 1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Write config with lower threshold (dir_path is the .wg dir in tests)
        let config_path = dir_path.join("config.toml");
        std::fs::write(&config_path, "[coordinator]\nmax_verify_failures = 2\n").unwrap();

        // First failure
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());

        // The second failure remains evidence and cannot terminalize the task.
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_err());

        let graph = load_graph(&graph_path(dir_path)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.verify_failures, 2);
    }

    #[test]
    fn test_done_separate_verify_setting_has_no_status_authority() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with verify", Status::InProgress);
        task.verify = Some("true".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Write config with verify_mode = "separate"
        std::fs::write(
            dir_path.join("config.toml"),
            "[coordinator]\nverify_mode = \"separate\"\n",
        )
        .unwrap();

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.completed_at.is_some());
        assert!(
            !task
                .log
                .iter()
                .any(|e| e.message.contains("verify_mode=separate"))
        );
    }

    #[test]
    fn test_done_inline_verify_still_works() {
        // Ensure backward compatibility: verify_mode=inline (default) runs verify inline
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with passing verify", Status::InProgress);
        task.verify = Some("true".to_string()); // always passes
        setup_workgraph(dir_path, vec![task]);

        // No config file = defaults to inline
        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.status,
            Status::Done,
            "inline verify should complete to Done"
        );
    }

    #[test]
    fn test_is_core_file() {
        assert!(is_core_file("src/lib.rs"));
        assert!(is_core_file("src/main.rs"));
        assert!(is_core_file("Cargo.toml"));
        assert!(is_core_file("Cargo.lock"));
        assert!(is_core_file("build.rs"));
        assert!(is_core_file("src/commands/mod.rs"));

        assert!(!is_core_file("src/commands/add.rs"));
        assert!(!is_core_file("src/graph.rs"));
        assert!(!is_core_file("tests/test_integration.rs"));
    }

    #[test]
    fn test_map_file_to_test_command() {
        // Test source file mapping
        assert_eq!(
            map_file_to_test_command("src/commands/add.rs"),
            Some("cargo test add".to_string())
        );
        assert_eq!(
            map_file_to_test_command("src/graph.rs"),
            Some("cargo test graph".to_string())
        );

        // Test direct test file mapping
        assert_eq!(
            map_file_to_test_command("tests/integration_multi_user.rs"),
            Some("cargo test --test integration_multi_user".to_string())
        );

        // Test non-mappable files
        assert_eq!(map_file_to_test_command("README.md"), None);
        assert_eq!(map_file_to_test_command("docs/guide.md"), None);
    }

    #[test]
    fn test_map_files_to_tests() {
        // Regular source files should map to scoped tests
        let files = vec!["src/commands/add.rs".to_string()];
        let result = map_files_to_tests(&files);
        assert_eq!(result, Some(vec!["cargo test add".to_string()]));

        // Core files should return None (fall back to full suite)
        let files = vec!["src/lib.rs".to_string()];
        let result = map_files_to_tests(&files);
        assert_eq!(result, None);

        // Multiple files should combine commands
        let files = vec![
            "src/commands/add.rs".to_string(),
            "src/graph.rs".to_string(),
        ];
        let result = map_files_to_tests(&files);
        assert_eq!(
            result,
            Some(vec![
                "cargo test add".to_string(),
                "cargo test graph".to_string()
            ])
        );

        // Empty files list should return None
        let files: Vec<String> = vec![];
        let result = map_files_to_tests(&files);
        assert_eq!(result, None);
    }

    // Smart verify detection tests

    #[test]
    fn test_is_free_text_verify_command_detects_descriptive_text() {
        assert!(is_free_text_verify_command(
            "documentation exists and is comprehensive"
        ));
        assert!(is_free_text_verify_command("tests pass for all modules"));
        assert!(is_free_text_verify_command("build succeeds without errors"));
        assert!(is_free_text_verify_command("code has been implemented"));
        assert!(is_free_text_verify_command("feature works correctly"));
        assert!(is_free_text_verify_command("ensure the module compiles"));
    }

    #[test]
    fn test_is_free_text_verify_command_allows_valid_commands() {
        assert!(!is_free_text_verify_command("cargo test"));
        assert!(!is_free_text_verify_command("npm test"));
        assert!(!is_free_text_verify_command("make build"));
        assert!(!is_free_text_verify_command("python -m pytest"));
        assert!(!is_free_text_verify_command("go test ./..."));
        assert!(!is_free_text_verify_command("true"));
        assert!(!is_free_text_verify_command("exit 0"));
    }

    #[test]
    fn test_is_free_text_verify_command_allows_shell_constructs() {
        assert!(!is_free_text_verify_command(
            "cargo test | grep -q 'test result: ok'"
        ));
        assert!(!is_free_text_verify_command("make build && echo 'success'"));
        assert!(!is_free_text_verify_command("test -f output.txt"));
        assert!(!is_free_text_verify_command("echo 'hello' > /tmp/test"));
        assert!(!is_free_text_verify_command("[ -d src ]"));
    }

    #[test]
    fn test_is_free_text_verify_command_edge_cases() {
        assert!(!is_free_text_verify_command(""));
        assert!(!is_free_text_verify_command("cargo"));
        assert!(!is_free_text_verify_command("   cargo test   "));
        assert!(is_free_text_verify_command(
            "unknown_command does something"
        ));
        assert!(is_free_text_verify_command(
            "this should be detected as free text"
        ));
    }

    #[test]
    fn free_text_verify_fails_deliberately_without_evaluator_authority() {
        let dir = tempdir().unwrap();
        let task = make_task("t1", "Task with free-text verify", Status::InProgress);
        let error = match run_verify_command(
            "documentation exists and is comprehensive",
            dir.path(),
            &task,
            &CoordinatorConfig::default(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("free-text verify unexpectedly acquired evaluator authority"),
        };
        assert_eq!(error.exit_code, "retired-free-text-verify");
        assert!(
            error
                .stderr
                .contains("free-text verify compatibility is retired")
        );
    }

    #[test]
    fn test_term_dumb_environment_set_for_shell_commands() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Task with shell verify", Status::InProgress);
        // Use a command that checks the TERM environment variable
        task.verify = Some("test \"$TERM\" = \"dumb\"".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);

        // The command should succeed, indicating TERM=dumb was set
        assert!(result.is_ok(), "TERM=dumb should be set for shell commands");

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_done_defers_verify_when_task_has_children() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // This test covers the deprecated .verify-deferred-* autospawn path.
        // Default is OFF as of 2026-04-17; the test enables it explicitly to
        // continue exercising the code path for the users who still opt in.
        std::fs::write(
            dir_path.join("config.toml"),
            "[coordinator]\nverify_autospawn_enabled = true\n",
        )
        .unwrap();

        // Parent task with a verify command that would fail (children haven't done work yet)
        let mut parent = make_task("parent", "Parent task", Status::InProgress);
        parent.verify = Some("exit 1".to_string()); // would fail normally
        parent.before = vec!["child-a".to_string(), "child-b".to_string()];

        // Children depend on parent
        let mut child_a = make_task("child-a", "Child A", Status::Open);
        child_a.after = vec!["parent".to_string()];

        let mut child_b = make_task("child-b", "Child B", Status::Open);
        child_b.after = vec!["parent".to_string()];

        setup_workgraph(dir_path, vec![parent, child_a, child_b]);

        // Parent's `wg done` should succeed because verify is deferred
        let result = run(dir_path, "parent", false, false, false, false, false);
        assert!(
            result.is_ok(),
            "Parent with children should defer verify, got: {:?}",
            result,
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();

        // Parent should be Done with verify cleared
        let parent = graph.get_task("parent").unwrap();
        assert_eq!(parent.status, Status::Done);
        assert!(
            parent.verify.is_none(),
            "Parent verify should be cleared after deferral"
        );

        // Deferred verify task should exist
        let deferred = graph.get_task(".verify-deferred-parent").unwrap();
        assert_eq!(deferred.status, Status::Open);
        assert_eq!(
            deferred.verify,
            Some("exit 1".to_string()),
            "Deferred task should inherit parent's verify command"
        );
        assert!(
            deferred.after.contains(&"child-a".to_string()),
            "Deferred task should depend on child-a"
        );
        assert!(
            deferred.after.contains(&"child-b".to_string()),
            "Deferred task should depend on child-b"
        );

        // Parent log should mention deferral
        let has_defer_log = parent
            .log
            .iter()
            .any(|e| e.message.contains("Verify deferred"));
        assert!(
            has_defer_log,
            "Parent log should mention verify deferral, got: {:?}",
            parent.log.iter().map(|e| &e.message).collect::<Vec<_>>()
        );

        // Children should have .verify-deferred-parent in their before list
        let child_a = graph.get_task("child-a").unwrap();
        assert!(
            child_a
                .before
                .contains(&".verify-deferred-parent".to_string()),
            "child-a.before should include deferred task"
        );
    }

    #[test]
    fn test_done_does_not_defer_verify_when_no_children() {
        // Verify still runs inline when there are no children
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Solo task", Status::InProgress);
        task.verify = Some("exit 0".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        // No deferred task should exist
        assert!(graph.get_task(".verify-deferred-t1").is_none());
    }

    #[test]
    fn test_done_does_not_defer_verify_for_system_children_only() {
        // System tasks (dot-prefixed) like .flip-*, .evaluate-* should not trigger deferral
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut parent = make_task("parent", "Parent", Status::InProgress);
        parent.verify = Some("exit 0".to_string());
        parent.before = vec![".flip-parent".to_string(), ".evaluate-parent".to_string()];

        let mut flip = make_task(".flip-parent", "FLIP", Status::Open);
        flip.after = vec!["parent".to_string()];

        let mut eval = make_task(".evaluate-parent", "Evaluate", Status::Open);
        eval.after = vec!["parent".to_string()];

        setup_workgraph(dir_path, vec![parent, flip, eval]);

        // Should run verify inline since only system children exist
        let result = run(dir_path, "parent", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        assert!(
            graph.get_task(".verify-deferred-parent").is_none(),
            "No deferred task for system-only children"
        );
    }

    #[test]
    fn test_done_deferred_verify_preserves_timeout() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // Deprecated-feature test (see test_done_defers_verify_when_task_has_children).
        // Enable the autospawn explicitly to exercise the path.
        std::fs::write(
            dir_path.join("config.toml"),
            "[coordinator]\nverify_autospawn_enabled = true\n",
        )
        .unwrap();

        let mut parent = make_task("parent", "Parent", Status::InProgress);
        parent.verify = Some("exit 1".to_string());
        parent.verify_timeout = Some("30m".to_string());
        parent.before = vec!["child".to_string()];

        let mut child = make_task("child", "Child", Status::Open);
        child.after = vec!["parent".to_string()];

        setup_workgraph(dir_path, vec![parent, child]);

        let result = run(dir_path, "parent", false, false, false, false, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let deferred = graph.get_task(".verify-deferred-parent").unwrap();
        assert_eq!(
            deferred.verify_timeout,
            Some("30m".to_string()),
            "Deferred task should inherit verify timeout"
        );
    }

    // ======================================================================
    // Push-on-merge tests (closes the gap from
    // docs/audit-unmerged-branches-2026-04-26.md). The fixtures here use a
    // local bare repo as `origin` so we can verify both:
    //   1. `git push origin main` advances `origin/main`, and
    //   2. `git push origin :refs/heads/<branch>` removes the agent branch
    //      from `origin`.
    // No network is required.
    // ======================================================================

    use std::path::PathBuf;
    use std::process::Command;

    /// Run a git command and assert success.
    fn git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {}", args, e));
        assert!(
            out.status.success(),
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Run a git command, return `(success, stdout)`.
    fn git_capture(cwd: &Path, args: &[&str]) -> (bool, String) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {}", args, e));
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
        )
    }

    /// Build a project repo whose `origin` remote is a local bare repo.
    /// Returns `(remote_bare_dir, project_root)`. Both are owned by the
    /// caller (tempdirs).
    ///
    /// The project repo has:
    ///   - `main` branch with one commit
    ///   - an agent branch `wg/agent-test/<task>` checked out with one new
    ///     commit on top of main (this is what the merge-back will squash)
    ///   - `main` checked out at the end (squash-merge requires being on main)
    fn make_repo_with_remote(
        task_id: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, PathBuf, String) {
        let remote = tempdir().unwrap();
        let project = tempdir().unwrap();
        let project_path = project.path().to_path_buf();

        // 1. Bare remote.
        git(remote.path(), &["init", "--bare", "-b", "main"]);

        // 2. Project repo on `main`.
        git(&project_path, &["init", "-b", "main"]);
        git(&project_path, &["config", "user.email", "test@test.com"]);
        git(&project_path, &["config", "user.name", "Test"]);
        std::fs::write(project_path.join("README.md"), "initial\n").unwrap();
        git(&project_path, &["add", "README.md"]);
        git(&project_path, &["commit", "-m", "initial"]);
        git(
            &project_path,
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        git(&project_path, &["push", "-u", "origin", "main"]);

        // 3. Agent branch with a commit on top of main.
        let branch = format!("wg/agent-test/{}", task_id);
        git(&project_path, &["checkout", "-b", &branch]);
        std::fs::write(project_path.join("agent_work.txt"), "agent change\n").unwrap();
        git(&project_path, &["add", "agent_work.txt"]);
        git(&project_path, &["commit", "-m", "agent work"]);
        git(&project_path, &["push", "-u", "origin", &branch]);

        // 4. Back to main so attempt_worktree_merge can squash-merge into it.
        git(&project_path, &["checkout", "main"]);

        (remote, project, project_path, branch)
    }

    fn make_wt(project_path: &Path, branch: &str) -> WorktreeInfo {
        WorktreeInfo {
            // attempt_worktree_merge only checks `<worktree_path>/.git` exists
            // when deciding whether to skip; we just point it at the project
            // root, which has a real `.git` dir, since these tests don't use
            // a separate worktree directory.
            worktree_path: project_path.to_str().unwrap().to_string(),
            branch: branch.to_string(),
            project_root: project_path.to_str().unwrap().to_string(),
            agent_id: Some("agent-test".to_string()),
            task_id: None,
        }
    }

    fn make_attribution_repo() -> (tempfile::TempDir, PathBuf, String) {
        let project = tempdir().unwrap();
        let project_path = project.path().to_path_buf();
        git(&project_path, &["init", "-b", "main"]);
        git(
            &project_path,
            &["config", "user.email", "merge@example.com"],
        );
        git(&project_path, &["config", "user.name", "Merge User"]);
        git(&project_path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(project_path.join("README.md"), "initial\n").unwrap();
        git(&project_path, &["add", "README.md"]);
        git(&project_path, &["commit", "-m", "initial"]);
        let branch = "wg/agent-test/attribution".to_string();
        git(&project_path, &["checkout", "-b", &branch]);
        (project, project_path, branch)
    }

    #[test]
    fn test_squash_merge_preserves_source_authors_and_coauthor_trailers() {
        let (_project, project_path, branch) = make_attribution_repo();

        std::fs::write(project_path.join("luca.txt"), "baseline\n").unwrap();
        git(&project_path, &["add", "luca.txt"]);
        git(
            &project_path,
            &[
                "commit",
                "--author",
                "Luca Pinello <lucapinello@gmail.com>",
                "-m",
                "source baseline\n\nMetadata containing record controls: \u{1e}\u{1f}\n\nCo-authored-by: Claude Opus 4.8 <noreply@anthropic.com>",
            ],
        );

        std::fs::write(project_path.join("integration.txt"), "integration\n").unwrap();
        git(&project_path, &["add", "integration.txt"]);
        git(
            &project_path,
            &[
                "commit",
                "--author",
                "Erik Integrator <erik@example.com>",
                "-m",
                "integrate baseline\n\nCo-authored-by: Luca Pinello <lucapinello@gmail.com>",
            ],
        );
        git(&project_path, &["checkout", "main"]);

        let result = attempt_worktree_merge(&make_wt(&project_path, &branch), "attribution")
            .expect("squash merge should succeed");
        assert!(matches!(result, WorktreeMergeResult::Merged { .. }));

        let (ok, commit) = git_capture(
            &project_path,
            &["show", "-s", "--format=%an <%ae>%n%B", "HEAD"],
        );
        assert!(ok);
        assert!(
            commit.starts_with("Luca Pinello <lucapinello@gmail.com>\n"),
            "oldest source author must remain the squash author: {commit}"
        );
        assert_eq!(
            commit
                .matches("Co-authored-by: Erik Integrator <erik@example.com>")
                .count(),
            1,
            "additional source authors must become one trailer: {commit}"
        );
        assert_eq!(
            commit
                .matches("Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>")
                .count(),
            1,
            "existing source trailers must survive once: {commit}"
        );
        assert!(
            !commit.contains("Co-authored-by: Luca Pinello"),
            "the primary author must not be duplicated as a coauthor: {commit}"
        );
        let (ok, parsed_trailers) = git_capture(
            &project_path,
            &[
                "show",
                "-s",
                "--format=%(trailers:key=Co-authored-by,valueonly)",
                "HEAD",
            ],
        );
        assert!(ok);
        assert!(parsed_trailers.contains("Erik Integrator <erik@example.com>"));
        assert!(parsed_trailers.contains("Claude Opus 4.8 <noreply@anthropic.com>"));
    }

    #[test]
    fn test_squash_merge_preserves_coauthors_from_single_source_commit() {
        let (_project, project_path, branch) = make_attribution_repo();

        std::fs::write(project_path.join("provider.txt"), "provider\n").unwrap();
        git(&project_path, &["add", "provider.txt"]);
        git(
            &project_path,
            &[
                "commit",
                "--author",
                "Erik Integrator <erik@example.com>",
                "-m",
                "provider fix\n\nCo-authored-by: Luca Pinello <lucapinello@gmail.com>\n\nCo-authored-by: Claude Opus 4.8 <noreply@anthropic.com>",
            ],
        );
        git(&project_path, &["checkout", "main"]);

        let result = attempt_worktree_merge(&make_wt(&project_path, &branch), "provider")
            .expect("squash merge should succeed");
        assert!(matches!(result, WorktreeMergeResult::Merged { .. }));

        let (ok, commit) = git_capture(
            &project_path,
            &["show", "-s", "--format=%an <%ae>%n%B", "HEAD"],
        );
        assert!(ok);
        assert!(commit.starts_with("Erik Integrator <erik@example.com>\n"));
        assert_eq!(
            commit
                .matches("Co-authored-by: Luca Pinello <lucapinello@gmail.com>")
                .count(),
            1,
            "source coauthor must survive the squash: {commit}"
        );
        assert_eq!(
            commit
                .matches("Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>")
                .count(),
            1,
            "all source coauthors must survive the squash: {commit}"
        );
    }

    #[test]
    fn test_done_pushes_main_and_deletes_branch_on_clean_merge() {
        let (_remote, _project, project_path, branch) = make_repo_with_remote("clean-merge");
        let wt = make_wt(&project_path, &branch);

        // Capture origin/main before the merge.
        let (_, before_sha) = git_capture(&project_path, &["rev-parse", "origin/main"]);
        let before_sha = before_sha.trim().to_string();

        // Run the merge path.
        let result = attempt_worktree_merge(&wt, "clean-merge").unwrap();

        match result {
            WorktreeMergeResult::Merged {
                push_outcome,
                commit_sha,
            } => {
                assert!(!commit_sha.is_empty(), "expected a squash commit sha");
                assert_eq!(
                    push_outcome,
                    PushOutcome::PushedAndDeleted,
                    "expected clean push + branch delete on origin"
                );
            }
            other => panic!("expected Merged, got {:?}", other),
        }

        // origin/main must have advanced.
        let (_, after_sha) = git_capture(&project_path, &["rev-parse", "origin/main"]);
        let after_sha = after_sha.trim().to_string();
        assert_ne!(
            before_sha, after_sha,
            "origin/main should advance to the squash commit"
        );

        // Local main HEAD == origin/main.
        let (_, local_main) = git_capture(&project_path, &["rev-parse", "main"]);
        assert_eq!(
            local_main.trim(),
            after_sha,
            "local main and origin/main should match after push"
        );

        // The agent branch must be gone on origin.
        let (_, refs) = git_capture(&project_path, &["ls-remote", "origin"]);
        assert!(
            !refs.contains(&format!("refs/heads/{}", branch)),
            "agent branch should be deleted on origin; ls-remote = {}",
            refs
        );
    }

    #[test]
    fn test_done_succeeds_when_remote_unavailable() {
        let (remote, _project, project_path, branch) = make_repo_with_remote("remote-unavailable");

        // Point `origin` at an unreachable URL. We do this *after* the
        // initial setup so the project is in a realistic post-spawn state.
        git(
            &project_path,
            &[
                "remote",
                "set-url",
                "origin",
                // file:// scheme + nonexistent path = guaranteed-fail push,
                // no DNS / network dependency.
                "file:///nonexistent/path/to/repo.git",
            ],
        );

        // Sanity: the bare remote we'd otherwise push to still exists, but
        // we no longer have a path to it.
        drop(remote);

        let wt = make_wt(&project_path, &branch);
        let result = attempt_worktree_merge(&wt, "remote-unavailable").unwrap();

        match result {
            WorktreeMergeResult::Merged {
                push_outcome,
                commit_sha,
            } => {
                assert!(!commit_sha.is_empty(), "local merge should still happen");
                match push_outcome {
                    PushOutcome::LocalOnly { push_error } => {
                        assert!(
                            !push_error.is_empty(),
                            "expected a non-empty push error reason"
                        );
                    }
                    other => panic!(
                        "expected LocalOnly when remote unreachable, got {:?}",
                        other
                    ),
                }
            }
            other => panic!("expected Merged, got {:?}", other),
        }

        // Local main must still have the squash commit.
        let (ok, log) = git_capture(&project_path, &["log", "main", "--oneline", "-1"]);
        assert!(ok, "git log on main failed");
        assert!(
            log.contains("remote-unavailable"),
            "squash commit should be on local main; got: {}",
            log
        );
    }

    #[test]
    fn test_done_no_remote_returns_no_remote_outcome() {
        // A repo with no `origin` remote should produce PushOutcome::NoRemote
        // and the merge log line should omit the push suffix.
        let project = tempdir().unwrap();
        let project_path = project.path().to_path_buf();

        git(&project_path, &["init", "-b", "main"]);
        git(&project_path, &["config", "user.email", "test@test.com"]);
        git(&project_path, &["config", "user.name", "Test"]);
        std::fs::write(project_path.join("README.md"), "x\n").unwrap();
        git(&project_path, &["add", "README.md"]);
        git(&project_path, &["commit", "-m", "initial"]);

        let branch = "wg/agent-test/no-remote".to_string();
        git(&project_path, &["checkout", "-b", &branch]);
        std::fs::write(project_path.join("a.txt"), "x\n").unwrap();
        git(&project_path, &["add", "a.txt"]);
        git(&project_path, &["commit", "-m", "agent work"]);
        git(&project_path, &["checkout", "main"]);

        let wt = make_wt(&project_path, &branch);
        let result = attempt_worktree_merge(&wt, "no-remote").unwrap();

        match result {
            WorktreeMergeResult::Merged {
                push_outcome,
                commit_sha,
            } => {
                assert!(!commit_sha.is_empty());
                assert_eq!(push_outcome, PushOutcome::NoRemote);
            }
            other => panic!("expected Merged, got {:?}", other),
        }
    }

    #[test]
    fn test_is_no_changes_to_commit_detects_all_variants() {
        // Clean tree, no untracked files
        assert!(is_no_changes_to_commit(
            "On branch main\nnothing to commit, working tree clean\n",
            "",
        ));
        // Clean tree but untracked files present (the bug scenario from fix-wg-done-2)
        assert!(is_no_changes_to_commit(
            "On branch main\nUntracked files: ...\nnothing added to commit but untracked files present\n",
            "",
        ));
        // Modified tracked files but none staged
        assert!(is_no_changes_to_commit(
            "Changes not staged for commit:\nno changes added to commit (use \"git add\")\n",
            "",
        ));
        // Also detected when message comes through stderr
        assert!(is_no_changes_to_commit(
            "",
            "nothing added to commit but untracked files present\n",
        ));
        // Real failure (e.g., hook rejection) must NOT be misclassified
        assert!(!is_no_changes_to_commit(
            "",
            "error: pre-commit hook failed\n",
        ));
        assert!(!is_no_changes_to_commit("", ""));
    }

    #[test]
    fn test_done_handles_already_merged_branch() {
        // Simulates the fix-wg-done-2 scenario: a branch that has already been
        // squash-merged into main. The second invocation must return NoCommits
        // rather than bail with "git commit failed", even when untracked files
        // are present (which makes git emit "nothing added to commit but
        // untracked files present" instead of "nothing to commit").
        use std::process::Command;

        let dir = tempdir().unwrap();
        let project_root = dir.path();

        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(project_root)
                .output()
                .expect("git command failed to spawn");
            assert!(
                out.status.success(),
                "git {:?} failed: stdout={} stderr={}",
                args,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
        };

        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "commit.gpgsign", "false"]);

        std::fs::write(project_root.join("README.md"), "init\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-m", "init"]);

        git(&["checkout", "-b", "feature"]);
        std::fs::write(project_root.join("file.txt"), "feature change\n").unwrap();
        git(&["add", "file.txt"]);
        git(&["commit", "-m", "feature change"]);

        git(&["checkout", "main"]);

        // Untracked file mimics stray .wg.* / .wg/ dirs that trigger the
        // "nothing added to commit but untracked files present" wording.
        std::fs::write(project_root.join(".wg.junk"), "junk\n").unwrap();

        let wt = WorktreeInfo {
            worktree_path: project_root.to_string_lossy().to_string(),
            branch: "feature".to_string(),
            project_root: project_root.to_string_lossy().to_string(),
            agent_id: Some("test-agent".to_string()),
            task_id: None,
        };

        let r1 = attempt_worktree_merge(&wt, "test-task").expect("first merge call must succeed");
        assert!(
            matches!(r1, WorktreeMergeResult::Merged { .. }),
            "first call should produce Merged, got {:?}",
            r1,
        );

        let r2 = attempt_worktree_merge(&wt, "test-task")
            .expect("second merge call must succeed (no bail)");
        assert!(
            matches!(r2, WorktreeMergeResult::NoCommits),
            "second call on an already-merged branch should return NoCommits, got {:?}",
            r2,
        );
    }

    // ------------------------------------------------------------------
    // chat-agent-loops bug B: git hygiene must skip chat-loop tasks and
    // filter WG-internal paths from the warning.
    // ------------------------------------------------------------------

    #[test]
    fn test_is_hygiene_ignored_path_recognises_workgraph_dirs() {
        assert!(is_hygiene_ignored_path(".wg/"));
        assert!(is_hygiene_ignored_path(".wg"));
        assert!(is_hygiene_ignored_path(".wg/lockfile"));
        assert!(is_hygiene_ignored_path(".wg-worktrees/"));
        assert!(is_hygiene_ignored_path(".wg-worktrees/agent-228/foo"));
        // Legacy `.workgraph/` and numbered worktree dirs.
        assert!(is_hygiene_ignored_path(".workgraph/"));
        assert!(is_hygiene_ignored_path(".workgraph.1/"));
        assert!(is_hygiene_ignored_path(".workgraph.42/graph.jsonl"));
        assert!(is_hygiene_ignored_path(".wg.1/"));
        assert!(is_hygiene_ignored_path(".wg.42/graph.jsonl"));
        // Real source paths must NOT be treated as ignored.
        assert!(!is_hygiene_ignored_path("src/foo.rs"));
        assert!(!is_hygiene_ignored_path("README.md"));
        assert!(!is_hygiene_ignored_path(".github/workflows/ci.yml"));
        // Similar names that must not match.
        assert!(!is_hygiene_ignored_path(".workgraph_old/foo"));
        assert!(!is_hygiene_ignored_path(".wgs/foo"));
    }

    #[test]
    fn test_porcelain_path_extracts_path_from_status_line() {
        assert_eq!(porcelain_path("?? .wg/"), Some(".wg/"));
        assert_eq!(porcelain_path("?? .workgraph.1/"), Some(".workgraph.1/"));
        assert_eq!(porcelain_path(" M src/foo.rs"), Some("src/foo.rs"));
        assert_eq!(porcelain_path("A  src/new.rs"), Some("src/new.rs"));
        // Renames: prefer the new path.
        assert_eq!(
            porcelain_path("R  src/old.rs -> src/new.rs"),
            Some("src/new.rs")
        );
        // Quoted path (whitespace) is unwrapped.
        assert_eq!(porcelain_path("?? \"foo bar.txt\""), Some("foo bar.txt"));
        // Garbage lines do not panic.
        assert_eq!(porcelain_path("xx"), None);
    }

    #[test]
    fn test_filter_hygiene_porcelain_drops_workgraph_paths() {
        let raw = "?? .wg/\n?? .workgraph.1/\n M src/foo.rs\n?? README.md\n";
        let kept = filter_hygiene_porcelain(raw);
        assert_eq!(kept, vec![" M src/foo.rs", "?? README.md"]);
    }

    #[test]
    fn test_filter_hygiene_porcelain_keeps_all_when_nothing_ignored() {
        let raw = " M src/foo.rs\nA  newfile.rs\n";
        let kept = filter_hygiene_porcelain(raw);
        assert_eq!(kept, vec![" M src/foo.rs", "A  newfile.rs"]);
    }

    #[test]
    fn test_filter_hygiene_porcelain_returns_empty_when_only_ignored() {
        // The user's repro: only the WG-internal noise is present.
        let raw = "?? .wg/\n?? .wg.1/\n";
        let kept = filter_hygiene_porcelain(raw);
        assert!(
            kept.is_empty(),
            "filter should drop all WG-internal paths, got {:?}",
            kept,
        );
    }

    /// Bug B regression-guard: `wg done` on a chat-loop tagged task must not
    /// run the git hygiene check at all. We can't easily observe the inner
    /// command (it shells out to `git`), but we can prove the gate by passing
    /// a task we know is *not* in any git repo and asserting the helper
    /// returns immediately without trying to invoke git. If the chat-loop
    /// gate is wired correctly the call is a no-op even when run from a
    /// non-git directory, which `tempdir()` provides for free.
    #[test]
    fn test_check_agent_git_hygiene_skips_chat_loop_tag() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // Just make sure the call doesn't panic and produces no output we
        // can assert on (eprintln is harder to capture, but we can at least
        // exercise both branches.)
        check_agent_git_hygiene(
            dir_path,
            ".chat-0",
            &[worksgood::chat_id::CHAT_LOOP_TAG.to_string()],
        );
        // Also accept the legacy form.
        check_agent_git_hygiene(
            dir_path,
            ".coordinator-0",
            &[worksgood::chat_id::LEGACY_COORDINATOR_LOOP_TAG.to_string()],
        );
        // Non-chat tags do not skip — but with no git repo at the parent
        // dir the function silently no-ops, which is fine for this test.
        check_agent_git_hygiene(dir_path, "regular-task", &["other".to_string()]);
    }

    // ---- Deliverable preflight (guardrail G1) ----
    //
    // These tests use a nested layout: `project_root/.wg/graph.jsonl` so
    // that deliverable paths resolve against `project_root` (==
    // `wg_dir.parent()`), matching production where `wg done` runs against
    // the `.wg` dir inside a repo root.
    fn setup_with_project_root(project_root: &Path, tasks: Vec<worksgood::graph::Task>) -> PathBuf {
        let wg_dir = project_root.join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        setup_workgraph(&wg_dir, tasks);
        wg_dir
    }

    fn task_with_desc(id: &str, desc: &str) -> worksgood::graph::Task {
        let mut t = make_task(id, id, Status::InProgress);
        t.description = Some(desc.to_string());
        t
    }

    fn setup_brokered_deliverable_case(
        task_id: &str,
        desc: &str,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let project = tempdir().unwrap();
        let project_root = project.path().to_path_buf();
        git(&project_root, &["init", "-b", "main"]);
        git(
            &project_root,
            &["config", "user.email", "broker@test.invalid"],
        );
        git(&project_root, &["config", "user.name", "Broker Test"]);
        std::fs::write(project_root.join("README.md"), b"root\n").unwrap();
        git(&project_root, &["add", "README.md"]);
        git(&project_root, &["commit", "-m", "root"]);

        let worktrees = project_root.join(".wg-worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        let worktree = worktrees.join("agent-1");
        let branch = format!("wg/agent-1/{task_id}");
        git(
            &project_root,
            &["worktree", "add", "-b", &branch, worktree.to_str().unwrap()],
        );

        let mut task = task_with_desc(task_id, desc);
        task.assigned = Some("agent-1".to_string());
        task.completion_contract = worksgood::graph::CompletionContract::Report;
        task.lifecycle.fence = 1;
        task.lifecycle.current_attempt = Some(worksgood::lifecycle::AttemptRef {
            id: "attempt-0-1".to_string(),
            generation: 0,
            fence: 1,
            actor_id: "agent-1".to_string(),
            disposition: None,
        });
        let wg_dir = setup_with_project_root(&project_root, vec![task]);

        let mut registry = AgentRegistry::new();
        let agent = registry.register_agent(std::process::id(), task_id, "test", "/dev/null");
        assert_eq!(agent, "agent-1");
        assert!(registry.set_worktree_path(&agent, &worktree));
        registry.save(&wg_dir).unwrap();

        (project, wg_dir, worktree)
    }

    #[test]
    fn brokered_done_preflights_retained_worktree_and_enters_task_owned_finish() {
        let desc = "## Deliverables\n- docs/atomic-save.md\n";
        let (_project, wg_dir, worktree) =
            setup_brokered_deliverable_case("brokered-present", desc);
        std::fs::create_dir_all(worktree.join("docs")).unwrap();
        std::fs::write(worktree.join("docs/atomic-save.md"), b"design\n").unwrap();
        git(&worktree, &["add", "docs/atomic-save.md"]);
        git(&worktree, &["commit", "-m", "add brokered deliverable"]);

        let result = run_from_worker_control(
            &wg_dir,
            "brokered-present",
            false,
            false,
            &worktree,
            "agent-1",
        );
        assert!(result.is_ok(), "got: {:#?}", result.err());
        assert!(
            !wg_dir
                .parent()
                .unwrap()
                .join("docs/atomic-save.md")
                .exists(),
            "brokered completion must not copy the deliverable to graph root"
        );
        let tx = worksgood::finalization::FinalizationStore::open(&wg_dir)
            .unwrap()
            .load_task("brokered-present")
            .unwrap()
            .expect("task-owned finish transaction must be created");
        assert_eq!(
            tx.phase,
            worksgood::finalization::FinalizationPhase::Reported,
            "clean preflight must proceed through task-owned report finalization"
        );
    }

    #[test]
    fn brokered_done_refuses_missing_worktree_deliverable_despite_stale_root_copy() {
        let desc = "## Deliverables\n- docs/atomic-save.md\n";
        let (_project, wg_dir, worktree) =
            setup_brokered_deliverable_case("brokered-missing", desc);
        let root_file = wg_dir.parent().unwrap().join("docs/atomic-save.md");
        std::fs::create_dir_all(root_file.parent().unwrap()).unwrap();
        std::fs::write(&root_file, b"stale root copy\n").unwrap();

        let error = run_from_worker_control(
            &wg_dir,
            "brokered-missing",
            false,
            false,
            &worktree,
            "agent-1",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("deliverable preflight refused"),
            "got: {error}"
        );
        assert!(error.contains("docs/atomic-save.md"), "got: {error}");
        assert!(
            worksgood::finalization::FinalizationStore::open(&wg_dir)
                .unwrap()
                .load_task("brokered-missing")
                .unwrap()
                .is_none(),
            "refused preflight must not enter task-owned finalization"
        );
    }

    #[test]
    fn human_done_keeps_graph_root_deliverable_behavior() {
        let project = tempdir().unwrap();
        let project_root = project.path();
        let desc = "## Deliverables\n- docs/human.md\n";
        let wg_dir =
            setup_with_project_root(project_root, vec![task_with_desc("human-root", desc)]);
        std::fs::create_dir_all(project_root.join("docs")).unwrap();
        std::fs::write(project_root.join("docs/human.md"), b"human output\n").unwrap();

        let result = run(&wg_dir, "human-root", false, false, false, false, false);
        assert!(result.is_ok(), "got: {:#?}", result.err());
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        assert_eq!(graph.get_task("human-root").unwrap().status, Status::Done);
    }

    #[test]
    fn brokered_done_fails_closed_on_authenticated_task_agent_mismatch() {
        let desc = "## Deliverables\n- docs/atomic-save.md\n";
        let (_project, wg_dir, worktree) =
            setup_brokered_deliverable_case("brokered-mismatch", desc);
        std::fs::create_dir_all(worktree.join("docs")).unwrap();
        std::fs::write(worktree.join("docs/atomic-save.md"), b"design\n").unwrap();

        let error = run_from_worker_control(
            &wg_dir,
            "brokered-mismatch",
            false,
            false,
            &worktree,
            "agent-other",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("done.worktree_context_mismatch"),
            "got: {error}"
        );
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        assert_eq!(
            graph.get_task("brokered-mismatch").unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    #[serial]
    fn done_refuses_missing_deliverable() {
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let desc = "## Description\nRefresh the e97 seed.\n\n## Deliverables\n- latest.pt\n- seed/manifest.json\n- registry:registry.json:e97\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);

        let wg_dir = project_root.join(".wg");
        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("deliverable preflight refused"), "got: {err}");
        assert!(err.contains("latest.pt"));
        assert!(err.contains("seed/manifest.json"));
        assert!(err.contains("registry:registry.json:e97"));

        // Refusal recorded with class deliverable-missing; status unchanged.
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.failure_class, Some(FailureClass::DeliverableMissing));
        assert!(
            task.failure_reason
                .as_deref()
                .unwrap_or("")
                .contains("latest.pt")
        );
        assert!(
            task.log
                .iter()
                .any(|e| e.actor == Some("deliverable-preflight".to_string()))
        );
    }

    #[test]
    fn done_passes_when_deliverables_present() {
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        std::fs::write(project_root.join("latest.pt"), b"checkpoint").unwrap();
        std::fs::create_dir_all(project_root.join("seed")).unwrap();
        std::fs::write(project_root.join("seed/manifest.json"), b"{}").unwrap();
        std::fs::write(project_root.join("registry.json"), b"{\"e97\": true}").unwrap();

        let desc = "## Description\nRefresh the e97 seed.\n\n## Deliverables\n- latest.pt\n- seed/manifest.json\n- registry:registry.json:e97\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);

        let wg_dir = project_root.join(".wg");
        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_ok(), "got: {:?}", result.err());

        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        // Prior-refusal marker is cleared on success.
        assert_eq!(task.failure_class, None);
        assert_eq!(task.failure_reason, None);
    }

    #[test]
    #[serial]
    fn done_clears_prior_deliverable_missing_marker_on_success() {
        // First refuse (no deliverables), then produce them and re-run.
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let desc = "## Deliverables\n- latest.pt\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);
        let wg_dir = project_root.join(".wg");

        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.failure_class, Some(FailureClass::DeliverableMissing));

        // Now produce the deliverable.
        std::fs::write(project_root.join("latest.pt"), b"ok").unwrap();
        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_ok(), "got: {:?}", result.err());
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.failure_class, None);
    }

    #[test]
    fn done_ignores_tasks_without_deliverables() {
        // No `## Deliverables` block; `## Validation` is a rubric, not a file
        // list. Preflight must be a no-op (no regression for research/review).
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let desc = "## Description\nResearch the design.\n\n## Validation\n- write a structured report\n- cite specific files\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);
        let wg_dir = project_root.join(".wg");

        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_ok(), "got: {:?}", result.err());
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.failure_class, None);
    }

    #[test]
    #[serial]
    fn done_refuses_empty_deliverable_file() {
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        // File exists but is empty.
        std::fs::write(project_root.join("latest.pt"), b"").unwrap();
        let desc = "## Deliverables\n- latest.pt\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);
        let wg_dir = project_root.join(".wg");

        let result = run(&wg_dir, "t1", false, false, false, false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("deliverable preflight refused"));
        assert!(err.contains("latest.pt"));
    }

    #[test]
    #[serial]
    fn done_env_override_cannot_bypass_missing_deliverable() {
        // Regression guard (PR #54, Erik CHANGES_REQUESTED): there is no
        // environment override for the deliverable gate. Setting the old
        // `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1` — which every spawned agent
        // could copy-paste — must NOT let `wg done` promote a task whose
        // deliverable is genuinely missing.
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let desc = "## Deliverables\n- latest.pt\n";
        setup_with_project_root(project_root, vec![task_with_desc("t1", desc)]);
        let wg_dir = project_root.join(".wg");

        // Baseline: refuses with no env var set.
        assert!(run(&wg_dir, "t1", false, false, false, false, false).is_err());

        // The removed override must have no effect — still refuses.
        unsafe { std::env::set_var("WG_DELIVERABLE_PREFLIGHT_OVERRIDE", "1") };
        let result = run(&wg_dir, "t1", false, false, false, false, false);
        unsafe { std::env::remove_var("WG_DELIVERABLE_PREFLIGHT_OVERRIDE") };

        assert!(
            result.is_err(),
            "env override must NOT bypass the deliverable gate"
        );
        let err = result.unwrap_err().to_string();
        assert!(err.contains("deliverable preflight refused"));
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        assert_ne!(graph.get_task("t1").unwrap().status, Status::Done);
    }

    #[test]
    #[serial]
    fn done_honors_explicit_deliverable_with_marker_in_name_for_assigned_worker() {
        // Regression guard (PR #54 round 3, Erik CHANGES_REQUESTED): an
        // explicit `## Deliverables` bullet whose filename contains a
        // negative-framing marker substring (`discard-policy.md`) must remain
        // a required deliverable. Previously `has_negative_framing` scanned the
        // whole bullet, so the filename self-suppressed and the assigned worker
        // could `wg done` a genuinely missing deliverable (exit 0, task Done,
        // no `deliverable-missing` marker). Here the file is absent, so `wg
        // done` — run as the assigned worker — must refuse, leave the task in
        // progress, and record the `deliverable-missing` failure class.
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let desc = "## Description\nDocument the discard policy.\n\n## Deliverables\n- discard-policy.md\n";
        let mut task = task_with_desc("explicit-discard-name", desc);
        task.assigned = Some("agent-worker-1".to_string());
        setup_with_project_root(project_root, vec![task]);
        let wg_dir = project_root.join(".wg");

        // Simulate the assigned worker running `wg done` (agent path).
        unsafe { std::env::set_var("WG_AGENT_ID", "agent-worker-1") };
        let result = run(
            &wg_dir,
            "explicit-discard-name",
            false,
            false,
            false,
            false,
            false,
        );
        unsafe { std::env::remove_var("WG_AGENT_ID") };

        assert!(
            result.is_err(),
            "assigned worker must not promote a missing explicit deliverable whose name contains a marker"
        );
        let err = result.unwrap_err().to_string();
        assert!(err.contains("deliverable preflight refused"), "got: {err}");
        assert!(err.contains("discard-policy.md"), "got: {err}");

        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("explicit-discard-name").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.failure_class, Some(FailureClass::DeliverableMissing));
        assert!(
            task.log
                .iter()
                .any(|e| e.actor == Some("deliverable-preflight".to_string())),
            "expected a deliverable-missing marker in the task log"
        );

        // And once the deliverable exists, the same worker can complete it.
        std::fs::write(project_root.join("discard-policy.md"), b"policy text").unwrap();
        unsafe { std::env::set_var("WG_AGENT_ID", "agent-worker-1") };
        let ok = run(
            &wg_dir,
            "explicit-discard-name",
            false,
            false,
            false,
            false,
            false,
        );
        unsafe { std::env::remove_var("WG_AGENT_ID") };
        assert!(ok.is_ok(), "got: {:?}", ok.err());
        let graph = load_graph(&graph_path(&wg_dir)).unwrap();
        let task = graph.get_task("explicit-discard-name").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.failure_class, None);
    }
}
