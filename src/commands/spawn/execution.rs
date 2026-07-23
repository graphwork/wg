//! Spawn execution — claims a task, assembles prompt, launches executor process,
//! and registers the agent.

use anyhow::{Context, Result};
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use worksgood::agency;
use worksgood::config::{CapBehavior, Config, EndpointConfig, ReasoningLevel};
use worksgood::dispatch::plan_spawn;
use worksgood::graph::{LogEntry, Node, Status, Task, is_system_task};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::service::executor::{ExecutorRegistry, PromptTemplate, TemplateVars, build_prompt};
use worksgood::service::registry::{AgentRegistry, LockedRegistry};

use super::context::{
    build_previous_attempt_context, build_scope_context, build_task_context, discover_test_files,
    format_test_discovery_context, resolve_task_exec_mode, resolve_task_scope,
};
use super::worktree;
use super::{
    SpawnResult, agent_output_dir, graph_path, parse_timeout_secs, prompt_file_command,
    sanitize_bash_path, shell_escape, strip_verbatim_prefix,
};

const OUTPUT_RESERVATION_FILE: &str = ".spawn-reservation";
const LAUNCH_GATE_FILE: &str = ".launch-permit";

#[cfg(test)]
thread_local! {
    static SPAWN_FAULT_BOUNDARY: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
    static LAST_GATED_CHILD_PID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn spawn_fault(boundary: &str) -> Result<()> {
    #[cfg(test)]
    {
        let injected = SPAWN_FAULT_BOUNDARY.with(|fault| *fault.borrow() == Some(boundary));
        if injected {
            anyhow::bail!("injected spawn transaction failure at {boundary}");
        }
    }
    let _ = boundary;
    Ok(())
}

/// Filesystem resources prepared before graph claim/process launch. Until
/// `commit`, Drop rolls back only resources carrying this transaction's exact
/// ownership token. A dirty worktree is never deleted; rollback reports the
/// preserved path loudly instead.
#[derive(Debug)]
struct PreparedSpawnWorkspace {
    agent_id: String,
    output_dir: PathBuf,
    output_token: String,
    worktree_info: Option<worktree::WorktreeInfo>,
    created_worktree: bool,
    removed_cleanup_marker: Option<Vec<u8>>,
    committed: bool,
}

impl PreparedSpawnWorkspace {
    fn prepare_launch(&mut self) -> Result<()> {
        let reservation = self.output_dir.join(OUTPUT_RESERVATION_FILE);
        let recorded = fs::read_to_string(&reservation).with_context(|| {
            format!(
                "spawn output reservation disappeared before launch: {}",
                reservation.display()
            )
        })?;
        if recorded != self.output_token {
            anyhow::bail!(
                "spawn output reservation ownership mismatch at {}; refusing launch",
                reservation.display()
            );
        }
        if let Some(ref info) = self.worktree_info {
            let marker = info
                .path
                .join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER);
            if marker.exists() {
                let contents = fs::read(&marker).with_context(|| {
                    format!(
                        "failed to snapshot stale cleanup marker before isolated launch: {}",
                        marker.display()
                    )
                })?;
                fs::remove_file(&marker).with_context(|| {
                    format!(
                        "failed to clear stale cleanup marker before isolated launch: {}",
                        marker.display()
                    )
                })?;
                self.removed_cleanup_marker = Some(contents);
            }
        }
        Ok(())
    }

    /// Called only after the atomic launch permit has been published. From
    /// this point the process is live and rollback must never delete its cwd.
    fn commit_after_launch(&mut self) {
        self.committed = true;
        self.removed_cleanup_marker = None;
        let reservation = self.output_dir.join(OUTPUT_RESERVATION_FILE);
        if let Err(error) = fs::remove_file(&reservation) {
            eprintln!(
                "[spawn] WARNING: live agent output reservation could not be cleared at {}: {}",
                reservation.display(),
                error
            );
        }
    }

    fn rollback(&mut self) {
        if self.committed {
            return;
        }
        if !self.created_worktree
            && let Some(contents) = self.removed_cleanup_marker.take()
            && let Some(ref info) = self.worktree_info
        {
            let marker = info
                .path
                .join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER);
            if worktree::verify_worktree_info(info).is_err() {
                eprintln!(
                    "[spawn] refusing to restore cleanup marker through an unverified retry worktree {}; inspect it manually",
                    info.path.display()
                );
            } else if !marker.exists()
                && let Err(error) = fs::write(&marker, contents)
            {
                eprintln!(
                    "[spawn] failed to restore retry cleanup marker {}: {}",
                    marker.display(),
                    error
                );
            }
        }
        if self.created_worktree
            && let Some(ref info) = self.worktree_info
            && let Err(error) = worktree::rollback_created_worktree(info)
        {
            eprintln!(
                "[spawn] ISOLATION ROLLBACK PRESERVED {}: {:#}",
                info.path.display(),
                error
            );
        }
        let reservation = self.output_dir.join(OUTPUT_RESERVATION_FILE);
        let owns_output =
            fs::read_to_string(&reservation).is_ok_and(|recorded| recorded == self.output_token);
        if owns_output {
            if let Err(error) = fs::remove_dir_all(&self.output_dir) {
                eprintln!(
                    "[spawn] failed to roll back owned agent output {}: {}",
                    self.output_dir.display(),
                    error
                );
            }
        } else if self.output_dir.exists() {
            eprintln!(
                "[spawn] refusing to remove unverified agent output {}; inspect it manually",
                self.output_dir.display()
            );
        }
    }
}

impl Drop for PreparedSpawnWorkspace {
    fn drop(&mut self) {
        self.rollback();
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn advance_agent_id(registry: &mut LockedRegistry) -> Result<()> {
    registry.next_agent_id = registry
        .next_agent_id
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("agent ID space exhausted while allocating isolation"))?;
    Ok(())
}

fn output_reservation(dir: &Path, agent_id: &str) -> Result<(PathBuf, String)> {
    let parent = dir.join("agents");
    fs::create_dir_all(&parent)
        .with_context(|| format!("failed to create agent output parent {}", parent.display()))?;
    let output_dir = agent_output_dir(dir, agent_id);
    fs::create_dir(&output_dir).with_context(|| {
        format!(
            "agent output collision or reservation failure at {}",
            output_dir.display()
        )
    })?;
    let token = uuid::Uuid::new_v4().to_string();
    if let Err(error) = fs::write(output_dir.join(OUTPUT_RESERVATION_FILE), &token) {
        let _ = fs::remove_dir_all(&output_dir);
        return Err(error).context("failed to persist agent output reservation");
    }
    Ok((output_dir, token))
}

fn reusable_worktree_is_available(
    registry: &LockedRegistry,
    info: &worktree::WorktreeInfo,
    task_id: &str,
) -> Result<()> {
    for agent in registry.all() {
        let claims_path = agent
            .worktree_path
            .as_deref()
            .is_some_and(|path| same_path(Path::new(path), &info.path))
            || agent.id == info.agent_id;
        if !claims_path {
            continue;
        }
        let process_alive = worksgood::service::is_process_alive(agent.pid);
        if agent.task_id != task_id || agent.is_alive() || process_alive {
            anyhow::bail!(
                "isolated worktree {} is owned by {} attempt {} for task '{}' (status {:?}); no process launched. Recover by terminating/reaping that attempt or archive the worktree explicitly after inspection",
                info.path.display(),
                if agent.is_alive() || process_alive {
                    "live"
                } else {
                    "terminal"
                },
                agent.id,
                agent.task_id,
                agent.status
            );
        }
    }
    Ok(())
}

fn prepare_spawn_workspace(
    dir: &Path,
    project_root: &Path,
    task_id: &str,
    needs_worktree: bool,
    registry: &mut LockedRegistry,
) -> Result<PreparedSpawnWorkspace> {
    let reusable = if needs_worktree {
        let attempted_agent = format!("agent-{}", registry.next_agent_id);
        let attempted_path = project_root.join(".wg-worktrees").join(&attempted_agent);
        let info = worktree::find_verified_worktree_for_task(project_root, dir, task_id)
            .with_context(|| {
                format!(
                    "REQUIRED ISOLATION preflight failed for {} at {} while checking retry ownership. No process launched; repair with `git worktree repair` or inspect/archive the named worktree explicitly",
                    attempted_agent,
                    attempted_path.display()
                )
            })?;
        if let Some(ref info) = info {
            reusable_worktree_is_available(registry, info, task_id)?;
        }
        info
    } else {
        None
    };
    let cache_ownership = worksgood::disk_sentinel::load_ownership(dir)
        .context("failed to inspect cache leases during collision-free agent allocation")?;

    loop {
        let agent_id = format!("agent-{}", registry.next_agent_id);
        let worktree_path = project_root.join(".wg-worktrees").join(&agent_id);
        let branch = format!("wg/{agent_id}/{task_id}");
        let output_dir = agent_output_dir(dir, &agent_id);
        let mut collisions = Vec::new();
        if registry.get_agent(&agent_id).is_some() {
            collisions.push("agent registry entry".to_string());
        }
        if cache_ownership
            .caches
            .iter()
            .any(|cache| cache.agent_id == agent_id)
        {
            collisions.push("cache ownership lease".to_string());
        }
        if output_dir.exists() {
            collisions.push(format!("output path {}", output_dir.display()));
        }
        if needs_worktree {
            if worktree_path.exists() {
                collisions.push(format!("worktree path {}", worktree_path.display()));
            }
            if worktree::agent_branch_exists(project_root, &agent_id).with_context(|| {
                format!(
                    "REQUIRED ISOLATION could not verify branch allocation for {} at {}. No process launched; repair the repository/worktree metadata and retry",
                    agent_id,
                    worktree_path.display()
                )
            })? {
                collisions.push(format!("branch namespace wg/{agent_id}/* (candidate {branch})"));
            }
        }
        if !collisions.is_empty() {
            eprintln!(
                "[spawn] ISOLATION COLLISION for {} at {}: {}; preserving unknown/dirty source and atomically trying the next agent ID. Inspect recovery with `git worktree list` and `wg worktree archive {} --remove` only after review",
                agent_id,
                worktree_path.display(),
                collisions.join(", "),
                agent_id
            );
            advance_agent_id(registry)?;
            continue;
        }

        let (reserved_output, output_token) = match output_reservation(dir, &agent_id) {
            Ok(reservation) => reservation,
            Err(error) if output_dir.exists() => {
                eprintln!(
                    "[spawn] agent output collision for {} at {}; preserving it and trying the next ID: {:#}",
                    agent_id,
                    output_dir.display(),
                    error
                );
                advance_agent_id(registry)?;
                continue;
            }
            Err(error) => return Err(error),
        };

        let (worktree_info, created_worktree) = if needs_worktree {
            if let Some(ref prior) = reusable {
                eprintln!(
                    "[spawn] Reusing verified prior worktree for task '{}' at {} (branch: {}) — retry-in-place",
                    task_id,
                    prior.path.display(),
                    prior.branch
                );
                (Some(prior.clone()), false)
            } else {
                match worktree::create_worktree(project_root, dir, &agent_id, task_id) {
                    Ok(info) => {
                        eprintln!(
                            "[spawn] Created and verified isolated worktree for {} at {} (branch: {})",
                            agent_id,
                            info.path.display(),
                            info.branch
                        );
                        (Some(info), true)
                    }
                    Err(error) if worktree::is_collision(&error) => {
                        let _ = fs::remove_dir_all(&reserved_output);
                        eprintln!(
                            "[spawn] ISOLATION COLLISION for {} at {}: {:#}; source preserved, trying the next agent ID",
                            agent_id,
                            worktree_path.display(),
                            error
                        );
                        advance_agent_id(registry)?;
                        continue;
                    }
                    Err(error) => {
                        let _ = fs::remove_dir_all(&reserved_output);
                        return Err(error).with_context(|| {
                            format!(
                                "REQUIRED ISOLATION FAILED for {} at {}. No worker/handler was launched and task '{}' remains dispatchable. Preserve unknown/dirty source; inspect `git worktree list`, repair with `git worktree repair`, or archive explicitly after review",
                                agent_id,
                                worktree_path.display(),
                                task_id
                            )
                        });
                    }
                }
            }
        } else {
            (None, false)
        };

        return Ok(PreparedSpawnWorkspace {
            agent_id,
            output_dir: reserved_output,
            output_token,
            worktree_info,
            created_worktree,
            removed_cleanup_marker: None,
            committed: false,
        });
    }
}

#[derive(Clone)]
struct TaskClaimSnapshot {
    status: Status,
    started_at: Option<String>,
    assigned: Option<String>,
}

fn claim_task_for_spawn(
    graph_path: &Path,
    task_id: &str,
    agent_id: &str,
) -> Result<TaskClaimSnapshot> {
    let mut snapshot = None;
    let mut claim_error = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            claim_error = Some(anyhow::anyhow!("Task '{}' not found", task_id));
            return false;
        };
        if !matches!(task.status, Status::Open | Status::Blocked | Status::Incomplete)
            || task.assigned.is_some()
        {
            claim_error = Some(anyhow::anyhow!(
                "Task '{}' changed during spawn and is no longer dispatchable (status={:?}, assigned={:?})",
                task_id,
                task.status,
                task.assigned
            ));
            return false;
        }
        snapshot = Some(TaskClaimSnapshot {
            status: task.status,
            started_at: task.started_at.clone(),
            assigned: task.assigned.clone(),
        });
        task.status = Status::InProgress;
        task.started_at = Some(Utc::now().to_rfc3339());
        task.assigned = Some(agent_id.to_string());
        true
    })
    .context("failed to atomically claim task for spawn")?;
    if let Some(error) = claim_error {
        return Err(error);
    }
    snapshot.ok_or_else(|| anyhow::anyhow!("task claim produced no transaction snapshot"))
}

fn rollback_task_claim(
    graph_path: &Path,
    task_id: &str,
    agent_id: &str,
    snapshot: &TaskClaimSnapshot,
) -> Result<()> {
    let mut ownership_lost = false;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            ownership_lost = true;
            return false;
        };
        if task.status != Status::InProgress || task.assigned.as_deref() != Some(agent_id) {
            ownership_lost = true;
            return false;
        }
        task.status = snapshot.status;
        task.started_at.clone_from(&snapshot.started_at);
        task.assigned.clone_from(&snapshot.assigned);
        true
    })
    .context("failed to roll back task claim")?;
    if ownership_lost {
        anyhow::bail!(
            "refused to roll back task '{}' because claim ownership changed from {}",
            task_id,
            agent_id
        );
    }
    Ok(())
}

fn publish_launch_permit_for_claim(
    graph_path: &Path,
    task_id: &str,
    agent_id: &str,
    output_dir: &Path,
    token: &str,
) -> Result<()> {
    let mut outcome = None;
    modify_graph(graph_path, |graph| {
        let valid = graph.get_task(task_id).is_some_and(|task| {
            task.status == Status::InProgress && task.assigned.as_deref() == Some(agent_id)
        });
        outcome = Some(if valid {
            // Publish while the graph lock is still held. The claim and permit
            // therefore have one commit point: another dispatcher cannot
            // unclaim/reassign between the ownership check and handler launch.
            release_launch_gate(output_dir, token)
        } else {
            Err(anyhow::anyhow!(
                "task '{}' claim ownership changed before launch (expected status=in-progress assigned={}); refusing to release handler gate",
                task_id,
                agent_id
            ))
        });
        false
    })
    .context("failed to lock/recheck task claim before launch")?;
    outcome.ok_or_else(|| anyhow::anyhow!("task claim launch check produced no outcome"))?
}

fn kill_spawned_child(child: &mut Child) {
    let pid = child.id();
    #[cfg(unix)]
    unsafe {
        // The wrapper calls setsid in pre_exec. Try the session/process group
        // first, then the exact PID in case it has not completed setsid yet.
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

fn release_launch_gate(output_dir: &Path, token: &str) -> Result<()> {
    let temporary = output_dir.join(".launch-permit.tmp");
    let gate = output_dir.join(LAUNCH_GATE_FILE);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("failed to create launch permit {}", temporary.display()))?;
    use std::io::Write as _;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, &gate).with_context(|| {
        format!(
            "failed to atomically publish launch permit {}",
            gate.display()
        )
    })?;
    Ok(())
}

/// Internal shared implementation for spawning an agent.
/// Both `run()` (CLI) and `spawn_agent()` (coordinator) delegate here.
pub(crate) fn spawn_agent_inner(
    dir: &Path,
    task_id: &str,
    executor_name: &str,
    timeout: Option<&str>,
    model: Option<&str>,
    spawned_by: &str,
) -> Result<SpawnResult> {
    spawn_agent_inner_with_reasoning(
        dir,
        task_id,
        executor_name,
        timeout,
        model,
        None,
        spawned_by,
    )
}

/// Internal shared implementation for spawning an agent with structured reasoning.
pub(crate) fn spawn_agent_inner_with_reasoning(
    dir: &Path,
    task_id: &str,
    executor_name: &str,
    timeout: Option<&str>,
    model: Option<&str>,
    reasoning: Option<&str>,
    spawned_by: &str,
) -> Result<SpawnResult> {
    let graph_path = graph_path(dir);

    if !graph_path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Load the graph and get task info
    let graph = load_graph(&graph_path).context("Failed to load graph")?;

    let task = graph.get_task_or_err(task_id)?;

    // Selection preflight must happen before plan resolution, worktree creation,
    // registry writes, or the atomic claim. Shell tasks remain graph-only.
    #[cfg(not(test))]
    if executor_name != "shell" && resolve_task_exec_mode(task, dir) != "shell" {
        let explicit = model
            .map(|m| (m, false))
            .or_else(|| task.model.as_deref().map(|m| (m, true)));
        worksgood::execution_selection::require(dir, explicit, "wg spawn")?;
    }

    // Capture audit info before mutable borrows
    let task_title_for_audit = task.title.clone();
    let task_agent_for_audit = task.agent.clone();

    // Look up agency agent preferences if task has an assigned agent identity.
    // These are used later in model/provider resolution.
    let (agent_preferred_model, agent_preferred_provider) =
        if let Some(ref agent_hash) = task_agent_for_audit {
            let agents_dir = dir.join("agency/cache/agents");
            match agency::find_agent_by_prefix(&agents_dir, agent_hash) {
                Ok(agent) => (
                    agent.preferred_model.clone(),
                    agent.preferred_provider.clone(),
                ),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

    // SINGLE SOURCE OF TRUTH: route spawn decisions through plan_spawn so that
    // {executor, model, endpoint} are decided in one place rather than
    // independently in the argv builder. The CLI's --executor flag
    // (`executor_name`) is treated as the caller's chosen executor floor and
    // passed in via the `agent_executor` slot — that gives it priority over
    // [dispatcher].executor, matching the expected CLI override semantics.
    //
    // The plan-derived endpoint is the only source consulted when assembling
    // native-executor argv flags below; there is no fallback ad-hoc lookup.
    let config = Config::load_merged(dir)
        .context("Cannot spawn while the project profile selection is invalid")?;
    // Get task model preference. Freeform task tags are inert labels, so they
    // never participate in executor/model routing.
    let task_model = task.model.clone().or_else(|| {
        if let Some(ref tier_str) = task.tier
            && let Ok(tier) = tier_str.parse::<worksgood::config::Tier>()
            && let Some(resolved) = config.resolve_tier(tier)
        {
            return Some(resolved.model);
        }
        None
    });
    let plan_default_model = task_model.as_deref().or(model);
    let explicit_reasoning = reasoning
        .map(str::parse::<ReasoningLevel>)
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = plan_spawn(task, &config, Some(executor_name), plan_default_model)?;
    eprintln!(
        "[{}] {}: {}",
        spawned_by,
        task_id,
        plan.provenance.log_line(&plan)
    );
    let resolved_executor_name = plan.executor.as_str();
    let resolved_model_for_spawn = Some(plan.model.raw.clone());
    let resolved_reasoning = explicit_reasoning.or(task.reasoning).or(plan.reasoning);
    // One opaque run identity ties the spawned route, completion outcome, and
    // dead-agent triage together. Unlike PID/timestamps, it cannot collide on
    // a fast restart or PID reuse.
    let spawn_run_id = uuid::Uuid::new_v4().to_string();
    let health_route = worksgood::service::HealthRouteKey::from_spawn_plan(&plan);

    // Only allow spawning on tasks that are Open or Blocked
    match task.status {
        Status::Open | Status::Blocked | Status::Incomplete => {}
        Status::InProgress => {
            let since = task
                .started_at
                .as_ref()
                .map(|t| format!(" (since {})", t))
                .unwrap_or_default();
            match &task.assigned {
                Some(assigned) => {
                    anyhow::bail!(
                        "Task '{}' is already claimed by @{}{}",
                        task_id,
                        assigned,
                        since
                    );
                }
                None => {
                    anyhow::bail!("Task '{}' is already in progress{}", task_id, since);
                }
            }
        }
        Status::Done => {
            anyhow::bail!("Task '{}' is already done", task_id);
        }
        Status::Failed => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is Failed. Use 'wg retry' first.",
                task_id
            );
        }
        Status::Abandoned => {
            anyhow::bail!("Cannot spawn on task '{}': task is Abandoned", task_id);
        }
        Status::Waiting => {
            anyhow::bail!("Cannot spawn on task '{}': task is Waiting", task_id);
        }
        Status::PendingValidation => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending validation",
                task_id
            );
        }
        Status::PendingEval => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending evaluation",
                task_id
            );
        }
        Status::FailedPendingEval => {
            anyhow::bail!(
                "Cannot spawn on task '{}': task is pending rescue evaluation",
                task_id
            );
        }
    }

    // Resolve context scope (config was loaded earlier for plan_spawn)

    // Check OpenRouter cost caps before proceeding with expensive operations
    if let Some(provider) = resolved_model_for_spawn
        .as_deref()
        .and_then(|m| worksgood::config::parse_model_spec(m).provider)
        && provider == "openrouter"
    {
        check_openrouter_cost_caps(&config, dir, task_id, resolved_model_for_spawn.as_deref())?;
    }

    let scope = resolve_task_scope(task, &config, dir);

    // Build context from dependencies
    let task_context = build_task_context(&graph, task);

    // Build scope context for prompt assembly
    let mut scope_ctx = build_scope_context(&graph, task, scope, &config, dir);

    // Inject previous attempt context on retry OR on in-place eval rescue
    // (rescue_count > 0 means the prior attempt failed the eval gate and we
    // are iterating in place — the evaluator's notes belong in the next
    // agent's context). See `commands::evaluate::check_eval_gate`.
    if task.retry_count > 0 || task.rescue_count > 0 {
        let max_tokens = config.checkpoint.retry_context_tokens;
        scope_ctx.previous_attempt_context = build_previous_attempt_context(task, dir, max_tokens);
    }

    // Create template variables
    let mut vars = TemplateVars::from_task(task, Some(&task_context), Some(dir));

    // Detect failed dependencies for triage mode
    let mut failed_deps_lines = Vec::new();
    for dep_id in &task.after {
        if let Some(dep_task) = graph.get_task(dep_id)
            && dep_task.status == Status::Failed
        {
            let reason = dep_task.failure_reason.as_deref().unwrap_or("unknown");
            failed_deps_lines.push(format!(
                "- {}: \"{}\" — Reason: {}",
                dep_id, dep_task.title, reason
            ));
        }
    }
    if !failed_deps_lines.is_empty() {
        vars.has_failed_deps = true;
        vars.failed_deps_info = failed_deps_lines.join("\n");
    }

    // Pre-task test discovery: scan for test files and inject into agent context.
    if config.coordinator.auto_test_discovery {
        let project_root = dir
            .canonicalize()
            .ok()
            .and_then(|abs| abs.parent().map(|p| p.to_path_buf()));
        if let Some(ref root) = project_root {
            let test_files = discover_test_files(root);
            if !test_files.is_empty() {
                eprintln!(
                    "[spawn] Test discovery: found {} test file(s) for task '{}'",
                    test_files.len(),
                    task_id
                );
                scope_ctx.discovered_tests = format_test_discovery_context(&test_files);
            }
        }
    }

    // Get task exec command for shell executor
    let task_exec = task.exec.clone();
    // Get per-task timeout override
    let task_timeout = task.timeout.clone();
    // Capture the task's quality tier (may be set by tier escalation on retry)
    let task_tier = task.tier.clone();
    // Get session_id for resume (from previous wg wait)
    let resume_session_id = task.session_id.clone();
    // Resolve exec_mode: task.exec_mode > role.default_exec_mode > "full"
    let resolved_exec_mode = resolve_task_exec_mode(task, dir);
    // Load executor config using the registry
    let executor_registry = ExecutorRegistry::new(dir);
    let executor_config = executor_registry.load_config(resolved_executor_name)?;

    // For shell executor, we need an exec command
    if executor_config.executor.executor_type == "shell" && task_exec.is_none() {
        anyhow::bail!("Task '{}' has no exec command for shell executor", task_id);
    }

    // --- Unified model + provider resolution ---
    // Resolves model and provider in a single pass through the precedence hierarchy.
    // At each tier, if the model uses `provider:model` format, the provider is
    // extracted automatically via parse_model_spec().
    let task_provider = graph.get_task(task_id).and_then(|t| t.provider.clone());
    let resolved_task_agent =
        config.resolve_model_for_role(worksgood::config::DispatchRole::TaskAgent);
    let resolved = resolve_model_and_provider(
        resolved_model_for_spawn.clone(),
        task_provider.clone(),
        agent_preferred_model,
        agent_preferred_provider.clone(),
        executor_config.executor.model.clone(),
        Some(resolved_task_agent.model.clone()),
        resolved_task_agent.provider.clone(),
        resolved_model_for_spawn.as_deref(),
        config.coordinator.provider.clone(),
    );

    // --- Model registry alias resolution ---
    // If the effective model string matches a registry entry, resolve it to the
    // actual API model ID, provider, and endpoint. Built-in tier aliases
    // (haiku/sonnet/opus) are kept as-is for backward compatibility with the
    // Claude CLI, which understands them natively.
    let (effective_model, registry_provider, registry_endpoint) = resolve_spawn_model_via_registry(
        resolved_executor_name,
        resolved.model,
        resolved_model_for_spawn.as_ref(),
        &config,
        dir,
    )?;

    // --- Pre-flight model validation ---
    // Validate OpenRouter-style models against the cached model list before spawning.
    // This is a warning/suggestion system, not a hard gate.
    let (effective_model, model_validation_warning) = {
        let mut model = effective_model;
        let mut warning: Option<String> = None;
        if resolved_executor_name != "pi"
            && let Some(ref m) = model
            && m.contains('/')
            && !BUILTIN_TIER_ALIASES.contains(&m.as_str())
        {
            let validation =
                worksgood::executor::native::openai_client::validate_openrouter_model(m, dir);
            if !validation.was_valid {
                if let Some(ref w) = validation.warning {
                    eprintln!("[spawn] WARNING: {}", w);
                }
                warning = validation.warning;
                model = Some(validation.model);
            } else {
                eprintln!("[spawn] Model '{}' validated against model cache", m);
            }
        }
        (model, warning)
    };

    // Provider is still resolved by resolve_model_and_provider() above.
    // The registry may contribute a provider if the model matched a registry entry;
    // use it only when the tier cascade didn't already produce one.
    let effective_provider: Option<String> = resolved.provider.or(registry_provider.clone());

    // Override model in template vars with the effective model. External CLI
    // adapters receive their native model spelling here too, so TOML-backed
    // configs that use `{{model}}` do not have to understand WG's
    // `provider:model` syntax.
    if let Some(ref m) = effective_model {
        vars.model = model_template_value_for_executor(
            &executor_config.executor.executor_type,
            Some(m),
            effective_provider.as_deref(),
        )
        .unwrap_or_else(|| m.clone());
    }

    // Fail-safe direct-spawn admission. The dispatcher performs the same
    // class-specific check so it can skip builds and continue evaluators, but
    // process creation repeats it under the registry lock below to close races.
    let build_class = worksgood::disk_sentinel::classify_task(task);

    // Load agent registry with lock for concurrent safety.
    // The lock is held until save() to prevent two concurrent spawns from
    // reading the same next_agent_id and overwriting each other's registration.
    // Lock hierarchy: graph lock (per-call in load/save_graph) < registry lock (held here).
    let mut locked_registry = AgentRegistry::load_locked(dir)?;

    // The registry lock serializes the measured projection + reservation with
    // process registration. Without this second, projected check two concurrent
    // spawns could both spend the same free bytes after passing the cheap level
    // check above.
    if build_class.is_build_capable() {
        let admission = worksgood::disk_sentinel::build_admission_reclaiming_owned(
            dir,
            &config.coordinator.resource_management,
            build_class,
        );
        if !admission.allowed {
            anyhow::bail!(
                "build admission refused: {} (candidate={} bytes, concurrent-reserve={} bytes; safe retry will reuse this worktree)",
                admission.reason,
                admission.candidate_bytes,
                admission.concurrent_reserved_bytes
            );
        }
    }

    if build_class.is_heavy() {
        let active_heavy = locked_registry
            .all()
            .filter(|agent| {
                agent.is_live(
                    config
                        .coordinator
                        .resource_management
                        .disk_agent_heartbeat_seconds,
                ) && graph
                    .get_task(&agent.task_id)
                    .is_some_and(|task| worksgood::disk_sentinel::classify_task(task).is_heavy())
            })
            .count();
        if active_heavy >= config.coordinator.resource_management.max_build_agents {
            anyhow::bail!(
                "build-heavy admission budget full ({}/{})",
                active_heavy,
                config.coordinator.resource_management.max_build_agents
            );
        }
    }
    // --- Workspace reservation and mandatory isolation ---
    // The registry lock is held across collision-free ID allocation, output
    // reservation, worktree creation/verification, graph claim, gated process
    // spawn, and registry commit. No isolation failure can reach cmd.spawn().
    let project_root = dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from {:?}", dir))?;
    let needs_worktree = should_create_worktree(
        config.coordinator.worktree_isolation,
        task_id,
        resolved_exec_mode.as_str(),
    );
    let mut workspace = prepare_spawn_workspace(
        dir,
        project_root,
        task_id,
        needs_worktree,
        &mut locked_registry,
    )?;
    spawn_fault("workspace-prepared")?;
    let temp_agent_id = workspace.agent_id.clone();
    let output_dir = workspace.output_dir.clone();
    let worktree_info = workspace.worktree_info.clone();
    if !config.coordinator.worktree_isolation {
        eprintln!(
            "[spawn] worktree isolation explicitly disabled by configuration for {}; shared/configured working directory mode is intentional",
            temp_agent_id
        );
    } else if !needs_worktree {
        eprintln!(
            "[spawn] Isolation not required for {} (task '{}', exec_mode={}): system or read-only execution mode",
            temp_agent_id, task_id, resolved_exec_mode
        );
    }

    let output_file = output_dir.join("output.log");
    let output_file_str = output_file.to_string_lossy().to_string();
    vars.in_worktree = worktree_info.is_some();

    let owned_target_path = if build_class.is_build_capable() {
        worksgood::disk_sentinel::target_path_for_agent(
            &config.coordinator.resource_management,
            worktree_info.as_ref().map(|wt| wt.path.as_path()),
            &temp_agent_id,
        )
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                worktree_info
                    .as_ref()
                    .map(|wt| wt.path.join(&path))
                    .or_else(|| std::env::current_dir().ok().map(|cwd| cwd.join(&path)))
                    .unwrap_or(path)
            }
        })
    } else {
        None
    };
    if let Some(path) = owned_target_path.as_ref() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create owned Cargo target {}", path.display()))?;
    }
    let owned_tmp_path = if build_class.is_build_capable() {
        let root = config
            .coordinator
            .resource_management
            .build_tmp_root
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Some(root.join(format!("wg-cargo-tmp-{temp_agent_id}")))
    } else {
        None
    };
    if let Some(path) = owned_tmp_path.as_ref() {
        fs::create_dir_all(path)
            .with_context(|| format!("Failed to create owned build scratch {}", path.display()))?;
    }

    // Apply templates to executor settings (with effective model in vars)
    let mut settings = executor_config.apply_templates(&vars);

    // Universal wg context injection for all executor types.
    // Ensures all executors receive consistent WG context in their prompts,
    // with model-appropriate knowledge tier based on context window and capabilities.
    let model_str = settings.model.as_deref().unwrap_or("");
    let model_tier = super::context::classify_model_tier(model_str);
    scope_ctx.wg_guide_content = super::context::build_tiered_guide(dir, model_tier, model_str);

    // Native executor exposes its own in-process file tools
    // (`read_file`/`write_file`/`edit_file`/`grep`/`glob`). When spawning
    // via native, inject the guidance section that teaches the model about
    // those tools and warns against bash-based file manipulation.
    scope_ctx.native_file_tools = settings.executor_type == "native";

    // Scope-based prompt assembly for built-in executors.
    // When no custom prompt_template is defined (built-in defaults),
    // use build_prompt() to assemble the prompt based on context scope.
    if settings.prompt_template.is_none() && executor_uses_auto_prompt(&settings.executor_type) {
        let prompt = build_prompt(&vars, scope, &scope_ctx);

        // Debug logging: capture spawn metadata if WG_DEBUG_PROMPTS is set
        if std::env::var("WG_DEBUG_PROMPTS").is_ok()
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/wg_debug_prompts.log")
        {
            use std::io::Write;
            let debug_info = format!(
                "=== WG DEBUG: Spawning Agent ===\n\
                    Task ID: {}\n\
                    Executor: {}\n\
                    Model: {}\n\
                    Context Scope: {:?}\n\
                    Execution Mode: {}\n\
                    Agent Identity: {}\n\
                    === End of Spawn Metadata ===\n\n",
                task_id,
                resolved_executor_name,
                vars.model,
                scope,
                resolved_exec_mode.as_str(),
                task.agent
                    .as_deref()
                    .unwrap_or("Default (no specific agent assigned)")
            );
            let _ = file.write_all(debug_info.as_bytes());
        }

        settings.prompt_template = Some(PromptTemplate { template: prompt });
    }

    // Use resolved exec_mode (already accounts for role defaults)
    let exec_mode = resolved_exec_mode.as_str();

    // Endpoint resolution: plan.endpoint is the single source of truth. For
    // executors that don't need an endpoint (claude/codex/shell),
    // plan.endpoint is None and the argv builder skips --endpoint-* flags
    // entirely. For native, plan.endpoint carries the resolved EndpointConfig.
    // No ad-hoc cascade lookup happens here anymore — that decision lives in
    // dispatch::plan::plan_spawn().
    let endpoint_config: Option<&EndpointConfig> = plan.endpoint.as_ref();
    let effective_endpoint: Option<String> = endpoint_config.map(|ep| ep.name.clone());
    let effective_endpoint_url: Option<String> = endpoint_config.and_then(|ep| ep.url.clone());
    let effective_api_key: Option<String> =
        endpoint_config.and_then(|ep| ep.resolve_api_key(Some(dir)).ok().flatten());

    let effective_working_dir = worktree_info
        .as_ref()
        .map(|wt| wt.path.as_path())
        .or_else(|| settings.working_dir.as_deref().map(Path::new));
    preflight_executor_command(&settings, resolved_executor_name, effective_working_dir)?;

    // Validate endpoint resolution for registry-resolved models — but only
    // when the plan actually selected an endpoint. If plan.endpoint is None
    // (e.g. executor=claude), the model registry's endpoint hint is irrelevant
    // to argv assembly and a missing/keyless endpoint must not block the spawn.
    if plan.endpoint.is_some()
        && let Some(ref reg_ep) = registry_endpoint
    {
        if endpoint_config.is_none() {
            anyhow::bail!(
                "Model references endpoint '{}' which is not configured.\n\
                 Add it with: wg endpoint add {} --provider <provider> --url <url>",
                reg_ep,
                reg_ep,
            );
        }
        if effective_api_key.is_none() {
            let ep = endpoint_config.unwrap(); // safe: checked above
            anyhow::bail!(
                "Endpoint '{}' (provider: {}) has no valid API key.\n\
                 Set one with: wg key set {} --value <key>",
                reg_ep,
                ep.provider,
                ep.provider,
            );
        }
    }

    // Build the inner command string first (with optional fallback for session resume)
    let (inner_command, fallback_command) = build_inner_command_with_reasoning(
        &settings,
        exec_mode,
        &output_dir,
        &effective_model,
        &effective_provider,
        resolved_reasoning,
        &effective_endpoint,
        &effective_endpoint_url,
        &effective_api_key,
        &vars,
        &task_exec,
        resume_session_id.as_deref(),
    )?;

    // Resolve effective timeout: CLI param > task.timeout > executor config > coordinator config.
    // Empty string means disabled.
    let effective_timeout_secs: Option<u64> = if let Some(t) = timeout {
        if t.is_empty() {
            None
        } else {
            Some(parse_timeout_secs(t).context("Invalid --timeout value")?)
        }
    } else if let Some(ref t) = task_timeout {
        if t.is_empty() {
            None
        } else {
            Some(parse_timeout_secs(t).context(format!(
                "Invalid task timeout value '{}'. \
                 Run `wg show {}` to inspect, then repair with \
                 `wg edit {} --timeout <30m|4h|1d>` or clear with \
                 `wg edit {} --timeout ''`.",
                t, task_id, task_id, task_id
            ))?)
        }
    } else if let Some(t) = settings.timeout {
        if t == 0 { None } else { Some(t) }
    } else {
        let agent_timeout = &config.coordinator.agent_timeout;
        if agent_timeout.is_empty() {
            None
        } else {
            Some(
                parse_timeout_secs(agent_timeout)
                    .context("Invalid coordinator.agent_timeout config")?,
            )
        }
    };

    // Build the actual command line, optionally wrapped with `timeout`.
    //
    // The timeout binary is resolved at runtime via $WG_TIMEOUT_BIN (set by
    // the wrapper header). macOS ships no GNU timeout — coreutils provides
    // it as `gtimeout`. When neither is available the wrapper warns and the
    // ${VAR:+...} expansion collapses to nothing, so the inner command runs
    // unwrapped instead of failing silently with empty stdin to the executor.
    let timed_command = if let Some(secs) = effective_timeout_secs {
        format!(
            r#"${{WG_TIMEOUT_BIN:+"$WG_TIMEOUT_BIN" --signal=TERM --kill-after=30 {} }}{}"#,
            secs, inner_command
        )
    } else {
        inner_command.clone()
    };
    let timed_fallback = fallback_command.map(|fb| {
        if let Some(secs) = effective_timeout_secs {
            format!(
                r#"${{WG_TIMEOUT_BIN:+"$WG_TIMEOUT_BIN" --signal=TERM --kill-after=30 {} }}{}"#,
                secs, fb
            )
        } else {
            fb
        }
    });

    // Create and write wrapper script
    let wrapper_path = write_wrapper_script(
        &output_dir,
        task_id,
        &output_file_str,
        &timed_command,
        effective_timeout_secs,
        &settings.executor_type,
        timed_fallback.as_deref(),
    )?;

    // Run the wrapper script. On Windows, `wrapper_path` often comes back
    // from PathBuf::canonicalize with the `\\?\` extended-length prefix
    // (e.g. `\\?\C:\src\ontempo\.workgraph\agents\agent-710\run.sh`). Most
    // Windows APIs accept that form, but Git-for-Windows' bash.exe does
    // not — it reports "No such file or directory" before the script can
    // run, and the agent dies instantly with no output.log. Strip the
    // prefix so bash sees a plain `C:\...` path, which it handles fine.
    // Resolve the bash binary via platform_bash so a stock Windows PATH
    // doesn't route us to the WSL shim (`C:\Windows\System32\bash.exe`).
    let bash_path = worksgood::platform_bash::bash_exe_path(config.bash.path.as_deref())
        .context("Failed to resolve bash executable for spawn wrapper")?;
    let mut cmd = Command::new(&bash_path);
    cmd.arg(strip_verbatim_prefix(&wrapper_path));

    // Set environment variables from executor config
    for (key, value) in &settings.env {
        cmd.env(key, value);
    }

    // Add task ID and agent ID to environment
    cmd.env("WG_TASK_ID", task_id);
    if let Some(chat_id) = worksgood::chat_id::parse_chat_task_id(task_id) {
        cmd.env("WG_CHAT_ID", task_id);
        cmd.env(
            "WG_CHAT_REF",
            worksgood::chat_id::format_chat_session_ref(chat_id),
        );
    }
    cmd.env("WG_AGENT_ID", &temp_agent_id);
    cmd.env("WG_EXECUTOR_TYPE", &settings.executor_type);
    // Time budget: inject timeout and spawn epoch for graceful completion
    if let Some(secs) = effective_timeout_secs {
        cmd.env("WG_TASK_TIMEOUT_SECS", secs.to_string());
    }
    cmd.env(
        "WG_SPAWN_EPOCH",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    );
    cmd.env("WG_SPAWN_RUN_ID", &spawn_run_id);
    cmd.env("WG_LAUNCH_GATE", output_dir.join(LAUNCH_GATE_FILE));
    cmd.env("WG_LAUNCH_TOKEN", &spawn_run_id);
    cmd.env("WG_LAUNCH_PARENT_PID", std::process::id().to_string());
    // Propagate user identity to spawned agents
    cmd.env("WG_USER", worksgood::current_user());
    if let Some(ref m) = effective_model {
        cmd.env("WG_MODEL", m);
    }
    if let Some(reasoning) = resolved_reasoning {
        cmd.env("WG_REASONING", reasoning.as_str());
    }
    {
        let tier_str =
            task_tier.as_deref().unwrap_or_else(
                || match worksgood::config::DispatchRole::TaskAgent.default_tier() {
                    worksgood::config::Tier::Fast => "fast",
                    worksgood::config::Tier::Standard => "standard",
                    worksgood::config::Tier::Premium => "premium",
                },
            );
        cmd.env("WG_TIER", tier_str);
    }
    if let Some(ref ep) = effective_endpoint {
        cmd.env("WG_ENDPOINT", ep);
        cmd.env("WG_ENDPOINT_NAME", ep);
    }
    if let Some(ref provider) = effective_provider {
        cmd.env("WG_LLM_PROVIDER", provider);
    }
    if let Some(ref url) = effective_endpoint_url {
        cmd.env("WG_ENDPOINT_URL", url);
    }
    inject_api_key_env(&mut cmd, endpoint_config, &effective_api_key);

    // Pass through the Claude Code OAuth token (if configured in [auth]
    // of config.toml and not already in the daemon's env). Task agents
    // shell out to the `claude` CLI for the claude executor; without
    // this, agents on a headless Windows install either can't
    // authenticate or the user has to export the token in every shell
    // before starting the daemon. No-op when `claude login` has been
    // run — the CLI picks up `~/.claude/credentials.json` on its own.
    if let Some(token) = config.auth.resolve_claude_oauth_token() {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }

    // Set working directory: worktree overrides settings.working_dir
    if let Some(ref wt) = worktree_info {
        // Strip `\\?\` verbatim prefix: bash.exe (MinGW/Git-for-Windows) silently
        // produces no output when its CWD is a verbatim extended-length path like
        // `\\?\C:\...`. Rust's PathBuf::canonicalize returns these on Windows.
        let clean_worktree_path = sanitize_bash_path(&wt.path.to_string_lossy());
        let clean_project_root = sanitize_bash_path(&wt.project_root.to_string_lossy());
        cmd.current_dir(&clean_worktree_path);
        cmd.env("WG_WORKTREE_PATH", &clean_worktree_path);
        cmd.env("WG_BRANCH", &wt.branch);
        cmd.env("WG_PROJECT_ROOT", &clean_project_root);
        // Signal to Claude Code (and other tools) that this session is already
        // inside a managed worktree — do not create a competing one.
        cmd.env("WG_WORKTREE_ACTIVE", "1");
    } else if let Some(ref wd) = settings.working_dir {
        cmd.current_dir(wd);
    }
    if let Some(path) = owned_target_path.as_ref() {
        // Isolate Cargo and make the exact absolute/temporary path explicit in
        // the ownership registry after the child PID identity is available.
        cmd.env("CARGO_TARGET_DIR", path);
    }
    if let Some(path) = owned_tmp_path.as_ref() {
        cmd.env("TMPDIR", path);
    }

    // Wrapper script handles output redirect internally
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    // Detach the agent into its own session so it survives daemon restart/crash.
    // setsid() creates a new session and process group, making the agent
    // independent of the daemon's process group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    // Windows equivalent: put the child at the root of its own process
    // group so console control events (Ctrl+Break, Ctrl+C, window-close)
    // that reach — or cascade through — the daemon's group don't also
    // terminate the task agent. The coordinator spawn in
    // `coordinator_agent::spawn_claude_process` already sets this flag
    // for the same reason; task agents need the same protection.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x0000_0200);
    }

    // Claim under the graph lock only after all fallible command/workspace
    // preparation. The closure re-checks status and assignment, closing the
    // stale-read race between concurrent dispatchers.
    let claim_snapshot = claim_task_for_spawn(&graph_path, task_id, &temp_agent_id)?;

    // The wrapper is spawned behind an unpublished launch gate. It cannot
    // start the handler until every durable transaction boundary succeeds.
    let mut child: Option<Child> = None;
    let mut registered_agent_id: Option<String> = None;
    let mut registered_caches = Vec::new();
    let launch_result = (|| -> Result<(String, u32)> {
        spawn_fault("claim")?;
        workspace.prepare_launch()?;
        if needs_worktree {
            let info = worktree_info.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "required isolation invariant lost for {} before launch",
                    temp_agent_id
                )
            })?;
            worktree::verify_worktree_info(info).with_context(|| {
                format!(
                    "REQUIRED ISOLATION verification failed for {} at {}; no handler launched",
                    temp_agent_id,
                    info.path.display()
                )
            })?;
        }

        child = Some(cmd.spawn().with_context(|| {
            format!(
                "failed to spawn gated executor '{}' (command: {})",
                resolved_executor_name, settings.command
            )
        })?);
        let pid = child.as_ref().expect("child assigned").id();
        #[cfg(test)]
        LAST_GATED_CHILD_PID.with(|recorded| recorded.set(pid));
        spawn_fault("wrapper-spawned")?;

        let agent_id = locked_registry.register_agent_with_model(
            pid,
            task_id,
            resolved_executor_name,
            &output_file_str,
            effective_model.as_deref(),
        );
        if agent_id != temp_agent_id {
            anyhow::bail!(
                "agent allocation changed inside locked spawn transaction: reserved {}, registry returned {}",
                temp_agent_id,
                agent_id
            );
        }
        registered_agent_id = Some(agent_id.clone());
        if let Some(ref worktree) = worktree_info {
            locked_registry.set_worktree_path(&agent_id, &worktree.path);
        }
        // Keep the registry lock held after the atomic write; rollback can
        // remove this exact entry without another dispatcher interleaving.
        locked_registry
            .save_ref()
            .context("failed to persist gated agent registry entry")?;
        spawn_fault("registry-saved")?;

        let lease_seconds = config
            .coordinator
            .resource_management
            .owned_cache_lease_seconds;
        let worktree_path = worktree_info
            .as_ref()
            .map(|worktree| worktree.path.as_path());
        if let Some(path) = owned_target_path.as_ref() {
            let cache = worksgood::disk_sentinel::make_owned_cache(
                path,
                worksgood::disk_sentinel::CacheKind::CargoTarget,
                task_id,
                &agent_id,
                pid,
                worktree_path,
                lease_seconds,
            );
            worksgood::disk_sentinel::register_owned_cache(dir, cache.clone())
                .context("failed to persist Cargo target ownership")?;
            registered_caches.push(cache);
        }
        if let Some(path) = owned_tmp_path.as_ref() {
            let cache = worksgood::disk_sentinel::make_owned_cache(
                path,
                worksgood::disk_sentinel::CacheKind::CargoInstallScratch,
                task_id,
                &agent_id,
                pid,
                worktree_path,
                lease_seconds,
            );
            worksgood::disk_sentinel::register_owned_cache(dir, cache.clone())
                .context("failed to persist build scratch ownership")?;
            registered_caches.push(cache);
        }
        spawn_fault("ownership-registered")?;

        let isolation_mode = if worktree_info.is_some() {
            "required-worktree"
        } else if !config.coordinator.worktree_isolation {
            "shared-explicitly-configured"
        } else {
            "shared-nonwriting-policy"
        };
        let metadata_path = output_dir.join("metadata.json");
        let mut metadata = serde_json::json!({
            "agent_id": agent_id,
            "pid": pid,
            "task_id": task_id,
            "executor": resolved_executor_name,
            "model": &effective_model,
            "reasoning": resolved_reasoning.map(|r| r.as_str()),
            "started_at": Utc::now().to_rfc3339(),
            "run_id": &spawn_run_id,
            "health_route": &health_route,
            "timeout_secs": effective_timeout_secs,
            "worktree_isolation_enabled": config.coordinator.worktree_isolation,
            "isolation_mode": isolation_mode,
        });
        if let Some(ref worktree) = worktree_info {
            metadata["worktree_path"] = serde_json::json!(worktree.path.to_string_lossy());
            metadata["worktree_branch"] = serde_json::json!(&worktree.branch);
            metadata["effective_cwd"] = serde_json::json!(worktree.path.to_string_lossy());
        } else if let Some(ref working_dir) = settings.working_dir {
            metadata["effective_cwd"] = serde_json::json!(working_dir);
        }
        metadata["owned_cache_paths"] = serde_json::json!(
            [owned_target_path.as_ref(), owned_tmp_path.as_ref()]
                .into_iter()
                .flatten()
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>()
        );
        fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?)
            .with_context(|| format!("failed to persist {}", metadata_path.display()))?;
        spawn_fault("metadata-written")?;

        // Re-verify after all setup and immediately before publishing the
        // permit. This is the last operation before the handler can execute.
        if let Some(ref worktree) = worktree_info {
            worktree::verify_worktree_info(worktree).with_context(|| {
                format!(
                    "REQUIRED ISOLATION changed before launch for {} at {}",
                    agent_id,
                    worktree.path.display()
                )
            })?;
        }
        spawn_fault("before-launch-permit")?;
        publish_launch_permit_for_claim(
            &graph_path,
            task_id,
            &agent_id,
            &output_dir,
            &spawn_run_id,
        )?;
        workspace.commit_after_launch();
        Ok((agent_id, pid))
    })();

    let (agent_id, pid) = match launch_result {
        Ok(result) => result,
        Err(error) => {
            if let Some(ref mut spawned) = child {
                kill_spawned_child(spawned);
            }
            let mut rollback_errors = Vec::new();
            if let Some(ref agent_id) = registered_agent_id {
                if let Err(rollback) =
                    worksgood::disk_sentinel::unregister_owned_caches(dir, &registered_caches)
                {
                    rollback_errors.push(format!("cache ownership: {rollback:#}"));
                }
                locked_registry.unregister_agent(agent_id);
                if let Err(rollback) = locked_registry.save_ref() {
                    rollback_errors.push(format!("agent registry: {rollback:#}"));
                }
            }
            if let Err(rollback) =
                rollback_task_claim(&graph_path, task_id, &temp_agent_id, &claim_snapshot)
            {
                rollback_errors.push(format!("task claim: {rollback:#}"));
            }
            return Err(error).with_context(|| {
                format!(
                    "spawn transaction for {} rolled back (task remains dispatchable; rollback diagnostics: {})",
                    temp_agent_id,
                    if rollback_errors.is_empty() {
                        "complete".to_string()
                    } else {
                        rollback_errors.join("; ")
                    }
                )
            });
        }
    };

    // The launch permit is the point of no return. Only now advance the new
    // agent's message cursor; an aborted gated attempt must not consume queued
    // messages. Audit records likewise follow the permit so a failed
    // transaction cannot leave a false "Spawned" log or assignment task.
    if let Ok(all_messages) = worksgood::messages::list_messages(dir, task_id)
        && let Some(last) = all_messages.last()
    {
        let _ = worksgood::messages::write_cursor(dir, &agent_id, task_id, last.id);
    }
    let task_id_for_audit = task_id.to_string();
    let agent_id_for_audit = agent_id.clone();
    let effective_model_for_audit = effective_model.clone();
    let model_warning_for_audit = model_validation_warning.clone();
    if let Err(error) = modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(&task_id_for_audit) else {
            return false;
        };
        if task.assigned.as_deref() != Some(agent_id_for_audit.as_str()) {
            return false;
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some(agent_id_for_audit.clone()),
            user: Some(worksgood::current_user()),
            message: format!(
                "Spawned by {} --executor {}{} --isolation {}",
                spawned_by,
                resolved_executor_name,
                effective_model_for_audit
                    .as_ref()
                    .map(|model| format!(" --model {model}"))
                    .unwrap_or_default(),
                if worktree_info.is_some() {
                    "required-worktree"
                } else if !config.coordinator.worktree_isolation {
                    "shared-explicitly-configured"
                } else {
                    "shared-nonwriting-policy"
                }
            ),
        });
        if let Some(ref warning) = model_warning_for_audit {
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("spawn".to_string()),
                user: None,
                message: format!("Pre-flight model validation: {warning}"),
            });
        }

        let assign_task_id = format!(".assign-{task_id_for_audit}");
        if !is_system_task(&task_id_for_audit) && graph.get_task(&assign_task_id).is_none() {
            let now = Utc::now().to_rfc3339();
            let description = task_agent_for_audit.as_ref().map_or_else(
                || format!(
                    "Direct dispatch: '{}'\nNo agent pre-assigned (auto_assign disabled or skipped)",
                    task_id_for_audit
                ),
                |agency_agent| format!(
                    "Direct dispatch: agent={} → '{}'\nNo lightweight assignment flow (auto_assign disabled or skipped)",
                    agency_agent, task_id_for_audit
                ),
            );
            graph.add_node(Node::Task(Task {
                id: assign_task_id,
                title: format!("Assign agent for: {task_title_for_audit}"),
                description: Some(description),
                status: Status::Done,
                before: vec![task_id_for_audit.clone()],
                tags: vec!["assignment".to_string(), "agency".to_string()],
                created_at: Some(now.clone()),
                started_at: Some(now.clone()),
                completed_at: Some(now),
                exec_mode: Some("bare".to_string()),
                visibility: "internal".to_string(),
                log: vec![LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: "Created at committed spawn time (no prior .assign-* task existed)"
                        .to_string(),
                }],
                ..Default::default()
            }));
        }
        true
    }) {
        eprintln!(
            "[spawn] WARNING: agent {} launched but spawn audit could not be appended: {}",
            agent_id, error
        );
    }

    Ok(SpawnResult {
        agent_id,
        pid,
        task_id: task_id.to_string(),
        executor: resolved_executor_name.to_string(),
        executor_type: settings.executor_type.clone(),
        output_file: output_file_str,
        model: effective_model,
        reasoning: resolved_reasoning.map(|r| r.to_string()),
    })
}

/// Decide whether a spawning agent should get its own git worktree.
///
/// Worktrees provide file-level isolation for agents that may edit source code.
/// They are skipped for tasks that don't touch the source tree:
///
/// - **System/meta tasks** (`.assign-*`, `.flip-*`, `.evaluate-*`, `.place-*`,
///   `.compact-*`, etc.) only call LLMs and mutate graph state — they never
///   write to the filesystem. Identified by the leading `.` prefix convention
///   documented in `graph.rs`.
/// - **`bare` exec mode** — coordination-only tasks with just the `wg` CLI;
///   cannot write to the source tree.
/// - **`light` exec mode** — read-only tools for research/review tasks;
///   cannot write to the source tree.
///
/// Code-touching tasks (`full`, `shell`) still get isolated worktrees.
pub(crate) fn should_create_worktree(
    worktree_isolation_enabled: bool,
    task_id: &str,
    exec_mode: &str,
) -> bool {
    if !worktree_isolation_enabled {
        return false;
    }
    if task_id.starts_with('.') {
        return false;
    }
    if matches!(exec_mode, "bare" | "light") {
        return false;
    }
    true
}

/// Built-in executors that ship without a `prompt_template` and rely on
/// `build_prompt()` to assemble the agent prompt at spawn time.
///
/// CLI handlers (claude, codex) and in-process handlers (native) all need
/// the same WG-context preamble, so the spawn pipeline auto-builds a
/// `PromptTemplate` for any of these when the user hasn't supplied one in
/// their executor config. Adding a new built-in handler without listing it
/// here means the spawn writes no prompt.txt and the resulting subprocess
/// receives empty stdin — exactly the codex bug.
fn executor_uses_auto_prompt(executor_type: &str) -> bool {
    matches!(
        executor_type,
        "claude"
            | "codex"
            | "native"
            | "opencode"
            | "aider"
            | "goose"
            | "qwen"
            | "qwen-code"
            | "qwen_code"
            | "cline"
            | "crush"
            | "amplifier"
            | "octomind"
            | "dexto"
            | "pi"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalCliModelStyle {
    /// `--model openrouter/<model-id>`
    ProviderSlashModel,
    /// `--provider openrouter --model <model-id>`
    ProviderFlagAndModel,
    /// `--model <model-id>` after the CLI has been configured for OpenRouter.
    BareOpenRouterModel,
    /// `--model openrouter:<model-id>` — the provider-colon-route spelling
    /// Octomind's `-m`/`--model` flag consumes (e.g.
    /// `--model openrouter:minimax/minimax-m3`).
    ProviderColonModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ExternalCliModelArgs {
    provider: Option<(&'static str, String)>,
    model: Option<(&'static str, String)>,
}

impl ExternalCliModelArgs {
    #[cfg(test)]
    fn to_vec(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some((flag, value)) = &self.provider {
            args.push((*flag).to_string());
            args.push(value.clone());
        }
        if let Some((flag, value)) = &self.model {
            args.push((*flag).to_string());
            args.push(value.clone());
        }
        args
    }
}

fn external_cli_model_style(executor_type: &str) -> Option<ExternalCliModelStyle> {
    match executor_type {
        "opencode" | "aider" | "crush" => Some(ExternalCliModelStyle::ProviderSlashModel),
        "goose" | "cline" | "pi" => Some(ExternalCliModelStyle::ProviderFlagAndModel),
        "qwen" | "qwen-code" | "qwen_code" => Some(ExternalCliModelStyle::BareOpenRouterModel),
        // Octomind's `-m` takes WG's `openrouter:<vendor>/<model>` spelling.
        // (Dexto is intentionally absent: its CLI rejects provider/model
        // routes and requires a generated agent YAML, so it has no worker-path
        // argv model style — prototype-octomind-dexto-chat.)
        "octomind" => Some(ExternalCliModelStyle::ProviderColonModel),
        _ => None,
    }
}

fn openrouter_model_id(model: &str, effective_provider: Option<&str>) -> Option<String> {
    let spec = worksgood::config::parse_model_spec(model);
    let provider_from_model = spec
        .provider
        .as_deref()
        .map(worksgood::config::provider_to_native_provider);
    let provider = effective_provider.or(provider_from_model);
    if provider == Some("openrouter") {
        Some(
            spec.model_id
                .strip_prefix("openrouter/")
                .unwrap_or(&spec.model_id)
                .to_string(),
        )
    } else {
        None
    }
}

fn external_cli_model_args(
    executor_type: &str,
    effective_model: Option<&str>,
    effective_provider: Option<&str>,
) -> ExternalCliModelArgs {
    let Some(model) = effective_model else {
        return ExternalCliModelArgs::default();
    };
    let Some(style) = external_cli_model_style(executor_type) else {
        return ExternalCliModelArgs::default();
    };

    if executor_type == "pi" {
        let inner = model.strip_prefix("pi:").unwrap_or(model).trim();
        if let Some((provider, model_id)) = inner.split_once(':').or_else(|| inner.split_once('/'))
        {
            let provider = provider.trim();
            let model_id = model_id.trim();
            if !provider.is_empty() && !model_id.is_empty() {
                return ExternalCliModelArgs {
                    provider: Some(("--provider", provider.to_string())),
                    model: Some(("--model", model_id.to_string())),
                };
            }
        }
    }

    if let Some(openrouter_model) = openrouter_model_id(model, effective_provider) {
        return match style {
            ExternalCliModelStyle::ProviderSlashModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", format!("openrouter/{}", openrouter_model))),
            },
            ExternalCliModelStyle::ProviderFlagAndModel => ExternalCliModelArgs {
                provider: Some(("--provider", "openrouter".to_string())),
                model: Some(("--model", openrouter_model)),
            },
            ExternalCliModelStyle::BareOpenRouterModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", openrouter_model)),
            },
            ExternalCliModelStyle::ProviderColonModel => ExternalCliModelArgs {
                provider: None,
                model: Some(("--model", format!("openrouter:{}", openrouter_model))),
            },
        };
    }

    ExternalCliModelArgs {
        provider: effective_provider.map(|p| ("--provider", p.to_string())),
        model: Some(("--model", model.to_string())),
    }
}

fn model_template_value_for_executor(
    executor_type: &str,
    effective_model: Option<&str>,
    effective_provider: Option<&str>,
) -> Option<String> {
    let model = effective_model?;
    let style = external_cli_model_style(executor_type)?;
    let openrouter_model = openrouter_model_id(model, effective_provider)?;
    Some(match style {
        ExternalCliModelStyle::ProviderSlashModel => format!("openrouter/{}", openrouter_model),
        ExternalCliModelStyle::ProviderColonModel => format!("openrouter:{}", openrouter_model),
        ExternalCliModelStyle::ProviderFlagAndModel
        | ExternalCliModelStyle::BareOpenRouterModel => openrouter_model,
    })
}

fn args_have_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| {
        flags.iter().any(|flag| {
            arg == flag
                || arg
                    .strip_prefix(flag)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

fn append_external_cli_model_args(
    cmd_parts: &mut Vec<String>,
    existing_args: &[String],
    model_args: ExternalCliModelArgs,
) {
    if let Some((flag, value)) = model_args.provider
        && !args_have_flag(existing_args, &["--provider", "-P"])
    {
        cmd_parts.push(flag.to_string());
        cmd_parts.push(shell_escape(&value));
    }
    if let Some((flag, value)) = model_args.model
        && !args_have_flag(existing_args, &["--model", "-m"])
    {
        cmd_parts.push(flag.to_string());
        cmd_parts.push(shell_escape(&value));
    }
}

fn args_have_codex_config(existing_args: &[String], key: &str) -> bool {
    existing_args.iter().any(|arg| {
        arg.trim_matches(['\'', '"'])
            .split_once('=')
            .is_some_and(|(configured_key, _)| configured_key.trim() == key)
    })
}

fn append_external_cli_reasoning_args(
    cmd_parts: &mut Vec<String>,
    existing_args: &[String],
    executor_type: &str,
    reasoning: Option<ReasoningLevel>,
) {
    let Some(level) = reasoning else {
        return;
    };
    match executor_type {
        "pi" if !args_have_flag(existing_args, &["--thinking"]) => {
            // Pi owns the WG vocabulary and accepts it verbatim.
            cmd_parts.push("--thinking".to_string());
            cmd_parts.push(shell_escape(level.as_str()));
        }
        "codex" if !args_have_codex_config(existing_args, "model_reasoning_effort") => {
            // Codex reasoning effort is a config override. Keep it independent
            // from the separately configured `model_verbosity` setting.
            cmd_parts.push("-c".to_string());
            cmd_parts.push(shell_escape(&format!(
                "model_reasoning_effort=\"{}\"",
                level.as_codex_effort()
            )));
        }
        _ => {}
    }
}

fn write_executor_prompt_file(
    output_dir: &Path,
    settings: &worksgood::service::executor::ExecutorSettings,
) -> Result<std::path::PathBuf> {
    let prompt_content = settings
        .prompt_template
        .as_ref()
        .map(|pt| pt.template.clone())
        .unwrap_or_default();
    let prompt_file = output_dir.join("prompt.txt");
    fs::write(&prompt_file, &prompt_content)
        .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
    Ok(prompt_file)
}

fn external_prompt_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    resolved_reasoning: Option<ReasoningLevel>,
    delivery: ExternalPromptDelivery,
) -> Result<String> {
    // Explicit-model contract: external CLIs that take a `--model` flag MUST
    // receive an explicitly resolved model. Running them with no model means
    // silently inheriting the CLI's own internal default — exactly the
    // "ran on the wrong model" class of bug WG forbids. If model resolution
    // yielded nothing AND the user hasn't hard-coded a model in
    // `[executor].args`, fail loudly instead of falling back. (Amplifier has
    // no model style and is exempt — it manages its own model.)
    if external_cli_model_style(&settings.executor_type).is_some()
        && effective_model.is_none()
        && !args_have_flag(&settings.args, &["--model", "-m"])
    {
        anyhow::bail!(
            "executor '{}' requires an explicitly resolved model, but model resolution \
             produced none. Set a model on the task (`-m {}:openrouter/<vendor>/<model>`), \
             the active profile, or `[agent].model` — WG will not fall back to the CLI's \
             internal default.",
            settings.executor_type,
            settings.executor_type,
        );
    }
    let prompt_file = write_executor_prompt_file(output_dir, settings)?;
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    for arg in &settings.args {
        cmd_parts.push(shell_escape(arg));
    }
    append_external_cli_model_args(
        &mut cmd_parts,
        &settings.args,
        external_cli_model_args(
            &settings.executor_type,
            effective_model.as_deref(),
            effective_provider.as_deref(),
        ),
    );
    append_external_cli_reasoning_args(
        &mut cmd_parts,
        &settings.args,
        &settings.executor_type,
        resolved_reasoning,
    );

    match delivery {
        ExternalPromptDelivery::OpenCodeFile => {
            cmd_parts.push(shell_escape("Complete the attached WG task prompt."));
            if !args_have_flag(&settings.args, &["--file", "-f", "--attach"]) {
                cmd_parts.push("--file".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::AiderMessageFile => {
            if !args_have_flag(&settings.args, &["--message-file", "--message"]) {
                cmd_parts.push("--message-file".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::GooseInputFile => {
            if !args_have_flag(
                &settings.args,
                &["-i", "--input", "--input-file", "-t", "--text"],
            ) {
                cmd_parts.push("-i".to_string());
                cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            }
            Ok(cmd_parts.join(" "))
        }
        ExternalPromptDelivery::QwenPromptAndStdin => {
            if !args_have_flag(&settings.args, &["--prompt", "-p"]) {
                cmd_parts.push("--prompt".to_string());
                cmd_parts.push(shell_escape(
                    "Complete the WG task prompt supplied on stdin.",
                ));
            }
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::ClinePositionalPromptAndStdin => {
            cmd_parts.push(shell_escape(
                "Complete the WG task prompt supplied on stdin.",
            ));
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::Stdin => {
            let command = cmd_parts.join(" ");
            Ok(prompt_file_command(
                &prompt_file.to_string_lossy(),
                &command,
            ))
        }
        ExternalPromptDelivery::Argument => Ok(prompt_file_as_last_argument_command(
            &prompt_file.to_string_lossy(),
            &cmd_parts,
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalPromptDelivery {
    OpenCodeFile,
    AiderMessageFile,
    GooseInputFile,
    QwenPromptAndStdin,
    ClinePositionalPromptAndStdin,
    Stdin,
    Argument,
}

fn prompt_file_as_last_argument_command(prompt_file: &str, cmd_parts: &[String]) -> String {
    let mut parts = vec![
        "bash".to_string(),
        "-c".to_string(),
        shell_escape(r#"PROMPT=$(cat "$1"); shift; exec "$@" "$PROMPT""#),
        "--".to_string(),
        shell_escape(prompt_file),
    ];
    parts.extend(cmd_parts.iter().cloned());
    parts.join(" ")
}

fn preflight_executor_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    executor_name: &str,
    working_dir: Option<&Path>,
) -> Result<()> {
    let command = settings.command.trim();
    if command.is_empty() {
        anyhow::bail!(
            "Executor '{}' has an empty command in .wg/executors/{}.toml. \
             Set [executor].command to an installed binary and put flags in [executor].args.",
            executor_name,
            executor_name,
        );
    }

    if command_contains_path_separator(command) {
        let command_path = Path::new(command);
        let candidate = if command_path.is_absolute() {
            command_path.to_path_buf()
        } else if let Some(wd) = working_dir {
            wd.join(command_path)
        } else {
            command_path.to_path_buf()
        };
        if is_executable_file(&candidate) {
            return Ok(());
        }
        anyhow::bail!(
            "Executor '{}' command '{}' is not an executable file at '{}'. \
             Check .wg/executors/{}.toml, install the binary, or set an absolute command path.{}",
            executor_name,
            command,
            candidate.display(),
            executor_name,
            executor_setup_hint(executor_name),
        );
    }

    if which_on_path(command).is_some() {
        return Ok(());
    }

    anyhow::bail!(
        "Executor '{}' command '{}' was not found on PATH. \
         Install the '{}' binary, put it on PATH, or set [executor].command in .wg/executors/{}.toml.{}",
        executor_name,
        command,
        command,
        executor_name,
        executor_setup_hint(executor_name),
    );
}

fn executor_setup_hint(executor_name: &str) -> &'static str {
    match executor_name {
        "amplifier" => {
            " Expected default command: amplifier run --mode single --output-format json --bundle wg <prompt>."
        }
        "crush" => {
            " The built-in Crush surface is experimental; verify `crush run --help` for your installed version or override the template."
        }
        _ => "",
    }
}

fn command_contains_path_separator(command: &str) -> bool {
    command.contains('/') || command.contains('\\')
}

fn which_on_path(command: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn inject_api_key_env(
    cmd: &mut Command,
    endpoint_config: Option<&EndpointConfig>,
    effective_api_key: &Option<String>,
) {
    if let Some(key) = effective_api_key {
        cmd.env("WG_API_KEY", key);
        // Also set the provider-specific env var (e.g. OPENROUTER_API_KEY)
        // so external CLIs can discover the key through their standard
        // environment without putting secrets in argv or run.sh.
        if let Some(ep) = endpoint_config {
            for var_name in EndpointConfig::env_var_names_for_provider(&ep.provider) {
                cmd.env(var_name, key);
            }
        }
    }
}

/// Build the inner command string for the executor.
///
/// Returns `(primary_command, Option<fallback_command>)`. The fallback is
/// provided when the primary attempts session resume — if the session no
/// longer exists, the wrapper can fall back to a fresh session.
#[allow(clippy::too_many_arguments)]
fn build_inner_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    effective_endpoint: &Option<String>,
    effective_endpoint_url: &Option<String>,
    effective_api_key: &Option<String>,
    vars: &TemplateVars,
    task_exec: &Option<String>,
    resume_session_id: Option<&str>,
) -> Result<(String, Option<String>)> {
    build_inner_command_with_reasoning(
        settings,
        exec_mode,
        output_dir,
        effective_model,
        effective_provider,
        None,
        effective_endpoint,
        effective_endpoint_url,
        effective_api_key,
        vars,
        task_exec,
        resume_session_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_inner_command_with_reasoning(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    effective_provider: &Option<String>,
    resolved_reasoning: Option<ReasoningLevel>,
    effective_endpoint: &Option<String>,
    effective_endpoint_url: &Option<String>,
    effective_api_key: &Option<String>,
    vars: &TemplateVars,
    task_exec: &Option<String>,
    resume_session_id: Option<&str>,
) -> Result<(String, Option<String>)> {
    let inner_command = match settings.executor_type.as_str() {
        "claude" if resume_session_id.is_some() && exec_mode != "bare" => {
            // Resume mode: use --resume <session_id> with checkpoint as follow-up message
            let session_id = resume_session_id.unwrap();
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--resume".to_string());
            cmd_parts.push(shell_escape(session_id));
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            // Write the resume context (checkpoint) as the follow-up message
            let resume_msg = vars.task_context.clone();
            let resume_file = output_dir.join("resume_message.txt");
            fs::write(&resume_file, &resume_msg)
                .with_context(|| format!("Failed to write resume message: {:?}", resume_file))?;
            let resume_command = prompt_file_command(&resume_file.to_string_lossy(), &claude_cmd);

            // Build a fresh-session fallback command (same as the full-mode
            // "claude" arm below) so the wrapper can retry if the session is
            // gone. Write prompt.txt alongside resume_message.txt.
            let fallback =
                build_claude_fresh_command(settings, exec_mode, output_dir, effective_model, vars)?;

            return Ok((resume_command, Some(fallback)));
        }
        "claude" if exec_mode == "bare" => {
            // Bare mode: lightweight execution with --system-prompt and no tools.
            // Used for pure-reasoning tasks (synthesis, triage, summarization).
            // The prompt is passed via --system-prompt and stdin provides the task input.
            let prompt_file = output_dir.join("prompt.txt");
            let prompt_content = settings
                .prompt_template
                .as_ref()
                .map(|pt| pt.template.clone())
                .unwrap_or_default();
            fs::write(&prompt_file, &prompt_content)
                .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;

            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--tools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*)"));
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*)"));
            cmd_parts.push("--disable-slash-commands".to_string());
            cmd_parts.push("--system-prompt".to_string());
            cmd_parts.push(shell_escape(&prompt_content));
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            // In bare mode, pipe the task title+description as the user message
            let user_message = format!(
                "Complete this task:\n\nTitle: {}\n\n{}",
                vars.task_id, vars.task_description
            );
            let user_msg_file = output_dir.join("user_message.txt");
            fs::write(&user_msg_file, &user_message).with_context(|| {
                format!("Failed to write user message file: {:?}", user_msg_file)
            })?;
            prompt_file_command(&user_msg_file.to_string_lossy(), &claude_cmd)
        }
        "claude" if exec_mode == "light" => {
            // Light mode: read-only file access + wg CLI tools.
            // Used for research, code review, exploration, analysis tasks.
            // Standard prompt-via-stdin flow with --allowedTools restriction.
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch"));
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape(
                "Edit,Write,NotebookEdit,Agent,EnterWorktree,ExitWorktree",
            ));

            cmd_parts.push("--disable-slash-commands".to_string());
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                // Write prompt to file for safe passing
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
            } else {
                claude_cmd
            }
        }
        "claude" => {
            // Full mode: standard Claude Code session with all tools
            // Write prompt to file and pipe to claude - avoids all quoting issues
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            // Prevent agents from spawning sub-agents outside WG
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));

            cmd_parts.push("--disable-slash-commands".to_string());
            // Add model flag if specified
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                // Write prompt to file for safe passing
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
            } else {
                claude_cmd
            }
        }
        "codex" => {
            // Codex runs non-interactively via `codex exec`, reading the prompt from stdin.
            // We keep this aligned with the Claude single-shot flow: write the assembled
            // prompt to disk, pipe it in, and let the wrapper capture JSONL output.
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            append_external_cli_reasoning_args(
                &mut cmd_parts,
                &settings.args,
                "codex",
                resolved_reasoning,
            );
            let codex_cmd = cmd_parts.join(" ");

            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                prompt_file_command(&prompt_file.to_string_lossy(), &codex_cmd)
            } else {
                codex_cmd
            }
        }
        "native" => {
            // Native executor: runs the agent loop in-process via `wg native-exec`.
            // Prompt is written to a file and passed as an argument. The bundle is
            // resolved from exec_mode by the native-exec subcommand.
            let prompt_content = settings
                .prompt_template
                .as_ref()
                .map(|pt| pt.template.clone())
                .unwrap_or_default();
            let prompt_file = output_dir.join("prompt.txt");
            fs::write(&prompt_file, &prompt_content)
                .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;

            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("native-exec".to_string());
            cmd_parts.push("--prompt-file".to_string());
            cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
            cmd_parts.push("--exec-mode".to_string());
            cmd_parts.push(shell_escape(exec_mode));
            cmd_parts.push("--task-id".to_string());
            cmd_parts.push(shell_escape(&vars.task_id));
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            if let Some(p) = effective_provider {
                cmd_parts.push("--provider".to_string());
                cmd_parts.push(shell_escape(p));
            }
            if let Some(ep) = effective_endpoint {
                cmd_parts.push("--endpoint-name".to_string());
                cmd_parts.push(shell_escape(ep));
            }
            if let Some(url) = effective_endpoint_url {
                cmd_parts.push("--endpoint-url".to_string());
                cmd_parts.push(shell_escape(url));
            }
            if let Some(key) = effective_api_key {
                cmd_parts.push("--api-key".to_string());
                cmd_parts.push(shell_escape(key));
            }
            cmd_parts.join(" ")
        }
        "opencode" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::OpenCodeFile,
        )?,
        "aider" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::AiderMessageFile,
        )?,
        "goose" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::GooseInputFile,
        )?,
        "qwen" | "qwen-code" | "qwen_code" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::QwenPromptAndStdin,
        )?,
        "cline" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::ClinePositionalPromptAndStdin,
        )?,
        "crush" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::Stdin,
        )?,
        "pi" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::QwenPromptAndStdin,
        )?,
        "amplifier" => external_prompt_command(
            settings,
            output_dir,
            effective_model,
            effective_provider,
            resolved_reasoning,
            ExternalPromptDelivery::Argument,
        )?,
        "shell" => {
            format!(
                "{} -c {}",
                shell_escape(&settings.command),
                shell_escape(task_exec.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("shell executor requires task exec command")
                })?)
            )
        }
        _ => {
            let mut parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                parts.push(shell_escape(arg));
            }
            parts.join(" ")
        }
    };
    Ok((inner_command, None))
}

/// Build the fresh (non-resume) claude command for fallback.
/// Mirrors the `"claude"` full-mode arm in `build_inner_command`.
fn build_claude_fresh_command(
    settings: &worksgood::service::executor::ExecutorSettings,
    exec_mode: &str,
    output_dir: &Path,
    effective_model: &Option<String>,
    _vars: &TemplateVars,
) -> Result<String> {
    match exec_mode {
        "light" => {
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            cmd_parts.push("--print".to_string());
            cmd_parts.push("--verbose".to_string());
            cmd_parts.push("--output-format".to_string());
            cmd_parts.push("stream-json".to_string());
            cmd_parts.push("--dangerously-skip-permissions".to_string());
            cmd_parts.push("--allowedTools".to_string());
            cmd_parts.push(shell_escape("Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch"));
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape(
                "Edit,Write,NotebookEdit,Agent,EnterWorktree,ExitWorktree",
            ));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");
            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                Ok(prompt_file_command(
                    &prompt_file.to_string_lossy(),
                    &claude_cmd,
                ))
            } else {
                Ok(claude_cmd)
            }
        }
        _ => {
            // Full mode
            let mut cmd_parts = vec![shell_escape(&settings.command)];
            for arg in &settings.args {
                cmd_parts.push(shell_escape(arg));
            }
            cmd_parts.push("--disallowedTools".to_string());
            cmd_parts.push(shell_escape("Agent,EnterWorktree,ExitWorktree"));
            cmd_parts.push("--disable-slash-commands".to_string());
            if let Some(m) = effective_model {
                cmd_parts.push("--model".to_string());
                cmd_parts.push(shell_escape(m));
            }
            let claude_cmd = cmd_parts.join(" ");
            if let Some(ref prompt_template) = settings.prompt_template {
                let prompt_file = output_dir.join("prompt.txt");
                fs::write(&prompt_file, &prompt_template.template)
                    .with_context(|| format!("Failed to write prompt file: {:?}", prompt_file))?;
                Ok(prompt_file_command(
                    &prompt_file.to_string_lossy(),
                    &claude_cmd,
                ))
            } else {
                Ok(claude_cmd)
            }
        }
    }
}

/// Create and write the wrapper shell script that runs the agent command
/// and handles completion/failure.
///
/// When `fallback_command` is provided (session resume mode), the wrapper
/// detects "No conversation found" errors and retries with a fresh session.
fn write_wrapper_script(
    output_dir: &Path,
    task_id: &str,
    output_file_str: &str,
    timed_command: &str,
    effective_timeout_secs: Option<u64>,
    executor_type: &str,
    fallback_command: Option<&str>,
) -> Result<std::path::PathBuf> {
    let complete_cmd = "wg done \"$TASK_ID\" 2>> \"$OUTPUT_FILE\" || echo \"[wrapper] WARNING: 'wg done' failed with exit code $?\" >> \"$OUTPUT_FILE\"".to_string();
    let complete_msg = "[wrapper] Agent exited successfully, marking task done";

    let timeout_note = if let Some(secs) = effective_timeout_secs {
        format!(
            r#"
# Hard timeout: {secs}s (SIGTERM, then SIGKILL after 30s).
# Resolve a GNU `timeout` binary. macOS ships no GNU timeout — coreutils
# provides it as `gtimeout`. Linux distros usually have `timeout` directly.
# When neither is available we warn and the ${{WG_TIMEOUT_BIN:+...}} expansion
# in the run command collapses to nothing, so the executor still runs (just
# without a hard timeout). Without this probe a missing `timeout` would
# silently break the pipeline and pipe empty stdin into the executor.
WG_TIMEOUT_BIN="$(command -v gtimeout 2>/dev/null || command -v timeout 2>/dev/null || true)"
if [ -z "$WG_TIMEOUT_BIN" ]; then
    echo "[wrapper] WARNING: GNU timeout not found (install coreutils on macOS: 'brew install coreutils'); running without hard timeout" >> "$OUTPUT_FILE"
fi
"#,
            secs = secs
        )
    } else {
        String::new()
    };

    // Pass debug environment variables to spawned subprocesses
    let debug_env_vars = if std::env::var("WG_DEBUG_PROMPTS").is_ok() {
        "export WG_DEBUG_PROMPTS=1\n".to_string()
    } else {
        String::new()
    };

    let stream_file = output_dir.join("stream.jsonl");
    let stream_file_str = stream_file.to_string_lossy().to_string();

    // For Claude executor: split stdout (JSONL) to raw_stream.jsonl, stderr to output.log.
    // Also tee stdout to output.log for backward compatibility.
    // For native: the agent loop writes stream.jsonl directly; wrapper just adds bookends.
    // For shell/other: wrapper emits Init+Result bookend events.
    let (run_command, fallback_run_command, stream_init, stream_result) = match executor_type {
        "claude" | "codex" => {
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            let raw_str = raw_stream_file.to_string_lossy().to_string();
            // Capture JSONL stdout to raw_stream.jsonl and also copy to output.log.
            // stderr goes to output.log only. `tee -a <path>` opens the file
            // itself and fails on `\\?\C:\...` verbatim paths, so sanitize.
            let cmd = format!(
                "{timed_command} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                timed_command = timed_command,
                raw = shell_escape(&sanitize_bash_path(&raw_str)),
            );
            let fb_cmd = fallback_command.map(|fb| {
                format!(
                    "{fb} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                    fb = fb,
                    raw = shell_escape(&raw_str),
                )
            });
            (cmd, fb_cmd, String::new(), String::new())
        }
        "native" => {
            // Native executor writes stream.jsonl itself; wrapper just runs the command.
            let cmd = format!(
                "{timed_command} >> \"$OUTPUT_FILE\" 2>&1",
                timed_command = timed_command,
            );
            (cmd, None, String::new(), String::new())
        }
        "pi" => {
            // Pi (`pi --mode json`) emits NDJSON on stdout. Capture it to
            // raw_stream.jsonl (so the TUI events pane can render per-step
            // events live, like claude/codex) and tee to output.log; stderr
            // -> output.log. After pi exits, `wg pi-stream-bridge` reads that
            // NDJSON and writes the canonical stream.jsonl with REAL summed
            // token/cost usage (replacing the old 0/0 bookend) + a session
            // summary. No bash-emitted bookend — the bridge owns stream.jsonl.
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            let raw_str = raw_stream_file.to_string_lossy().to_string();
            let cmd = format!(
                "{timed_command} > >(tee -a {raw} >> \"$OUTPUT_FILE\") 2>> \"$OUTPUT_FILE\"",
                timed_command = timed_command,
                raw = shell_escape(&raw_str),
            );
            let bridge = format!(
                "wg pi-stream-bridge --agent-dir {dir} --exit-code $EXIT_CODE 2>> \"$OUTPUT_FILE\" \
                 || echo \"[wrapper] WARNING: 'wg pi-stream-bridge' failed with exit code $?\" >> \"$OUTPUT_FILE\"",
                dir = shell_escape(&output_dir.to_string_lossy()),
            );
            (cmd, None, String::new(), bridge)
        }
        _ => {
            // Shell and custom executors: wrapper writes bookend events.
            let cmd = format!(
                "{timed_command} >> \"$OUTPUT_FILE\" 2>&1",
                timed_command = timed_command,
            );
            let ts_cmd = "date +%s%3N"; // milliseconds since epoch
            let init = format!(
                "echo '{{\"type\":\"init\",\"executor_type\":\"{etype}\",\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                etype = executor_type,
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_ok = format!(
                "echo '{{\"type\":\"result\",\"success\":true,\"usage\":{{\"input_tokens\":0,\"output_tokens\":0}},\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_fail = format!(
                "echo '{{\"type\":\"result\",\"success\":false,\"usage\":{{\"input_tokens\":0,\"output_tokens\":0}},\"timestamp_ms\":'$({ts})'}}' >> {sf}",
                ts = ts_cmd,
                sf = shell_escape(&stream_file_str),
            );
            let result_block = format!(
                "if [ $EXIT_CODE -eq 0 ]; then\n    {result_ok}\nelse\n    {result_fail}\nfi",
                result_ok = result_ok,
                result_fail = result_fail,
            );
            (cmd, None, init, result_block)
        }
    };

    // Raw stream path for the failure classifier. claude/codex/pi write this file.
    let raw_stream_shell_var = match executor_type {
        "claude" | "codex" | "pi" => {
            let raw_stream_file = output_dir.join("raw_stream.jsonl");
            format!(
                "RAW_STREAM={}",
                shell_escape(&raw_stream_file.to_string_lossy())
            )
        }
        _ => "RAW_STREAM=".to_string(),
    };

    // Session resume fallback block: when the primary command tried to resume
    // a stale session (e.g., claude --resume <uuid>), detect the error and
    // retry with a fresh session.
    let session_fallback_block = if let Some(ref fb_cmd) = fallback_run_command {
        format!(
            r#"
# Session resume fallback: if the session no longer exists, start fresh
if [ $EXIT_CODE -ne 0 ]; then
    if grep -qE "No conversation found|session.*not found|invalid session|Could not resume" "$OUTPUT_FILE" 2>/dev/null; then
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Session not resumable, starting fresh session" >> "$OUTPUT_FILE"
        wg log "$TASK_ID" "session not resumable, falling back to fresh session" 2>/dev/null || true
        # The heartbeat guard writer belongs only to this wrapper. Close it
        # around the fallback executor exactly as for the primary executor, so
        # wrapper death produces immediate EOF even while the fallback lives.
        {{
            {fallback_run_command}
        }} {{HEARTBEAT_GUARD_FD}}>&-
        EXIT_CODE=$?
    fi
fi
"#,
            fallback_run_command = fb_cmd,
        )
    } else {
        String::new()
    };

    let wrapper_script = format!(
        r#"#!/bin/bash
TASK_ID={escaped_task_id}
OUTPUT_FILE={escaped_output_file}
{raw_stream_shell_var}

# Transactional launch gate. The wrapper exists so WG can obtain a PID, but
# no handler/worker command is allowed to start until graph claim, registry,
# ownership records, metadata, and final isolation verification are durable.
if [ -n "${{WG_LAUNCH_GATE:-}}" ]; then
    WG_GATE_WAITS=0
    while [ ! -f "$WG_LAUNCH_GATE" ]; do
        if [ -n "${{WG_LAUNCH_PARENT_PID:-}}" ] && ! kill -0 "$WG_LAUNCH_PARENT_PID" 2>/dev/null; then
            exit 125
        fi
        WG_GATE_WAITS=$((WG_GATE_WAITS + 1))
        if [ "$WG_GATE_WAITS" -ge 3000 ]; then
            exit 125
        fi
        sleep 0.02
    done
    WG_GATE_VALUE=$(cat "$WG_LAUNCH_GATE" 2>/dev/null || true)
    if [ "$WG_GATE_VALUE" != "${{WG_LAUNCH_TOKEN:-}}" ]; then
        exit 125
    fi
    rm -f "$WG_LAUNCH_GATE" 2>/dev/null || true
    unset WG_LAUNCH_GATE WG_LAUNCH_TOKEN WG_LAUNCH_PARENT_PID
fi

# Allow nested Claude Code sessions (spawned agents are independent).
# The MANAGED_BY_HOST / SDK_HAS_OAUTH_REFRESH vars in particular leak
# through when the daemon was launched from inside a Claude Code
# session and make the spawned claude CLI prefer an inaccessible host
# bridge over the configured token.
unset CLAUDECODE
unset CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST
unset CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH
{timeout_note}
{debug_env_vars}
{stream_init}
# Guarded heartbeat watcher — keeps registry heartbeat fresh while this wrapper
# owns the anonymous pipe's write descriptor. The executor runs with that
# descriptor closed, so even an untrappable wrapper death produces immediate
# EOF and the watcher exits instead of orphaning a `sleep 120` subprocess.
exec {{HEARTBEAT_GUARD_FD}}> >(wg heartbeat-watch "$WG_AGENT_ID" --supervised-pid "$$" 2>/dev/null)
HEARTBEAT_PID=$!

# Run the agent command without inheriting the heartbeat guard writer.
{{
    {run_command}
}} {{HEARTBEAT_GUARD_FD}}>&-
EXIT_CODE=$?
{session_fallback_block}
# Stop the heartbeat watcher and close its guard on normal completion.
exec {{HEARTBEAT_GUARD_FD}}>&-
kill $HEARTBEAT_PID 2>/dev/null; wait $HEARTBEAT_PID 2>/dev/null
{stream_result}

# Check if task is still in progress (agent didn't mark it done/failed)
TASK_STATUS=$(wg show "$TASK_ID" --json 2>/dev/null | grep -o '"status": *"[^"]*"' | head -1 | sed 's/.*"status": *"//;s/"//' || echo "unknown")

if [ "$TASK_STATUS" = "in-progress" ]; then
    if [ $EXIT_CODE -eq 124 ]; then
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Agent killed by hard timeout, marking task failed" >> "$OUTPUT_FILE"
        FAIL_CLASS=$(wg classify-failure --exit-code $EXIT_CODE 2>/dev/null || echo "agent-hard-timeout")
        wg fail "$TASK_ID" --class "$FAIL_CLASS" --reason "Agent exceeded hard timeout" 2>> "$OUTPUT_FILE" || echo "[wrapper] WARNING: 'wg fail' failed with exit code $?" >> "$OUTPUT_FILE"
    elif [ $EXIT_CODE -eq 0 ]; then
        echo "" >> "$OUTPUT_FILE"
        # Safety net: check for unread messages the agent may have missed
        UNREAD=$(wg msg read "$TASK_ID" --agent "$WG_AGENT_ID" 2>/dev/null)
        if [ -n "$UNREAD" ] && ! echo "$UNREAD" | grep -q "No unread messages"; then
            echo "[wrapper] WARNING: Agent finished with unread messages:" >> "$OUTPUT_FILE"
            echo "$UNREAD" >> "$OUTPUT_FILE"
        fi

        # Minimum-work gate: refuse to auto-mark done with no evidence of real work.
        # Catches models that exit 0 with a prose summary and no tool use (e.g. gpt-5.x lazy-completion).
        WG_GIT_DIR="$WG_WORKTREE_PATH"
        if [ -z "$WG_GIT_DIR" ]; then WG_GIT_DIR="."; fi
        LOG_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"event"' || echo 0)
        ARTIFACT_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"artifact"' || echo 0)
        DIFF_BYTES=$(git -C "$WG_GIT_DIR" diff HEAD --stat 2>/dev/null | wc -c || echo 0)
        COMMITS_AHEAD=$(git -C "$WG_GIT_DIR" rev-list --count HEAD ^origin/HEAD 2>/dev/null || echo 0)

        if [ "$LOG_COUNT" -lt 1 ] && [ "$ARTIFACT_COUNT" -lt 1 ] && [ "$DIFF_BYTES" -lt 50 ] && [ "$COMMITS_AHEAD" -lt 1 ]; then
            echo "[wrapper] FAIL-GATE: agent exited 0 with no logs, no artifacts, no diff, no commits — refusing to auto-mark done" >> "$OUTPUT_FILE"
            wg fail "$TASK_ID" --class "agent-no-work" --reason "Agent exited 0 without producing any work (no wg log, no artifacts, no diff, no commits)" 2>> "$OUTPUT_FILE" || true
        elif [ "$ARTIFACT_COUNT" -lt 1 ] && [ "$DIFF_BYTES" -lt 50 ] && [ "$COMMITS_AHEAD" -lt 1 ]; then
            # Guardrail G4 (NoOperationalOutput): the agent "talked but
            # didn't act" — it wrote logs/prose (LOG_COUNT >= 1) but produced
            # no artifacts and no file writes. Ask `wg classify-no-op` for the
            # verdict (single source of truth: the Rust pure function
            # `classify_no_operational_output` in raw_stream_classifier.rs,
            # which also scans output.log for mutation tokens). On a positive
            # verdict, fail with the machine-readable class so the retry path
            # (G3) injects the no-op directive instead of repeating
            # meta/observation work.
            NO_OP_CLASS=$(wg classify-no-op --output-log "$OUTPUT_FILE" --clean-exit --artifacts-empty 2>/dev/null || echo none)
            if [ "$NO_OP_CLASS" = "no-operational-output" ]; then
                echo "[wrapper] FAIL-GATE: agent exited 0 with prose/logs but no artifacts and no file writes (no-operational-output) — refusing to auto-mark done" >> "$OUTPUT_FILE"
                wg fail "$TASK_ID" --class "no-operational-output" --reason "Agent produced observation/summary work only (no artifacts, no file writes, non-empty output.log) — perform the concrete operational actions" 2>> "$OUTPUT_FILE" || true
            else
                echo "{complete_msg}" >> "$OUTPUT_FILE"
                {complete_cmd}
            fi
        else
            echo "{complete_msg}" >> "$OUTPUT_FILE"
            {complete_cmd}
        fi
    else
        echo "" >> "$OUTPUT_FILE"
        echo "[wrapper] Agent exited with code $EXIT_CODE, marking task failed" >> "$OUTPUT_FILE"
        if tail -c 65536 "$OUTPUT_FILE" 2>/dev/null | grep -Eiq 'no space left on device|os error 28|ENOSPC|disk quota exceeded'; then
            FAIL_CLASS="resource-exhausted-disk"
            FAIL_REASON="Disk resource exhausted during agent execution (exit code $EXIT_CODE); source preserved for retry-in-place"
        else
            FAIL_CLASS=$(wg classify-failure --raw-stream "$RAW_STREAM" --exit-code $EXIT_CODE 2>/dev/null || echo "agent-exit-nonzero")
            FAIL_REASON="Agent exited with code $EXIT_CODE"
        fi
        wg fail "$TASK_ID" --class "$FAIL_CLASS" --reason "$FAIL_REASON" 2>> "$OUTPUT_FILE" || echo "[wrapper] WARNING: 'wg fail' failed with exit code $?" >> "$OUTPUT_FILE"
    fi
fi

# --- Worktree Cleanup (merge-back is handled by wg done) ---
# The merge-back squash is now performed inline by `wg done` while the agent is
# still alive, so it can react to conflicts. This wrapper only handles the
# cleanup marker for the explicit worktree cleanup surface.
if [ -n "$WG_WORKTREE_PATH" ] && [ -n "$WG_BRANCH" ] && [ -n "$WG_PROJECT_ROOT" ]; then
    CURRENT_DIR_REAL=$(pwd -P 2>/dev/null || pwd)
    WORKTREE_PATH_REAL=$(cd "$WG_WORKTREE_PATH" 2>/dev/null && pwd -P || printf '%s' "$WG_WORKTREE_PATH")
    if [ "$CURRENT_DIR_REAL" != "$WORKTREE_PATH_REAL" ]; then
        echo "[wrapper] WARNING: Skipping worktree cleanup because current directory '$CURRENT_DIR_REAL' does not match WG_WORKTREE_PATH '$WORKTREE_PATH_REAL' — possible inherited parent agent environment" >> "$OUTPUT_FILE"
    else
    if [ ! -e "$WG_WORKTREE_PATH/.git" ]; then
        echo "[wrapper] WARNING: Worktree .git pointer missing at $WG_WORKTREE_PATH — possible worktree escape detected" >> "$OUTPUT_FILE"
    fi

    TASK_STATUS_FINAL=$(wg show "$TASK_ID" --json 2>/dev/null | grep -o '"status": *"[^"]*"' | head -1 | sed 's/.*"status": *"//;s/"//' || echo "unknown")

    # Mark worktree for cleanup sweep (wg done already placed this marker on
    # the happy path, but the wrapper is a safety net for cases where the agent
    # crashed or was killed before reaching wg done).
    touch "$WG_WORKTREE_PATH/.wg-cleanup-pending" 2>/dev/null || true
    echo "[wrapper] Task finished with status '$TASK_STATUS_FINAL' — marked worktree $WG_WORKTREE_PATH for explicit cleanup (inspect/remove with: wg worktree archive $WG_AGENT_ID --remove)" >> "$OUTPUT_FILE"

    # Build caches are never removed here. The owned-cache sentinel waits for
    # terminal owner/task state, stale exact PID identity, lease expiry, a clean
    # worktree, no registered artifacts, and no open files.
    fi
fi

exit $EXIT_CODE
"#,
        escaped_task_id = shell_escape(task_id),
        // `$OUTPUT_FILE` is used throughout the wrapper as a `>>` redirect
        // target. Bash on Windows (Git-for-Windows) refuses `\\?\C:\...`
        // verbatim paths for redirects with "No such file or directory",
        // so output.log never appears and the agent looks silently dead.
        escaped_output_file = shell_escape(&sanitize_bash_path(output_file_str)),
        raw_stream_shell_var = raw_stream_shell_var,
        run_command = run_command,
        session_fallback_block = session_fallback_block,
        timeout_note = timeout_note,
        debug_env_vars = debug_env_vars,
        stream_init = stream_init,
        stream_result = stream_result,
        complete_cmd = complete_cmd,
        complete_msg = complete_msg,
    );

    // Write wrapper script
    let wrapper_path = output_dir.join("run.sh");
    fs::write(&wrapper_path, &wrapper_script)
        .with_context(|| format!("Failed to write wrapper script: {:?}", wrapper_path))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))?;
    }

    Ok(wrapper_path)
}

/// A resolved model+provider pair from the unified resolution cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedModelProvider {
    /// The winning model string, potentially still containing a `provider:` prefix.
    /// Downstream `resolve_model_via_registry()` handles prefix stripping.
    pub model: Option<String>,
    /// The resolved provider, extracted from the highest-priority tier that
    /// supplies one — either an explicit provider field or a `provider:model` spec.
    pub provider: Option<String>,
}

/// A single tier in the model/provider resolution cascade.
/// Each tier may supply a model, a provider, or both. If the model string
/// contains a `provider:model` spec, the provider is extracted automatically.
struct ResolutionTier {
    model: Option<String>,
    provider: Option<String>,
}

impl ResolutionTier {
    fn new(model: Option<String>, provider: Option<String>) -> Self {
        // If there's an explicit provider, use it directly.
        // Otherwise, try to extract provider from the model spec.
        if provider.is_some() {
            return Self { model, provider };
        }
        if let Some(ref m) = model {
            let spec = worksgood::config::parse_model_spec(m);
            if let Some(ref p) = spec.provider {
                return Self {
                    model,
                    provider: Some(worksgood::config::provider_to_native_provider(p).to_string()),
                };
            }
        }
        Self { model, provider }
    }
}

/// Resolve model and provider from the unified precedence hierarchy.
///
/// Resolution tiers (highest to lowest priority):
///   1. Task-level (task.model, task.provider)
///   2. Agent preferences (agent.preferred_model, agent.preferred_provider)
///   3. Executor defaults (executor.model)
///   4. Role-based config (role_config.model, role_config.provider)
///   5. Coordinator defaults (coordinator.model, coordinator.provider)
///
/// At each tier, if the model string uses `provider:model` format, the provider
/// is extracted from it via `parse_model_spec()`. An explicit provider at the
/// same tier takes precedence over a provider embedded in the model spec.
///
/// Model and provider are resolved independently: the highest-priority tier
/// that supplies a model wins for model, and likewise for provider. This means
/// a task can set `model = "openrouter:deepseek-v3"` (setting both) or just
/// `provider = "openrouter"` (overriding only the provider).
///
/// The returned model string retains any `provider:` prefix so that downstream
/// `resolve_model_via_registry()` can handle prefix stripping per executor type.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_model_and_provider(
    task_model: Option<String>,
    task_provider: Option<String>,
    agent_preferred_model: Option<String>,
    agent_preferred_provider: Option<String>,
    executor_model: Option<String>,
    role_model: Option<String>,
    role_provider: Option<String>,
    coordinator_model: Option<&str>,
    coordinator_provider: Option<String>,
) -> ResolvedModelProvider {
    let tiers = [
        ResolutionTier::new(task_model, task_provider),
        ResolutionTier::new(agent_preferred_model, agent_preferred_provider),
        ResolutionTier::new(executor_model, None),
        ResolutionTier::new(role_model, role_provider),
        ResolutionTier::new(
            coordinator_model.map(|s| s.to_string()),
            coordinator_provider,
        ),
    ];

    let model = tiers.iter().find_map(|t| t.model.clone());
    let provider = tiers.iter().find_map(|t| t.provider.clone());

    ResolvedModelProvider { model, provider }
}

/// Built-in tier alias IDs that the Claude CLI understands natively.
const BUILTIN_TIER_ALIASES: &[&str] = &["haiku", "sonnet", "opus"];

/// Resolve a model string through the model registry.
///
/// If the model matches a registry entry:
/// - Built-in tier aliases (haiku/sonnet/opus) are kept as-is (Claude CLI understands them)
/// - Custom aliases are resolved to their full API model ID
/// - The entry's provider and endpoint are returned for downstream resolution
///
/// If the model is not in the registry:
/// - If the task explicitly specified it → error (user should register it first)
/// - Otherwise (from executor/coordinator defaults) → pass through unchanged
///
/// Returns `(effective_model, registry_provider, registry_endpoint)`.
fn resolve_spawn_model_via_registry(
    executor_name: &str,
    effective_model: Option<String>,
    task_model: Option<&String>,
    config: &Config,
    dir: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    if executor_name == "pi" {
        return Ok((effective_model, None, None));
    }
    resolve_model_via_registry(effective_model, task_model, config, dir)
}

fn resolve_model_via_registry(
    effective_model: Option<String>,
    task_model: Option<&String>,
    config: &Config,
    dir: &Path,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let model_str = match effective_model {
        Some(ref s) => s.clone(),
        None => return Ok((None, None, None)),
    };

    // Parse unified provider:model spec. If the model has an explicit provider
    // prefix (e.g. "openrouter:deepseek/deepseek-v3.2"), extract it and use
    // the model ID for registry lookup.
    let spec = worksgood::config::parse_model_spec(&model_str);
    if let Some(ref provider_prefix) = spec.provider {
        let native_provider =
            Some(worksgood::config::provider_to_native_provider(provider_prefix).to_string());
        // Try registry lookup on the bare model part for endpoint resolution
        let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
        let endpoint = merged
            .registry_lookup(&spec.model_id)
            .or_else(|| {
                merged
                    .effective_registry()
                    .into_iter()
                    .find(|e| e.model == spec.model_id)
            })
            .and_then(|e| e.endpoint.clone());
        // CLI-backed executors do not understand provider:model format; pass only
        // the bare model ID. Native/API-backed executors preserve the full spec
        // so downstream provider resolution can re-parse the prefix. For the
        // claude CLI, expand friendly aliases with no CLI shortcut
        // (`claude:fable` → `claude-fable-5`); opus/sonnet/haiku pass through.
        let effective = match worksgood::config::provider_to_executor(provider_prefix) {
            "claude" => worksgood::config::claude_cli_model_arg(&spec.model_id),
            "codex" => spec.model_id.clone(),
            _ => model_str.clone(),
        };
        return Ok((Some(effective), native_provider, endpoint));
    }

    // No provider prefix — fall back to existing resolution logic.
    // Load merged config for registry lookup (includes global + local + builtins)
    let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());

    // Look up by short ID first, then by full model field (e.g., "deepseek/deepseek-chat"
    // matching a registry entry with model = "deepseek/deepseek-chat").
    let registry_entry = merged.registry_lookup(&model_str).or_else(|| {
        merged
            .effective_registry()
            .into_iter()
            .find(|e| e.model == model_str)
    });

    if let Some(entry) = registry_entry {
        // Found in registry
        let is_builtin = BUILTIN_TIER_ALIASES.contains(&model_str.as_str());
        let resolved_model = if is_builtin {
            // Keep tier alias as-is for backward compat with Claude CLI
            model_str
        } else {
            // Custom alias → use actual API model ID
            entry.model.clone()
        };
        Ok((
            Some(resolved_model),
            Some(entry.provider.clone()),
            entry.endpoint.clone(),
        ))
    } else if task_model.is_some() && task_model.map(|s| s.as_str()) == effective_model.as_deref() {
        // Task explicitly specified a model that's not in the registry.
        if model_str.contains('/') {
            // Full provider/model ID (e.g., "deepseek/deepseek-chat") — pass through.
            // The native executor's create_provider_ext() auto-detects the provider
            // from the slash in the model name.
            Ok((effective_model, None, None))
        } else {
            // Short alias that's not registered — try resolving against model cache.
            let resolution = worksgood::executor::native::openai_client::resolve_short_model_name(
                &model_str, dir,
            );
            if let Some(resolved_id) = resolution.resolved {
                eprintln!(
                    "[spawn] Resolved short model name '{}' → 'openrouter:{}'",
                    model_str, resolved_id
                );
                // Re-resolve with the full provider:model format
                let full_spec = format!("openrouter:{}", resolved_id);
                let spec = worksgood::config::parse_model_spec(&full_spec);
                let native_provider = Some(
                    worksgood::config::provider_to_native_provider(
                        spec.provider.as_deref().unwrap_or("openrouter"),
                    )
                    .to_string(),
                );
                let merged = Config::load_merged(dir).unwrap_or_else(|_| config.clone());
                let endpoint = merged
                    .registry_lookup(&spec.model_id)
                    .or_else(|| {
                        merged
                            .effective_registry()
                            .into_iter()
                            .find(|e| e.model == spec.model_id)
                    })
                    .and_then(|e| e.endpoint.clone());
                Ok((Some(full_spec), native_provider, endpoint))
            } else {
                // No resolution possible — error with suggestions.
                let suggestions = if resolution.suggestions.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n  Did you mean one of:\n{}",
                        resolution
                            .suggestions
                            .iter()
                            .map(|s| format!("    - openrouter:{}", s))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                anyhow::bail!(
                    "Model '{}' not found in config or model cache.{}\n  \
                     Try: `wg models search {}` to find valid alternatives\n  \
                     Or:  `wg models list` to see the local registry\n  \
                     Add: `wg model add {} --provider <provider> --model-id <model-id>` to register it\n  \
                     Tip: `openrouter/auto` is a safe default that auto-routes to the best model.",
                    model_str,
                    suggestions,
                    model_str,
                    model_str,
                );
            }
        }
    } else {
        // Model came from executor/coordinator defaults — pass through unchanged.
        // It may be a direct model ID the executor understands.
        Ok((effective_model, None, None))
    }
}

/// Check OpenRouter cost caps before spawning an agent
fn check_openrouter_cost_caps(
    config: &Config,
    workgraph_dir: &Path,
    task_id: &str,
    model: Option<&str>,
) -> Result<()> {
    use crate::commands::service::CoordinatorState;
    use worksgood::executor::native::openai_client::{
        fetch_openrouter_key_status_blocking, resolve_openai_api_key_from_dir,
    };

    // No [openrouter] section → no caps to enforce. The section is
    // emitted only on the openrouter route or when explicitly added.
    let Some(openrouter_config) = config.openrouter.as_ref() else {
        return Ok(());
    };

    // Early exit if no cost caps are configured
    if openrouter_config.cost_cap_global_usd.is_none()
        && openrouter_config.cost_cap_session_usd.is_none()
        && openrouter_config.cost_cap_task_usd.is_none()
    {
        return Ok(());
    }

    // Get OpenRouter API key for status checking
    let api_key = match resolve_openai_api_key_from_dir(workgraph_dir) {
        Ok(key) => key,
        Err(_) => {
            // If no API key available, we can't check costs, so allow the operation
            return Ok(());
        }
    };

    let service_dir = workgraph_dir.join(".wg/service");

    // Load current coordinator state for session cost tracking
    let mut coordinator_state = CoordinatorState::load_for(&service_dir, 0).unwrap_or_default();

    // Check if we should refresh key status
    if coordinator_state
        .cost_tracking
        .should_check_key_status(openrouter_config.key_status_check_interval_minutes)
    {
        match fetch_openrouter_key_status_blocking(&api_key, None) {
            Ok(key_status) => {
                coordinator_state
                    .cost_tracking
                    .update_key_status(key_status);
                // Save updated state
                coordinator_state.save_for(&service_dir, 0);
            }
            Err(e) => {
                // Log warning but don't block operation
                eprintln!("Warning: Failed to check OpenRouter key status: {}", e);
            }
        }
    }

    // Check session cost cap
    if let Some(session_cap) = openrouter_config.cost_cap_session_usd
        && coordinator_state.cost_tracking.session_cost_usd >= session_cap
    {
        return handle_cost_cap_violation(
            &openrouter_config.cap_behavior,
            &format!(
                "Session cost cap of ${:.2} exceeded (current: ${:.2})",
                session_cap, coordinator_state.cost_tracking.session_cost_usd
            ),
            openrouter_config.fallback_model.as_deref(),
            task_id,
            model,
        );
    }

    // Check global cost cap using key status if available
    if let (Some(global_cap), Some(key_status)) = (
        openrouter_config.cost_cap_global_usd,
        &coordinator_state.cost_tracking.key_status,
    ) && key_status.usage >= global_cap
    {
        return handle_cost_cap_violation(
            &openrouter_config.cap_behavior,
            &format!(
                "Global cost cap of ${:.2} exceeded (current: ${:.2})",
                global_cap, key_status.usage
            ),
            openrouter_config.fallback_model.as_deref(),
            task_id,
            model,
        );
    }

    // Check warning thresholds
    if let Some(key_status) = &coordinator_state.cost_tracking.key_status
        && key_status.is_above_threshold(openrouter_config.warn_at_usage_percent as f64)
    {
        eprintln!(
            "Warning: OpenRouter usage at {:.1}% of limit (${:.2}/${:.2})",
            key_status.usage_percentage(),
            key_status.usage,
            key_status.limit
        );
    }

    Ok(())
}

/// Handle cost cap violation according to the configured behavior
fn handle_cost_cap_violation(
    behavior: &CapBehavior,
    message: &str,
    fallback_model: Option<&str>,
    task_id: &str,
    _current_model: Option<&str>,
) -> Result<()> {
    match behavior {
        CapBehavior::Fail => {
            anyhow::bail!("Cost cap exceeded: {}", message);
        }
        CapBehavior::Fallback => {
            if let Some(fallback) = fallback_model {
                eprintln!(
                    "Warning: {} - would fallback to model '{}' for task '{}' (fallback not implemented yet)",
                    message, fallback, task_id
                );
                Ok(())
            } else {
                anyhow::bail!(
                    "Cost cap exceeded: {} (no fallback model configured)",
                    message
                );
            }
        }
        CapBehavior::Escalate => {
            eprintln!("Warning: {} - continuing due to escalate behavior", message);
            Ok(())
        }
        CapBehavior::Readonly => {
            eprintln!("Warning: {} - entering read-only mode", message);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::config::{CLAUDE_FABLE_MODEL_ID, CLAUDE_OPUS_MODEL_ID};
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::{load_graph, save_graph};
    use worksgood::service::registry::{AgentRegistry, AgentStatus};

    // --- executor_uses_auto_prompt tests ---

    // Regression for codex-handler-doesn: the spawn pipeline used to hard-code
    // the auto-prompt list as `claude | native`, which silently dropped
    // codex. Codex agents spawned with no prompt_template, fell through to
    // the codex case in build_inner_command's `else { codex_cmd }` branch,
    // and the resulting run.sh had no `cat prompt.txt | ...` prefix. Codex
    // CLI sat reading stdin, got nothing, exited with 'No prompt provided
    // via stdin'. Pin the three built-in handlers here.
    #[test]
    fn test_executor_uses_auto_prompt_includes_codex() {
        assert!(executor_uses_auto_prompt("codex"));
    }

    #[test]
    fn test_executor_uses_auto_prompt_includes_all_builtins() {
        for kind in ["claude", "codex", "native"] {
            assert!(
                executor_uses_auto_prompt(kind),
                "{} must auto-build prompt",
                kind
            );
        }
    }

    #[test]
    fn test_executor_uses_auto_prompt_includes_external_cli_adapters() {
        for kind in worksgood::dispatch::ExecutorKind::EXTERNAL_CLIS
            .iter()
            .map(|kind| kind.as_str())
            .chain(["qwen-code", "qwen_code"])
        {
            assert!(
                executor_uses_auto_prompt(kind),
                "{} must auto-build prompt",
                kind
            );
        }
    }

    #[test]
    fn test_executor_uses_auto_prompt_excludes_shell_and_unknown() {
        assert!(!executor_uses_auto_prompt("shell"));
        assert!(!executor_uses_auto_prompt(""));
        assert!(!executor_uses_auto_prompt("custom"));
    }

    // --- external CLI model normalization tests ---

    fn normalized_args(executor_type: &str) -> Vec<String> {
        external_cli_model_args(
            executor_type,
            Some("openrouter:deepseek/deepseek-v3.2"),
            Some("openrouter"),
        )
        .to_vec()
    }

    #[test]
    fn test_external_cli_model_args_opencode_and_aider_use_provider_slash() {
        for executor_type in ["opencode", "aider"] {
            assert_eq!(
                normalized_args(executor_type),
                vec![
                    "--model".to_string(),
                    "openrouter/deepseek/deepseek-v3.2".to_string()
                ],
                "{} should receive OpenRouter provider/model slash syntax",
                executor_type
            );
        }
    }

    #[test]
    fn test_external_cli_model_args_goose_uses_provider_flag_and_bare_model() {
        assert_eq!(
            normalized_args("goose"),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_cline_uses_provider_flag_and_bare_model() {
        assert_eq!(
            normalized_args("cline"),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_octomind_uses_provider_colon_model() {
        // Octomind's `-m` takes WG's `openrouter:<vendor>/<model>` spelling, so
        // a worker spawn preserves the typed route rather than silently falling
        // back to octomind's default (prototype-octomind-dexto-chat).
        assert_eq!(
            normalized_args("octomind"),
            vec![
                "--model".to_string(),
                "openrouter:deepseek/deepseek-v3.2".to_string()
            ]
        );
        // minimax/minimax-m3 specifically (the task's named regression model).
        assert_eq!(
            external_cli_model_args("octomind", Some("minimax/minimax-m3"), Some("openrouter"))
                .to_vec(),
            vec![
                "--model".to_string(),
                "openrouter:minimax/minimax-m3".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_qwen_uses_bare_model() {
        assert_eq!(
            normalized_args("qwen"),
            vec!["--model".to_string(), "deepseek/deepseek-v3.2".to_string()]
        );
    }

    #[test]
    fn test_external_cli_model_args_crush_uses_provider_slash() {
        assert_eq!(
            normalized_args("crush"),
            vec![
                "--model".to_string(),
                "openrouter/deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_accept_provider_from_resolution() {
        assert_eq!(
            external_cli_model_args(
                "opencode",
                Some("deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .to_vec(),
            vec![
                "--model".to_string(),
                "openrouter/deepseek/deepseek-v3.2".to_string()
            ]
        );
    }

    #[test]
    fn test_external_cli_model_args_do_not_duplicate_openrouter_prefix() {
        for executor_type in ["opencode", "aider"] {
            assert_eq!(
                external_cli_model_args(
                    executor_type,
                    Some("openrouter/deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                )
                .to_vec(),
                vec![
                    "--model".to_string(),
                    "openrouter/deepseek/deepseek-v3.2".to_string()
                ],
                "{} should accept already-normalized OpenRouter model syntax",
                executor_type
            );
        }
    }

    #[test]
    fn test_pi_external_cli_model_args_split_custom_provider_colon_model() {
        assert_eq!(
            external_cli_model_args("pi", Some("lunaroute:glm-5.2-nvfp4"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "lunaroute".to_string(),
                "--model".to_string(),
                "glm-5.2-nvfp4".to_string(),
            ],
            "Pi custom provider:model routes must be split for pi argv"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:lunaroute:glm-5.2-nvfp4"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "lunaroute".to_string(),
                "--model".to_string(),
                "glm-5.2-nvfp4".to_string(),
            ],
            "The Pi executor prefix should be accepted defensively too"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openai:gpt-4.1"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openai".to_string(),
                "--model".to_string(),
                "gpt-4.1".to_string(),
            ],
            "Pi provider names are Pi-owned and must not be mapped through WG native aliases"
        );
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openai-codex:gpt-5.6-sol"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openai-codex".to_string(),
                "--model".to_string(),
                "gpt-5.6-sol".to_string(),
            ],
            "Pi Codex routes must become --provider openai-codex --model gpt-5.6-sol"
        );
    }

    #[test]
    fn test_external_cli_reasoning_args_preserve_pi_and_adapt_codex() {
        let existing: Vec<String> = Vec::new();
        let mut pi_parts = Vec::new();
        append_external_cli_reasoning_args(
            &mut pi_parts,
            &existing,
            "pi",
            Some(ReasoningLevel::Xhigh),
        );
        assert_eq!(
            pi_parts,
            vec!["--thinking".to_string(), "'xhigh'".to_string()]
        );

        let mut max_parts = Vec::new();
        append_external_cli_reasoning_args(
            &mut max_parts,
            &existing,
            "pi",
            Some(ReasoningLevel::Max),
        );
        assert_eq!(
            max_parts,
            vec!["--thinking".to_string(), "'max'".to_string()]
        );

        let mut omitted = Vec::new();
        append_external_cli_reasoning_args(&mut omitted, &existing, "pi", None);
        assert!(omitted.is_empty());

        let mut codex = Vec::new();
        append_external_cli_reasoning_args(
            &mut codex,
            &existing,
            "codex",
            Some(ReasoningLevel::High),
        );
        assert_eq!(
            codex,
            vec![
                "-c".to_string(),
                "'model_reasoning_effort=\"high\"'".to_string(),
            ],
            "Codex must use its config override rather than Pi's --thinking flag"
        );

        let mut existing_flag = Vec::new();
        append_external_cli_reasoning_args(
            &mut existing_flag,
            &["--thinking".to_string(), "low".to_string()],
            "pi",
            Some(ReasoningLevel::High),
        );
        assert!(existing_flag.is_empty(), "explicit Pi executor args win");

        let mut existing_codex = Vec::new();
        append_external_cli_reasoning_args(
            &mut existing_codex,
            &[
                "-c".to_string(),
                "model_reasoning_effort=\"medium\"".to_string(),
                "-c".to_string(),
                "model_verbosity=\"high\"".to_string(),
            ],
            "codex",
            Some(ReasoningLevel::Xhigh),
        );
        assert!(
            existing_codex.is_empty(),
            "explicit Codex effort must win independently of verbosity"
        );
    }

    #[test]
    fn test_pi_external_cli_model_args_keep_openrouter_slash_model() {
        assert_eq!(
            external_cli_model_args("pi", Some("pi:openrouter/test/model"), None).to_vec(),
            vec![
                "--provider".to_string(),
                "openrouter".to_string(),
                "--model".to_string(),
                "test/model".to_string(),
            ],
            "Existing Pi OpenRouter provider/model spelling must keep working"
        );
    }

    #[test]
    fn test_external_cli_model_args_do_not_change_builtin_or_amplifier_defaults() {
        for executor_type in ["claude", "codex", "native", "amplifier"] {
            assert!(
                external_cli_model_args(
                    executor_type,
                    Some("openrouter:deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                )
                .to_vec()
                .is_empty(),
                "{} model behavior should stay on its existing path",
                executor_type
            );
            assert_eq!(
                model_template_value_for_executor(
                    executor_type,
                    Some("openrouter:deepseek/deepseek-v3.2"),
                    Some("openrouter"),
                ),
                None
            );
        }
    }

    #[test]
    fn test_external_cli_model_template_value_matches_cli_style() {
        assert_eq!(
            model_template_value_for_executor(
                "opencode",
                Some("openrouter:deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .as_deref(),
            Some("openrouter/deepseek/deepseek-v3.2")
        );
        assert_eq!(
            model_template_value_for_executor(
                "goose",
                Some("openrouter:deepseek/deepseek-v3.2"),
                Some("openrouter"),
            )
            .as_deref(),
            Some("deepseek/deepseek-v3.2")
        );
    }

    // --- should_create_worktree tests ---

    #[test]
    fn test_worktree_gate_disabled_globally() {
        assert!(!should_create_worktree(false, "my-task", "full"));
        assert!(!should_create_worktree(false, "my-task", "shell"));
    }

    #[test]
    fn test_worktree_gate_full_and_shell_get_worktree() {
        assert!(should_create_worktree(true, "my-task", "full"));
        assert!(should_create_worktree(true, "my-task", "shell"));
    }

    #[test]
    fn test_worktree_gate_bare_and_light_skip() {
        assert!(!should_create_worktree(true, "my-task", "bare"));
        assert!(!should_create_worktree(true, "my-task", "light"));
    }

    #[test]
    fn test_worktree_gate_meta_tasks_skip_regardless_of_exec_mode() {
        // System/meta tasks never get worktrees — they only touch graph state.
        for prefix in [".assign-", ".flip-", ".evaluate-", ".place-", ".compact-"] {
            let task_id = format!("{}my-real-task", prefix);
            for exec_mode in ["full", "shell", "bare", "light"] {
                assert!(
                    !should_create_worktree(true, &task_id, exec_mode),
                    "meta task {} with exec_mode={} should not get worktree",
                    task_id,
                    exec_mode
                );
            }
        }
    }

    #[test]
    fn test_worktree_gate_unknown_exec_mode_defaults_to_worktree() {
        // Conservative default: unrecognized modes get a worktree (fail-safe for writes).
        assert!(should_create_worktree(true, "my-task", "future-mode-xyz"));
    }

    // --- resolve_model_and_provider tests ---

    /// Helper to call resolve_model_and_provider with all None defaults except specified args.
    fn resolve(
        task_model: Option<&str>,
        task_provider: Option<&str>,
        agent_model: Option<&str>,
        agent_provider: Option<&str>,
        executor_model: Option<&str>,
        role_model: Option<&str>,
        role_provider: Option<&str>,
        coordinator_model: Option<&str>,
        coordinator_provider: Option<&str>,
    ) -> ResolvedModelProvider {
        resolve_model_and_provider(
            task_model.map(|s| s.to_string()),
            task_provider.map(|s| s.to_string()),
            agent_model.map(|s| s.to_string()),
            agent_provider.map(|s| s.to_string()),
            executor_model.map(|s| s.to_string()),
            role_model.map(|s| s.to_string()),
            role_provider.map(|s| s.to_string()),
            coordinator_model,
            coordinator_provider.map(|s| s.to_string()),
        )
    }

    #[test]
    fn test_unified_model_task_overrides_agent() {
        let r = resolve(
            Some("task-model"),
            None,
            Some("agent-model"),
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("task-model".to_string()));
    }

    #[test]
    fn test_unified_model_agent_when_no_task() {
        let r = resolve(
            None,
            None,
            Some("agent-model"),
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("agent-model".to_string()));
    }

    #[test]
    fn test_unified_model_executor_when_no_agent() {
        let r = resolve(
            None,
            None,
            None,
            None,
            Some("executor-model"),
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("executor-model".to_string()));
    }

    #[test]
    fn test_unified_model_role_when_no_executor() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            Some("role-model"),
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("role-model".to_string()));
    }

    #[test]
    fn test_unified_model_coordinator_fallback() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("coordinator-model"),
            None,
        );
        assert_eq!(r.model, Some("coordinator-model".to_string()));
    }

    #[test]
    fn test_unified_none_when_all_empty() {
        let r = resolve(None, None, None, None, None, None, None, None, None);
        assert_eq!(r.model, None);
        assert_eq!(r.provider, None);
    }

    #[test]
    fn test_unified_provider_task_overrides_agent() {
        let r = resolve(
            None,
            Some("task-provider"),
            None,
            Some("agent-provider"),
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("task-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_agent_when_no_task() {
        let r = resolve(
            None,
            None,
            None,
            Some("agent-provider"),
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("agent-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_config_fallback() {
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            Some("config-provider"),
            None,
            None,
        );
        assert_eq!(r.provider, Some("config-provider".to_string()));
    }

    #[test]
    fn test_unified_provider_none_when_all_empty() {
        let r = resolve(None, None, None, None, None, None, None, None, None);
        assert_eq!(r.provider, None);
    }

    #[test]
    fn test_unified_provider_from_model_spec() {
        // provider:model in task.model extracts provider automatically
        let r = resolve(
            Some("openrouter:deepseek/deepseek-v3"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.model, Some("openrouter:deepseek/deepseek-v3".to_string()));
        assert_eq!(r.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_unified_explicit_provider_beats_model_spec() {
        // An explicit task_provider takes precedence over provider in model spec
        let r = resolve(
            Some("openrouter:deepseek/deepseek-v3"),
            Some("anthropic"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_unified_model_spec_at_agent_tier() {
        // provider:model at agent tier extracts provider when no task-level provider
        let r = resolve(
            None,
            None,
            Some("openai:gpt-5"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(r.model, Some("openai:gpt-5".to_string()));
        assert_eq!(r.provider, Some("oai-compat".to_string()));
    }

    #[test]
    fn test_unified_model_spec_at_coordinator_tier() {
        // provider:model at coordinator tier as last resort
        let r = resolve(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("claude:opus"),
            None,
        );
        assert_eq!(r.model, Some("claude:opus".to_string()));
        assert_eq!(r.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_unified_unassigned_task_uses_executor_model() {
        let r = resolve(
            None,
            None,
            None,
            None,
            Some("executor-default"),
            None,
            None,
            Some("coordinator-fallback"),
            None,
        );
        assert_eq!(r.model, Some("executor-default".to_string()));
    }

    #[test]
    fn test_unified_task_provider_overrides_lower_model_spec() {
        // Task has explicit provider, coordinator has provider:model
        // Task provider should win even though coordinator model has a spec
        let r = resolve(
            None,
            Some("anthropic"),
            None,
            None,
            None,
            None,
            None,
            Some("openrouter:deepseek/deepseek-v3"),
            None,
        );
        assert_eq!(r.provider, Some("anthropic".to_string()));
        assert_eq!(r.model, Some("openrouter:deepseek/deepseek-v3".to_string()));
    }

    /// Helper to build an EndpointsConfig for endpoint resolution tests.
    fn test_endpoints_config() -> worksgood::config::EndpointsConfig {
        worksgood::config::EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                worksgood::config::EndpointConfig {
                    name: "my-openrouter".to_string(),
                    provider: "openrouter".to_string(),
                    url: Some("https://openrouter.ai/api/v1".to_string()),
                    api_key: Some("sk-or-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    model: None,
                    api_key_ref: None,
                    is_default: true,
                    context_window: None,
                },
                worksgood::config::EndpointConfig {
                    name: "my-anthropic".to_string(),
                    provider: "anthropic".to_string(),
                    url: Some("https://api.anthropic.com".to_string()),
                    api_key: Some("sk-ant-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    model: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
            ],
        }
    }

    #[test]
    fn test_endpoint_resolution_task_endpoint_takes_priority() {
        let endpoints = test_endpoints_config();

        // task.endpoint is set — should win over everything
        let task_endpoint = Some("my-openrouter".to_string());
        let task_provider: Option<String> = Some("anthropic".to_string());
        let agent_provider: Option<String> = Some("anthropic".to_string());
        let role_endpoint: Option<String> = Some("my-anthropic".to_string());

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_task_provider_lookup() {
        let endpoints = test_endpoints_config();

        // No task.endpoint, but task.provider → find matching endpoint
        let task_endpoint: Option<String> = None;
        let task_provider = Some("openrouter".to_string());
        let agent_provider: Option<String> = None;
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_agent_provider_fallback() {
        let endpoints = test_endpoints_config();

        // No task.endpoint or task.provider, agent.preferred_provider finds endpoint
        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider = Some("anthropic".to_string());
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-anthropic".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_role_config_fallback() {
        let endpoints = test_endpoints_config();

        // Nothing else set, role config endpoint is used
        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider: Option<String> = None;
        let role_endpoint = Some("my-anthropic".to_string());

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, Some("my-anthropic".to_string()));
    }

    #[test]
    fn test_endpoint_resolution_none_when_all_empty() {
        let endpoints = test_endpoints_config();

        let task_endpoint: Option<String> = None;
        let task_provider: Option<String> = None;
        let agent_provider: Option<String> = None;
        let role_endpoint: Option<String> = None;

        let effective = task_endpoint
            .or_else(|| {
                task_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or_else(|| {
                agent_provider
                    .as_ref()
                    .and_then(|prov| endpoints.find_for_provider(prov))
                    .map(|ep| ep.name.clone())
            })
            .or(role_endpoint);

        assert_eq!(effective, None);
    }

    #[test]
    fn test_endpoint_api_key_resolved_from_config() {
        let endpoints = test_endpoints_config();
        let ep = endpoints.find_by_name("my-openrouter").unwrap();
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key, Some("sk-or-test".to_string()));
    }

    // --- resolve_model_via_registry tests ---

    fn setup_registry_dir() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir).unwrap();

        // Create a config with a custom model registry entry
        let mut config = Config::default();
        config.model_registry = vec![worksgood::config::ModelRegistryEntry {
            id: "my-custom".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-3.5-sonnet".to_string(),
            tier: worksgood::config::Tier::Standard,
            endpoint: Some("my-openrouter".to_string()),
            ..Default::default()
        }];
        config.save(dir).unwrap();
        tmp
    }

    #[test]
    fn test_registry_resolves_custom_alias_to_model_id() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("my-custom".to_string()),
            Some(&"my-custom".to_string()),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Custom alias should resolve to actual model ID"
        );
        assert_eq!(
            provider,
            Some("openrouter".to_string()),
            "Provider should come from registry entry"
        );
        assert_eq!(
            endpoint,
            Some("my-openrouter".to_string()),
            "Endpoint should come from registry entry"
        );
    }

    #[test]
    fn test_registry_keeps_builtin_alias_unchanged() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        for alias in &["haiku", "sonnet", "opus"] {
            let (model, provider, _endpoint) = resolve_model_via_registry(
                Some(alias.to_string()),
                Some(&alias.to_string()),
                &config,
                dir,
            )
            .unwrap();

            assert_eq!(
                model.as_deref(),
                Some(*alias),
                "Built-in alias '{}' should be kept as-is",
                alias
            );
            assert_eq!(
                provider,
                Some("anthropic".to_string()),
                "Built-in alias '{}' should resolve to anthropic provider",
                alias
            );
        }
    }

    #[test]
    fn test_registry_errors_on_unknown_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let result = resolve_model_via_registry(
            Some("nonexistent-model".to_string()),
            Some(&"nonexistent-model".to_string()),
            &config,
            dir,
        );

        assert!(
            result.is_err(),
            "Should error when task model is not in registry"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found in config"),
            "Error should mention 'not found in config': {}",
            err
        );
        assert!(
            err.contains("wg model add"),
            "Error should suggest how to register: {}",
            err
        );
    }

    #[test]
    fn test_registry_bypass_for_pi_custom_provider_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let pi_model = "lunaroute:glm-5.2-nvfp4".to_string();
        let (model, provider, endpoint) = resolve_spawn_model_via_registry(
            "pi",
            Some(pi_model.clone()),
            Some(&pi_model),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(pi_model),
            "Pi owns provider:model resolution; WG must not consult its registry/cache"
        );
        assert_eq!(provider, None);
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_still_rejects_custom_provider_colon_for_non_pi_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let custom_provider_model = "lunaroute:glm-5.2-nvfp4".to_string();
        let result = resolve_spawn_model_via_registry(
            "native",
            Some(custom_provider_model.clone()),
            Some(&custom_provider_model),
            &config,
            dir,
        );

        assert!(
            result.is_err(),
            "Non-Pi executors should still validate unknown task models against WG registry/cache"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in config"),
            "Non-Pi validation should keep the existing registry/cache error"
        );
    }

    #[test]
    fn test_registry_passes_through_non_task_model() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // Model came from executor/coordinator, not from task — should pass through.
        // CLAUDE_OPUS_MODEL_ID matches the builtin "opus" entry's model field, so
        // it resolves with provider info from that entry.
        let (model, provider, _endpoint) = resolve_model_via_registry(
            Some(CLAUDE_OPUS_MODEL_ID.to_string()),
            None, // no task model
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(CLAUDE_OPUS_MODEL_ID.to_string()),
            "Non-task model should resolve to the same model ID"
        );
        assert_eq!(
            provider,
            Some("anthropic".to_string()),
            "Should find provider from builtin registry entry"
        );
    }

    #[test]
    fn test_registry_claude_fable_expands_to_full_cli_id() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // `claude:fable` has the claude provider prefix → the claude CLI branch
        // must expand the friendly alias `fable` to the full CLI model id
        // `claude-fable-5` (the CLI has no bare `fable` shortcut).
        let (model, provider, _endpoint) = resolve_model_via_registry(
            Some("claude:fable".to_string()),
            Some(&"claude:fable".to_string()),
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some(CLAUDE_FABLE_MODEL_ID.to_string()),
            "claude:fable must resolve to the full CLI id claude-fable-5"
        );
        assert_eq!(
            provider,
            Some("anthropic".to_string()),
            "claude provider prefix maps to the anthropic native provider"
        );

        // Bare `fable` (no prefix) resolves via the builtin registry entry,
        // whose model field also carries the full CLI id.
        let (bare_model, _p, _e) = resolve_model_via_registry(
            Some("fable".to_string()),
            Some(&"fable".to_string()),
            &config,
            dir,
        )
        .unwrap();
        assert_eq!(bare_model, Some(CLAUDE_FABLE_MODEL_ID.to_string()));
    }

    #[test]
    fn test_registry_truly_unknown_non_task_model_passes_through() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // A model not in the registry at all, from executor/coordinator
        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("totally-unknown-model".to_string()),
            None, // no task model
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("totally-unknown-model".to_string()),
            "Unknown non-task model should pass through unchanged"
        );
        assert_eq!(
            provider, None,
            "No registry provider for truly unknown model"
        );
        assert_eq!(
            endpoint, None,
            "No registry endpoint for truly unknown model"
        );
    }

    #[test]
    fn test_registry_none_model_returns_none() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) =
            resolve_model_via_registry(None, None, &config, dir).unwrap();

        assert_eq!(model, None);
        assert_eq!(provider, None);
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_non_task_model_matching_alias_still_resolves() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        // Model came from executor config but happens to match a registry entry
        let (model, provider, endpoint) = resolve_model_via_registry(
            Some("my-custom".to_string()),
            None, // not from task
            &config,
            dir,
        )
        .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Should still resolve even if not from task"
        );
        assert_eq!(provider, Some("openrouter".to_string()));
        assert_eq!(endpoint, Some("my-openrouter".to_string()));
    }

    #[test]
    fn test_registry_strips_codex_prefix_for_codex_executor_models() {
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let (model, provider, endpoint) =
            resolve_model_via_registry(Some("codex:gpt-5-codex".to_string()), None, &config, dir)
                .unwrap();

        assert_eq!(model, Some("gpt-5-codex".to_string()));
        assert_eq!(provider, Some("oai-compat".to_string()));
        assert_eq!(endpoint, None);
    }

    #[test]
    fn test_registry_full_model_id_passthrough_for_task() {
        // Full model IDs with "/" should pass through even when task-specified,
        // allowing OpenRouter-style "provider/model" to work without registration.
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let full_model = "deepseek/deepseek-chat".to_string();
        let (model, provider, endpoint) =
            resolve_model_via_registry(Some(full_model.clone()), Some(&full_model), &config, dir)
                .unwrap();

        assert_eq!(
            model,
            Some("deepseek/deepseek-chat".to_string()),
            "Full model ID with / should pass through unchanged"
        );
        assert_eq!(
            provider, None,
            "No provider from registry — auto-detection will handle it"
        );
        assert_eq!(endpoint, None, "No endpoint from registry");
    }

    #[test]
    fn test_registry_lookup_by_model_field() {
        // If a registry entry has model = "anthropic/claude-3.5-sonnet",
        // using --model "anthropic/claude-3.5-sonnet" should find it.
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let full_model = "anthropic/claude-3.5-sonnet".to_string();
        let (model, provider, endpoint) =
            resolve_model_via_registry(Some(full_model.clone()), Some(&full_model), &config, dir)
                .unwrap();

        assert_eq!(
            model,
            Some("anthropic/claude-3.5-sonnet".to_string()),
            "Should match registry entry by model field"
        );
        assert_eq!(
            provider,
            Some("openrouter".to_string()),
            "Should get provider from matched entry"
        );
        assert_eq!(
            endpoint,
            Some("my-openrouter".to_string()),
            "Should get endpoint from matched entry"
        );
    }

    #[test]
    fn test_registry_short_alias_still_errors_when_unknown() {
        // Short aliases (no "/") that aren't registered should still error
        let tmp = setup_registry_dir();
        let dir = tmp.path();
        let config = Config::load_or_default(dir);

        let unknown = "some-unknown-alias".to_string();
        let result =
            resolve_model_via_registry(Some(unknown.clone()), Some(&unknown), &config, dir);

        assert!(result.is_err(), "Short unknown aliases should still error");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in config"),
            "Error should mention registration"
        );
    }

    fn external_test_settings(
        executor_type: &str,
        command: &str,
        args: &[&str],
    ) -> worksgood::service::executor::ExecutorSettings {
        worksgood::service::executor::ExecutorSettings {
            executor_type: executor_type.to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Investigate task".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        }
    }

    fn test_template_vars() -> TemplateVars {
        TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        }
    }

    fn default_external_settings(
        workgraph_dir: &Path,
        executor_type: &str,
    ) -> worksgood::service::executor::ExecutorSettings {
        let registry = ExecutorRegistry::new(workgraph_dir);
        let mut settings = registry.load_config(executor_type).unwrap().executor;
        settings.prompt_template = Some(PromptTemplate {
            template: "Investigate task".to_string(),
        });
        settings
    }

    #[test]
    fn test_build_inner_command_pi_external_emits_model_and_thinking() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings("pi", "pi", &["--mode", "json"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command_with_reasoning(
            &settings,
            "full",
            output_dir,
            &Some("pi:openai-codex:gpt-5.6-sol".to_string()),
            &None,
            Some(ReasoningLevel::High),
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("--provider 'openai-codex'"),
            "Pi external command must carry the provider split: {}",
            command
        );
        assert!(
            command.contains("--model 'gpt-5.6-sol'"),
            "Pi external command must carry the model split: {}",
            command
        );
        assert!(
            command.contains("--thinking 'high'"),
            "Pi external command must carry structured reasoning: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_opencode_default_uses_run_json_prompt_file_and_openrouter_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "opencode");
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("'opencode' 'run'"),
            "OpenCode must use documented non-interactive `opencode run`: {}",
            command
        );
        assert!(
            command.contains("'--format' 'json'"),
            "OpenCode should request JSON-capable output: {}",
            command
        );
        assert!(
            command.contains("--model 'openrouter/deepseek/deepseek-v3.2'"),
            "OpenCode should receive provider/model slash syntax: {}",
            command
        );
        assert!(
            command.contains("--file "),
            "OpenCode should receive the WG prompt as an attached file: {}",
            command
        );
        let message_pos = command
            .find("'Complete the attached WG task prompt.'")
            .expect("OpenCode command should include an explicit message");
        let file_pos = command
            .find("--file ")
            .expect("OpenCode command should attach prompt file");
        assert!(
            message_pos < file_pos,
            "OpenCode --file is an array option; message must come before --file so it is not parsed as a second file: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into OpenCode argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_opencode_errors_when_model_unresolved() {
        // Explicit-model contract (fix-opencode-build req #3): the opencode
        // worker path must FAIL when model resolution produced nothing, never
        // silently omit `--model` and inherit opencode's internal default.
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "opencode");
        let vars = test_template_vars();

        let result = build_inner_command(
            &settings, "full", output_dir, &None, // <- no resolved model
            &None, &None, &None, &None, &vars, &None, None,
        );

        let err = result.expect_err("opencode with no resolved model must be a hard error");
        let msg = format!("{err}");
        assert!(
            msg.contains("requires an explicitly resolved model"),
            "error must explain the explicit-model contract, got: {msg}"
        );
    }

    #[test]
    fn test_build_inner_command_aider_default_uses_message_file_and_openrouter_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = default_external_settings(output_dir, "aider");
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("'aider'"),
            "Aider command should invoke the aider CLI: {}",
            command
        );
        assert!(
            command.contains("'--yes-always'"),
            "Aider should avoid confirmation prompts in batch mode: {}",
            command
        );
        assert!(
            command.contains("--model 'openrouter/deepseek/deepseek-v3.2'"),
            "Aider should receive provider/model slash syntax: {}",
            command
        );
        assert!(
            command.contains("--message-file "),
            "Aider should receive the WG prompt through --message-file: {}",
            command
        );
        assert!(
            !command.contains(" --message '"),
            "Aider must avoid interactive chat and inline --message prompts: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Aider argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_goose_normalizes_provider_and_model_without_key_leak() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "goose",
            "goose",
            &["run", "--no-session", "--output-format", "json"],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("--provider 'openrouter'"),
            "Goose should receive a provider flag: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Goose should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            command.contains("-i "),
            "Goose should receive the WG prompt as an input file: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_qwen_uses_prompt_headless_model_and_output() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings =
            external_test_settings("qwen", "qwen", &["--output-format", "json", "--yolo"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.starts_with("cat "),
            "Qwen should receive WG's assembled prompt through stdin: {}",
            command
        );
        assert!(
            command.contains("--prompt 'Complete the WG task prompt supplied on stdin.'"),
            "Qwen should run in documented --prompt headless mode: {}",
            command
        );
        assert!(
            command.contains("'--output-format' 'json'"),
            "Qwen should request machine-readable output where supported: {}",
            command
        );
        assert!(
            command.contains("'--yolo'"),
            "Qwen Code's unattended approval flag is deliberately experimental and covered here so future changes are explicit: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Qwen should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            !command.contains("--provider"),
            "Qwen's OpenRouter path relies on configured provider/auth, not a provider argv flag: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Qwen argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_amplifier_uses_prompt_argument_bridge() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "amplifier",
            "amplifier",
            &[
                "run",
                "--mode",
                "single",
                "--output-format",
                "json",
                "--bundle",
                "wg",
            ],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("PROMPT=$(cat \"$1\"); shift; exec \"$@\" \"$PROMPT\""),
            "Amplifier should bridge prompt.txt into a positional argument: {}",
            command
        );
        assert!(
            command.contains(
                "'amplifier' 'run' '--mode' 'single' '--output-format' 'json' '--bundle' 'wg'"
            ),
            "Amplifier default command shape should be preserved: {}",
            command
        );
        assert!(
            !command.contains("--model"),
            "Amplifier defaults should not treat amplifier as a native model provider: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret"),
            "External CLI argv must not contain API keys: {}",
            command
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("prompt.txt")).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_cline_uses_headless_auto_approve_provider_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings =
            external_test_settings("cline", "cline", &["--json", "--auto-approve", "true"]);
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.starts_with("cat "),
            "Cline should receive WG's assembled prompt through stdin: {}",
            command
        );
        assert!(
            command.contains("--json"),
            "Cline should run in JSON-capable headless mode: {}",
            command
        );
        assert!(
            command.contains("'--auto-approve' 'true'"),
            "Cline's unattended auto-approval flag is deliberately experimental and covered here so future changes are explicit: {}",
            command
        );
        assert!(
            command.contains("--provider 'openrouter'"),
            "Cline should receive a provider flag where supported: {}",
            command
        );
        assert!(
            command.contains("--model 'deepseek/deepseek-v3.2'"),
            "Cline should receive the bare OpenRouter model ID: {}",
            command
        );
        assert!(
            command.ends_with("'Complete the WG task prompt supplied on stdin.'"),
            "Cline should get a positional headless task prompt: {}",
            command
        );
        assert!(
            !command.contains("openrouter:deepseek"),
            "WG provider:model syntax must not leak into Cline argv: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "External CLI argv must not contain API keys or key file paths: {}",
            command
        );
    }

    #[test]
    fn test_build_inner_command_generic_experimental_fallback_is_raw_argv_only() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = external_test_settings(
            "experimental-runner",
            "experimental-runner",
            &["run", "--json", "--flag=value"],
        );
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("openrouter:deepseek/deepseek-v3.2".to_string()),
            &Some("openrouter".to_string()),
            &None,
            &None,
            &Some("sk-or-secret".to_string()),
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert_eq!(
            command, "'experimental-runner' 'run' '--json' '--flag=value'",
            "Unknown experimental executor types should stay on the raw configured argv fallback"
        );
        assert!(
            !output_dir.join("prompt.txt").exists(),
            "The generic fallback must not imply prompt delivery; first-class adapters need explicit branches"
        );
        assert!(
            !command.contains("--model") && !command.contains("--provider"),
            "Generic fallback should not guess model/provider flags: {}",
            command
        );
        assert!(
            !command.contains("sk-or-secret") && !command.contains(".openrouter.key"),
            "Generic fallback argv must not leak API credentials: {}",
            command
        );
    }

    #[test]
    fn test_preflight_executor_command_missing_binary_actionable() {
        let settings =
            external_test_settings("amplifier", "wg-missing-amplifier-test-binary", &["run"]);
        let err = preflight_executor_command(&settings, "amplifier", None)
            .expect_err("missing binary should fail preflight")
            .to_string();

        assert!(
            err.contains("amplifier") && err.contains("not found on PATH"),
            "error should name the missing executor and PATH failure: {}",
            err
        );
        assert!(
            err.contains(".wg/executors/amplifier.toml") && err.contains("Install"),
            "error should tell the user how to fix the executor command: {}",
            err
        );
        assert!(
            err.contains("amplifier run --mode single --output-format json --bundle wg"),
            "Amplifier-specific setup hint should include the expected command: {}",
            err
        );
    }

    #[test]
    fn test_preflight_executor_command_relative_path_uses_working_dir() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let runner = temp_dir.path().join("runner");
        std::fs::write(&runner, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&runner).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&runner, perms).unwrap();
        }

        let settings = external_test_settings("custom", "./runner", &[]);
        preflight_executor_command(&settings, "custom", Some(temp_dir.path()))
            .expect("relative command should resolve from working_dir");
    }

    #[test]
    fn test_inject_api_key_env_sets_openrouter_env_without_file_path() {
        let endpoint = EndpointConfig {
            name: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            url: Some("https://openrouter.ai/api/v1".to_string()),
            model: None,
            api_key: None,
            api_key_file: Some("~/.openrouter.key".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        };
        let mut cmd = Command::new("true");
        inject_api_key_env(&mut cmd, Some(&endpoint), &Some("sk-or-secret".to_string()));

        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();

        assert!(envs.contains(&("WG_API_KEY".to_string(), Some("sk-or-secret".to_string()))));
        assert!(envs.contains(&(
            "OPENROUTER_API_KEY".to_string(),
            Some("sk-or-secret".to_string())
        )));
        assert!(
            envs.iter().all(|(key, value)| {
                !key.contains(".openrouter.key")
                    && value
                        .as_deref()
                        .map_or(true, |value| !value.contains(".openrouter.key"))
            }),
            "The configured key file path should not be propagated or printed: {:?}",
            envs
        );
    }

    #[test]
    fn test_direct_codex_worker_argv_keeps_verbosity_separate_from_reasoning() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let mut settings = default_external_settings(output_dir, "codex");
        settings.prompt_template = Some(PromptTemplate {
            template: "Implement task".to_string(),
        });
        let vars = test_template_vars();

        let (command, fallback) = build_inner_command_with_reasoning(
            &settings,
            "full",
            output_dir,
            &Some("gpt-5.6-sol".to_string()),
            &None,
            Some(ReasoningLevel::Minimal),
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(fallback.is_none());
        assert!(
            command.contains("model_reasoning_effort=\"low\""),
            "WG minimal must map to Codex low effort: {command}"
        );
        assert!(
            command.contains("model_verbosity=\"high\""),
            "the independently configured Codex verbosity must remain present: {command}"
        );
    }

    #[test]
    fn test_direct_codex_reasoning_effort_mapping_is_explicit_for_every_level() {
        let expected = [
            (ReasoningLevel::Off, "none"),
            (ReasoningLevel::Minimal, "low"),
            (ReasoningLevel::Low, "low"),
            (ReasoningLevel::Medium, "medium"),
            (ReasoningLevel::High, "high"),
            (ReasoningLevel::Xhigh, "xhigh"),
            (ReasoningLevel::Max, "max"),
        ];
        for (level, effort) in expected {
            assert_eq!(level.as_codex_effort(), effort);
        }
    }

    #[test]
    fn test_build_inner_command_codex_uses_prompt_file_and_model() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "codex".to_string(),
            command: "codex".to_string(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Investigate task".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("gpt-5-codex".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            None,
        )
        .unwrap();

        assert!(
            fallback.is_none(),
            "Codex should not have a fallback command"
        );
        assert!(
            command.contains("cat "),
            "Expected prompt to be piped from a file: {}",
            command
        );
        assert!(
            command.contains("'codex' 'exec'"),
            "Expected codex exec invocation: {}",
            command
        );
        assert!(
            command.contains("'--json'"),
            "Expected codex JSON mode: {}",
            command
        );
        assert!(
            command.contains("'--skip-git-repo-check'"),
            "Expected codex git check bypass flag: {}",
            command
        );
        assert!(
            command.contains("--model 'gpt-5-codex'"),
            "Expected codex model flag: {}",
            command
        );
        assert!(
            !command.contains("model_reasoning_effort"),
            "unset WG reasoning must inherit ~/.codex/config.toml: {command}"
        );
        let prompt_file = output_dir.join("prompt.txt");
        assert_eq!(
            std::fs::read_to_string(prompt_file).unwrap(),
            "Investigate task"
        );
    }

    #[test]
    fn test_build_inner_command_claude_resume_produces_fallback() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "claude".to_string(),
            command: "claude".to_string(),
            args: vec![
                "--print".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Full task prompt".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Resume context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("sonnet".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            Some("fake-session-id-12345"),
        )
        .unwrap();

        // Primary command should use --resume
        assert!(
            command.contains("--resume"),
            "Resume command should contain --resume: {}",
            command
        );
        assert!(
            command.contains("fake-session-id-12345"),
            "Resume command should contain session ID: {}",
            command
        );

        // Fallback should exist and NOT contain --resume
        let fb = fallback.expect("Claude resume should produce a fallback command");
        assert!(
            !fb.contains("--resume"),
            "Fallback command should NOT contain --resume: {}",
            fb
        );
        assert!(
            fb.contains("prompt.txt"),
            "Fallback should use prompt.txt: {}",
            fb
        );

        // Both prompt files should be written
        assert!(
            output_dir.join("resume_message.txt").exists(),
            "resume_message.txt should be written"
        );
        assert!(
            output_dir.join("prompt.txt").exists(),
            "prompt.txt should be written for fallback"
        );
    }

    #[test]
    fn test_build_inner_command_claude_no_resume_no_fallback() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();
        let settings = worksgood::service::executor::ExecutorSettings {
            executor_type: "claude".to_string(),
            command: "claude".to_string(),
            args: vec![
                "--print".to_string(),
                "--verbose".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            env: std::collections::HashMap::new(),
            prompt_template: Some(PromptTemplate {
                template: "Full task prompt".to_string(),
            }),
            working_dir: Some("/tmp".to_string()),
            timeout: None,
            model: None,
        };
        let vars = TemplateVars {
            task_id: "task-1".to_string(),
            task_title: "Task".to_string(),
            task_description: "Desc".to_string(),
            task_context: "Context".to_string(),
            task_identity: String::new(),
            bound_session_summary: String::new(),
            working_dir: "/tmp".to_string(),
            skills_preamble: String::new(),
            model: String::new(),
            task_loop_info: String::new(),
            task_verify: None,
            max_child_tasks: 0,
            max_task_depth: 0,
            has_failed_deps: false,
            failed_deps_info: String::new(),
            in_worktree: false,
        };

        let (command, fallback) = build_inner_command(
            &settings,
            "full",
            output_dir,
            &Some("sonnet".to_string()),
            &None,
            &None,
            &None,
            &None,
            &vars,
            &None,
            None, // No resume session
        )
        .unwrap();

        assert!(
            !command.contains("--resume"),
            "Fresh command should NOT contain --resume: {}",
            command
        );
        assert!(
            fallback.is_none(),
            "Fresh spawn should not have a fallback command"
        );
    }

    #[test]
    fn test_wrapper_script_contains_session_fallback_when_fallback_provided() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let wrapper_path = write_wrapper_script(
            output_dir,
            "test-task",
            "/tmp/output.log",
            "claude --resume fake-id --print",
            None,
            "claude",
            Some("claude --print < prompt.txt"),
        )
        .unwrap();

        let script = std::fs::read_to_string(&wrapper_path).unwrap();
        assert!(
            script.contains("Session not resumable, starting fresh session"),
            "Wrapper should contain session fallback logic"
        );
        assert!(
            script.contains("No conversation found"),
            "Wrapper should detect 'No conversation found' error"
        );
        assert!(
            script.contains("claude --print < prompt.txt"),
            "Wrapper should contain the fallback command"
        );
        let fallback = script
            .split("Session not resumable, starting fresh session")
            .nth(1)
            .expect("fallback block");
        assert!(
            fallback.contains("} {HEARTBEAT_GUARD_FD}>&-"),
            "fallback executor must not inherit the heartbeat guard writer"
        );
    }

    #[test]
    fn test_wrapper_script_no_fallback_when_none() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let output_dir = temp_dir.path();

        let wrapper_path = write_wrapper_script(
            output_dir,
            "test-task",
            "/tmp/output.log",
            "claude --print",
            None,
            "claude",
            None,
        )
        .unwrap();

        let script = std::fs::read_to_string(&wrapper_path).unwrap();
        assert!(
            !script.contains("Session not resumable"),
            "Wrapper should NOT contain session fallback when no fallback provided"
        );
        assert!(
            script.contains("exec {HEARTBEAT_GUARD_FD}> >(wg heartbeat-watch \"$WG_AGENT_ID\""),
            "wrapper must launch the pipe-guarded heartbeat watcher"
        );
        assert!(
            script.matches("{HEARTBEAT_GUARD_FD}>&-").count() >= 2,
            "executor must close the guard writer and wrapper must close it on completion"
        );
        assert!(
            !script
                .lines()
                .any(|line| line.trim_start().starts_with("sleep 120")),
            "wrapper must not generate the orphan-prone heartbeat sleep command"
        );
        assert!(
            script.contains("Transactional launch gate"),
            "wrapper must not start a handler before the spawn transaction commits"
        );
    }

    struct GlobalConfigGuard {
        saved: Option<std::ffi::OsString>,
        _global: TempDir,
    }

    impl GlobalConfigGuard {
        fn isolated() -> Self {
            let global = TempDir::new().unwrap();
            let saved = std::env::var_os("WG_GLOBAL_DIR");
            unsafe { std::env::set_var("WG_GLOBAL_DIR", global.path()) };
            Self {
                saved,
                _global: global,
            }
        }
    }

    impl Drop for GlobalConfigGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(saved) = self.saved.take() {
                    std::env::set_var("WG_GLOBAL_DIR", saved);
                } else {
                    std::env::remove_var("WG_GLOBAL_DIR");
                }
            }
        }
    }

    fn init_spawn_project(task_ids: &[&str], isolation: bool) -> TempDir {
        let project = TempDir::new().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(project.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "spawn@test.invalid"]);
        git(&["config", "user.name", "Spawn Test"]);
        fs::write(project.path().join("source.txt"), "shared checkout\n").unwrap();
        git(&["add", "source.txt"]);
        git(&["commit", "-qm", "initial"]);

        let dir = project.path().join(".wg");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.toml"),
            format!(
                "[dispatcher]\nworktree_isolation = {}\nauto_test_discovery = false\n\
                 \n[dispatcher.resource_management]\ndisk_sentinel_enabled = false\n",
                isolation
            ),
        )
        .unwrap();
        let mut graph = WorkGraph::new();
        for task_id in task_ids {
            graph.add_node(Node::Task(Task {
                id: (*task_id).to_string(),
                title: format!("isolation transaction {task_id}"),
                exec: Some("sleep 30".to_string()),
                exec_mode: Some("shell".to_string()),
                ..Task::default()
            }));
        }
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();
        project
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(not(unix))]
    fn process_is_alive(_pid: u32) -> bool {
        false
    }

    #[cfg(unix)]
    fn terminate_spawn(pid: u32) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
            libc::waitpid(pid as i32, std::ptr::null_mut(), 0);
        }
    }

    #[cfg(not(unix))]
    fn terminate_spawn(_pid: u32) {}

    #[cfg(target_os = "linux")]
    fn child_pids(pid: u32) -> Vec<u32> {
        fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|value| value.parse().ok())
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn wait_for_handler_child(pid: u32) -> Option<u32> {
        for _ in 0..100 {
            let direct = child_pids(pid);
            for child in direct {
                let cmdline = fs::read(format!("/proc/{child}/cmdline")).unwrap_or_default();
                if String::from_utf8_lossy(&cmdline).contains("sleep") {
                    return Some(child);
                }
                for grandchild in child_pids(child) {
                    let cmdline =
                        fs::read(format!("/proc/{grandchild}/cmdline")).unwrap_or_default();
                    if String::from_utf8_lossy(&cmdline).contains("sleep") {
                        return Some(grandchild);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[serial_test::serial]
    fn stale_initial_worktree_reallocates_and_worker_and_handler_cwd_are_isolated() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["collision-task"], true);
        let dir = project.path().join(".wg");
        let stale = project.path().join(".wg-worktrees/agent-1");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("unknown-dirty.txt"), "preserve exactly").unwrap();

        let result =
            spawn_agent_inner(&dir, "collision-task", "shell", Some("1m"), None, "test").unwrap();
        assert_eq!(result.agent_id, "agent-2");
        assert_eq!(
            fs::read_to_string(stale.join("unknown-dirty.txt")).unwrap(),
            "preserve exactly"
        );
        let isolated = project.path().join(".wg-worktrees/agent-2");
        assert_eq!(
            fs::read_link(format!("/proc/{}/cwd", result.pid)).unwrap(),
            isolated.canonicalize().unwrap()
        );
        #[cfg(target_os = "linux")]
        {
            let handler = wait_for_handler_child(result.pid).expect("sleep handler child");
            assert_eq!(
                fs::read_link(format!("/proc/{handler}/cwd")).unwrap(),
                isolated.canonicalize().unwrap()
            );
        }
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("agents/agent-2/metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["isolation_mode"], "required-worktree");
        terminate_spawn(result.pid);
    }

    #[test]
    #[serial_test::serial]
    fn corrupt_registered_retry_worktree_fails_closed_without_shared_spawn() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["corrupt-retry"], true);
        let dir = project.path().join(".wg");
        let info = worktree::create_worktree(project.path(), &dir, "agent-prior", "corrupt-retry")
            .unwrap();
        fs::write(info.path.join("valuable.txt"), "preserve retry source").unwrap();
        let pointer_path = info.path.join(".git");
        let original_pointer = fs::read_to_string(&pointer_path).unwrap();
        fs::write(&pointer_path, "corrupt-interrupted-pointer\n").unwrap();

        let error = spawn_agent_inner(&dir, "corrupt-retry", "shell", Some("1m"), None, "test")
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("REQUIRED ISOLATION preflight failed"));
        assert!(message.contains("corrupt Git indirection"));
        assert_eq!(
            fs::read_to_string(info.path.join("valuable.txt")).unwrap(),
            "preserve retry source"
        );
        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("corrupt-retry").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.assigned.is_none());
        assert!(AgentRegistry::load(&dir).unwrap().agents.is_empty());
        assert!(!dir.join("agents/agent-1").exists());

        fs::write(pointer_path, original_pointer).unwrap();
        worktree::remove_worktree(project.path(), &info.path, &info.branch).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn explicit_shared_mode_remains_supported_and_recorded() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["shared-task"], false);
        let dir = project.path().join(".wg");
        let result =
            spawn_agent_inner(&dir, "shared-task", "shell", Some("1m"), None, "test").unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("agents/agent-1/metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["isolation_mode"], "shared-explicitly-configured");
        assert_eq!(metadata["worktree_isolation_enabled"], false);
        terminate_spawn(result.pid);
    }

    #[test]
    #[serial_test::serial]
    fn launch_gate_recheck_rejects_a_changed_graph_claim() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["claim-race"], false);
        let graph_path = project.path().join(".wg/graph.jsonl");
        let snapshot = claim_task_for_spawn(&graph_path, "claim-race", "agent-1").unwrap();
        modify_graph(&graph_path, |graph| {
            let task = graph.get_task_mut("claim-race").unwrap();
            task.assigned = Some("agent-other".to_string());
            true
        })
        .unwrap();
        let output_dir = project.path().join(".wg/agents/claim-race-check");
        fs::create_dir_all(&output_dir).unwrap();
        let error = publish_launch_permit_for_claim(
            &graph_path,
            "claim-race",
            "agent-1",
            &output_dir,
            "token",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("claim ownership changed before launch"));
        assert!(!output_dir.join(LAUNCH_GATE_FILE).exists());
        assert!(
            rollback_task_claim(&graph_path, "claim-race", "agent-1", &snapshot).is_err(),
            "rollback must not overwrite the newer owner"
        );
    }

    #[test]
    #[serial_test::serial]
    fn fault_injection_rolls_back_every_spawn_boundary_without_phantoms() {
        let _global = GlobalConfigGuard::isolated();
        for boundary in [
            "workspace-prepared",
            "claim",
            "wrapper-spawned",
            "registry-saved",
            "ownership-registered",
            "metadata-written",
            "before-launch-permit",
        ] {
            let project = init_spawn_project(&["fault-task"], true);
            let dir = project.path().join(".wg");
            LAST_GATED_CHILD_PID.with(|pid| pid.set(0));
            SPAWN_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = Some(boundary));
            let result = spawn_agent_inner(&dir, "fault-task", "shell", Some("1m"), None, "test");
            SPAWN_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = None);
            assert!(result.is_err(), "{boundary} should fail");

            let graph = load_graph(dir.join("graph.jsonl")).unwrap();
            let task = graph.get_task("fault-task").unwrap();
            assert_eq!(task.status, Status::Open, "boundary={boundary}");
            assert!(task.assigned.is_none(), "boundary={boundary}");
            assert!(
                graph.get_task(".assign-fault-task").is_none(),
                "boundary={boundary}"
            );
            assert!(
                AgentRegistry::load(&dir).unwrap().agents.is_empty(),
                "boundary={boundary}"
            );
            assert!(
                worksgood::disk_sentinel::load_ownership(&dir)
                    .unwrap()
                    .caches
                    .is_empty(),
                "boundary={boundary}"
            );
            assert!(
                fs::read_dir(dir.join("agents"))
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true),
                "boundary={boundary}"
            );
            assert!(
                fs::read_dir(project.path().join(".wg-worktrees"))
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true),
                "boundary={boundary}"
            );
            let porcelain = Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(project.path())
                .output()
                .unwrap();
            assert!(porcelain.status.success(), "boundary={boundary}");
            assert_eq!(
                String::from_utf8_lossy(&porcelain.stdout)
                    .lines()
                    .filter(|line| line.starts_with("worktree "))
                    .count(),
                1,
                "boundary={boundary}: leaked Git worktree registration"
            );
            assert!(
                !worktree::branch_exists(project.path(), "wg/agent-1/fault-task").unwrap(),
                "boundary={boundary}: leaked worktree branch"
            );
            let pid = LAST_GATED_CHILD_PID.with(std::cell::Cell::get);
            if pid != 0 {
                assert!(
                    !process_is_alive(pid),
                    "boundary={boundary}, leaked pid={pid}"
                );
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn stale_cache_lease_is_a_collision_and_is_preserved() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["lease-task"], true);
        let dir = project.path().join(".wg");
        let stale = worksgood::disk_sentinel::make_owned_cache(
            &project.path().join("valuable-stale-cache"),
            worksgood::disk_sentinel::CacheKind::Temporary,
            "old-task",
            "agent-1",
            std::process::id(),
            None,
            3_600,
        );
        worksgood::disk_sentinel::register_owned_cache(&dir, stale.clone()).unwrap();

        let mut locked = AgentRegistry::load_locked(&dir).unwrap();
        let workspace =
            prepare_spawn_workspace(&dir, project.path(), "lease-task", true, &mut locked).unwrap();
        assert_eq!(workspace.agent_id, "agent-2");
        drop(workspace);
        drop(locked);
        assert_eq!(
            worksgood::disk_sentinel::load_ownership(&dir)
                .unwrap()
                .caches,
            vec![stale]
        );
    }

    #[test]
    #[serial_test::serial]
    fn aborted_retry_restores_existing_cleanup_marker() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["retry-marker"], true);
        let dir = project.path().join(".wg");
        let info =
            worktree::create_worktree(project.path(), &dir, "agent-prior", "retry-marker").unwrap();
        let marker = info
            .path
            .join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER);
        fs::write(&marker, b"prior-attempt\n").unwrap();
        let mut registry = AgentRegistry::new();
        let prior =
            registry.register_agent(u32::MAX - 1, "retry-marker", "shell", "/tmp/prior-output");
        registry.set_worktree_path(&prior, &info.path);
        registry.set_status(&prior, AgentStatus::Done);
        registry.save(&dir).unwrap();

        SPAWN_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = Some("wrapper-spawned"));
        let result = spawn_agent_inner(&dir, "retry-marker", "shell", Some("1m"), None, "test");
        SPAWN_FAULT_BOUNDARY.with(|fault| *fault.borrow_mut() = None);
        assert!(result.is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"prior-attempt\n");
        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("retry-marker").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.assigned.is_none());

        worktree::remove_worktree(project.path(), &info.path, &info.branch).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn live_or_other_terminal_attempt_cannot_reuse_worktree_path() {
        let _global = GlobalConfigGuard::isolated();
        for live in [false, true] {
            let project = init_spawn_project(&["owner-task"], true);
            let dir = project.path().join(".wg");
            let info =
                worktree::create_worktree(project.path(), &dir, "agent-1", "owner-task").unwrap();
            let mut registry = AgentRegistry::new();
            let id = registry.register_agent(
                if live {
                    std::process::id()
                } else {
                    u32::MAX - 1
                },
                if live { "owner-task" } else { "different-task" },
                "shell",
                "/tmp/out",
            );
            registry.set_worktree_path(&id, &info.path);
            if !live {
                registry.set_status(&id, AgentStatus::Done);
            }
            registry.save(&dir).unwrap();
            let mut locked = AgentRegistry::load_locked(&dir).unwrap();
            let error =
                prepare_spawn_workspace(&dir, project.path(), "owner-task", true, &mut locked)
                    .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("owned by"), "{message}");
            assert!(info.path.exists());
            drop(locked);
            worktree::remove_worktree(project.path(), &info.path, &info.branch).unwrap();
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[serial_test::serial]
    fn concurrent_spawns_allocate_unique_ids_branches_paths_and_cwds() {
        let _global = GlobalConfigGuard::isolated();
        let project = init_spawn_project(&["parallel-a", "parallel-b"], true);
        let dir = project.path().join(".wg");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for task in ["parallel-a", "parallel-b"] {
            let barrier = barrier.clone();
            let dir = dir.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                spawn_agent_inner(&dir, task, "shell", Some("1m"), None, "test").unwrap()
            }));
        }
        barrier.wait();
        let mut results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        assert_eq!(results[0].agent_id, "agent-1");
        assert_eq!(results[1].agent_id, "agent-2");
        let registry = AgentRegistry::load(&dir).unwrap();
        assert_eq!(registry.agents.len(), 2);
        let mut paths = std::collections::HashSet::new();
        for result in &results {
            let entry = registry.get_agent(&result.agent_id).unwrap();
            let path = PathBuf::from(entry.worktree_path.as_ref().unwrap());
            assert!(paths.insert(path.clone()));
            assert_eq!(
                fs::read_link(format!("/proc/{}/cwd", result.pid)).unwrap(),
                path.canonicalize().unwrap()
            );
            assert!(
                worktree::branch_exists(
                    project.path(),
                    &format!("wg/{}/{}", result.agent_id, result.task_id)
                )
                .unwrap()
            );
        }
        for result in results {
            terminate_spawn(result.pid);
        }
    }
}
