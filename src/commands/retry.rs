use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::config::{DispatchRole, ReasoningLevel, Tier};
use worksgood::graph::{LogEntry, Status};
use worksgood::lifecycle::LifecycleActor;
use worksgood::parser::modify_graph;
use worksgood::service::AgentRegistry;

use super::claim_lifecycle;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use worksgood::parser::load_graph;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetryProfileSelection {
    profile_name: String,
    /// The selected profile content fingerprint is its immutable generation.
    profile_generation: String,
    profile_selected_at: String,
    route: String,
    executor: String,
    reasoning: Option<ReasoningLevel>,
}

fn resolve_current_profile(dir: &Path) -> Result<RetryProfileSelection> {
    let association_before = worksgood::profile::project::read_association(dir)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No project profile is selected. Select one with `wg profile select <name>` before using `wg retry --current-profile`; no global profile fallback was attempted."
            )
        })?;

    // This loader verifies the association's project identity and exact profile
    // fingerprint before applying the profile as the authoritative routing
    // overlay. Resolve once, here in the operator command — never in a later
    // dispatcher tick.
    let config = worksgood::config::Config::load_merged(dir)
        .context("Cannot resolve the current project profile for retry")?;
    let association_after = worksgood::profile::project::read_association(dir)?
        .ok_or_else(|| anyhow::anyhow!("Project profile selection was cleared during retry"))?;
    if association_before != association_after {
        anyhow::bail!(
            "Project profile changed while retry routing was being resolved. Nothing was retried; run the command again."
        );
    }

    let resolved = config.resolve_model_for_role(DispatchRole::TaskAgent);
    let route = resolved.spawn_model_spec();
    let (handler, remainder) = route.split_once(':').ok_or_else(|| {
        anyhow::anyhow!(
            "Current project profile '{}' resolved an implicit task-agent route {:?}. Retry refused because --current-profile never guesses an execution system.",
            association_before.profile,
            route
        )
    })?;
    if remainder.trim().is_empty() || !matches!(handler, "pi" | "claude" | "codex") {
        anyhow::bail!(
            "Current project profile '{}' resolved unsupported task-agent route {:?}. Expected an explicit pi:, claude:, or codex: route; no fallback was attempted.",
            association_before.profile,
            route
        );
    }
    if handler == "pi" {
        worksgood::config::parse_exact_pi_route(&route).with_context(|| {
            format!(
                "Current project profile '{}' has an invalid Pi task-agent route",
                association_before.profile
            )
        })?;
    }
    let executor = worksgood::dispatch::handler_for_model(&route)
        .as_str()
        .to_string();

    Ok(RetryProfileSelection {
        profile_name: association_before.profile,
        profile_generation: association_before.profile_fingerprint,
        profile_selected_at: association_before.selected_at,
        route,
        executor,
        reasoning: resolved.reasoning,
    })
}

fn pin_retry_profile(task: &mut worksgood::graph::Task, selection: &RetryProfileSelection) {
    // Exact task fields beat every dispatcher/profile cascade. Clear stale
    // route selectors at command time, then store the resolved route +
    // reasoning snapshot. Session selectors remain owned by the old attempt
    // until the exact-owner release transaction clears them.
    task.model = Some(selection.route.clone());
    task.reasoning = selection.reasoning;
    task.provider = None;
    task.endpoint = None;
    task.profile = None;
    task.executor_preset_name = None;
    task.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: Some("retry-current-profile".to_string()),
        user: Some(worksgood::current_user()),
        message: format!(
            "Retry route pinned at command time: profile={} generation={} selected_at={} executor={} model={} reasoning={} (stale task route/profile/session selection cleared)",
            selection.profile_name,
            selection.profile_generation,
            selection.profile_selected_at,
            selection.executor,
            selection.route,
            selection
                .reasoning
                .map(|level| level.to_string())
                .unwrap_or_else(|| "omitted".to_string())
        ),
    });
}

pub fn run(
    dir: &Path,
    id: &str,
    preserve_session: bool,
    fresh: bool,
    reason: Option<&str>,
) -> Result<()> {
    run_with_selection(dir, id, preserve_session, fresh, reason, None)
}

pub fn run_with_current_profile(
    dir: &Path,
    id: &str,
    fresh: bool,
    reason: Option<&str>,
) -> Result<()> {
    let selection = resolve_current_profile(dir)?;
    run_with_selection(dir, id, false, fresh, reason, Some(selection))
}

fn run_with_selection(
    dir: &Path,
    id: &str,
    _preserve_session: bool,
    fresh: bool,
    reason: Option<&str>,
    profile_selection: Option<RetryProfileSelection>,
) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Look up the task's current status to decide which retry path to take.
    // For InProgress tasks we kill the assigned agent and reset to Open
    // (incrementing retry_count, which fail/incomplete normally do for us).
    // For Failed/Incomplete we follow the existing reset path.
    let initial_status = {
        let graph = worksgood::parser::load_graph(&path).context("Failed to load graph")?;
        graph.get_task(id).map(|t| t.status)
    };

    if initial_status == Some(Status::InProgress) {
        return retry_in_progress(
            dir,
            &path,
            id,
            false,
            fresh,
            reason,
            profile_selection.as_ref(),
        );
    }

    // Worktree mutation is deliberately deferred to the exact-owner reaper.
    // In particular, --fresh must never delete a worktree while its prior Pi
    // writer is live. Default retry-in-place preserves all WIP bytes.
    let fresh_removed_path: Option<std::path::PathBuf> = None;
    if !fresh
        && let Some(project_root) = dir.parent()
        && let Some((wt_path, _)) =
            crate::commands::spawn::worktree::find_worktree_for_task(project_root, id)
    {
        let marker = wt_path.join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER);
        if marker.exists() {
            let _ = std::fs::remove_file(&marker);
        }
    }

    let config = worksgood::config::Config::load_or_default(dir);
    let escalate_on_retry = config.coordinator.escalate_on_retry;

    let mut error: Option<anyhow::Error> = None;
    let mut prev_failure_reason: Option<String> = None;
    let mut attempt: u32 = 0;
    let mut retry_count: u32 = 0;
    let mut max_retries: Option<u32> = None;
    let mut was_incomplete = false;
    let mut was_pending_eval_stuck = false;
    let mut tier_escalation_msg: Option<String> = None;
    let mut downstream_cleared: Vec<String> = Vec::new();
    let mut breaker_reset = false;

    // Snapshot the registry once outside the graph lock — eager
    // downstream walk consults it to decide whether each downstream
    // claim is stale (Dead-or-missing-or-unreachable agent).
    let registry_snapshot = AgentRegistry::load(dir).unwrap_or_else(|_| AgentRegistry::new());

    modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        // PendingEval/FailedPendingEval are also retriable (fix-no-cli): a task
        // stuck in `operator-required-ambiguity` (eval-pipeline-repair-exhausted)
        // has no other sanctioned CLI exit. `wg retry` clears the stuck gate by
        // minting a fresh evaluation attempt (`begin_source_attempt` below) so
        // the operator never needs graph.jsonl surgery.
        let was_pending_eval = matches!(
            task.status,
            Status::PendingEval | Status::FailedPendingEval
        );
        if !matches!(
            task.status,
            Status::Failed
                | Status::Abandoned
                | Status::Incomplete
                | Status::PendingEval
                | Status::FailedPendingEval
        ) {
            error = Some(anyhow::anyhow!(
                "Task '{}' is not retriable (status: {:?}). Only failed, abandoned, incomplete, pending-eval, failed-pending-eval, or in-progress tasks can be retried.",
                id,
                task.status
            ));
            return false;
        }

        was_incomplete = task.status == Status::Incomplete;
        was_pending_eval_stuck = was_pending_eval;

        // Check if max retries exceeded (for failed tasks)
        if task.status == Status::Failed
            && let Some(max) = task.max_retries
            && task.retry_count >= max
        {
            error = Some(anyhow::anyhow!(
                "Task '{}' has reached max retries ({}/{}). Consider abandoning or increasing max_retries.",
                id,
                task.retry_count,
                max
            ));
            return false;
        }

        prev_failure_reason = task.failure_reason.clone();
        attempt = task.retry_count + 1;

        // Persist intent first. The generation remains non-runnable until the
        // exact old owner is quiescent; late old-owner events are fenced now.
        let (_, newly_requested) = match super::reopen::request(
            task,
            "retry",
            fresh,
            false,
            if fresh {
                "fresh retry after WorkSave/discard"
            } else {
                "new attempt using retained work"
            },
            LifecycleActor::operator(worksgood::current_user()),
            "explicit_retry",
        ) {
            Ok(result) => result,
            Err(rejection) => {
                error = Some(anyhow::anyhow!(rejection));
                return false;
            }
        };
        if !newly_requested {
            return false;
        }
        task.failure_reason = None;
        task.ready_after = None;
        // Reset the per-task spawn circuit breaker so dispatch resumes WITHOUT
        // a graph.jsonl edit (fix-spawn-failures). The breaker may have tripped
        // (status was Incomplete) and left spawn_failures at the threshold.
        let breaker_was_tripped = task.spawn_failures > 0;
        task.spawn_failures = 0;
        task.last_spawn_failure_at = None;
        task.tags.retain(|t| t != "converged");
        if let Some(selection) = profile_selection.as_ref() {
            pin_retry_profile(task, selection);
        }

        // Tier escalation on retry: bump fast→standard→premium
        if escalate_on_retry && !task.no_tier_escalation {
            let current_tier: Tier = task
                .tier
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Tier::Standard);
            let next_tier = current_tier.escalate();
            if next_tier != current_tier {
                task.tier = Some(next_tier.to_string());
                let msg = format!("Tier escalated on retry: {} → {}", current_tier, next_tier);
                task.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: None,
                    user: Some(worksgood::current_user()),
                    message: msg.clone(),
                });
                tier_escalation_msg = Some(msg);
            }
        }

        let source = if was_incomplete {
            "incomplete"
        } else if was_pending_eval {
            "pending-eval/failed-pending-eval (stuck evaluation gate)"
        } else {
            "failed"
        };
        let reason_suffix = reason
            .map(|r| format!(" — reason: {}", r))
            .unwrap_or_default();
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: Some(worksgood::current_user()),
            message: format!(
                "Task reset for retry from {} (attempt #{}){}",
                source,
                task.retry_count + 1,
                reason_suffix
            ),
        });
        // fix-no-cli: when retrying out of a stuck PendingEval/FailedPendingEval
        // (operator-required-ambiguity), record the explicit gate-clear so the
        // operator's sanctioned action is visible in the log. The fresh
        // evaluation attempt minted by `begin_source_attempt` below replaces the
        // exhausted lifecycle (diagnostic + repair_attempts reset).
        if was_pending_eval {
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("eval-lifecycle-clear".to_string()),
                user: Some(worksgood::current_user()),
                message: "Cleared stuck evaluation gate via `wg retry` — a fresh attempt will be minted; no graph.jsonl edit required.".to_string(),
            });
        }
        if breaker_was_tripped {
            breaker_reset = true;
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("spawn-circuit-breaker".to_string()),
                user: Some(worksgood::current_user()),
                message: "Spawn circuit breaker reset via `wg retry` — dispatch will resume.".to_string(),
            });
        }

        retry_count = task.retry_count;
        max_retries = task.max_retries;

        // Eager downstream-claim cleanup (design-claim-lifecycle):
        // walk the forward closure from this seed and clear any
        // downstream task whose claim references a dead agent. This is
        // the user-intent path — `wg retry <upstream>` says "the
        // scheduling context for everything below has changed". Live
        // agents are deliberately untouched; the lazy reconciler
        // catches them later if they die.
        let report = claim_lifecycle::clear_stale_downstream_claims(
            graph,
            &registry_snapshot,
            id,
            id,
        );
        downstream_cleared = report.cleared;

        // Evaluation attempt rearming happens atomically with owner release.
        true
    })
    .context("Failed to modify graph")?;

    if let Some(e) = error {
        return Err(e);
    }

    super::notify_graph_changed(dir);
    let _ = super::reopen::reconcile_pending(dir)?;

    // Record operation
    let _ = worksgood::provenance::record(
        dir,
        "retry",
        Some(id),
        None,
        serde_json::json!({
            "attempt": attempt,
            "prev_failure_reason": prev_failure_reason,
            "was_incomplete": was_incomplete,
            "was_pending_eval_stuck": was_pending_eval_stuck,
            "tier_escalation": tier_escalation_msg,
            "reason": reason,
            "current_profile": profile_selection.as_ref().map(|selection| serde_json::json!({
                "name": &selection.profile_name,
                "generation": &selection.profile_generation,
                "selected_at": &selection.profile_selected_at,
                "executor": &selection.executor,
                "model": &selection.route,
                "reasoning": selection.reasoning.map(|level| level.to_string()),
            })),
        }),
        config.log.rotation_threshold,
    );

    let reopened_graph = worksgood::parser::load_graph(&path)?;
    if let Some(intent) = reopened_graph
        .get_task(id)
        .and_then(|task| task.lifecycle.reopen_intent.as_ref())
    {
        println!("{}", super::reopen::hold_label(intent));
        return Ok(());
    }

    let source = if was_incomplete {
        "incomplete"
    } else if was_pending_eval_stuck {
        "pending-eval/failed-pending-eval (stuck evaluation gate)"
    } else {
        "failed"
    };
    println!(
        "Reset '{}' from {} to open for retry (attempt #{})",
        id,
        source,
        retry_count + 1
    );

    if was_pending_eval_stuck {
        println!(
            "  Cleared stuck evaluation gate — fresh attempt will be minted (no graph.jsonl edit required)."
        );
    }

    if let Some(max) = max_retries {
        println!("  Retries remaining after this: {}", max - retry_count);
    }

    if let Some(ref msg) = tier_escalation_msg {
        println!("  {}", msg);
    }

    if let Some(selection) = profile_selection.as_ref() {
        println!(
            "  Current profile pinned now: {} generation={} executor={} model={} reasoning={}",
            selection.profile_name,
            selection.profile_generation,
            selection.executor,
            selection.route,
            selection
                .reasoning
                .map(|level| level.to_string())
                .unwrap_or_else(|| "omitted".to_string())
        );
    }

    if !downstream_cleared.is_empty() {
        println!(
            "  Cleared stale claim on {} downstream task(s): {}",
            downstream_cleared.len(),
            downstream_cleared.join(", ")
        );
    }

    if breaker_reset {
        println!("  Spawn circuit breaker cleared — worker will dispatch on the next tick.");
    }

    if let Some(p) = fresh_removed_path {
        println!("  --fresh: discarded prior worktree at {:?}", p);
    } else if !fresh {
        // Inform the user that the next attempt will resume in-place if a
        // prior worktree exists.
        if let Some(project_root) = dir.parent() {
            if let Some((wt, _)) =
                crate::commands::spawn::worktree::find_worktree_for_task(project_root, id)
            {
                println!("  New attempt will use retained work at {:?}", wt);
            }
        }
    }

    Ok(())
}

/// Retry an in-progress task by first persisting a fenced reopen intent.
/// Signalling/reaping the exact process owner happens only after that durable
/// hold exists, so a crash cannot leave an unfenced dead owner whose task is
/// redispatched by another reconciler.
///
/// Repeated callers coalesce on the same source generation and cannot mint a
/// second attempt.
fn retry_in_progress(
    dir: &Path,
    path: &Path,
    id: &str,
    _preserve_session: bool,
    fresh: bool,
    reason: Option<&str>,
    profile_selection: Option<&RetryProfileSelection>,
) -> Result<()> {
    // Persist intent before sending any signal. The best-effort reconciliation
    // after the graph transaction requests graceful exit; coordinator restarts
    // resume the same idempotent intent.
    let config = worksgood::config::Config::load_or_default(dir);
    let escalate_on_retry = config.coordinator.escalate_on_retry;
    let mut error: Option<anyhow::Error> = None;
    let mut attempt: u32 = 0;
    let mut tier_escalation_msg: Option<String> = None;
    let mut downstream_cleared: Vec<String> = Vec::new();
    let mut retry_generation_already_started = false;

    // Snapshot claim liveness once outside the graph lock for downstream
    // cleanup. The source owner's registry entry remains live until exact reap.
    let registry_snapshot = AgentRegistry::load(dir).unwrap_or_else(|_| AgentRegistry::new());

    modify_graph(path, |graph| {
        let task = match graph.get_task_mut(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        retry_generation_already_started = task
            .lifecycle
            .reopen_intent
            .as_ref()
            .is_some_and(|intent| intent.operation == "retry")
            || (task.status == Status::Open
                && task.assigned.is_none()
                && task.lifecycle.current_attempt.is_none());
        let requested_retry_count = task.retry_count.saturating_add(1);

        // Honor max_retries before starting a generation ourselves.
        if !retry_generation_already_started
            && let Some(max) = task.max_retries
            && task.retry_count >= max
        {
            error = Some(anyhow::anyhow!(
                "Task '{}' has reached max retries ({}/{}). Consider abandoning or increasing max_retries.",
                id,
                task.retry_count,
                max
            ));
            return false;
        }

        if !retry_generation_already_started {
            task.retry_count = task.retry_count.max(requested_retry_count);
        }
        attempt = task.retry_count;
        if !retry_generation_already_started {
            let (_, newly_requested) = match super::reopen::request(
                task,
                "retry",
                fresh,
                false,
                if fresh {
                    "fresh in-progress retry after WorkSave/discard"
                } else {
                    "new attempt using retained work"
                },
                LifecycleActor::operator(worksgood::current_user()),
                "retry_in_progress",
            ) {
                Ok(result) => result,
                Err(rejection) => {
                    error = Some(anyhow::anyhow!(rejection));
                    return false;
                }
            };
            if !newly_requested {
                retry_generation_already_started = true;
            }
        }
        task.failure_reason = None;
        task.ready_after = None;
        // Reset the per-task spawn circuit breaker (fix-spawn-failures) so an
        // in-progress retry dispatches instead of being skipped forever.
        task.spawn_failures = 0;
        task.last_spawn_failure_at = None;
        task.tags.retain(|t| t != "converged");
        if let Some(selection) = profile_selection {
            pin_retry_profile(task, selection);
        }

        if escalate_on_retry && !task.no_tier_escalation {
            let current_tier: Tier = task
                .tier
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Tier::Standard);
            let next_tier = current_tier.escalate();
            if next_tier != current_tier {
                task.tier = Some(next_tier.to_string());
                let msg = format!("Tier escalated on retry: {} → {}", current_tier, next_tier);
                task.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: None,
                    user: Some(worksgood::current_user()),
                    message: msg.clone(),
                });
                tier_escalation_msg = Some(msg);
            }
        }

        let reason_suffix = reason
            .map(|r| format!(" — reason: {}", r))
            .unwrap_or_default();
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: Some(worksgood::current_user()),
            message: format!(
                "Task fenced for retry from in-progress (attempt #{}) — waiting for exact owner release{}",
                task.retry_count, reason_suffix
            ),
        });

        // Eager downstream-claim cleanup — see the failed-path branch
        // for rationale. Same call, same semantics.
        let report = claim_lifecycle::clear_stale_downstream_claims(
            graph,
            &registry_snapshot,
            id,
            id,
        );
        downstream_cleared = report.cleared;

        true
    })
    .context("Failed to modify graph")?;

    if let Some(e) = error {
        return Err(e);
    }

    // Reaper owns fresh deletion and generation enablement in that order.
    super::notify_graph_changed(dir);
    let _ = super::reopen::reconcile_pending(dir)?;

    let _ = worksgood::provenance::record(
        dir,
        "retry",
        Some(id),
        None,
        serde_json::json!({
            "attempt": attempt,
            "was_in_progress": true,
            "owner_release": "durable-intent-then-exact-reap",
            "tier_escalation": tier_escalation_msg,
            "reason": reason,
            "current_profile": profile_selection.map(|selection| serde_json::json!({
                "name": &selection.profile_name,
                "generation": &selection.profile_generation,
                "selected_at": &selection.profile_selected_at,
                "executor": &selection.executor,
                "model": &selection.route,
                "reasoning": selection.reasoning.map(|level| level.to_string()),
            })),
        }),
        config.log.rotation_threshold,
    );

    let reopened_graph = worksgood::parser::load_graph(path)?;
    if let Some(intent) = reopened_graph
        .get_task(id)
        .and_then(|task| task.lifecycle.reopen_intent.as_ref())
    {
        println!("{}", super::reopen::hold_label(intent));
        return Ok(());
    }

    println!(
        "Reset '{}' from in-progress to open (attempt #{}) after exact owner release",
        id, attempt
    );
    if let Some(msg) = tier_escalation_msg {
        println!("  {}", msg);
    }
    if let Some(selection) = profile_selection {
        println!(
            "  Current profile pinned now: {} generation={} executor={} model={} reasoning={}",
            selection.profile_name,
            selection.profile_generation,
            selection.executor,
            selection.route,
            selection
                .reasoning
                .map(|level| level.to_string())
                .unwrap_or_else(|| "omitted".to_string())
        );
    }
    if !downstream_cleared.is_empty() {
        println!(
            "  Cleared stale claim on {} downstream task(s): {}",
            downstream_cleared.len(),
            downstream_cleared.join(", ")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn make_task(id: &str, title: &str, status: Status) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            status,
            ..Task::default()
        }
    }

    fn setup_workgraph(dir: &Path, tasks: Vec<Task>) -> std::path::PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = graph_path(dir);
        let mut graph = WorkGraph::new();
        for task in tasks {
            graph.add_node(Node::Task(task));
        }
        save_graph(&graph, &path).unwrap();
        path
    }

    fn source_with_eval_satellites(status: Status) -> Vec<Task> {
        use worksgood::config::{Config, ReasoningLevel, RoleModelConfig};
        use worksgood::eval_lifecycle::{
            AgencyStage, DispatchSelectionSource, EvaluationLifecycle, build_plan,
        };

        let initial = make_task("t1", "Test task", Status::InProgress);
        let mut config = Config::default();
        let route = RoleModelConfig {
            provider: None,
            model: Some("pi:openai-codex:gpt-5.6-sol".into()),
            tier: None,
            endpoint: Some("pinned-endpoint".into()),
            reasoning: Some(ReasoningLevel::Xhigh),
        };
        config.models.evaluator = Some(route.clone());
        config.models.flip_inference = Some(route.clone());
        config.models.flip_comparison = Some(route);
        let flip_plan = build_plan(
            &config,
            &initial,
            ".flip-t1",
            DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap();
        assert!(
            flip_plan
                .calls
                .iter()
                .any(|call| call.stage == AgencyStage::FlipComparison)
        );
        let eval_plan = build_plan(
            &config,
            &initial,
            ".evaluate-t1",
            DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap();
        let mut source = initial;
        source.status = status;
        source.retry_count = 1;
        let mut attempt_one = source.clone();
        attempt_one.retry_count = 0;
        source.evaluation_lifecycle = Some(EvaluationLifecycle::for_source(&attempt_one));
        vec![
            source,
            Task {
                id: ".flip-t1".into(),
                title: "flip".into(),
                status: Status::Done,
                model: Some("pi:openai-codex:gpt-5.6-sol".into()),
                reasoning: Some(ReasoningLevel::Xhigh),
                agency_dispatch: Some(flip_plan),
                ..Task::default()
            },
            Task {
                id: ".evaluate-t1".into(),
                title: "eval".into(),
                status: Status::Done,
                model: Some("pi:openai-codex:gpt-5.6-sol".into()),
                reasoning: Some(ReasoningLevel::Xhigh),
                agency_dispatch: Some(eval_plan),
                ..Task::default()
            },
        ]
    }

    #[test]
    fn test_failed_retry_mints_parent_and_rearms_unconsumed_attempt_routes() {
        let dir = tempdir().unwrap();
        let tasks = source_with_eval_satellites(Status::Failed);
        let old_calls: Vec<_> = tasks[1..]
            .iter()
            .map(|task| task.agency_dispatch.as_ref().unwrap().calls.clone())
            .collect();
        setup_workgraph(dir.path(), tasks);

        run(dir.path(), "t1", false, false, None).unwrap();
        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let source = graph.get_task("t1").unwrap();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.source_attempt, 2);
        for (index, task_id) in [".flip-t1", ".evaluate-t1"].iter().enumerate() {
            let satellite = graph.get_task(task_id).unwrap();
            let plan = satellite.agency_dispatch.as_ref().unwrap();
            assert_eq!(satellite.status, Status::Open);
            assert_eq!(plan.pipeline_id, lifecycle.pipeline_id);
            assert_eq!(plan.source_attempt, 2);
            assert_eq!(plan.calls, old_calls[index]);
        }
    }

    #[test]
    fn test_fresh_failed_retry_rearms_same_immutable_routes() {
        let dir = tempdir().unwrap();
        let tasks = source_with_eval_satellites(Status::Failed);
        let old_calls: Vec<_> = tasks[1..]
            .iter()
            .map(|task| task.agency_dispatch.as_ref().unwrap().calls.clone())
            .collect();
        setup_workgraph(dir.path(), tasks);

        run(dir.path(), "t1", false, true, None).unwrap();
        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let source = graph.get_task("t1").unwrap();
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.source_attempt, 2);
        for (index, task_id) in [".flip-t1", ".evaluate-t1"].iter().enumerate() {
            let satellite = graph.get_task(task_id).unwrap();
            let plan = satellite.agency_dispatch.as_ref().unwrap();
            assert_eq!(satellite.status, Status::Open);
            assert_eq!(plan.pipeline_id, lifecycle.pipeline_id);
            assert_eq!(plan.calls, old_calls[index]);
        }
    }

    /// fix-no-cli: `wg retry` MUST accept a task stuck in PendingEval with an
    /// `operator-required-ambiguity` (eval-pipeline-repair-exhausted) diagnostic
    /// and clear the stuck gate by minting a fresh evaluation attempt — with NO
    /// graph.jsonl edit. This is the recurring tar pit (5 tasks this session).
    #[test]
    fn test_retry_pending_eval_clears_stuck_gate() {
        let dir = tempdir().unwrap();
        let mut tasks = source_with_eval_satellites(Status::PendingEval);
        // Simulate the stuck state: repair exhausted + operator-required diagnostic.
        {
            let source = &mut tasks[0];
            let lifecycle = source.evaluation_lifecycle.as_mut().unwrap();
            lifecycle.repair_attempts = u16::MAX;
            lifecycle.diagnostic = Some(
                "error[WG-EVAL-PIPELINE-REPAIR-EXHAUSTED]: bounded repair already ran for t1-p1-s1"
                    .to_string(),
            );
        }
        setup_workgraph(dir.path(), tasks);

        run(dir.path(), "t1", false, false, None).unwrap();

        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let source = graph.get_task("t1").unwrap();
        assert_eq!(source.status, Status::Open);
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        // A fresh attempt mints a new pipeline + bumps source_attempt.
        assert_eq!(
            lifecycle.source_attempt, 2,
            "retry must mint a fresh evaluation attempt"
        );
        assert_eq!(
            lifecycle.repair_attempts, 0,
            "fresh attempt must reset repair_attempts"
        );
        assert!(
            lifecycle.diagnostic.is_none(),
            "fresh attempt must clear the stuck diagnostic, got: {:?}",
            lifecycle.diagnostic
        );
        assert!(
            worksgood::eval_lifecycle::evaluation_health(&graph, "t1").is_none(),
            "an Open retry must no longer report operator-required evaluation health"
        );
        // Satellites are re-armed for the fresh attempt.
        for task_id in [".flip-t1", ".evaluate-t1"] {
            let satellite = graph.get_task(task_id).unwrap();
            assert_eq!(
                satellite.status,
                Status::Open,
                "{task_id} should be re-armed to Open"
            );
            let plan = satellite.agency_dispatch.as_ref().unwrap();
            assert_eq!(plan.source_attempt, lifecycle.source_attempt);
            assert_eq!(plan.pipeline_id, lifecycle.pipeline_id);
        }
        assert!(
            source.log.iter().any(|e| e
                .message
                .contains("Cleared stuck evaluation gate via `wg retry`")),
            "retry should log the explicit eval-gate clear"
        );
    }

    /// fix-no-cli: `wg retry` also accepts FailedPendingEval (agent exited without
    /// `wg done`, awaiting rescue eval that got stuck).
    #[test]
    fn test_retry_failed_pending_eval_clears_stuck_gate() {
        let dir = tempdir().unwrap();
        let mut tasks = source_with_eval_satellites(Status::FailedPendingEval);
        {
            let source = &mut tasks[0];
            let lifecycle = source.evaluation_lifecycle.as_mut().unwrap();
            lifecycle.repair_attempts = u16::MAX;
            lifecycle.diagnostic = Some(
                "error[WG-EVAL-PIPELINE-REPAIR-EXHAUSTED]: bounded repair already ran".to_string(),
            );
        }
        setup_workgraph(dir.path(), tasks);

        run(dir.path(), "t1", false, false, None).unwrap();

        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let source = graph.get_task("t1").unwrap();
        assert_eq!(source.status, Status::Open);
        let lifecycle = source.evaluation_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.repair_attempts, 0);
        assert!(lifecycle.diagnostic.is_none());
    }

    #[test]
    fn test_retry_failed_task_transitions_to_open() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.failure_reason = Some("timeout".to_string());
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[test]
    fn test_retry_incomplete_task_transitions_to_open() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Incomplete);
        task.retry_count = 1;
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[test]
    fn test_retry_incomplete_clears_ready_after() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Incomplete);
        task.retry_count = 1;
        task.ready_after = Some("2099-01-01T00:00:00Z".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.ready_after, None,
            "Retry should clear ready_after cooldown"
        );
    }

    #[test]
    fn test_retry_non_failed_task_errors_open() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not retriable"),
            "Expected error about status, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_retry_in_progress_task_resets_to_open() {
        // An InProgress task with no recorded owner releases immediately;
        // real live-owner hold/reap behavior is covered by the Fake-Pi smoke.
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.retry_count = 0;
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, Some("hung 20min"));
        assert!(
            result.is_ok(),
            "retry on in-progress should succeed: {:?}",
            result
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(
            task.retry_count, 1,
            "in-progress retry must increment retry_count"
        );
        assert_eq!(task.assigned, None);
        assert!(
            task.log.iter().any(|e| e.message.contains("hung 20min")),
            "reason must be recorded in task log"
        );
    }

    #[test]
    fn test_retry_in_progress_does_not_double_count_reconciler_race() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Open);
        task.retry_count = 1;
        setup_workgraph(dir_path, vec![task]);

        let path = graph_path(dir_path);
        retry_in_progress(dir_path, &path, "t1", false, false, None, None).unwrap();

        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.retry_count, 1, "same preemption must not count twice");
        assert!(
            task.evaluation_lifecycle.is_none(),
            "same preemption must not mint another evaluation generation"
        );
    }

    #[test]
    fn test_retry_non_failed_task_errors_done() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Done)]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not retriable"));
    }

    #[test]
    fn test_retry_preserves_retry_count() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 3;
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.retry_count, 3,
            "retry_count should be preserved, not reset"
        );
    }

    #[test]
    fn test_retry_clears_failure_reason() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.failure_reason = Some("compilation error".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.failure_reason, None);
    }

    /// fix-spawn-failures: `wg retry` MUST clear the per-task spawn circuit
    /// breaker so dispatch resumes WITHOUT a graph.jsonl edit. The breaker
    /// trips on Incomplete (its terminal state), so retry from Incomplete is
    /// the canonical recovery path.
    #[test]
    fn test_retry_clears_spawn_failures() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Incomplete);
        task.retry_count = 1;
        task.spawn_failures = 5;
        task.last_spawn_failure_at = Some(Utc::now().to_rfc3339());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.spawn_failures, 0, "retry must clear spawn_failures");
        assert_eq!(task.status, Status::Open);
        assert!(
            task.last_spawn_failure_at.is_none(),
            "retry must clear last_spawn_failure_at"
        );
        assert!(
            task.log.iter().any(|e| e
                .message
                .contains("Spawn circuit breaker reset via `wg retry`")),
            "retry should log the breaker reset"
        );
    }

    /// fix-spawn-failures: `wg retry` on an in-progress task also clears the
    /// breaker so the next tick dispatches instead of being skipped.
    #[test]
    fn test_retry_in_progress_clears_spawn_failures() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.retry_count = 0;
        task.spawn_failures = 5;
        task.last_spawn_failure_at = Some(Utc::now().to_rfc3339());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, Some("hung")).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.spawn_failures, 0,
            "in-progress retry must clear breaker"
        );
        assert!(task.last_spawn_failure_at.is_none());
    }

    #[test]
    fn test_retry_clears_assigned() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.assigned, None);
    }

    #[test]
    fn test_retry_max_retries_exceeded() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 3;
        task.max_retries = Some(3);
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("max retries"),
            "Expected 'max retries' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_retry_within_max_retries_succeeds() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.max_retries = Some(3);
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", false, false, None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[test]
    fn test_retry_adds_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 2;
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.log.is_empty());
        let last_log = task.log.last().unwrap();
        assert!(
            last_log.message.contains("retry"),
            "Log message should mention retry, got: {}",
            last_log.message
        );
        assert!(
            last_log.message.contains("3"),
            "Log message should contain attempt number 3, got: {}",
            last_log.message
        );
    }

    #[test]
    fn test_retry_task_not_found() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Failed)]);

        let result = run(dir_path, "nonexistent", false, false, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_retry_clears_session_id() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.session_id = Some("fce3a8ba-549c-440d-882d-dbfd5d2b371a".to_string());
        task.checkpoint = Some("Previous checkpoint context".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.session_id, None,
            "Retry should clear session_id to avoid --resume with dead session"
        );
        assert_eq!(
            task.checkpoint, None,
            "Retry should clear checkpoint along with session_id"
        );
    }

    #[test]
    fn test_retry_preserve_session_is_lineage_not_continuation() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.session_id = Some("keep-me-alive".to_string());
        task.checkpoint = Some("checkpoint content".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", true, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.session_id, None,
            "retry is a new attempt and must not claim exact-session continuation"
        );
        assert_eq!(
            task.checkpoint, None,
            "retained work may provide lineage, but ambient continuation selectors are fenced"
        );
    }

    #[test]
    fn test_retry_clears_converged_tag() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tags.push("converged".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(
            !task.tags.contains(&"converged".to_string()),
            "Retry should clear converged tag"
        );
    }

    #[test]
    fn test_retry_incomplete_log_mentions_incomplete() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Incomplete);
        task.retry_count = 1;
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        let last_log = task.log.last().unwrap();
        assert!(
            last_log.message.contains("incomplete"),
            "Log should mention source was incomplete, got: {}",
            last_log.message
        );
    }

    fn current_profile_selection() -> RetryProfileSelection {
        RetryProfileSelection {
            profile_name: "current".to_string(),
            profile_generation: "b3:current-generation".to_string(),
            profile_selected_at: "2026-07-26T00:00:00Z".to_string(),
            route: "pi:openai-codex:current-worker".to_string(),
            executor: "pi".to_string(),
            reasoning: Some(ReasoningLevel::High),
        }
    }

    #[test]
    fn current_profile_retry_atomically_replaces_stale_selection_in_all_held_states() {
        for status in [
            Status::Failed,
            Status::Incomplete,
            Status::PendingEval,
            Status::FailedPendingEval,
        ] {
            let dir = tempdir().unwrap();
            let mut task = make_task("t1", "Test task", status);
            task.retry_count = 1;
            task.model = Some("pi:openrouter:old-worker".to_string());
            task.reasoning = Some(ReasoningLevel::Low);
            task.provider = Some("old-provider".to_string());
            task.endpoint = Some("old-endpoint".to_string());
            task.profile = Some("old-profile".to_string());
            task.executor_preset_name = Some("codex".to_string());
            task.session_id = Some("old-session".to_string());
            task.checkpoint = Some("old-checkpoint".to_string());
            setup_workgraph(dir.path(), vec![task]);

            run_with_selection(
                dir.path(),
                "t1",
                false,
                false,
                None,
                Some(current_profile_selection()),
            )
            .unwrap();

            let graph = load_graph(&graph_path(dir.path())).unwrap();
            let task = graph.get_task("t1").unwrap();
            assert_eq!(task.status, Status::Open, "source status: {status:?}");
            assert_eq!(
                task.model.as_deref(),
                Some("pi:openai-codex:current-worker")
            );
            assert_eq!(task.reasoning, Some(ReasoningLevel::High));
            assert!(task.provider.is_none());
            assert!(task.endpoint.is_none());
            assert!(task.profile.is_none());
            assert!(task.executor_preset_name.is_none());
            assert!(task.session_id.is_none());
            assert!(task.checkpoint.is_none());
            let route_log = task
                .log
                .iter()
                .find(|entry| entry.actor.as_deref() == Some("retry-current-profile"))
                .expect("route audit log");
            assert!(route_log.message.contains("profile=current"));
            assert!(
                route_log
                    .message
                    .contains("generation=b3:current-generation")
            );
            assert!(route_log.message.contains("executor=pi"));
            assert!(route_log.message.contains("reasoning=high"));
        }
    }

    #[test]
    fn current_profile_retry_repins_in_progress_task() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.model = Some("pi:openrouter:old-worker".to_string());
        task.reasoning = Some(ReasoningLevel::Low);
        task.session_id = Some("old-session".to_string());
        setup_workgraph(dir.path(), vec![task]);

        run_with_selection(
            dir.path(),
            "t1",
            false,
            false,
            Some("hung"),
            Some(current_profile_selection()),
        )
        .unwrap();

        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.retry_count, 1);
        assert_eq!(
            task.model.as_deref(),
            Some("pi:openai-codex:current-worker")
        );
        assert_eq!(task.reasoning, Some(ReasoningLevel::High));
        assert!(task.session_id.is_none());
    }

    #[test]
    fn retry_snapshot_survives_later_profile_change_before_spawn() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        setup_workgraph(dir.path(), vec![task]);

        run_with_selection(
            dir.path(),
            "t1",
            false,
            false,
            None,
            Some(current_profile_selection()),
        )
        .unwrap();

        // A later profile flip changes dispatcher defaults, but cannot mutate
        // the exact task fields written by the retry transaction.
        let mut later_config = worksgood::config::Config::default();
        later_config.models.task_agent = Some(worksgood::config::RoleModelConfig {
            provider: None,
            model: Some("pi:openrouter:later-worker".to_string()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::Low),
        });
        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        let plan = worksgood::dispatch::plan_spawn(
            task,
            &later_config,
            None,
            Some("pi:openrouter:later-worker"),
        )
        .unwrap();
        assert_eq!(plan.executor, worksgood::dispatch::ExecutorKind::Pi);
        assert_eq!(plan.reasoning, Some(ReasoningLevel::High));
        assert!(plan.provenance.model_source.starts_with("task.model"));
        assert_eq!(
            task.model.as_deref(),
            Some("pi:openai-codex:current-worker")
        );
    }

    fn setup_config_with_escalation(dir: &Path) {
        let config_path = dir.join("config.toml");
        fs::write(config_path, "[coordinator]\nescalate_on_retry = true\n").unwrap();
    }

    #[test]
    fn test_retry_escalates_tier_standard_to_premium() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tier = Some("standard".to_string());
        setup_workgraph(dir_path, vec![task]);
        setup_config_with_escalation(dir_path);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.tier, Some("premium".to_string()));
        assert!(
            task.log
                .iter()
                .any(|e| e.message.contains("Tier escalated")),
            "Should log tier escalation"
        );
    }

    #[test]
    fn test_retry_escalates_tier_fast_to_standard() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tier = Some("fast".to_string());
        setup_workgraph(dir_path, vec![task]);
        setup_config_with_escalation(dir_path);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.tier, Some("standard".to_string()));
    }

    #[test]
    fn test_retry_caps_at_premium() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tier = Some("premium".to_string());
        setup_workgraph(dir_path, vec![task]);
        setup_config_with_escalation(dir_path);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.tier,
            Some("premium".to_string()),
            "Premium should not escalate further"
        );
        assert!(
            !task
                .log
                .iter()
                .any(|e| e.message.contains("Tier escalated")),
            "No escalation log when already at premium"
        );
    }

    #[test]
    fn test_retry_no_escalation_without_config() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tier = Some("fast".to_string());
        setup_workgraph(dir_path, vec![task]);
        // No escalation config — default is false

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.tier,
            Some("fast".to_string()),
            "Should not escalate when config is off"
        );
    }

    #[test]
    fn test_retry_no_escalation_with_opt_out() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        task.tier = Some("fast".to_string());
        task.no_tier_escalation = true;
        setup_workgraph(dir_path, vec![task]);
        setup_config_with_escalation(dir_path);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.tier,
            Some("fast".to_string()),
            "Should not escalate when no_tier_escalation is set"
        );
    }

    #[test]
    fn test_retry_default_tier_escalates() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        // No tier set — defaults to Standard
        setup_workgraph(dir_path, vec![task]);
        setup_config_with_escalation(dir_path);

        run(dir_path, "t1", false, false, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.tier,
            Some("premium".to_string()),
            "Default tier (standard) should escalate to premium"
        );
    }

    /// Helper: init a git repo with a "main" branch and one commit.
    fn init_git_repo(path: &Path) {
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .arg(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .current_dir(path)
            .output()
            .unwrap();
        fs::write(path.join("seed.txt"), "seed").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
    }

    /// Helper: create a worktree with the wg/<agent>/<task> branch convention.
    fn create_worktree(project: &Path, agent_id: &str, task_id: &str) -> std::path::PathBuf {
        let branch = format!("wg/{}/{}", agent_id, task_id);
        let wt = project.join(".wg-worktrees").join(agent_id);
        fs::create_dir_all(project.join(".wg-worktrees")).unwrap();
        std::process::Command::new("git")
            .args(["worktree", "add"])
            .arg(&wt)
            .args(["-b", &branch, "HEAD"])
            .current_dir(project)
            .output()
            .unwrap();
        wt
    }

    /// New retention policy (worktree-retention-don):
    /// `wg retry` (default) reuses the existing worktree + branch — does NOT
    /// remove the dir, does NOT delete the branch. Clears the cleanup-pending
    /// marker so the next sweep doesn't reap before the new agent picks up.
    #[test]
    fn test_retry_reuses_existing_worktree_by_default() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();

        let mut task = make_task("retry-here", "test", Status::Failed);
        task.retry_count = 1;
        setup_workgraph(&wg_dir, vec![task]);

        // Pretend a prior agent ran in this worktree, made a commit, and
        // exited with a cleanup-pending marker.
        let wt = create_worktree(&project, "agent-prior", "retry-here");
        fs::write(wt.join("wip.txt"), "uncommitted-wip").unwrap();
        fs::write(
            wt.join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER),
            "",
        )
        .unwrap();

        let result = run(&wg_dir, "retry-here", false, /*fresh=*/ false, None);
        assert!(result.is_ok(), "retry should succeed: {:?}", result);

        // Default behavior: worktree dir SURVIVES.
        assert!(wt.exists(), "retry must NOT remove worktree by default");
        assert!(wt.join("wip.txt").exists(), "uncommitted WIP must survive");
        // Cleanup-pending marker should be cleared so the next sweep doesn't reap.
        assert!(
            !wt.join(crate::commands::service::worktree::CLEANUP_PENDING_MARKER)
                .exists(),
            "cleanup-pending marker must be cleared on retry-in-place"
        );
        // Branch survives in git
        let branches = std::process::Command::new("git")
            .args(["branch", "--list", "wg/agent-prior/retry-here"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).contains("wg/agent-prior/retry-here"),
            "branch must survive retry-in-place"
        );
    }

    /// Helper: write a registry with one Dead agent at `dir/registry.json`.
    /// Used by the downstream-claim TDD tests below.
    fn write_dead_agent_registry(dir: &Path, agent_id: &str) {
        use worksgood::service::registry::{AgentEntry, AgentRegistry, AgentStatus};
        let mut reg = AgentRegistry::new();
        reg.agents.insert(
            agent_id.to_string(),
            AgentEntry {
                id: agent_id.to_string(),
                pid: 99999,
                task_id: "irrelevant".to_string(),
                executor: "claude".to_string(),
                status: AgentStatus::Dead,
                started_at: Utc::now().to_rfc3339(),
                last_heartbeat: "2020-01-01T00:00:00Z".to_string(),
                completed_at: Some(Utc::now().to_rfc3339()),
                output_file: "/tmp/output.log".to_string(),
                model: None,
                worktree_path: None,
            },
        );
        reg.save(dir).unwrap();
    }

    /// TDD for bug-retry-doesnt-clear-stale-downstream-claims.
    /// `wg retry <upstream>` must walk the forward closure and clear any
    /// downstream task claimed by a now-dead agent. Without this the
    /// dispatcher silently skips the downstream task forever (its
    /// `assigned` field is non-null so it's not "ready").
    #[test]
    fn test_wg_retry_clears_downstream_claims_on_dead_agents() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut upstream = make_task("upstream", "Upstream", Status::Failed);
        upstream.retry_count = 1;
        upstream.before = vec!["downstream".into()];

        let mut downstream = make_task("downstream", "Downstream", Status::Open);
        downstream.after = vec!["upstream".into()];
        downstream.assigned = Some("agent-dead-1".to_string());
        downstream.started_at = Some("2026-01-01T00:00:00Z".to_string());

        setup_workgraph(dir_path, vec![upstream, downstream]);
        write_dead_agent_registry(dir_path, "agent-dead-1");

        run(
            dir_path,
            "upstream",
            false,
            false,
            Some("downstream-clear-test"),
        )
        .unwrap();

        let g = load_graph(&graph_path(dir_path)).unwrap();
        let down = g.get_task("downstream").unwrap();
        assert!(
            down.assigned.is_none(),
            "wg retry must clear stale downstream claim — found: {:?}",
            down.assigned
        );
        assert!(
            down.started_at.is_none(),
            "started_at must also be cleared so dispatcher won't think it's mid-run"
        );
        assert!(
            down.log
                .iter()
                .any(|e| e.message.contains("stale-claim cleared via retry")),
            "downstream log must record the cause: {:?}",
            down.log.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Live agents downstream of a retry seed must NOT have their claim
    /// cleared — eager path is conservative.
    #[test]
    fn test_wg_retry_preserves_live_downstream_claims() {
        use worksgood::service::registry::{AgentEntry, AgentRegistry, AgentStatus};
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut upstream = make_task("upstream", "Upstream", Status::Failed);
        upstream.retry_count = 1;
        upstream.before = vec!["downstream".into()];

        let mut downstream = make_task("downstream", "Downstream", Status::Open);
        downstream.after = vec!["upstream".into()];
        downstream.assigned = Some("agent-alive-1".to_string());

        setup_workgraph(dir_path, vec![upstream, downstream]);

        let mut reg = AgentRegistry::new();
        reg.agents.insert(
            "agent-alive-1".to_string(),
            AgentEntry {
                id: "agent-alive-1".to_string(),
                pid: std::process::id(),
                task_id: "downstream".to_string(),
                executor: "claude".to_string(),
                status: AgentStatus::Working,
                started_at: Utc::now().to_rfc3339(),
                last_heartbeat: Utc::now().to_rfc3339(),
                completed_at: None,
                output_file: "/tmp/output.log".to_string(),
                model: None,
                worktree_path: None,
            },
        );
        reg.save(dir_path).unwrap();

        run(dir_path, "upstream", false, false, None).unwrap();

        let g = load_graph(&graph_path(dir_path)).unwrap();
        let down = g.get_task("downstream").unwrap();
        assert_eq!(
            down.assigned,
            Some("agent-alive-1".to_string()),
            "live-agent claim must be preserved on retry"
        );
    }

    /// `wg retry --fresh` discards the prior worktree + branch so the next
    /// spawn allocates a clean one off main.
    #[test]
    fn test_retry_fresh_flag_allocates_new_worktree() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        init_git_repo(&project);

        let wg_dir = project.join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();

        let mut task = make_task("retry-fresh", "test", Status::Failed);
        task.retry_count = 1;
        setup_workgraph(&wg_dir, vec![task]);

        let wt = create_worktree(&project, "agent-prior", "retry-fresh");
        assert!(wt.exists());

        let result = run(&wg_dir, "retry-fresh", false, /*fresh=*/ true, None);
        assert!(result.is_ok(), "retry --fresh should succeed: {:?}", result);

        // --fresh: worktree dir is REMOVED.
        assert!(!wt.exists(), "retry --fresh must remove the prior worktree");
        // Branch is also deleted
        let branches = std::process::Command::new("git")
            .args(["branch", "--list", "wg/agent-prior/retry-fresh"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&branches.stdout).contains("wg/agent-prior/retry-fresh"),
            "branch must be deleted on --fresh"
        );
    }
}
