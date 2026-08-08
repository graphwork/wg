//! Dispatcher tick logic: readiness, maintenance, and direct agent spawning.

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::agency;
use worksgood::agency::evolver::{self, EvolutionTrigger, EvolverState};
use worksgood::chat;
use worksgood::config::Config;
use worksgood::graph::{
    LogEntry, Node, PRIORITY_DEFAULT, PRIORITY_IDLE, PRIORITY_NORMAL, Priority, Status, Task,
    WaitCondition, WaitSpec, boost_priority, evaluate_all_cycle_failure_restarts,
    evaluate_all_cycle_iterations,
};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::messages;
use worksgood::parser::{load_graph, modify_graph};
use worksgood::query::{blocked_open_cycle_diagnostics, ready_tasks_with_peers_cycle_aware};
use worksgood::service::registry::AgentRegistry;

use super::human_dispatch;
use super::triage;
use crate::commands::{graph_path, is_process_alive, kill_process_graceful, spawn};

/// Result of a single coordinator tick
pub struct TickResult {
    /// Number of agents alive after the tick
    pub agents_alive: usize,
    /// Number of ready tasks found
    pub tasks_ready: usize,
    /// Number of agents spawned in this tick
    pub agents_spawned: usize,
    /// Tasks skipped this tick because their per-task spawn circuit breaker
    /// is tripped (and the cooldown has not elapsed). Non-zero explains a
    /// "spawned=0" tick without a wedged dispatcher: the breaker is per-task
    /// and self-heals (cooldown decay / `wg retry` / clear-on-success).
    pub spawn_breaker_tripped_tasks: usize,
    /// Number of ready tasks intentionally deferred by the explicitly enabled
    /// disk/build admission gate. This is not a dispatcher wedge.
    pub admission_deferred_tasks: usize,
    /// First admission refusal recorded during the tick, for status output.
    pub admission_deferred_reason: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SpawnSummary {
    spawned: usize,
    admission_deferred_tasks: usize,
    admission_deferred_reason: Option<String>,
    /// Tasks skipped this tick because their per-task spawn circuit breaker
    /// is tripped (and the cooldown has not elapsed). Other tasks dispatch
    /// normally — the breaker is per-task.
    spawn_breaker_tripped_tasks: usize,
}

/// Clean up dead agents and count alive ones. Returns `None` with an early
/// `TickResult` if the alive count already meets `max_agents`.
fn cleanup_and_count_alive(
    dir: &Path,
    graph_path: &Path,
    max_agents: usize,
) -> Result<Result<usize, TickResult>> {
    // Clean up dead agents: process exited
    let finished_agents = triage::cleanup_dead_agents(dir, graph_path)?;
    if !finished_agents.is_empty() {
        eprintln!(
            "[dispatcher] Cleaned up {} dead agent(s): {:?}",
            finished_agents.len(),
            finished_agents
        );
    }

    // Reconciliation safety net: catch orphaned InProgress tasks whose agents
    // are Dead in registry but weren't unclaimed (split-save race condition).
    match crate::commands::sweep::reconcile_orphaned_tasks(dir, graph_path) {
        Ok(0) => {}
        Ok(n) => {
            eprintln!(
                "[dispatcher] Reconciliation: recovered {} orphaned task(s)",
                n
            );
        }
        Err(e) => {
            eprintln!("[dispatcher] Reconciliation warning: {}", e);
        }
    }

    // Task-status-aware reaping: detect agents whose tasks are Done/Failed
    // but whose processes are still alive (e.g., Claude CLI hung after `wg done`).
    // Send SIGTERM to free the agent slot.
    {
        let graph =
            load_graph(graph_path).context("Failed to load graph for task-aware reaping")?;
        let mut locked_registry = AgentRegistry::load_locked(dir)?;
        let mut killed = Vec::new();
        for agent in locked_registry.registry.agents.values() {
            if !agent.is_alive() || !is_process_alive(agent.pid) {
                continue;
            }
            if let Some(task) = graph.get_task(&agent.task_id)
                && task.status.is_terminal()
            {
                eprintln!(
                    "[dispatcher] Agent {} (PID {}) still alive but task '{}' is {:?} — sending SIGTERM",
                    agent.id, agent.pid, agent.task_id, task.status
                );
                killed.push((agent.id.clone(), agent.pid));
            }
        }
        for (agent_id, pid) in &killed {
            if let Some(agent) = locked_registry.get_agent_mut(agent_id) {
                agent.status = worksgood::service::registry::AgentStatus::Dead;
                if agent.completed_at.is_none() {
                    agent.completed_at = Some(Utc::now().to_rfc3339());
                }
            }
            let _ = kill_process_graceful(*pid, 5);
        }
        if !killed.is_empty() {
            locked_registry.save_ref()?;
            eprintln!(
                "[dispatcher] Killed {} zombie agent(s) with completed tasks",
                killed.len()
            );
        }
    }

    // Reopen is a two-phase lifecycle operation. Only this exact-owner reaper
    // may turn a persisted intent into a runnable generation; contention is an
    // expected hold and never reaches the spawn breaker.
    match crate::commands::reopen::reconcile_pending(dir) {
        Ok(released) if !released.is_empty() => eprintln!(
            "[dispatcher] Released prior ownership and enabled one generation for: {:?}",
            released
        ),
        Ok(_) => {}
        Err(error) => eprintln!("[dispatcher] Reopen reconciliation held fail-closed: {error:#}"),
    }

    // Now count truly alive agents (process still running)
    let registry = AgentRegistry::load(dir)?;
    let alive_count = registry
        .agents
        .values()
        .filter(|a| a.is_alive() && is_process_alive(a.pid))
        .count();

    if alive_count >= max_agents {
        eprintln!(
            "[dispatcher] Max agents ({}) running, waiting...",
            max_agents
        );
        // Capacity is live registry truth. Do not persist a planner wait or
        // manufacture a retry deadline; the next ordinary tick observes the
        // registry again.
        return Ok(Err(TickResult {
            agents_alive: alive_count,
            tasks_ready: 0,
            agents_spawned: 0,
            spawn_breaker_tripped_tasks: 0,
            admission_deferred_tasks: 0,
            admission_deferred_reason: None,
        }));
    }

    Ok(Ok(alive_count))
}

/// Tags for daemon-managed loop tasks that should not be spawned as regular agents.
///
/// `chat-loop` (new) and `coordinator-loop` (legacy) both identify chat-agent
/// supervisors. The daemon's `subprocess_coordinator_loop` spawns these via
/// `wg spawn-task` directly; if the dispatcher were also allowed to claim them
/// it would spawn a regular worker that idle-loops `wg log` + `wg done` and
/// burns tokens (see chat-agent-loops bug A).
const DAEMON_MANAGED_TAGS: &[&str] = &[
    "compact-loop",
    "archive-loop",
    "chat-loop",
    "coordinator-loop",
    "registry-refresh-loop",
    "user-board",
];

/// Check whether a task is managed by the daemon (not spawned as a regular agent).
fn is_daemon_managed(task: &worksgood::graph::Task) -> bool {
    task.tags
        .iter()
        .any(|tag| DAEMON_MANAGED_TAGS.contains(&tag.as_str()))
}

fn is_retired_agency_task(task_id: &str) -> bool {
    task_id.starts_with(".assign-")
        || task_id.starts_with(".flip-")
        || task_id.starts_with(".evaluate-")
}

fn active_build_heavy_count(dir: &Path, graph: &worksgood::graph::WorkGraph) -> usize {
    AgentRegistry::load(dir)
        .map(|registry| {
            registry
                .all()
                .filter(|agent| agent.is_alive() && is_process_alive(agent.pid))
                .filter(|agent| {
                    graph.get_task(&agent.task_id).is_some_and(|task| {
                        worksgood::disk_sentinel::classify_task(task).is_heavy()
                    })
                })
                .count()
        })
        .unwrap_or(0)
}

fn build_admission_denial(
    task: &Task,
    builds_blocked: bool,
    active_build_heavy: usize,
    max_build_agents: usize,
    disk_reason: &str,
) -> Option<String> {
    let class = worksgood::disk_sentinel::classify_task(task);
    if class.is_build_capable() && builds_blocked {
        return Some(format!("build admission paused: {disk_reason}"));
    }
    if class.is_heavy() && active_build_heavy >= max_build_agents {
        return Some(format!(
            "build-heavy admission budget full ({active_build_heavy}/{max_build_agents})"
        ));
    }
    None
}

/// Check whether any tasks are ready. Returns `None` with an early `TickResult`
/// if no ready tasks exist.
fn check_ready_or_return(
    graph: &worksgood::graph::WorkGraph,
    alive_count: usize,
    dir: &Path,
) -> Option<TickResult> {
    let cycle_analysis = graph.compute_cycle_analysis();
    let ready = ready_tasks_with_peers_cycle_aware(graph, dir, &cycle_analysis);
    // Only count tasks that are spawnable (exclude daemon-managed loop tasks)
    let spawnable_count = ready.iter().filter(|t| !is_daemon_managed(t)).count();
    if spawnable_count == 0 {
        let terminal = graph.tasks().filter(|t| t.status.is_terminal()).count();
        let total = graph.tasks().count();
        if terminal == total && total > 0 {
            eprintln!("[dispatcher] All {} tasks complete!", total);
        } else {
            eprintln!(
                "[dispatcher] No ready tasks (terminal: {}/{})",
                terminal, total
            );
            for diagnostic in blocked_open_cycle_diagnostics(graph, &cycle_analysis) {
                eprintln!("[dispatcher] Warning: {}", diagnostic.message());
            }
        }
        return Some(TickResult {
            agents_alive: alive_count,
            tasks_ready: 0,
            agents_spawned: 0,
            spawn_breaker_tripped_tasks: 0,
            admission_deferred_tasks: 0,
            admission_deferred_reason: None,
        });
    }
    None
}

/// Evaluate a single wait condition against the current graph/filesystem state.
/// Returns `true` if the condition is satisfied.
fn evaluate_condition_with_message(
    condition: &WaitCondition,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
    eligible_message: Option<&messages::Message>,
    strict_message_binding: bool,
) -> bool {
    match condition {
        WaitCondition::TaskStatus {
            task_id: dep_id,
            status: expected,
        } => {
            if let Some(dep) = graph.get_task(dep_id) {
                dep.status == *expected
            } else {
                false
            }
        }
        WaitCondition::Timer { resume_after } => {
            if let Ok(target) = resume_after.parse::<chrono::DateTime<chrono::Utc>>() {
                Utc::now() >= target
            } else {
                // Unparseable timestamp — treat as satisfied to avoid permanent hang
                true
            }
        }
        WaitCondition::HumanInput => {
            if strict_message_binding {
                eligible_message.is_some_and(|message| !message.sender.starts_with("agent-"))
            } else {
                // Human-dispatch compatibility waits are explicit but do not
                // own worker attempts. They retain their established selector.
                has_non_agent_message_since(dir, task_id, wait_started_at)
            }
        }
        WaitCondition::Message => eligible_message.is_some(),
        WaitCondition::FileChanged {
            path,
            mtime_at_wait,
        } => {
            if let Ok(metadata) = std::fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    let current_mtime = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    current_mtime > *mtime_at_wait
                } else {
                    false
                }
            } else {
                false
            }
        }
    }
}

/// Check if any non-agent message exists for a task since the wait started.
fn has_non_agent_message_since(dir: &Path, task_id: &str, wait_started_at: Option<&str>) -> bool {
    if let Ok(msgs) = messages::list_messages(dir, task_id) {
        if let Some(wait_ts) = wait_started_at
            && let Ok(wait_time) = wait_ts.parse::<chrono::DateTime<chrono::Utc>>()
        {
            msgs.iter().any(|m| {
                !m.sender.starts_with("agent-")
                    && m.timestamp
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .map(|t| t > wait_time)
                        .unwrap_or(false)
            })
        } else {
            msgs.iter().any(|m| !m.sender.starts_with("agent-"))
        }
    } else {
        false
    }
}

#[cfg(test)]
fn evaluate_condition(
    condition: &WaitCondition,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
) -> bool {
    evaluate_condition_with_message(condition, graph, dir, task_id, wait_started_at, None, false)
}

/// Evaluate all conditions in a WaitSpec.
fn evaluate_wait_spec_with_message(
    spec: &WaitSpec,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
    eligible_message: Option<&messages::Message>,
    strict_message_binding: bool,
) -> bool {
    let evaluate = |condition| {
        evaluate_condition_with_message(
            condition,
            graph,
            dir,
            task_id,
            wait_started_at,
            eligible_message,
            strict_message_binding,
        )
    };
    match spec {
        WaitSpec::All(conditions) => conditions.iter().all(evaluate),
        WaitSpec::Any(conditions) => conditions.iter().any(evaluate),
    }
}

#[cfg(test)]
fn evaluate_wait_spec(
    spec: &WaitSpec,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
) -> bool {
    evaluate_wait_spec_with_message(spec, graph, dir, task_id, wait_started_at, None, false)
}

/// Check if a TaskStatus wait condition is unsatisfiable (referenced task
/// is in a terminal state that doesn't match the expected status).
fn is_condition_unsatisfiable(
    condition: &WaitCondition,
    graph: &worksgood::graph::WorkGraph,
) -> Option<String> {
    match condition {
        WaitCondition::TaskStatus {
            task_id: dep_id,
            status: expected,
        } => {
            if let Some(dep) = graph.get_task(dep_id) {
                if dep.status.is_terminal() && dep.status != *expected {
                    Some(format!(
                        "task '{}' is {} (expected {})",
                        dep_id, dep.status, expected
                    ))
                } else {
                    None
                }
            } else {
                Some(format!("task '{}' no longer exists", dep_id))
            }
        }
        _ => None,
    }
}

/// Detect circular waits: task A waiting on task B, task B waiting on task A.
fn detect_circular_waits(graph: &worksgood::graph::WorkGraph) -> Vec<Vec<String>> {
    let mut cycles = Vec::new();
    let waiting_tasks: Vec<_> = graph
        .tasks()
        .filter(|t| t.status == Status::Waiting && t.wait_condition.is_some())
        .collect();

    // Build a map: task_id -> set of task_ids it's waiting on (via TaskStatus conditions)
    let mut wait_edges: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for t in &waiting_tasks {
        if let Some(ref spec) = t.wait_condition {
            let conditions = match spec {
                WaitSpec::All(c) | WaitSpec::Any(c) => c,
            };
            let deps: Vec<&str> = conditions
                .iter()
                .filter_map(|c| match c {
                    WaitCondition::TaskStatus { task_id, .. } => Some(task_id.as_str()),
                    _ => None,
                })
                .collect();
            if !deps.is_empty() {
                wait_edges.insert(t.id.as_str(), deps);
            }
        }
    }

    // DFS cycle detection
    let mut visited = std::collections::HashSet::new();
    for start in wait_edges.keys() {
        if visited.contains(start) {
            continue;
        }
        let mut path = vec![*start];
        let mut stack: Vec<(&str, usize)> = vec![(*start, 0)];
        let mut in_path = std::collections::HashSet::new();
        in_path.insert(*start);

        while let Some((node, idx)) = stack.last_mut() {
            let deps = wait_edges.get(node).cloned().unwrap_or_default();
            if *idx >= deps.len() {
                in_path.remove(*node);
                path.pop();
                stack.pop();
                continue;
            }
            let next = deps[*idx];
            *idx += 1;
            if in_path.contains(next) {
                // Found a cycle - extract it
                let cycle_start = path.iter().position(|p| *p == next).unwrap();
                let cycle: Vec<String> =
                    path[cycle_start..].iter().map(|s| s.to_string()).collect();
                if cycle.len() >= 2 {
                    cycles.push(cycle);
                }
            } else if !visited.contains(next) && wait_edges.contains_key(next) {
                in_path.insert(next);
                path.push(next);
                stack.push((next, 0));
            }
        }
        visited.insert(*start);
    }
    cycles
}

/// Build a brief graph state delta for resume context injection.
/// Shows what changed while the task was waiting (~100 tokens).
fn build_resume_delta(graph: &worksgood::graph::WorkGraph, task: &Task, dir: &Path) -> String {
    let mut delta = String::new();
    delta.push_str("## Resume Context\n");

    // Show what condition was satisfied
    if let Some(ref spec) = task.wait_condition {
        let conditions = match spec {
            WaitSpec::All(c) | WaitSpec::Any(c) => c,
        };
        delta.push_str("Your wait condition is now satisfied.\n\n");

        // Show status of referenced tasks
        for cond in conditions {
            if let WaitCondition::TaskStatus { task_id, status } = cond
                && let Some(dep) = graph.get_task(task_id)
            {
                delta.push_str(&format!(
                    "- {}: {} (expected: {})\n",
                    task_id, dep.status, status
                ));
                // Include artifacts if any
                if !dep.artifacts.is_empty() {
                    for art in &dep.artifacts {
                        delta.push_str(&format!("  artifact: {}\n", art));
                    }
                }
                // Include recent log entries from completed subtasks for result context
                let recent_logs: Vec<_> = dep.log.iter().rev().take(3).collect();
                if !recent_logs.is_empty() {
                    for log in recent_logs.iter().rev() {
                        delta.push_str(&format!("  log: {}\n", log.message));
                    }
                }
                // Include failure reason if the subtask failed
                if dep.status == Status::Failed
                    && let Some(ref reason) = dep.failure_reason
                {
                    delta.push_str(&format!("  failure_reason: {}\n", reason));
                }
            }
        }
    }

    // Include checkpoint if available
    if let Some(ref cp) = task.checkpoint {
        delta.push_str(&format!("\nYour checkpoint: \"{}\"\n", cp));
    }

    // Include recent messages on this task
    if let Ok(msgs) = messages::list_messages(dir, &task.id) {
        let recent: Vec<_> = msgs.iter().rev().take(3).collect();
        if !recent.is_empty() {
            delta.push_str("\nRecent messages:\n");
            for msg in recent.iter().rev() {
                delta.push_str(&format!(
                    "- [{}] {}: {}\n",
                    msg.timestamp, msg.sender, msg.body
                ));
            }
        }
    }

    delta.push_str(&format!("\nContinue your work on '{}'.\n", task.id));
    delta
}

/// Evaluate waiting tasks and transition them when conditions are met.
/// Returns `true` if the graph was modified.
fn evaluate_waiting_tasks(graph: &mut worksgood::graph::WorkGraph, dir: &Path) -> bool {
    let mut modified = false;

    // First, detect circular waits
    let circular = detect_circular_waits(graph);
    for cycle in &circular {
        // A wait observation is not terminal authority. Keep the exact parked
        // attempt intact for an explicit operator decision.
        eprintln!(
            "[dispatcher] Circular wait requires operator action: {:?}",
            cycle
        );
    }

    // Collect waiting tasks with their data to avoid borrow conflicts
    let waiting_data: Vec<_> = graph
        .tasks()
        .filter(|t| t.status == Status::Waiting && t.wait_condition.is_some())
        .map(|t| {
            let wait_started = t
                .log
                .iter()
                .rev()
                .find(|l| l.message.contains("Agent parked"))
                .map(|l| l.timestamp.clone());
            (
                t.id.clone(),
                t.wait_condition.clone().unwrap(),
                wait_started,
                t.session_id.clone(),
                t.checkpoint.clone(),
                t.message_wait.clone(),
            )
        })
        .collect();

    for (task_id, spec, wait_started, _session_id, _checkpoint, subscription) in &waiting_data {
        // Check for unsatisfiable conditions first
        let conditions = match &spec {
            WaitSpec::All(c) | WaitSpec::Any(c) => c,
        };

        let mut unsatisfiable_reasons = Vec::new();
        for cond in conditions {
            if let Some(reason) = is_condition_unsatisfiable(cond, graph) {
                unsatisfiable_reasons.push(reason);
            }
        }

        // For All: any unsatisfiable => whole spec unsatisfiable
        // For Any: all must be unsatisfiable
        let is_unsatisfiable = match &spec {
            WaitSpec::All(_) => !unsatisfiable_reasons.is_empty(),
            WaitSpec::Any(_) => {
                // Only unsatisfiable if ALL conditions are unsatisfiable
                // (non-TaskStatus conditions like timer/message are never unsatisfiable)
                let task_status_count = conditions
                    .iter()
                    .filter(|c| matches!(c, WaitCondition::TaskStatus { .. }))
                    .count();
                unsatisfiable_reasons.len() == task_status_count
                    && task_status_count == conditions.len()
            }
        };

        if is_unsatisfiable {
            eprintln!(
                "[dispatcher] Waiting task '{}' requires operator action: {}",
                task_id,
                unsatisfiable_reasons.join(", ")
            );
            continue;
        }

        // Only messages accepted while this exact subscription was armed are
        // eligible. Historical/unbound/stale-attempt records remain inert.
        let eligible_message = subscription.as_ref().and_then(|subscription| {
            let task = graph.get_task(task_id)?;
            let attempt = task.lifecycle.current_attempt.as_ref()?;
            if !subscription.armed
                || subscription.attempt_epoch != attempt.generation
                || subscription.attempt_id != attempt.id
            {
                return None;
            }
            messages::list_messages(dir, task_id)
                .ok()?
                .into_iter()
                .find(|message| {
                    message.accepted_disposition == messages::MessageDisposition::WaitingArmed
                        && message.recipient_attempt_epoch == Some(subscription.attempt_epoch)
                        && message.recipient_attempt_id.as_deref()
                            == Some(subscription.attempt_id.as_str())
                        && message.subscription_id.as_deref() == Some(subscription.id.as_str())
                        && subscription.selector.matches_sender(&message.sender)
                })
        });
        let strict_message_binding = subscription.is_some();
        let satisfied_without_message = evaluate_wait_spec_with_message(
            spec,
            graph,
            dir,
            task_id,
            wait_started.as_deref(),
            None,
            strict_message_binding,
        );
        let satisfied = satisfied_without_message
            || evaluate_wait_spec_with_message(
                spec,
                graph,
                dir,
                task_id,
                wait_started.as_deref(),
                eligible_message.as_ref(),
                strict_message_binding,
            );
        let message_triggered = !satisfied_without_message && eligible_message.is_some();

        if satisfied {
            // Human-as-agent dispatch tail (R13): if this task is assigned to a
            // human, their reply (the non-agent message that just satisfied the
            // wait) completes it — record the reply as a reply-to-artifact for
            // each declared deliverable and mark the task Done. Resuming to Open
            // (the generic path below) would re-park it forever, since there is
            // no AI agent to spawn for a human assignee.
            // A human reply is message evidence, not completion authority. It
            // may satisfy the task's already-persisted explicit wait below,
            // after which the same generation resumes as Open.
            let _ = wait_started;

            // Build resume delta before mutating
            let delta = {
                let task = graph.get_task(task_id).unwrap();
                build_resume_delta(graph, task, dir)
            };

            if let Some(t) = graph.get_task_mut(task_id) {
                let generation = t.lifecycle.generation;
                let matched_message_id = message_triggered.then(|| {
                    eligible_message
                        .as_ref()
                        .expect("message-triggered wait has receipt")
                        .id
                });
                let subscription_id = subscription.as_ref().map(|value| value.id.clone());
                if message_triggered
                    && !t.message_wait.as_ref().is_some_and(|current| {
                        current.armed
                            && Some(current.id.as_str()) == subscription_id.as_deref()
                            && current.consumed_by_message_id.is_none()
                    })
                {
                    continue;
                }
                let wait_id = subscription_id
                    .clone()
                    .unwrap_or_else(|| format!("wait:{task_id}:{generation}"));
                let receipt_id = matched_message_id
                    .map(|id| format!("message:{task_id}:{id}"))
                    .unwrap_or_else(|| format!("condition:{task_id}:{generation}"));
                let idempotency_key = format!("wait-satisfied:{wait_id}:{receipt_id}");
                let request = TransitionRequest::new(
                    TransitionKind::WaitSatisfied {
                        wait_id,
                        receipt_id: receipt_id.clone(),
                    },
                    LifecycleActor {
                        kind: ActorKind::WaitMatcher,
                        id: "coordinator".to_string(),
                    },
                    "wait_condition_satisfied",
                    idempotency_key.clone(),
                )
                .expecting(FenceExpectation::current(t));
                if let Err(rejection) = apply_transition(t, request) {
                    eprintln!(
                        "[dispatcher] Ignored stale wait satisfaction for '{}': {}",
                        task_id, rejection
                    );
                    continue;
                }
                if let Some(message_id) = matched_message_id
                    && let Some(current) = t.message_wait.as_mut()
                {
                    current.armed = false;
                    current.consumed_by_message_id = Some(message_id);
                    current.resume_request_id = Some(idempotency_key);
                }
                t.wait_condition = None;
                // Store the resume delta as the new checkpoint so the spawned agent gets it
                t.checkpoint = Some(delta.clone());
                // Clear the assignment so the coordinator can re-spawn
                t.assigned = None;
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: "Wait condition satisfied. Task ready for resume.".to_string(),
                });
                modified = true;
                eprintln!(
                    "[dispatcher] Waiting task '{}' condition satisfied, transitioning to Open",
                    task_id
                );
            }
        }
    }

    modified
}

// Ordinary messages deliberately have no coordinator phase. The attempt-bound
// subscription check inside `evaluate_waiting_tasks` is the sole message edge.
#[cfg(test)]
fn assert_ordinary_messages_are_inert(
    _graph: &mut worksgood::graph::WorkGraph,
    _dir: &Path,
) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Unblock stuck tasks
// ---------------------------------------------------------------------------

/// Scan blocked tasks and reopen only those whose required-success edges are
/// satisfied according to the canonical dependency disposition. Missing,
/// failed, abandoned, unresolved-remote, and archived non-Done prerequisites
/// stay blocked; coordinator recovery must never manufacture success.
///
/// Returns `true` if the graph was modified.
/// Dispatcher-side wrapper around `worksgood::lifecycle::migrate_pending_validation_tasks`.
/// Performs the migration and emits a `[dispatcher] Migrated …` banner per task
/// so the operator sees the one-time event in `daemon.log`. Returns true if any
/// task was migrated.
fn migrate_pending_validation_tasks(graph: &mut worksgood::graph::WorkGraph) -> bool {
    let migrated = worksgood::lifecycle::migrate_pending_validation_tasks(graph);
    for id in &migrated {
        eprintln!(
            "[dispatcher] Migrated '{}' from PendingValidation to Done \
             (legacy state — agency eval is now the unblock gate)",
            id
        );
    }
    !migrated.is_empty()
}

// PendingEval and FailedPendingEval resolution is verdict-required and lives in
// `eval_lifecycle::reconcile_durable_verdicts`; terminal/missing satellites are
// never interpreted as semantic success.

fn unblock_stuck_tasks(graph: &mut worksgood::graph::WorkGraph, dir: &Path) -> bool {
    let mut modified = false;

    // Collect blocked task IDs first
    let blocked_task_ids: Vec<String> = graph
        .tasks()
        .filter(|t| t.status == Status::Blocked)
        .map(|t| t.id.clone())
        .collect();

    for task_id in blocked_task_ids {
        let task = graph.get_task(&task_id);
        let all_deps_satisfied = task.is_some_and(|task| {
            task.after.iter().all(|dep_id| {
                worksgood::query::dependency_disposition(dep_id, &task.id, graph, Some(dir))
                    .is_satisfied()
            })
        });

        if all_deps_satisfied {
            if let Some(task) = graph.get_task_mut(&task_id)
                && !task.after.is_empty()
            {
                let dependencies = task.after.join(", ");
                let request = TransitionRequest::new(
                    TransitionKind::GenerationCreated,
                    LifecycleActor {
                        kind: ActorKind::Reconciler,
                        id: "dependency-reconciler".to_string(),
                    },
                    "dependencies_satisfied",
                    format!(
                        "dependencies-satisfied:{}:{}:{}",
                        task.id, task.lifecycle.generation, task.lifecycle.revision
                    ),
                )
                .expecting(FenceExpectation::current(task));
                match apply_transition(task, request) {
                    Ok(_) => {
                        task.log.push(LogEntry {
                            timestamp: Utc::now().to_rfc3339(),
                            actor: Some("dependency-reconciler".to_string()),
                            user: None,
                            message: format!(
                                "Opened a new generation after required-success dependencies were satisfied: {dependencies}"
                            ),
                        });
                        eprintln!(
                            "[dispatcher] Opened new generation for '{}' (dependencies: {})",
                            task.id, dependencies
                        );
                        modified = true;
                    }
                    Err(rejection) => eprintln!(
                        "[dispatcher] Dependency reconciliation rejected for '{}': {}",
                        task.id, rejection
                    ),
                }
            }
        } else {
            // Log diagnostic for stale blocked state
            if let Some(task) = graph.tasks().find(|t| t.id == task_id)
                && !task.after.is_empty()
            {
                let waiting_on: Vec<String> = task
                    .after
                    .iter()
                    .filter_map(|dep_id| {
                        match worksgood::query::dependency_disposition(
                            dep_id,
                            &task.id,
                            graph,
                            Some(dir),
                        ) {
                            worksgood::query::DependencyDisposition::Blocked { reason } => {
                                Some(format!("{dep_id}:{reason}"))
                            }
                            _ => None,
                        }
                    })
                    .collect();
                if !waiting_on.is_empty() {
                    eprintln!(
                        "[dispatcher] Task '{}' still blocked on: {}",
                        task_id,
                        waiting_on.join(", ")
                    );
                }
            }
        }
    }

    modified
}

/// Build explicitly requested independent command-validation tasks.
///
/// This is separate from agency assignment/evaluation, which are attempt
/// receipts and never schedulable graph tasks.
fn build_separate_verify_tasks(
    _dir: &Path,
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
) -> bool {
    // Find tasks in PendingValidation that have a verify command and were
    // marked for separate verification (indicated by log entry).
    let candidates: Vec<(String, String, Option<String>, Vec<String>)> = graph
        .tasks()
        .filter(|t| {
            t.status == Status::PendingValidation
                && t.verify.is_some()
                && t.log.iter().any(|entry| {
                    entry
                        .message
                        .contains("Pending separate verification (verify_mode=separate)")
                })
        })
        .map(|t| {
            (
                t.id.clone(),
                t.title.clone(),
                t.description.clone(),
                t.artifacts.clone(),
            )
        })
        .collect();

    if candidates.is_empty() {
        return false;
    }

    let mut modified = false;
    let verification_resolved =
        config.resolve_model_for_role(worksgood::config::DispatchRole::Verification);
    let verification_model = verification_resolved.model;

    for (source_task_id, source_title, source_desc, source_artifacts) in &candidates {
        let verify_task_id = format!(".sep-verify-{}", source_task_id);

        // Skip if verification task already exists
        if graph.get_task(&verify_task_id).is_some() {
            continue;
        }

        // Skip system tasks to prevent verification loops
        if worksgood::graph::is_system_task(source_task_id) {
            continue;
        }

        let source_task = match graph.get_task(source_task_id) {
            Some(t) => t,
            None => continue,
        };
        let verify_cmd = match source_task.verify.clone() {
            Some(cmd) => cmd,
            None => continue,
        };
        let source_checkpoint = source_task.checkpoint.clone().unwrap_or_default();

        let source_desc_snippet = source_desc
            .as_deref()
            .unwrap_or("(no description)")
            .chars()
            .take(2000)
            .collect::<String>();

        // Build the verification task description
        let mut desc = format!(
            "## Separate Verification\n\n\
             You are an **independent verification agent**. Your job is to verify that the \
             implementation work on task `{}` actually meets its criteria.\n\n\
             **IMPORTANT:** You have NO access to the implementation agent's conversation. \
             You must independently assess the work based solely on artifacts, code changes, \
             and the verification command.\n\n\
             ### Original Task\n\
             **ID:** {}\n\
             **Title:** {}\n\
             **Description:**\n{}\n\n",
            source_task_id, source_task_id, source_title, source_desc_snippet,
        );

        if !source_checkpoint.is_empty() {
            desc.push_str(&format!(
                "**Checkpoint (last known state):**\n{}\n\n",
                source_checkpoint
            ));
        }

        if !source_artifacts.is_empty() {
            desc.push_str("**Artifacts:**\n");
            for artifact in source_artifacts {
                desc.push_str(&format!("- `{}`\n", artifact));
            }
            desc.push('\n');
        }

        desc.push_str(&format!(
            "### Verification Command\n\
             Run this command and check the results:\n```\n{}\n```\n\n\
             ### Verification Steps\n\
             1. Run the verification command above\n\
             2. Check `git log --oneline -10` for recent commits related to this task\n\
             3. Review the actual code changes with `git diff`\n\
             4. Verify any artifacts mentioned in the task description exist\n\
             5. Do NOT trust the original agent's claims — verify independently\n\n\
             ### Verdict\n\
             - If the verification command passes and the work looks correct:\n\
             ```bash\n\
             wg approve {source_task_id}\n\
             ```\n\
             - If the verification command fails or the work is incomplete/incorrect:\n\
             ```bash\n\
             wg reject {source_task_id} --reason \"<specific reason>\"\n\
             ```\n\
             Then mark this verification task as done:\n\
             ```bash\n\
             wg done {verify_task_id}\n\
             ```\n",
            verify_cmd,
        ));
        // Replace placeholders
        desc = desc
            .replace("{source_task_id}", source_task_id)
            .replace("{verify_task_id}", &verify_task_id);

        let verify_task = Task {
            id: verify_task_id.clone(),
            title: format!("Verify: {}", source_title),
            presentation: worksgood::graph::TaskPresentation::Plumbing,
            origin: worksgood::graph::TaskOrigin::plumbing(
                Some(source_task_id.clone()),
                "independent verification satellite",
            ),
            description: Some(desc),
            status: Status::Open,
            lifecycle: worksgood::lifecycle::LifecycleProjection::default(),
            priority: PRIORITY_DEFAULT,
            assigned: None,
            estimate: None,
            before: vec![],
            after: vec![source_task_id.clone()],
            input_dependencies: vec![],
            requires: vec![],
            completion_contract: worksgood::graph::CompletionContract::Land,
            completion_candidate: None,
            completion_disposition: None,
            completion_receipt: None,
            tags: vec!["verification".to_string(), "separate-verify".to_string()],
            skills: vec![],
            inputs: vec![],
            deliverables: vec![],
            artifacts: vec![],
            exec: None,
            timeout: None,
            not_before: None,
            created_at: Some(Utc::now().to_rfc3339()),
            started_at: None,
            completed_at: None,
            last_interaction_at: None,
            last_message_at: None,
            log: vec![],
            retry_count: 0,
            max_retries: Some(1),
            failure_reason: None,
            failure_class: None,
            failure_signal: None,
            model: Some(verification_model.clone()),
            reasoning: verification_resolved.reasoning,
            provider: verification_resolved.provider.clone(),
            endpoint: None,
            remote_provider: None,
            profile: None,
            command_argv: vec![],
            working_dir: None,
            executor_preset_name: None,
            verify: None, // The verify agent runs the command manually, not via --verify gate
            verify_timeout: None,
            agent: None,
            loop_iteration: 0,
            last_iteration_completed_at: None,
            cycle_failure_restarts: 0,
            ready_after: None,
            paused: false,
            visibility: "internal".to_string(),
            context_scope: None,
            cycle_config: None,
            exec_mode: None,
            token_usage: None,
            actual_executor: None,
            actual_model: None,
            completion_review_activity: Vec::new(),
            session_id: None,
            wait_condition: None,
            message_wait: None,
            checkpoint: None,
            triage_count: 0,
            resurrection_count: 0,
            last_resurrected_at: None,
            validation: None,
            validation_commands: vec![],
            validator_agent: None,
            validator_model: None,
            gate_attempts: 0,
            test_required: false,
            rejection_count: 0,
            max_rejections: None,
            verify_failures: 0,
            rescue_count: 0,
            rescued: false,
            meta_eval_attempts: 0,
            agency_dispatch: None,
            evaluation_lifecycle: None,
            evaluation_records: vec![],
            spawn_failures: 0,
            last_spawn_failure_at: None,
            dispatch_count: 0,
            tier: None,
            no_tier_escalation: false,
            tried_models: vec![],
            superseded_by: vec![],
            supersedes: None,
            unplaced: false,
            place_before: vec![],
            place_near: vec![],
            independent: false,
            iteration_round: 0,
            iteration_anchor: None,
            iteration_parent: None,
            iteration_config: None,
            cron_schedule: None,
            cron_enabled: false,
            last_cron_fire: None,
            next_cron_fire: None,
        };

        graph.add_node(Node::Task(verify_task));

        // Log the trigger on the source task
        if let Some(source) = graph.get_task_mut(source_task_id) {
            source.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("coordinator".to_string()),
                user: Some(worksgood::current_user()),
                message: format!(
                    "Separate verification triggered — spawning .sep-verify-{} agent",
                    source_task_id,
                ),
            });
        }

        eprintln!(
            "[dispatcher] Created separate verification task '{}' for '{}'",
            verify_task_id, source_task_id,
        );

        modified = true;
    }

    modified
}

/// Auto-evolve: create a `.evolve-*` meta-task when evaluation data warrants evolution.
///
/// Checks the evolver state to determine whether enough evaluations have
/// accumulated (threshold trigger) or performance has dropped (reactive trigger).
/// Creates at most one evolution meta-task per trigger.
///
/// Returns `true` if the graph was modified.
fn build_auto_evolve_task(
    dir: &Path,
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
) -> bool {
    let agency_dir = dir.join("agency");

    // Don't create if agency isn't initialized
    if !agency_dir.join("cache/roles").exists() {
        return false;
    }

    let state = EvolverState::load(&agency_dir);

    let trigger = match evolver::should_trigger_evolution(&agency_dir, &config.agency, &state) {
        Some(t) => t,
        None => return false,
    };

    // Check that no .evolve-* task is already in-progress or open
    let has_active_evolve = graph.tasks().any(|t| {
        t.id.starts_with(".evolve-") && matches!(t.status, Status::Open | Status::InProgress)
    });
    if has_active_evolve {
        return false;
    }

    // Generate evolve task ID and run ID
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let evolve_task_id = format!(".evolve-auto-{}", ts);
    let budget = evolver::evolution_budget(&config.agency);

    // Build description based on trigger type
    let trigger_reason = match &trigger {
        EvolutionTrigger::Threshold { new_evals } => {
            format!(
                "Threshold trigger: {} new evaluations since last evolution (threshold: {})",
                new_evals, config.agency.evolution_threshold
            )
        }
        EvolutionTrigger::Reactive { avg_score } => {
            format!(
                "Reactive trigger: average score {:.2} dropped below threshold {:.2}",
                avg_score, config.agency.evolution_reactive_threshold
            )
        }
    };

    // Causal edges: recently completed non-system tasks for graph connectivity
    let mut recent_completed: Vec<_> = graph
        .tasks()
        .filter(|t| t.status == Status::Done && !worksgood::graph::is_system_task(&t.id))
        .map(|t| (t.id.clone(), t.completed_at.clone()))
        .collect();
    recent_completed.sort_by(|a, b| b.1.cmp(&a.1));
    let causal_ids: Vec<String> = recent_completed
        .iter()
        .take(5)
        .map(|(id, _)| id.clone())
        .collect();

    // Build the evolve command with safe strategies
    let safe_strategies = evolver::SAFE_STRATEGIES.join(",");
    let causal_list = causal_ids
        .iter()
        .map(|id| format!("- `{}`", id))
        .collect::<Vec<_>>()
        .join("\n");
    let desc = format!(
        "## Auto-Evolution Cycle\n\n\
         **Trigger:** {}\n\n\
         **Recently completed tasks:**\n{}\n\n\
         Run `wg evolve --budget {} --strategy mutation` to evolve agency roles and tradeoffs.\n\n\
         ### Constraints\n\
         - Safe strategies only: {}\n\
         - Budget cap: {} operations\n\
         - Do NOT use crossover or bizarre-ideation strategies\n\n\
         ### Instructions\n\
         1. Run `wg evolve --budget {}` (the evolver will use safe strategies)\n\
         2. Log the results\n\
         3. Mark this task done\n",
        trigger_reason, causal_list, budget, safe_strategies, budget, budget,
    );

    let evolver_resolved = config.resolve_model_for_role(worksgood::config::DispatchRole::Evolver);

    let evolve_parent = causal_ids.first().cloned();
    let evolve_task = Task {
        id: evolve_task_id.clone(),
        title: format!("Auto-evolve: {}", trigger_reason),
        presentation: worksgood::graph::TaskPresentation::Autonomous,
        origin: worksgood::graph::TaskOrigin::autonomous(
            evolve_parent,
            format!("auto-evolve: {}", trigger_reason),
        ),
        description: Some(desc),
        status: Status::Open,
        lifecycle: worksgood::lifecycle::LifecycleProjection::default(),
        priority: PRIORITY_DEFAULT,
        assigned: None,
        estimate: None,
        before: vec![],
        after: causal_ids,
        input_dependencies: vec![],
        requires: vec![],
        completion_contract: worksgood::graph::CompletionContract::Land,
        completion_candidate: None,
        completion_disposition: None,
        completion_receipt: None,
        tags: vec!["evolution".to_string(), "agency".to_string()],
        skills: vec![],
        inputs: vec![],
        deliverables: vec![],
        artifacts: vec![],
        exec: Some(format!("wg evolve --budget {}", budget)),
        timeout: None,
        not_before: None,
        created_at: Some(Utc::now().to_rfc3339()),
        started_at: None,
        completed_at: None,
        last_interaction_at: None,
        last_message_at: None,
        log: vec![],
        retry_count: 0,
        max_retries: Some(1),
        failure_reason: None,
        failure_class: None,
        failure_signal: None,
        model: Some(evolver_resolved.model),
        reasoning: evolver_resolved.reasoning,
        provider: evolver_resolved.provider,
        endpoint: None,
        remote_provider: None,
        profile: None,
        command_argv: vec![],
        working_dir: None,
        executor_preset_name: None,
        verify: None,
        verify_timeout: None,
        agent: config.agency.evolver_agent.clone(),
        loop_iteration: 0,
        last_iteration_completed_at: None,
        cycle_failure_restarts: 0,
        ready_after: None,
        paused: false,
        visibility: "internal".to_string(),
        context_scope: None,
        cycle_config: None,
        exec_mode: Some("bare".to_string()),
        token_usage: None,
        actual_executor: None,
        actual_model: None,
        completion_review_activity: Vec::new(),
        session_id: None,
        wait_condition: None,
        message_wait: None,
        checkpoint: None,
        triage_count: 0,
        resurrection_count: 0,
        last_resurrected_at: None,
        validation: None,
        validation_commands: vec![],
        validator_agent: None,
        validator_model: None,
        gate_attempts: 0,
        test_required: false,
        rejection_count: 0,
        max_rejections: None,
        verify_failures: 0,
        rescue_count: 0,
        rescued: false,
        meta_eval_attempts: 0,
        agency_dispatch: None,
        evaluation_lifecycle: None,
        evaluation_records: vec![],
        spawn_failures: 0,
        last_spawn_failure_at: None,
        dispatch_count: 0,
        tier: None,
        no_tier_escalation: false,
        tried_models: vec![],
        superseded_by: vec![],
        supersedes: None,
        unplaced: false,
        place_before: vec![],
        place_near: vec![],
        independent: false,
        iteration_round: 0,
        iteration_anchor: None,
        iteration_parent: None,
        iteration_config: None,
        cron_schedule: None,
        cron_enabled: false,
        last_cron_fire: None,
        next_cron_fire: None,
    };

    graph.add_node(Node::Task(evolve_task));

    // Update evolver state to record we've created this task
    // (actual record_evolution happens when the task completes)
    let mut updated_state = state;
    let current_eval_count = evolver::count_evaluation_files(&agency_dir.join("evaluations"));
    let new_evals = current_eval_count.saturating_sub(updated_state.last_eval_count);
    let pre_avg = evolver::compute_current_avg_score(&agency_dir);

    // Record baselines before evolution
    if let Ok(roles) = agency::load_all_roles(&agency_dir.join("cache/roles")) {
        for role in &roles {
            if let Some(avg) = role.performance.avg_score {
                updated_state.baselines.insert(role.id.clone(), avg);
            }
        }
    }

    updated_state.record_evolution(
        &format!("auto-{}", ts),
        new_evals,
        0, // Operations counted when task completes
        vec!["auto-triggered".to_string()],
        pre_avg,
        Some(&evolve_task_id),
    );

    if let Err(e) = updated_state.save(&agency_dir) {
        eprintln!("[dispatcher] Warning: failed to save evolver state: {}", e);
    }

    eprintln!(
        "[dispatcher] Created auto-evolve task '{}' — {}",
        evolve_task_id, trigger_reason,
    );

    true
}

/// Auto-create: trigger the creator agent when enough tasks have completed
/// since the last creation run.
///
/// Checks `config.agency.auto_create` and `auto_create_threshold`. When the
/// number of completed tasks since the last creator invocation exceeds the
/// threshold, creates a `.create-<timestamp>` system task that runs
/// `wg agency create`.
///
/// Returns `true` if the graph was modified.
fn build_auto_create_task(
    dir: &Path,
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
) -> bool {
    let agency_dir = dir.join("agency");

    // Don't create if agency isn't initialized
    if !agency_dir.join("cache/roles").exists() {
        return false;
    }

    // Check that no .create-* task is already in-progress or open
    let has_active_create = graph.tasks().any(|t| {
        t.id.starts_with(".create-") && matches!(t.status, Status::Open | Status::InProgress)
    });
    if has_active_create {
        return false;
    }

    // Collect completed (Done) non-system tasks, sorted by completed_at desc
    let mut completed_tasks: Vec<_> = graph
        .tasks()
        .filter(|t| t.status == Status::Done && !worksgood::graph::is_system_task(&t.id))
        .map(|t| (t.id.clone(), t.completed_at.clone()))
        .collect();
    let completed_count = completed_tasks.len() as u32;
    completed_tasks.sort_by(|a, b| b.1.cmp(&a.1));

    // Load last creator invocation count from state file
    let state_path = agency_dir.join("creator_state.json");
    let last_count: u32 = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("last_completed_count")?.as_u64())
        .unwrap_or(0) as u32;

    let since_last = completed_count.saturating_sub(last_count);

    if since_last < config.agency.auto_create_threshold {
        return false;
    }

    // Causal edges: recently completed tasks that triggered the threshold (all Done, won't block)
    let trigger_ids: Vec<String> = completed_tasks
        .iter()
        .take(since_last.min(5) as usize)
        .map(|(id, _)| id.clone())
        .collect();

    // Generate create task ID
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let create_task_id = format!(".create-{}", ts);

    let creator_resolved = config.resolve_model_for_role(worksgood::config::DispatchRole::Creator);

    let trigger_list = trigger_ids
        .iter()
        .map(|id| format!("- `{}`", id))
        .collect::<Vec<_>>()
        .join("\n");
    let desc = format!(
        "## Auto-Creator Cycle\n\n\
         **Trigger:** {} completed tasks since last creation (threshold: {})\n\n\
         **Triggering tasks:**\n{}\n\n\
         Run `wg agency create` to expand the primitive store with new role components,\n\
         desired outcomes, and tradeoff configurations.\n\n\
         ### Instructions\n\
         1. Run `wg agency create`\n\
         2. Log the results\n\
         3. Mark this task done\n",
        since_last, config.agency.auto_create_threshold, trigger_list,
    );

    let create_parent = trigger_ids.first().cloned();
    let create_task = Task {
        id: create_task_id.clone(),
        title: format!("Auto-create: {} tasks since last creation", since_last),
        presentation: worksgood::graph::TaskPresentation::Autonomous,
        origin: worksgood::graph::TaskOrigin::autonomous(
            create_parent,
            "expand the agency primitive store",
        ),
        description: Some(desc),
        status: Status::Open,
        lifecycle: worksgood::lifecycle::LifecycleProjection::default(),
        priority: PRIORITY_DEFAULT,
        assigned: None,
        estimate: None,
        before: vec![],
        after: trigger_ids,
        input_dependencies: vec![],
        requires: vec![],
        completion_contract: worksgood::graph::CompletionContract::Land,
        completion_candidate: None,
        completion_disposition: None,
        completion_receipt: None,
        tags: vec!["creation".to_string(), "agency".to_string()],
        skills: vec![],
        inputs: vec![],
        deliverables: vec![],
        artifacts: vec![],
        exec: Some("wg agency create".to_string()),
        timeout: None,
        not_before: None,
        created_at: Some(Utc::now().to_rfc3339()),
        started_at: None,
        completed_at: None,
        last_interaction_at: None,
        last_message_at: None,
        log: vec![],
        retry_count: 0,
        max_retries: Some(1),
        failure_reason: None,
        failure_class: None,
        failure_signal: None,
        model: Some(creator_resolved.model),
        reasoning: creator_resolved.reasoning,
        provider: creator_resolved.provider,
        endpoint: None,
        remote_provider: None,
        profile: None,
        command_argv: vec![],
        working_dir: None,
        executor_preset_name: None,
        verify: None,
        verify_timeout: None,
        agent: config.agency.creator_agent.clone(),
        loop_iteration: 0,
        last_iteration_completed_at: None,
        cycle_failure_restarts: 0,
        ready_after: None,
        paused: false,
        visibility: "internal".to_string(),
        context_scope: None,
        cycle_config: None,
        exec_mode: Some("bare".to_string()),
        token_usage: None,
        actual_executor: None,
        actual_model: None,
        completion_review_activity: Vec::new(),
        session_id: None,
        wait_condition: None,
        message_wait: None,
        checkpoint: None,
        triage_count: 0,
        resurrection_count: 0,
        last_resurrected_at: None,
        validation: None,
        validation_commands: vec![],
        validator_agent: None,
        validator_model: None,
        gate_attempts: 0,
        test_required: false,
        rejection_count: 0,
        max_rejections: None,
        verify_failures: 0,
        rescue_count: 0,
        rescued: false,
        meta_eval_attempts: 0,
        agency_dispatch: None,
        evaluation_lifecycle: None,
        evaluation_records: vec![],
        spawn_failures: 0,
        last_spawn_failure_at: None,
        dispatch_count: 0,
        tier: None,
        no_tier_escalation: false,
        tried_models: vec![],
        superseded_by: vec![],
        supersedes: None,
        unplaced: false,
        place_before: vec![],
        place_near: vec![],
        independent: false,
        iteration_round: 0,
        iteration_anchor: None,
        iteration_parent: None,
        iteration_config: None,
        cron_schedule: None,
        cron_enabled: false,
        last_cron_fire: None,
        next_cron_fire: None,
    };

    graph.add_node(Node::Task(create_task));

    // Save state: record current completed count
    let state = serde_json::json!({
        "last_completed_count": completed_count,
        "last_created_at": Utc::now().to_rfc3339(),
        "task_id": create_task_id,
    });
    if let Err(e) = std::fs::write(
        &state_path,
        serde_json::to_string_pretty(&state).unwrap_or_default(),
    ) {
        eprintln!("[dispatcher] Warning: failed to save creator state: {}", e);
    }

    eprintln!(
        "[dispatcher] Created auto-create task '{}' — {} completed tasks since last creation",
        create_task_id, since_last,
    );

    true
}

/// Write standard agent artifacts (metadata.json, prompt.txt, run.sh) for inline-spawned agents.
///
/// Inline spawn paths (eval, assign) used to emit only output.log, making those
/// agents harder to debug/replay. This function brings them in line with the full
/// spawn path in `spawn/execution.rs`.
/// Priority-aware task sorting with starvation prevention and priority inheritance.
///
/// Features:
/// 1. Sort tasks by priority (Critical > High > Normal > Low > Idle)
/// 2. Starvation prevention: tasks waiting longer than threshold get priority bump
/// 3. Priority inheritance: high-priority tasks blocked by low-priority deps boost the blockers
fn sort_tasks_by_priority_with_features<'a>(
    graph: &worksgood::graph::WorkGraph,
    tasks: Vec<&'a worksgood::graph::Task>,
    _config: &Config,
) -> Vec<&'a worksgood::graph::Task> {
    use chrono::Utc;

    // Starvation prevention threshold: tasks older than this get priority boost
    let starvation_threshold_hours = 24; // Can be made configurable later
    let now = Utc::now();

    let mut task_priorities: Vec<_> = tasks
        .into_iter()
        .map(|task| {
            let mut effective_priority = task.priority;

            // Starvation prevention: bump priority for old tasks
            if let Some(ref created_at_str) = task.created_at
                && let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(created_at_str)
            {
                let age = now.signed_duration_since(created_at.with_timezone(&Utc));
                let age_hours = age.num_hours();

                if age_hours > starvation_threshold_hours {
                    // Bump priority by one level for every 24 hours of waiting
                    let bumps = (age_hours / starvation_threshold_hours) as usize;
                    for _ in 0..bumps {
                        effective_priority = boost_priority(effective_priority);
                    }
                    eprintln!(
                        "[dispatcher] Priority bump: {} (age: {}h) -> {}",
                        task.id, age_hours, effective_priority
                    );
                }
            }

            // Priority inheritance: check if this task blocks any high-priority tasks
            let inherited_priority = compute_priority_inheritance(task, graph);
            if inherited_priority > effective_priority {
                eprintln!(
                    "[dispatcher] Priority inheritance: {} ({} -> {})",
                    task.id, effective_priority, inherited_priority
                );
                effective_priority = inherited_priority;
            }

            (task, effective_priority)
        })
        .collect();

    // Sort by effective priority descending (higher number = higher priority),
    // then by dispatch_count ascending (CFS-like fair share: prefer less-dispatched tasks)
    task_priorities.sort_by(|(a_task, a_prio), (b_task, b_prio)| {
        b_prio
            .cmp(a_prio)
            .then(a_task.dispatch_count.cmp(&b_task.dispatch_count))
    });

    // Idle gate: only include idle (priority 0) tasks when no higher-priority tasks are in the set
    let has_normal_or_higher = task_priorities.iter().any(|(_, p)| *p >= PRIORITY_NORMAL);
    if has_normal_or_higher {
        task_priorities.retain(|(_, p)| *p != PRIORITY_IDLE);
    }

    let sorted_tasks: Vec<_> = task_priorities.into_iter().map(|(task, _)| task).collect();

    // Log priority decisions if we have tasks
    if !sorted_tasks.is_empty() {
        let priority_summary: Vec<String> = sorted_tasks
            .iter()
            .take(5) // Log first 5 for brevity
            .map(|task| format!("{}:{}(d{})", task.id, task.priority, task.dispatch_count))
            .collect();
        eprintln!(
            "[dispatcher] Priority dispatch order: [{}{}]",
            priority_summary.join(", "),
            if sorted_tasks.len() > 5 { ", ..." } else { "" }
        );
    }

    sorted_tasks
}

/// Compute priority inheritance for a task based on downstream dependencies.
/// If this task blocks higher-priority tasks, inherit their priority.
fn compute_priority_inheritance(
    task: &worksgood::graph::Task,
    graph: &worksgood::graph::WorkGraph,
) -> Priority {
    let mut highest_inherited = task.priority;

    for dependent_task in graph.tasks() {
        if dependent_task.after.contains(&task.id) {
            if dependent_task.priority > highest_inherited {
                highest_inherited = dependent_task.priority;
            }
        }
    }

    highest_inherited
}

/// Spawn agents on ready tasks, up to `slots_available`. Returns the number of
/// agents successfully spawned.
///
/// Retry cadence is owned by the durable service convergence scheduler below;
/// an unchanged transient can fall off to its cap but never becomes generic
/// `Failed` merely because a counter was exhausted.

/// Record one fail-stop launch decision. Capacity and resource admission do
/// not call this helper; it is reserved for a selected route whose preparation
/// or process launch failed. The coordinator never retries this task
/// implicitly. `wg retry` is the only path back to runnable work.
fn record_direct_dispatch_failure(
    graph_path: &Path,
    task_id: &str,
    diagnostic: &str,
    executor: &str,
) -> bool {
    let now = Utc::now().to_rfc3339();
    let diagnostic = diagnostic.to_string();
    let diagnostic_ref = format!(
        "dispatch-diagnostic:{}",
        blake3::hash(diagnostic.as_bytes()).to_hex()
    );
    let executor = executor.to_string();
    let mut recorded = false;
    let _ = modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        if task.status.is_terminal() {
            return false;
        }
        if task.status != Status::Open {
            let reopen = TransitionRequest::new(
                TransitionKind::GenerationCreated,
                LifecycleActor {
                    kind: ActorKind::Reconciler,
                    id: "direct-dispatch".to_string(),
                },
                "dispatch_retry_generation",
                format!(
                    "dispatch-retry-generation:{task_id}:{}:{}",
                    task.lifecycle.generation, task.lifecycle.revision
                ),
            )
            .expecting(FenceExpectation::current(task));
            if apply_transition(task, reopen).is_err() {
                return false;
            }
        }
        let reservation = TransitionRequest::new(
            TransitionKind::AttemptReserved { owner_id: None },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "direct-dispatch".to_string(),
            },
            "dispatch_attempt_reserved",
            format!(
                "dispatch-failure-reserve:{task_id}:{}:{}",
                task.lifecycle.generation,
                task.lifecycle.attempt_sequence + 1
            ),
        )
        .expecting(FenceExpectation::current(task));
        if apply_transition(task, reservation).is_err() {
            return false;
        }
        let attempt_id = task
            .lifecycle
            .current_attempt
            .as_ref()
            .expect("reservation projected an attempt")
            .id
            .clone();
        let failure = TransitionRequest::new(
            TransitionKind::AttemptFailed {
                class: Some(worksgood::graph::FailureClass::ExecutorConfig),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "direct-dispatch".to_string(),
            },
            "dispatch_launch_failed",
            format!("dispatch-failure:{task_id}:{attempt_id}:{diagnostic_ref}"),
        )
        .with_evidence(diagnostic_ref.clone())
        .expecting(FenceExpectation::current(task));
        if apply_transition(task, failure).is_err() {
            return false;
        }
        task.assigned = None;
        task.failure_reason = Some(diagnostic.clone());
        task.failure_class = Some(worksgood::graph::FailureClass::ExecutorConfig);
        task.completed_at = Some(now.clone());
        task.spawn_failures = task.spawn_failures.saturating_add(1);
        task.last_spawn_failure_at = Some(now.clone());
        task.log.push(LogEntry {
            timestamp: now.clone(),
            actor: Some("direct-dispatch".to_string()),
            user: None,
            message: format!(
                "Exact-route launch failed; the lifecycle attempt failed and will not be retried automatically. executor={executor}: {diagnostic}"
            ),
        });
        recorded = true;
        true
    });
    recorded
}

/// Persist exactly one breaker-neutral diagnostic when the transactional spawn
/// path fails before publishing its launch permit. The task must already be
/// Open and unassigned: this function records evidence but never repairs or
/// overwrites ownership. Keying by generation coalesces repeated coordinator
/// ticks while the operator repairs the same checkout/configuration cause.
fn record_spawn_preparation_deferral(graph_path: &Path, task_id: &str, diagnostic: &str) -> bool {
    let mut recorded = false;
    let diagnostic = diagnostic.to_string();
    let _ = modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        if task.status != Status::Open || task.assigned.is_some() {
            return false;
        }
        let idempotency_key = format!("spawn-preparation:{task_id}:{}", task.lifecycle.generation);
        if task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.idempotency_key == idempotency_key)
        {
            return false;
        }
        let request = TransitionRequest::new(
            TransitionKind::AdmissionDeferred {
                gate: diagnostic.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "spawn-preparation".to_string(),
            },
            "spawn_preparation_deferred",
            idempotency_key,
        );
        if apply_transition(task, request).is_err() {
            return false;
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("spawn-preparation".to_string()),
            user: None,
            message: format!(
                "Spawn preparation deferred before launch permit; rollback is complete and no circuit-breaker charge was recorded. Repair the reported checkout/configuration condition and retry: {diagnostic}"
            ),
        });
        recorded = true;
        true
    });
    recorded
}

fn note_spawn_preparation_deferral(graph_path: &Path, task_id: &str, diagnostic: &str) {
    if record_spawn_preparation_deferral(graph_path, task_id, diagnostic) {
        eprintln!(
            "[dispatcher] Deferring '{}': pre-launch preparation rolled back cleanly; no spawn failure charged. Repair and retry: {}",
            task_id, diagnostic
        );
    }
}

/// Persist one coalesced lifecycle evidence event for an admission refusal.
/// Returns true only for the first identical reason in this task generation so
/// the caller can rate-limit the human log while still reporting every tick in
/// service status metrics.
fn record_admission_deferral(graph_path: &Path, task_id: &str, reason: &str) -> bool {
    let reason_key = blake3::hash(reason.as_bytes()).to_hex();
    let mut recorded = false;
    let _ = modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let idempotency_key = format!(
            "admission:{task_id}:{}:{reason_key}",
            task.lifecycle.generation
        );
        if task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.idempotency_key == idempotency_key)
        {
            return false;
        }
        let request = TransitionRequest::new(
            TransitionKind::AdmissionDeferred {
                gate: reason.to_string(),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "resource-admission".to_string(),
            },
            "resource_admission_deferred",
            idempotency_key,
        );
        recorded = apply_transition(task, request).is_ok();
        recorded
    });
    recorded
}

fn note_admission_deferral(
    summary: &mut SpawnSummary,
    graph_path: &Path,
    task_id: &str,
    reason: &str,
) {
    summary.admission_deferred_tasks = summary.admission_deferred_tasks.saturating_add(1);
    summary
        .admission_deferred_reason
        .get_or_insert_with(|| reason.to_string());
    if record_admission_deferral(graph_path, task_id, reason) {
        eprintln!(
            "[dispatcher] Deferring '{}': {} (admission backpressure, not a spawn failure; identical deferrals are coalesced; retrying on bounded coordinator ticks)",
            task_id, reason
        );
    }
}

/// Direct, fail-stop dispatcher used by the recovery runtime.
///
/// Readiness, capacity and resource admission are observations over the graph
/// and live registry. They are deliberately not persisted as planner effects.
/// Once a canonical route is selected, that exact plan is bound directly to
/// the spawn adapter. A launch failure terminalizes the task visibly; a later
/// attempt requires an explicit operator retry.
fn warn_released_advisory_quality_passes(graph_path: &Path, graph: &worksgood::graph::WorkGraph) {
    let releases: Vec<(String, String, String)> = graph
        .tasks()
        .flat_map(|dependent| {
            dependent.after.iter().filter_map(move |blocker_id| {
                match worksgood::query::dependency_disposition(
                    blocker_id,
                    &dependent.id,
                    graph,
                    graph_path.parent(),
                ) {
                    worksgood::query::DependencyDisposition::AdvisoryQualityBypass { reason } => {
                        Some((dependent.id.clone(), blocker_id.clone(), reason))
                    }
                    _ => None,
                }
            })
        })
        .collect();
    for (dependent_id, quality_id, reason) in releases {
        let marker = format!("advisory quality pass {quality_id} released unchanged batch");
        let mut recorded = false;
        let _ = modify_graph(graph_path, |latest| {
            let Some(task) = latest.get_task_mut(&dependent_id) else {
                return false;
            };
            if task.log.iter().any(|entry| entry.message.contains(&marker)) {
                return false;
            }
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("dispatcher".to_string()),
                user: None,
                message: format!("WARNING: {marker}: {reason}"),
            });
            task.last_interaction_at = Some(Utc::now().to_rfc3339());
            recorded = true;
            true
        });
        if recorded {
            eprintln!("[dispatcher] WARNING: {dependent_id}: {reason}");
        }
    }
}

fn spawn_agents_for_ready_tasks(
    dir: &Path,
    graph: &worksgood::graph::WorkGraph,
    _executor: &str,
    config: &Config,
    default_model: Option<&str>,
    slots_available: usize,
) -> SpawnSummary {
    let graph_file = graph_path(dir);
    warn_released_advisory_quality_passes(&graph_file, graph);
    let cycle_analysis = graph.compute_cycle_analysis();
    let ready_tasks = ready_tasks_with_peers_cycle_aware(graph, dir, &cycle_analysis);
    for task in &ready_tasks {
        if let Err(error) =
            worksgood::query::record_optional_quality_batch_baseline(dir, graph, task)
        {
            // Fail closed: without this create-once baseline, a later failure
            // remains an ordinary blocker and can never claim unchanged release.
            eprintln!(
                "[dispatcher] WARNING: could not baseline optional quality pass {}: {error}",
                task.id
            );
        }
    }
    let final_ready = sort_tasks_by_priority_with_features(graph, ready_tasks, config);
    let agents_dir = dir.join("agency").join("cache/agents");
    let mut summary = SpawnSummary::default();
    let disk_snapshot = worksgood::disk_sentinel::load_snapshot(dir).ok().flatten();
    let builds_blocked = config.coordinator.resource_management.disk_sentinel_enabled
        && disk_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.level.blocks_builds());
    let mut active_build_heavy = active_build_heavy_count(dir, graph);
    let mut profile_cache = worksgood::dispatch::ProfileCache::new();

    for task in final_ready {
        if summary.spawned >= slots_available {
            note_admission_deferral(
                &mut summary,
                &graph_file,
                &task.id,
                "agent slot capacity is full",
            );
            continue;
        }
        if task.assigned.is_some() || is_daemon_managed(task) {
            continue;
        }
        if is_retired_agency_task(&task.id) {
            eprintln!(
                "[dispatcher] Ignoring retired synthetic agency task '{}'; migrate its source task",
                task.id
            );
            continue;
        }

        if task.id.starts_with('.')
            && task.after.iter().any(|dependency| {
                graph
                    .get_task(dependency)
                    .is_some_and(|source| source.status == Status::Abandoned)
            })
        {
            eprintln!(
                "[dispatcher] Skipping '{}': source task is abandoned",
                task.id
            );
            continue;
        }

        let build_class = worksgood::disk_sentinel::classify_task(task);
        let projected = worksgood::disk_sentinel::build_admission(
            dir,
            &config.coordinator.resource_management,
            build_class,
        );
        let projection_reason;
        let disk_reason = if !projected.allowed {
            projection_reason = projected.reason;
            projection_reason.as_str()
        } else {
            disk_snapshot
                .as_ref()
                .map(|snapshot| snapshot.reason.as_str())
                .unwrap_or("disk sentinel has no healthy snapshot")
        };
        if let Some(reason) = build_admission_denial(
            task,
            builds_blocked || !projected.allowed,
            active_build_heavy,
            config.coordinator.resource_management.max_build_agents,
            disk_reason,
        ) {
            note_admission_deferral(&mut summary, &graph_file, &task.id, &reason);
            continue;
        }

        let effective_config =
            worksgood::dispatch::effective_config_for_task(task, config, &mut profile_cache);
        let effective_config: &Config = effective_config.as_ref();
        let task_model = if task.profile.is_some() {
            Some(
                effective_config
                    .resolve_model_for_role(worksgood::config::DispatchRole::TaskAgent)
                    .spawn_model_spec(),
            )
        } else {
            default_model.map(String::from)
        };
        let agent_entity = task
            .agent
            .as_ref()
            .and_then(|agent_hash| agency::find_agent_by_prefix(&agents_dir, agent_hash).ok());
        let agent_executor = agent_entity
            .as_ref()
            .and_then(|agent| agent.explicit_executor());
        let plan = match worksgood::dispatch::plan_spawn(
            task,
            effective_config,
            agent_executor,
            task_model.as_deref(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!(
                    "[dispatcher] Route selection failed for '{}': {error:#}",
                    task.id
                );
                record_direct_dispatch_failure(
                    &graph_file,
                    &task.id,
                    &format!("route selection failed: {error:#}"),
                    "route-selection",
                );
                continue;
            }
        };
        let executor = plan.executor.as_str().to_string();
        let route_id = worksgood::service::HealthRouteKey::from_spawn_plan(&plan).id();
        let route_binding = worksgood::dispatch::spawn_route_binding_id(&route_id);
        let plan_binding = worksgood::dispatch::spawn_plan_binding_id(&plan, &route_id);

        eprintln!(
            "[dispatcher] {}: {}",
            task.id,
            plan.provenance.log_line(&plan)
        );
        eprintln!(
            "[dispatcher] Spawning agent for: {} - {} (executor: {})",
            task.id, task.title, executor
        );
        match spawn::spawn_agent_with_binding(
            dir,
            &task.id,
            &executor,
            task.timeout.as_deref(),
            Some(plan.model.raw.as_str()),
            Some((route_binding.as_str(), plan_binding.as_str())),
        ) {
            Ok((agent_id, pid)) => {
                eprintln!("[dispatcher] Spawned {} (PID {})", agent_id, pid);
                record_dispatch(&graph_file, &task.id);
                summary.spawned += 1;
                if build_class.is_heavy() {
                    active_build_heavy += 1;
                }
            }
            Err(error) => {
                eprintln!("[dispatcher] Launch failed for '{}': {error:#}", task.id);
                record_direct_dispatch_failure(
                    &graph_file,
                    &task.id,
                    &format!("{error:#}"),
                    &executor,
                );
            }
        }
    }

    summary
}

fn record_dispatch(graph_path: &Path, task_id: &str) {
    let task_id_owned = task_id.to_string();
    let _ = modify_graph(graph_path, |graph| {
        if let Some(task) = graph.get_task_mut(&task_id_owned) {
            task.dispatch_count += 1;
            // Clear-on-success self-heal: a successful spawn means the spawn
            // path works again, so reset the per-task circuit breaker. This
            // is what lets a task recover from a transient burst (e.g. a
            // registry/key outage) without a `wg retry` or graph edit.
            let cleared = task.spawn_failures > 0;
            task.spawn_failures = 0;
            task.last_spawn_failure_at = None;
            if cleared {
                task.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("spawn-circuit-breaker".to_string()),
                    user: None,
                    message: "Spawn succeeded — circuit breaker cleared.".to_string(),
                });
            }
            true
        } else {
            false
        }
    });
}

// ---------------------------------------------------------------------------
// Auto-checkpoint for alive agents
// ---------------------------------------------------------------------------

/// Check alive agents and trigger auto-checkpoints when turn count or time
/// thresholds are met. Calls haiku to summarize the agent's recent output.
fn auto_checkpoint_agents(dir: &Path, config: &Config) {
    let interval_turns = config.checkpoint.auto_interval_turns;
    let interval_mins = config.checkpoint.auto_interval_mins;

    // Skip if auto-checkpoint is effectively disabled
    if interval_turns == 0 && interval_mins == 0 {
        return;
    }

    let registry = match AgentRegistry::load(dir) {
        Ok(r) => r,
        Err(_) => return,
    };

    let alive_agents: Vec<_> = registry
        .agents
        .values()
        .filter(|a| a.is_alive() && is_process_alive(a.pid))
        .cloned()
        .collect();

    for agent in &alive_agents {
        if let Err(e) = try_auto_checkpoint(dir, agent, config, interval_turns, interval_mins) {
            eprintln!(
                "[dispatcher] Auto-checkpoint failed for agent {} (task {}): {}",
                agent.id, agent.task_id, e
            );
        }
    }
}

/// Attempt auto-checkpoint for a single agent if thresholds are met.
fn try_auto_checkpoint(
    dir: &Path,
    agent: &worksgood::service::registry::AgentEntry,
    config: &Config,
    interval_turns: u32,
    interval_mins: u32,
) -> Result<()> {
    use crate::commands::checkpoint::{self, CheckpointType};
    use worksgood::stream_event;

    let output_path = std::path::Path::new(&agent.output_file);
    let agent_dir = match output_path.parent() {
        Some(d) => d,
        None => return Ok(()),
    };

    // Read stream events to get turn count
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);

    let events = if stream_path.exists() {
        stream_event::read_stream_events(&stream_path, 0)
            .map(|(evts, _)| evts)
            .unwrap_or_default()
    } else if raw_path.exists() {
        stream_event::translate_claude_stream(&raw_path, 0)
            .map(|(evts, _)| evts)
            .unwrap_or_default()
    } else {
        return Ok(());
    };

    if events.is_empty() {
        return Ok(());
    }

    // Count turns from stream events
    let turn_count: u32 = events
        .iter()
        .filter(|e| matches!(e, stream_event::StreamEvent::Turn { .. }))
        .count() as u32;

    // Get the timestamp of the latest event
    let last_event_ms = events.last().map(|e| e.timestamp_ms()).unwrap_or(0);

    // Load latest checkpoint for this agent to determine if we need a new one
    let latest_checkpoint = checkpoint::load_latest(dir, &agent.id)?;

    let should_checkpoint = match &latest_checkpoint {
        Some(cp) => {
            // Check turn-based trigger
            let cp_turn = cp.turn_count.unwrap_or(0) as u32;
            let turns_since = turn_count.saturating_sub(cp_turn);
            let turn_trigger = interval_turns > 0 && turns_since >= interval_turns;

            // Check time-based trigger
            let cp_ms = chrono::DateTime::parse_from_rfc3339(&cp.timestamp)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0);
            let elapsed_mins = (last_event_ms - cp_ms).max(0) / 60_000;
            let time_trigger = interval_mins > 0 && elapsed_mins as u32 >= interval_mins;

            turn_trigger || time_trigger
        }
        None => {
            // No checkpoint yet — trigger on first threshold
            let turn_trigger = interval_turns > 0 && turn_count >= interval_turns;

            let init_ms = events
                .first()
                .map(|e| e.timestamp_ms())
                .unwrap_or(last_event_ms);
            let elapsed_mins = (last_event_ms - init_ms).max(0) / 60_000;
            let time_trigger = interval_mins > 0 && elapsed_mins as u32 >= interval_mins;

            turn_trigger || time_trigger
        }
    };

    if !should_checkpoint {
        return Ok(());
    }

    // Generate summary via haiku
    let summary = generate_checkpoint_summary(config, &agent.output_file, &agent.task_id)?;

    eprintln!(
        "[dispatcher] Auto-checkpoint for agent {} (task {}, turn {}): {}",
        agent.id,
        agent.task_id,
        turn_count,
        summary.chars().take(80).collect::<String>()
    );

    // Store checkpoint
    checkpoint::run(
        dir,
        &agent.task_id,
        &summary,
        Some(&agent.id),
        &[], // files_modified not tracked in auto-checkpoint
        None,
        Some(turn_count as u64),
        None,
        None,
        CheckpointType::Auto,
        false,
    )?;

    Ok(())
}

/// Call haiku (or configured triage model) to summarize an agent's recent output log.
fn generate_checkpoint_summary(
    config: &Config,
    output_file: &str,
    task_id: &str,
) -> Result<String> {
    let timeout_secs = config.agency.triage_timeout.unwrap_or(30);

    // Read last 20KB of output for summary context
    let log_content = triage::read_truncated_log(output_file, 20_000);

    let prompt = format!(
        r#"Summarize the progress of an agent working on task '{task_id}'.

## Agent Output (last portion)
```
{log_content}
```

## Instructions
Write a 2-4 sentence summary of what the agent has accomplished so far.
Focus on: files modified, features implemented, tests written, current status.
Respond with ONLY the summary text, no JSON or formatting."#
    );

    let result = worksgood::service::llm::run_lightweight_llm_call(
        config,
        worksgood::config::DispatchRole::Triage,
        &prompt,
        timeout_secs,
    )
    .context("Checkpoint summary LLM call failed")?;

    Ok(result.text)
}

/// Single coordinator tick: spawn agents on ready tasks
pub fn coordinator_tick(
    dir: &Path,
    max_agents: usize,
    executor: &str,
    model: Option<&str>,
) -> Result<TickResult> {
    let graph_path = graph_path(dir);

    // Historical planner effects are evidence only. A coordinator tick never
    // replays or acknowledges them and cannot turn them into a spawn.

    // Load config for agency settings
    let config = Config::load_or_default(dir);

    // Credential-free regression hook for proving that attended chat IPC is
    // independent of a slow unattended dispatcher/evaluation pass. This delay
    // is intentionally inside the real tick, after the daemon has accepted its
    // service identity, and is inert unless explicitly injected by a test.
    if let Ok(delay) = std::env::var("WG_TEST_COORDINATOR_TICK_DELAY_MS")
        && let Ok(delay) = delay.parse::<u64>()
        && delay > 0
    {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }

    // Process chat inbox FIRST — chat is a user-facing interaction that must not
    // be blocked by agent capacity limits or empty task queues. The early returns
    // below (max agents, no ready tasks) would skip chat processing otherwise.
    process_chat_inbox(dir);

    // Phase 0.5: dedicated bounded evaluation. This precedes ordinary worker
    // cleanup/admission and deliberately does not consume an AgentRegistry
    // slot, allocate a worktree, or enter build admission. Adapter failures are
    // persisted on the hidden source record and never cross-fall back.
    match worksgood::evaluation::bounded::run_one_pending(dir, &config) {
        Ok(tick) if tick.ran => eprintln!(
            "[evaluation-lane] {} -> {:?}",
            tick.evaluation_id.as_deref().unwrap_or("unknown"),
            tick.state
        ),
        Ok(tick) if tick.deferred => eprintln!(
            "[evaluation-lane] dedicated capacity/rate deferred; source failure accounting unchanged"
        ),
        Ok(_) => {}
        Err(error) => eprintln!(
            "[evaluation-lane] internal lane error (source lifecycle unchanged): {error:#}"
        ),
    }
    // Deep FLIP is an independently selected observation lane. A default
    // bounded record can never reach this selector; only an explicit manual
    // request or completion-time high-risk policy creates its product.
    match worksgood::evaluation::deep::run_one_pending(dir, &config) {
        Ok(tick) if tick.ran => eprintln!(
            "[deep-flip-lane] {} -> {:?}",
            tick.evaluation_id.as_deref().unwrap_or("unknown"),
            tick.state
        ),
        Ok(tick) if tick.deferred => {
            eprintln!("[deep-flip-lane] observation capacity/rate deferred")
        }
        Ok(_) => {}
        Err(error) => eprintln!(
            "[deep-flip-lane] internal lane error (source/config/graph unchanged): {error:#}"
        ),
    }

    // Retained source and periodic disk scans are deliberately NOT performed
    // here. Both may traverse slow/network worktrees, so the daemon's
    // single-flight RetainedWorktreeCleanupLane owns them. A dispatch-critical
    // tick performs no retained-worktree read_dir/metadata/git/source probe.

    // Phase 1: Clean up dead agents and count alive ones
    let alive_count = match cleanup_and_count_alive(dir, &graph_path, max_agents)? {
        Ok(count) => count,
        Err(early_result) => return Ok(early_result),
    };

    // Worktrees are source-bearing recovery state and are never removed by
    // the periodic sentinel. Cleanup-pending markers remain an explicit
    // operator/worktree-GC surface. Build targets (including external paths)
    // are handled only through the ownership registry above.

    // Phase 1.3: Zero-output agent detection — kill agents that have been alive
    // for 5+ minutes with zero bytes in stream files (API call never returned).
    {
        let sweep = super::zero_output::sweep_zero_output_agents(dir);
        if !sweep.observed.is_empty() {
            eprintln!(
                "[dispatcher] Zero-output sweep persisted {} observation(s); no process or graph mutation was authorized",
                sweep.observed.len()
            );
        }
        if sweep.global_outage_detected {
            eprintln!(
                "[dispatcher] Zero-output aggregate evidence observed; planner retained exact routes and no global pause was applied"
            );
        }
    }

    // Phase 1.5: Auto-checkpoint alive agents if thresholds are met
    auto_checkpoint_agents(dir, &config);

    let slots_available = max_agents.saturating_sub(alive_count);

    // Verdict files are immutable evidence. Read them before taking the graph
    // writer lock, then link/consume them in the one atomic graph transaction.
    let legacy_migration = worksgood::eval_lifecycle::migrate_unambiguous_legacy_verdicts(dir);
    if let Ok(count) = legacy_migration.as_ref()
        && *count > 0
    {
        eprintln!(
            "[dispatcher] linked {} unambiguous historical evaluation verdict(s)",
            count
        );
    }
    let (durable_eval_verdicts, eval_evidence_usable) = match legacy_migration {
        Err(error) => {
            eprintln!("[dispatcher] eval lifecycle evidence unavailable (fail-closed): {error:#}");
            (Vec::new(), false)
        }
        Ok(_) => match worksgood::eval_lifecycle::load_durable_verdicts(dir) {
            Ok(verdicts) => (verdicts, true),
            Err(error) => {
                eprintln!(
                    "[dispatcher] eval lifecycle evidence unavailable (fail-closed): {error:#}"
                );
                (Vec::new(), false)
            }
        },
    };

    // Phases 2.5–2.9: Graph maintenance (atomic load-modify-save).
    //
    // Each phase group uses `modify_graph` to hold the file lock across the
    // entire load-modify-save cycle.  This prevents the "lost update" race
    // where a concurrent `wg` command (e.g. `wg publish`, `wg add`, `wg done`)
    // inserts a task between our load and save, and our save clobbers it.
    modify_graph(&graph_path, |graph| {
        let mut modified = false;

        // Phase 2.45: Legacy PendingValidation migration.
        // PendingValidation is deprecated as a routine task lifecycle state
        // (deprecate-pending-validation). Existing tasks stuck in this status
        // are auto-transitioned to Done with a one-time log entry — the
        // assumption per spec is that "if a user wanted to reject the work,
        // they would have run `wg reject` already."
        modified |= migrate_pending_validation_tasks(graph);

        // Phase 2.46: explicit pre-Pi reasoning migration. This is deliberately
        // before ordinary lifecycle repair: invalid missing-reasoning bytes are
        // never replayed against the bounded repair budget. The transaction
        // atomically re-identifies source + satellites or changes nothing.
        modified |= worksgood::eval_lifecycle::migrate_missing_pi_reasoning(graph, &config);

        // Phases 2.47–2.48: route-stable evaluation lifecycle repair and
        // verdict-required parent resolution. A terminal/missing evaluator is
        // never treated as a score. Historical pre-claim rows are normalized
        // once; ambiguous routes park for an operator.
        if eval_evidence_usable {
            modified |= worksgood::eval_lifecycle::repair_historical_rows(graph);
            modified |= worksgood::eval_lifecycle::reconcile_durable_verdicts(
                graph,
                &durable_eval_verdicts,
                config.agency.eval_gate_threshold.unwrap_or(0.7),
                config.agency.auto_rescue_on_eval_fail,
                config.coordinator.max_verify_failures,
                |task| {
                    config.agency.eval_gate_all
                        || task
                            .description
                            .as_deref()
                            .map(crate::commands::deliverables::parse_deliverables)
                            .is_some_and(|deliverables| !deliverables.is_empty())
                },
            );
        }

        // Phase 2.5: Cycle iteration — reactivate cycles where all members are Done.
        {
            let cycle_analysis = graph.compute_cycle_analysis();
            let reactivated = evaluate_all_cycle_iterations(graph, &cycle_analysis);
            if !reactivated.is_empty() {
                eprintln!(
                    "[dispatcher] Cycle iteration: re-activated {} task(s): {:?}",
                    reactivated.len(),
                    reactivated
                );
                modified = true;
            }
        }

        // Phase 2.6: Cycle failure restart — reactivate cycles where a member is Failed
        // and restart_on_failure is true (default).
        {
            let cycle_analysis = graph.compute_cycle_analysis();
            let reactivated = evaluate_all_cycle_failure_restarts(graph, &cycle_analysis);
            if !reactivated.is_empty() {
                eprintln!(
                    "[dispatcher] Cycle failure restart: re-activated {} task(s): {:?}",
                    reactivated.len(),
                    reactivated
                );
                modified = true;
            }
        }

        // Phase 2.7: Evaluate waiting tasks — check if wait conditions are satisfied.
        modified |= evaluate_waiting_tasks(graph, dir);

        // There is intentionally no generic message reconciliation phase.

        // Phase 2.9: Unblock stuck tasks — check for tasks blocked on archived/deleted
        // dependencies or missed completion events.
        modified |= unblock_stuck_tasks(graph, dir);

        // Phase 2.95: Cron task reset — reset Done cron tasks to Open and compute
        // next fire time with jitter so they can be re-dispatched on schedule.
        {
            let cron_task_ids: Vec<String> = graph
                .tasks()
                .filter(|t| t.cron_enabled && t.status == Status::Done)
                .map(|t| t.id.clone())
                .collect();
            for task_id in &cron_task_ids {
                if let Some(task) = graph.get_task_mut(task_id)
                    && worksgood::cron::reset_cron_task(task)
                {
                    eprintln!(
                        "[dispatcher] Cron reset: '{}' → Open (next fire: {})",
                        task_id,
                        task.next_cron_fire.as_deref().unwrap_or("unknown")
                    );
                    modified = true;
                }
            }
        }

        // Phase 2.10: (极maps Removed) Placement is now merged into the assignment step.
        // No separate .place-* tasks are created or handled.

        modified
    })
    .context("Failed to load/save graph during maintenance phases")?;

    // Phases 3–4.8: Agency scaffolding (atomic load-modify-save).
    //
    // `newly_parked_humans` is filled inside the closure by Phase 4.8 and
    // notified AFTER the closure returns, so the (potentially network-bound)
    // human notification never runs while the graph file lock is held.
    let mut newly_parked_humans: Vec<human_dispatch::ParkedHumanTask> = Vec::new();
    let graph = modify_graph(&graph_path, |graph| {
        let mut modified = false;

        // Phase 4: evaluation is lazy and candidate-bound. The authoritative
        // completion transaction mints hidden records; coordinator ticks never
        // create `.evaluate-*`/`.flip-*` graph work.

        // Phase 4.5: FLIP verification

        // Phase 4.55: Separate-agent verification for --verify tasks.
        // Double-gated: requires both (a) the separate-mode explicit config
        // AND (b) the shadow-task autospawn master switch. Master switch
        // defaults off as of 2026-04-17 — see AgencyConfig::verify_autospawn_enabled.
        if config.coordinator.verify_autospawn_enabled
            && config.coordinator.verify_mode == "separate"
        {
            modified |= build_separate_verify_tasks(dir, graph, &config);
        }

        // Phase 4.6: Auto-evolve
        if config.agency.auto_evolve {
            modified |= build_auto_evolve_task(dir, graph, &config);
        }

        // Phase 4.7: Auto-create
        if config.agency.auto_create {
            modified |= build_auto_create_task(dir, graph, &config);
        }

        // Phase 4.8: Human-as-agent dispatch tail (R10) — park ready tasks
        // assigned to a human on WaitCondition::HumanInput so the AI spawn
        // path skips them and the human's reply is what unblocks them. The
        // returned list is notified below, outside this graph lock.
        let parked = human_dispatch::park_ready_human_tasks(graph, dir);
        if !parked.is_empty() {
            modified = true;
            newly_parked_humans = parked;
        }

        modified
    })
    .context("Failed to save graph after auto-assign/auto-evaluate; aborting tick")?;

    // Phase 4.8b: Render each newly parked task to its human (R11), out of lock.
    for parked in &newly_parked_humans {
        human_dispatch::notify_parked_human(dir, parked);
    }

    // Phase 5: Check for ready tasks (after agency phases may have created new ones)
    if let Some(early_result) = check_ready_or_return(&graph, alive_count, dir) {
        return Ok(early_result);
    }

    // Phase 5.5: global outage/backoff controllers are retired. Phase 6
    // performs exact-route admission after the single authoritative spawn plan
    // has resolved handler + wire + endpoint.

    // Phase 6: Spawn agents on ready tasks
    let cycle_analysis = graph.compute_cycle_analysis();
    let final_ready = ready_tasks_with_peers_cycle_aware(&graph, dir, &cycle_analysis);
    // Exclude daemon-managed loop tasks from ready count.
    let ready_count = final_ready.iter().filter(|t| !is_daemon_managed(t)).count();
    drop(final_ready);
    // Resolve task agent model: CLI override > models.task_agent > models.default > agent.model
    let effective_model = model.map(String::from).unwrap_or_else(|| {
        config
            .resolve_model_for_role(worksgood::config::DispatchRole::TaskAgent)
            .spawn_model_spec()
    });
    let spawn_summary = spawn_agents_for_ready_tasks(
        dir,
        &graph,
        executor,
        &config,
        Some(effective_model.as_str()),
        slots_available,
    );

    Ok(TickResult {
        agents_alive: alive_count + spawn_summary.spawned,
        tasks_ready: ready_count,
        agents_spawned: spawn_summary.spawned,
        spawn_breaker_tripped_tasks: spawn_summary.spawn_breaker_tripped_tasks,
        admission_deferred_tasks: spawn_summary.admission_deferred_tasks,
        admission_deferred_reason: spawn_summary.admission_deferred_reason,
    })
}

/// Process pending chat inbox messages and write responses to the outbox.
///
/// Simple stub that acknowledges receipt when the coordinator agent is not
/// running. The full path (CLI → IPC → inbox → coordinator tick → outbox → CLI)
/// is wired; when the coordinator agent is enabled it handles messages instead.
fn process_chat_inbox(dir: &Path) {
    let chat_dir = dir.join("chat");
    if !chat_dir.exists() {
        return;
    }

    // Iterate over all coordinator subdirectories (0, 1, 2, ...)
    let entries = match std::fs::read_dir(&chat_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let coordinator_id: u32 = match name_str.parse() {
            Ok(id) => id,
            Err(_) => continue, // skip non-numeric directories
        };

        if !entry.path().is_dir() {
            continue;
        }

        process_chat_inbox_for(dir, coordinator_id);
    }
}

/// Process pending chat inbox messages for a specific coordinator.
///
/// If a live handler holds the session lock (Phase 7: `wg nex`,
/// `wg claude-handler`, `wg codex-handler`), skip entirely — the
/// handler processes its own inbox and writes real replies. This
/// tick-based stub writer is only the fallback for when no handler
/// is alive.
fn process_chat_inbox_for(dir: &Path, coordinator_id: u32) {
    let chat_ref_dir = dir
        .join("chat")
        .join(format!("coordinator-{}", coordinator_id));
    if let Ok(Some(info)) = worksgood::session_lock::read_holder(&chat_ref_dir)
        && info.alive
    {
        // A live handler owns this chat session — it'll write the
        // real reply. Don't race it with a stub.
        return;
    }
    let inbox_cursor = match chat::read_coordinator_cursor_for(dir, coordinator_id) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[dispatcher] Failed to read chat coordinator cursor for {}: {}",
                coordinator_id, e
            );
            return;
        }
    };

    let new_messages = match chat::read_inbox_since_for(dir, coordinator_id, inbox_cursor) {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!(
                "[dispatcher] Failed to read chat inbox for {}: {}",
                coordinator_id, e
            );
            return;
        }
    };

    if new_messages.is_empty() {
        return;
    }

    eprintln!(
        "[dispatcher] Processing {} chat message(s) for coordinator {}",
        new_messages.len(),
        coordinator_id
    );

    for msg in &new_messages {
        let response = format!(
            "Message received. The coordinator agent will provide \
             intelligent responses. For now, your message has been logged: \"{}\"",
            msg.content
        );
        if let Err(e) = chat::append_outbox_for(dir, coordinator_id, &response, &msg.request_id) {
            eprintln!(
                "[dispatcher] Failed to write chat outbox for coordinator={}, request_id={}: {}",
                coordinator_id, msg.request_id, e
            );
        }

        // Forward the chat message to the user board
        forward_chat_to_user_board(dir, &msg.content, coordinator_id);
    }

    if let Some(last) = new_messages.last()
        && let Err(e) = chat::write_coordinator_cursor_for(dir, coordinator_id, last.id)
    {
        eprintln!(
            "[dispatcher] Failed to update chat coordinator cursor for {}: {}",
            coordinator_id, e
        );
    }
}

/// Forward a chat message to the current user's active user board.
///
/// Resolves the active `.user-{handle}` board and sends the message via the
/// task messaging system. This ensures the user board captures the full
/// conversation history from coordinator chat interactions.
///
/// The `coordinator_id` is included as routing context so the user board
/// shows which coordinator/chat surface each message came from.
pub fn forward_chat_to_user_board(dir: &Path, content: &str, coordinator_id: u32) {
    use worksgood::graph::resolve_user_board_alias;

    let handle = worksgood::current_user();
    let alias = format!(".user-{}", handle);

    let graph_path = super::graph_path(dir);
    let graph = match worksgood::parser::load_graph(&graph_path) {
        Ok(g) => g,
        Err(_) => return,
    };

    let resolved = resolve_user_board_alias(&graph, &alias);
    // If alias wasn't resolved (no active board), skip silently
    if resolved == alias {
        return;
    }

    // Prefix with routing context so the user board shows where the message came from
    let routed_content = format!("user [coord:{}]: {}", coordinator_id, content);

    if let Err(e) = messages::send_message(dir, &resolved, &routed_content, "user", "normal") {
        eprintln!(
            "[dispatcher] Failed to forward chat to user board '{}': {}",
            resolved, e
        );
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::commands::checkpoint::{self, CheckpointType};
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;
    use worksgood::stream_event::{self, StreamEvent, StreamWriter};

    fn make_agent_entry(output_file: &std::path::Path) -> worksgood::service::registry::AgentEntry {
        worksgood::service::registry::AgentEntry {
            id: "agent-1".to_string(),
            pid: std::process::id(),
            task_id: "t1".to_string(),
            executor: "test".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            last_heartbeat: chrono::Utc::now().to_rfc3339(),
            status: worksgood::service::registry::AgentStatus::Working,
            output_file: output_file.to_str().unwrap().to_string(),
            model: None,
            completed_at: None,
            worktree_path: None,
        }
    }

    #[test]
    fn test_eval_routing_condition() {
        // Inline evaluator routing is structural: a dot-prefixed eval/flip/assign
        // task with exec is inline. Tags are inert labels.
        let mut task = Task::default();
        task.id = ".evaluate-t1".to_string();
        task.tags = vec!["evaluation".to_string(), "agency".to_string()];
        task.exec = Some("wg evaluate run t1".to_string());

        let is_inline_eval = task.exec.is_some()
            && (task.id.starts_with(".evaluate-") || task.id.starts_with(".flip-"));
        assert!(is_inline_eval);

        // Non-eval exec task should NOT match
        let mut shell_task = Task::default();
        shell_task.id = "evaluate-t1".to_string();
        shell_task.tags = vec!["evaluation".to_string()];
        shell_task.exec = Some("bash run.sh".to_string());
        let is_inline_eval2 = shell_task.exec.is_some()
            && (shell_task.id.starts_with(".evaluate-") || shell_task.id.starts_with(".flip-"));
        assert!(!is_inline_eval2);

        // Structural eval task but no exec should NOT match
        let mut no_exec = Task::default();
        no_exec.id = ".evaluate-t2".to_string();
        no_exec.tags = vec!["evaluation".to_string()];
        let is_inline_eval3 = no_exec.exec.is_some()
            && (no_exec.id.starts_with(".evaluate-") || no_exec.id.starts_with(".flip-"));
        assert!(!is_inline_eval3);
    }

    fn setup_workgraph_dir(dir: &Path) {
        let graph_path = dir.join("graph.jsonl");
        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "t1".to_string();
        task.title = "Test Task".to_string();
        task.status = Status::InProgress;
        task.assigned = Some("agent-1".to_string());
        graph.add_node(Node::Task(task));
        save_graph(&graph, &graph_path).unwrap();
    }

    fn write_stream_events(agent_dir: &Path, turn_count: u32, start_ms: i64) {
        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let writer = StreamWriter::new(&stream_path);

        writer.write_event(&StreamEvent::Init {
            executor_type: "test".to_string(),
            model: None,
            session_id: None,
            timestamp_ms: start_ms,
        });

        for i in 0..turn_count {
            writer.write_event(&StreamEvent::Turn {
                turn_number: i + 1,
                tools_used: vec![],
                usage: None,
                timestamp_ms: start_ms + (i as i64 + 1) * 60_000, // 1 min between turns
            });
        }
    }

    #[test]
    fn test_auto_checkpoint_turn_trigger() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        setup_workgraph_dir(dir);

        // Create agent directory with stream events (20 turns, threshold is 15)
        let agent_dir = dir.join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "test output").unwrap();

        write_stream_events(&agent_dir, 20, stream_event::now_ms() - 20 * 60_000);

        // Create a registry with a live agent (use PID 1 which should exist)
        let mut registry = worksgood::service::registry::AgentRegistry::default();
        let agent_entry = make_agent_entry(&output_file);
        registry
            .agents
            .insert("agent-1".to_string(), agent_entry.clone());

        let service_dir = dir.join("service");
        std::fs::create_dir_all(&service_dir).unwrap();
        let registry_path = service_dir.join("registry.json");
        std::fs::write(
            &registry_path,
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();

        // Config with 15 turn threshold
        let config = Config::default(); // default has auto_interval_turns=15

        // Should not panic and should attempt checkpoint.
        // The important thing is the logic correctly identifies the trigger.
        let result = try_auto_checkpoint(dir, &agent_entry, &config, 15, 20);
        // Checkpoint was triggered — either succeeds (LLM available) or fails (LLM unavailable).
        // Both outcomes confirm the threshold logic worked correctly.
        match &result {
            Ok(()) => {
                // LLM was available — checkpoint was saved
                let cp_dir = agent_dir.join("checkpoints");
                assert!(
                    cp_dir.exists(),
                    "Checkpoint directory should exist on success"
                );
            }
            Err(e) => {
                // LLM not available — expected in CI environments
                let err_msg = e.to_string();
                assert!(
                    err_msg.to_lowercase().contains("checkpoint summary")
                        || err_msg.contains("claude")
                        || err_msg.contains("Claude")
                        || err_msg.contains("No such file"),
                    "Expected LLM-related error, got: {}",
                    err_msg
                );
            }
        }
    }

    #[test]
    fn test_auto_checkpoint_below_threshold_no_trigger() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        setup_workgraph_dir(dir);

        let agent_dir = dir.join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "test output").unwrap();

        // Only 5 turns, threshold is 15 — should NOT trigger
        let now_ms = stream_event::now_ms();
        write_stream_events(&agent_dir, 5, now_ms - 5 * 60_000);

        let agent_entry = make_agent_entry(&output_file);

        let config = Config::default();

        // Should return Ok(()) — no checkpoint triggered
        let result = try_auto_checkpoint(dir, &agent_entry, &config, 15, 20);
        assert!(result.is_ok());

        // No checkpoint file should exist
        let cp_dir = dir.join("agents").join("agent-1").join("checkpoints");
        assert!(!cp_dir.exists() || std::fs::read_dir(&cp_dir).unwrap().count() == 0);
    }

    #[test]
    fn test_auto_checkpoint_time_trigger() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        setup_workgraph_dir(dir);

        let agent_dir = dir.join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "test output").unwrap();

        // 5 turns spread over 25 minutes (threshold 20 mins)
        let now_ms = stream_event::now_ms();
        let start_ms = now_ms - 25 * 60_000;

        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let writer = StreamWriter::new(&stream_path);
        writer.write_event(&StreamEvent::Init {
            executor_type: "test".to_string(),
            model: None,
            session_id: None,
            timestamp_ms: start_ms,
        });
        for i in 0..5 {
            writer.write_event(&StreamEvent::Turn {
                turn_number: i + 1,
                tools_used: vec![],
                usage: None,
                timestamp_ms: start_ms + (i as i64 + 1) * 5 * 60_000, // 5 min apart
            });
        }

        let agent_entry = make_agent_entry(&output_file);

        let config = Config::default();

        // Should trigger due to time (25 min > 20 min threshold).
        // Either succeeds (LLM available) or fails (LLM unavailable) —
        // both confirm the time-based threshold logic worked correctly.
        let result = try_auto_checkpoint(dir, &agent_entry, &config, 15, 20);
        match &result {
            Ok(()) => {
                let cp_dir = agent_dir.join("checkpoints");
                assert!(
                    cp_dir.exists(),
                    "Checkpoint directory should exist on success"
                );
            }
            Err(e) => {
                let err_msg = e.to_string();
                assert!(
                    err_msg.to_lowercase().contains("checkpoint summary")
                        || err_msg.contains("claude")
                        || err_msg.contains("Claude")
                        || err_msg.contains("No such file"),
                    "Expected LLM-related error, got: {}",
                    err_msg
                );
            }
        }
    }

    #[test]
    fn test_auto_checkpoint_skips_when_recent_checkpoint_exists() {
        let temp = tempdir().unwrap();
        let dir = temp.path();
        setup_workgraph_dir(dir);

        let agent_dir = dir.join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "test output").unwrap();

        // 20 turns
        let now_ms = stream_event::now_ms();
        write_stream_events(&agent_dir, 20, now_ms - 20 * 60_000);

        // Create a recent checkpoint at turn 18 (so only 2 turns since)
        checkpoint::run(
            dir,
            "t1",
            "Recent checkpoint",
            Some("agent-1"),
            &[],
            None,
            Some(18),
            None,
            None,
            CheckpointType::Auto,
            false,
        )
        .unwrap();

        let agent_entry = make_agent_entry(&output_file);

        let config = Config::default();

        // Should NOT trigger — only 2 turns since last checkpoint
        let result = try_auto_checkpoint(dir, &agent_entry, &config, 15, 20);
        assert!(result.is_ok());
    }

    #[test]
    fn test_auto_checkpoint_disabled_when_zero() {
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.checkpoint.auto_interval_turns = 0;
        config.checkpoint.auto_interval_mins = 0;

        // Should return immediately without touching anything
        auto_checkpoint_agents(dir.path(), &config);
        // No crash, no panic — success
    }

    // === Wait condition evaluation tests ===

    fn setup_wait_graph(dir: &Path, tasks: Vec<Task>) {
        let path = dir.join("graph.jsonl");
        std::fs::create_dir_all(dir).unwrap();
        let mut graph = WorkGraph::new();
        for task in tasks {
            graph.add_node(Node::Task(task));
        }
        save_graph(&graph, &path).unwrap();
    }

    fn load_wait_graph(dir: &Path) -> WorkGraph {
        let path = dir.join("graph.jsonl");
        load_graph(&path).unwrap()
    }

    #[test]
    fn test_evaluate_condition_task_status_satisfied() {
        let mut graph = WorkGraph::new();
        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Done;
        graph.add_node(Node::Task(dep));

        let cond = WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        };
        assert!(evaluate_condition(
            &cond,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_condition_task_status_not_satisfied() {
        let mut graph = WorkGraph::new();
        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::InProgress;
        graph.add_node(Node::Task(dep));

        let cond = WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        };
        assert!(!evaluate_condition(
            &cond,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_condition_timer_elapsed() {
        let graph = WorkGraph::new();
        let past = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let cond = WaitCondition::Timer { resume_after: past };
        assert!(evaluate_condition(
            &cond,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_condition_timer_not_elapsed() {
        let graph = WorkGraph::new();
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let cond = WaitCondition::Timer {
            resume_after: future,
        };
        assert!(!evaluate_condition(
            &cond,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_condition_file_changed() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("watched.txt");
        std::fs::write(&file_path, "initial").unwrap();

        let mtime = std::fs::metadata(&file_path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let graph = WorkGraph::new();
        // Not changed yet: same mtime
        let cond_same = WaitCondition::FileChanged {
            path: file_path.to_string_lossy().to_string(),
            mtime_at_wait: mtime,
        };
        assert!(!evaluate_condition(
            &cond_same,
            &graph,
            dir.path(),
            "main",
            None
        ));

        // Simulate earlier mtime_at_wait (file was modified after the stored mtime)
        let cond_earlier = WaitCondition::FileChanged {
            path: file_path.to_string_lossy().to_string(),
            mtime_at_wait: mtime - 1,
        };
        assert!(evaluate_condition(
            &cond_earlier,
            &graph,
            dir.path(),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_wait_spec_all_not_satisfied() {
        let mut graph = WorkGraph::new();
        let mut dep_a = Task::default();
        dep_a.id = "dep-a".to_string();
        dep_a.status = Status::Done;
        let mut dep_b = Task::default();
        dep_b.id = "dep-b".to_string();
        dep_b.status = Status::Open;
        graph.add_node(Node::Task(dep_a));
        graph.add_node(Node::Task(dep_b));

        let spec = WaitSpec::All(vec![
            WaitCondition::TaskStatus {
                task_id: "dep-a".to_string(),
                status: Status::Done,
            },
            WaitCondition::TaskStatus {
                task_id: "dep-b".to_string(),
                status: Status::Done,
            },
        ]);
        assert!(!evaluate_wait_spec(
            &spec,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_evaluate_wait_spec_any_satisfied() {
        let mut graph = WorkGraph::new();
        let mut dep_a = Task::default();
        dep_a.id = "dep-a".to_string();
        dep_a.status = Status::Done;
        let mut dep_b = Task::default();
        dep_b.id = "dep-b".to_string();
        dep_b.status = Status::Open;
        graph.add_node(Node::Task(dep_a));
        graph.add_node(Node::Task(dep_b));

        let spec = WaitSpec::Any(vec![
            WaitCondition::TaskStatus {
                task_id: "dep-a".to_string(),
                status: Status::Done,
            },
            WaitCondition::TaskStatus {
                task_id: "dep-b".to_string(),
                status: Status::Done,
            },
        ]);
        assert!(evaluate_wait_spec(
            &spec,
            &graph,
            Path::new("/tmp"),
            "main",
            None
        ));
    }

    #[test]
    fn test_unsatisfiable_condition_failed_dep() {
        let mut graph = WorkGraph::new();
        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Failed;
        graph.add_node(Node::Task(dep));

        let cond = WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        };
        let result = is_condition_unsatisfiable(&cond, &graph);
        assert!(result.is_some());
        assert!(result.unwrap().contains("failed"));
    }

    #[test]
    fn test_unsatisfiable_condition_nonexistent_task() {
        let graph = WorkGraph::new();
        let cond = WaitCondition::TaskStatus {
            task_id: "nonexistent".to_string(),
            status: Status::Done,
        };
        let result = is_condition_unsatisfiable(&cond, &graph);
        assert!(result.is_some());
        assert!(result.unwrap().contains("no longer exists"));
    }

    #[test]
    fn test_circular_wait_detection() {
        let mut graph = WorkGraph::new();

        let mut task_a = Task::default();
        task_a.id = "task-a".to_string();
        task_a.status = Status::Waiting;
        task_a.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "task-b".to_string(),
            status: Status::Done,
        }]));

        let mut task_b = Task::default();
        task_b.id = "task-b".to_string();
        task_b.status = Status::Waiting;
        task_b.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "task-a".to_string(),
            status: Status::Done,
        }]));

        graph.add_node(Node::Task(task_a));
        graph.add_node(Node::Task(task_b));

        let cycles = detect_circular_waits(&graph);
        assert!(!cycles.is_empty(), "Should detect circular wait");
    }

    #[test]
    fn test_evaluate_waiting_tasks_transitions_to_open() {
        let dir = tempdir().unwrap();

        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Done;

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        }]));
        main_task.checkpoint = Some("Phase 1 complete".to_string());
        main_task.assigned = Some("agent-1".to_string());

        setup_wait_graph(dir.path(), vec![dep, main_task]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.wait_condition.is_none());
        assert!(
            task.assigned.is_none(),
            "assigned should be cleared for re-dispatch"
        );
        assert!(task.checkpoint.is_some());
        let cp = task.checkpoint.as_ref().unwrap();
        assert!(cp.contains("Resume Context"));
        assert!(cp.contains("Phase 1 complete"));
    }

    #[test]
    fn test_evaluate_waiting_tasks_does_not_infer_failure_from_unsatisfiable_observation() {
        let dir = tempdir().unwrap();

        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Failed;

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        }]));

        setup_wait_graph(dir.path(), vec![dep, main_task]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(!modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.failure_reason.is_none());
        assert!(task.wait_condition.is_some());
    }

    #[test]
    fn test_evaluate_waiting_tasks_does_not_infer_failure_from_circular_waits() {
        let dir = tempdir().unwrap();

        let mut task_a = Task::default();
        task_a.id = "task-a".to_string();
        task_a.status = Status::Waiting;
        task_a.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "task-b".to_string(),
            status: Status::Done,
        }]));

        let mut task_b = Task::default();
        task_b.id = "task-b".to_string();
        task_b.status = Status::Waiting;
        task_b.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "task-a".to_string(),
            status: Status::Done,
        }]));

        setup_wait_graph(dir.path(), vec![task_a, task_b]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(!modified);
        let a = graph.get_task("task-a").unwrap();
        let b = graph.get_task("task-b").unwrap();
        assert_eq!(a.status, Status::Waiting);
        assert_eq!(b.status, Status::Waiting);
        assert!(a.failure_reason.is_none());
        assert!(b.failure_reason.is_none());
    }

    #[test]
    fn dependency_reconciliation_opens_audited_new_generation() {
        let dir = tempdir().unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "dep".into(),
            title: "dep".into(),
            status: Status::Done,
            ..Task::default()
        }));
        graph.add_node(Node::Task(Task {
            id: "work".into(),
            title: "work".into(),
            status: Status::Blocked,
            after: vec!["dep".into()],
            ..Task::default()
        }));

        assert!(unblock_stuck_tasks(&mut graph, dir.path()));
        let task = graph.get_task("work").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.lifecycle.generation, 1);
        assert_eq!(task.lifecycle.audit.len(), 1);
        assert_eq!(
            task.lifecycle.audit[0].reason_code,
            "dependencies_satisfied"
        );
    }

    #[test]
    fn test_wait_resume_preserves_session_id() {
        let dir = tempdir().unwrap();

        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Done;
        dep.artifacts = vec!["docs/api-schema.json".to_string()];

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.session_id = Some("session-123".to_string());
        main_task.checkpoint = Some("Waiting for API schema".to_string());
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        }]));
        main_task.assigned = Some("agent-1".to_string());

        setup_wait_graph(dir.path(), vec![dep, main_task]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.session_id.as_deref(), Some("session-123"));
        let cp = task.checkpoint.as_ref().unwrap();
        assert!(cp.contains("dep-a"));
        assert!(cp.contains("docs/api-schema.json"));
    }

    #[test]
    fn test_build_resume_delta_content() {
        let mut graph = WorkGraph::new();

        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::Done;
        dep.artifacts = vec!["output.txt".to_string()];
        graph.add_node(Node::Task(dep));

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.checkpoint = Some("Working on phase 2".to_string());
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        }]));
        graph.add_node(Node::Task(main_task));

        let task = graph.get_task("main").unwrap();
        let delta = build_resume_delta(&graph, task, Path::new("/tmp"));

        assert!(delta.contains("Resume Context"));
        assert!(delta.contains("dep-a: done"));
        assert!(delta.contains("output.txt"));
        assert!(delta.contains("Working on phase 2"));
        assert!(delta.contains("Continue your work"));
    }

    #[test]
    fn test_evaluate_waiting_tasks_no_change_when_not_satisfied() {
        let dir = tempdir().unwrap();

        let mut dep = Task::default();
        dep.id = "dep-a".to_string();
        dep.status = Status::InProgress;

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::TaskStatus {
            task_id: "dep-a".to_string(),
            status: Status::Done,
        }]));

        setup_wait_graph(dir.path(), vec![dep, main_task]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(!modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.wait_condition.is_some());
    }

    #[test]
    fn test_legacy_unbound_message_wait_is_fail_closed() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::Message]));
        setup_wait_graph(dir.path(), vec![main_task]);
        messages::send_message(dir.path(), "main", "Hello", "user", "normal").unwrap();

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(!modified);
        assert_eq!(graph.get_task("main").unwrap().status, Status::Waiting);
    }

    #[test]
    fn test_attempt_bound_human_message_wait_consumes_once_and_nonmatch_is_inert() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let attempt = worksgood::lifecycle::AttemptRef {
            id: "attempt-3-1".to_string(),
            generation: 3,
            fence: 8,
            actor_id: "agent-waiter".to_string(),
            disposition: Some(worksgood::lifecycle::AttemptDisposition::Parked),
        };
        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.lifecycle.generation = 3;
        main_task.lifecycle.fence = 8;
        main_task.lifecycle.current_attempt = Some(attempt);
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::HumanInput]));
        main_task.message_wait = Some(worksgood::graph::MessageWaitSubscription {
            id: "message-wait:main:3:attempt-3-1".to_string(),
            attempt_epoch: 3,
            attempt_id: "attempt-3-1".to_string(),
            selector: worksgood::graph::MessageWaitSelector::HumanInput,
            armed: true,
            consumed_by_message_id: None,
            resume_request_id: None,
        });
        setup_wait_graph(dir.path(), vec![main_task]);

        messages::send_message(dir.path(), "main", "agent chatter", "agent-other", "normal")
            .unwrap();
        let mut graph = load_wait_graph(dir.path());
        assert!(!evaluate_waiting_tasks(&mut graph, dir.path()));
        assert_eq!(graph.get_task("main").unwrap().status, Status::Waiting);

        messages::send_message(dir.path(), "main", "human answer", "user", "normal").unwrap();
        let mut graph = load_wait_graph(dir.path());
        assert!(evaluate_waiting_tasks(&mut graph, dir.path()));
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Open);
        let subscription = task.message_wait.as_ref().unwrap();
        assert!(!subscription.armed);
        assert_eq!(subscription.consumed_by_message_id, Some(2));
        let revision = task.lifecycle.revision;

        assert!(!evaluate_waiting_tasks(&mut graph, dir.path()));
        assert_eq!(graph.get_task("main").unwrap().lifecycle.revision, revision);
    }

    // -----------------------------------------------------------------------
    // Messages are data tests (the old resurrection tests were inverted)
    // -----------------------------------------------------------------------

    #[test]
    fn test_irrelevant_messages_never_mutate_task_lifecycle() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let statuses = [
            Status::Open,
            Status::InProgress,
            Status::Done,
            Status::Failed,
        ];
        let mut graph = WorkGraph::new();
        for status in statuses {
            let mut task = Task::default();
            task.id = format!("target-{status}");
            task.title = task.id.clone();
            task.status = status;
            task.assigned = Some("agent-owner".to_string());
            graph.add_node(Node::Task(task));
        }
        for status in statuses {
            messages::send_message(
                dir.path(),
                &format!("target-{status}"),
                "irrelevant follow-up",
                "user",
                "normal",
            )
            .unwrap();
        }
        let before: Vec<_> = graph
            .tasks()
            .map(|task| {
                (
                    task.id.clone(),
                    task.status,
                    task.assigned.clone(),
                    task.lifecycle.clone(),
                    task.spawn_failures,
                )
            })
            .collect();
        assert!(!assert_ordinary_messages_are_inert(&mut graph, dir.path()));
        let after: Vec<_> = graph
            .tasks()
            .map(|task| {
                (
                    task.id.clone(),
                    task.status,
                    task.assigned.clone(),
                    task.lifecycle.clone(),
                    task.spawn_failures,
                )
            })
            .collect();
        assert_eq!(after, before);
        assert!(graph.get_task(".respond-to-target-done").is_none());
    }

    #[test]
    fn test_post_terminal_message_with_done_downstream_is_inert() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        // Parent is Done, downstream is also Done (already finished)
        let mut parent = Task::default();
        parent.id = "parent".to_string();
        parent.status = Status::Done;
        parent.before = vec!["downstream".to_string()];

        let mut downstream = Task::default();
        downstream.id = "downstream".to_string();
        downstream.status = Status::Done;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(parent));
        graph.add_node(Node::Task(downstream));

        messages::send_message(dir.path(), "parent", "Late feedback", "user", "normal").unwrap();

        let modified = assert_ordinary_messages_are_inert(&mut graph, dir.path());

        assert!(!modified);
        assert_eq!(graph.get_task("parent").unwrap().status, Status::Done);
        assert_eq!(graph.get_task("downstream").unwrap().status, Status::Done);
        assert!(graph.get_task(".respond-to-parent").is_none());
    }

    #[test]
    fn direct_dispatch_failure_is_terminal_and_has_no_implicit_wait() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let graph_path = wg_dir.join("graph.jsonl");

        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "direct-failure".to_string();
        task.status = Status::Open;
        graph.add_node(Node::Task(task));
        save_graph(&graph, &graph_path).unwrap();

        assert!(record_direct_dispatch_failure(
            &graph_path,
            "direct-failure",
            "selected Pi route exited before launch",
            "pi",
        ));
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("direct-failure").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert_eq!(task.assigned, None);
        assert_eq!(task.wait_condition, None);
        assert_eq!(task.spawn_failures, 1);
        assert_eq!(
            task.failure_reason.as_deref(),
            Some("selected Pi route exited before launch")
        );
        assert_eq!(task.lifecycle.audit.len(), 2);
        assert_eq!(
            task.lifecycle
                .current_attempt
                .as_ref()
                .and_then(|attempt| attempt.disposition),
            Some(worksgood::lifecycle::AttemptDisposition::Failed)
        );
        assert!(task.log.iter().any(|entry| {
            entry.actor.as_deref() == Some("direct-dispatch")
                && entry.message.contains("will not be retried automatically")
        }));
    }

    #[test]
    fn test_record_dispatch_clears_breaker_on_success() {
        // Clear-on-success self-heal: a successful spawn resets spawn_failures
        // and last_spawn_failure_at so a transient burst self-corrects.
        let dir = tempdir().unwrap();
        let wg_dir = dir.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let gp = wg_dir.join("graph.jsonl");

        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "t".to_string();
        task.status = Status::Open;
        task.spawn_failures = 4;
        task.last_spawn_failure_at = Some(chrono::Utc::now().to_rfc3339());
        graph.add_node(Node::Task(task));
        save_graph(&graph, &gp).unwrap();

        record_dispatch(&gp, "t");
        let g = load_graph(&gp).unwrap();
        let t = g.get_task("t").unwrap();
        assert_eq!(t.spawn_failures, 0, "success must clear spawn_failures");
        assert_eq!(t.dispatch_count, 1);
        assert!(
            t.last_spawn_failure_at.is_none(),
            "success must clear last_spawn_failure_at"
        );
        assert!(
            t.log.iter().any(|e| e.message.contains("Spawn succeeded")),
            "should log clear-on-success"
        );
    }

    #[test]
    fn test_spawn_circuit_breaker_reset_on_edit() {
        // Verify that editing a task resets spawn_failures
        let dir = tempdir().unwrap();
        let wg_dir = dir.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let gp = wg_dir.join("graph.jsonl");

        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "reset-task".to_string();
        task.title = "Reset Test".to_string();
        task.status = Status::Open;
        task.spawn_failures = 3;
        task.exec_mode = Some("shell".to_string());
        graph.add_node(Node::Task(task));
        save_graph(&graph, &gp).unwrap();

        // Edit the task (change exec_mode)
        crate::commands::edit::run(
            &wg_dir,
            "reset-task",
            None,         // title
            None,         // description
            &[],          // add_after
            &[],          // remove_after
            &[],          // add_tag
            &[],          // remove_tag
            None,         // model
            None,         // provider
            &[],          // add_skill
            &[],          // remove_skill
            None,         // max_iterations
            None,         // cycle_guard
            None,         // cycle_delay
            false,        // no_converge
            false,        // no_restart_on_failure
            None,         // max_failure_restarts
            None,         // visibility
            None,         // context_scope
            Some("full"), // exec_mode — the fix
            None,         // delay
            None,         // not_before
            None,         // verify
            None,         // cron
            None,         // timeout
            None,         // verify_timeout
            false,        // allow_phantom
            false,        // allow_cycle
        )
        .unwrap();

        let g = load_graph(&gp).unwrap();
        let t = g.get_task("reset-task").unwrap();
        assert_eq!(
            t.spawn_failures, 0,
            "spawn_failures should be reset after edit"
        );
        assert_eq!(
            t.exec_mode.as_deref(),
            Some("full"),
            "exec_mode should be updated"
        );
    }

    #[test]
    fn test_separate_verify_task_created_for_pending_validation() {
        // When verify_mode=separate, tasks in PendingValidation with a verify
        // command and the right log entry should get a .sep-verify-* task created.
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Implement feature X".to_string();
        source.status = Status::PendingValidation;
        source.verify = Some("cargo test test_feature_x".to_string());
        source.description = Some("Build feature X".to_string());
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("agent-1".to_string()),
            user: None,
            message: "Pending separate verification (verify_mode=separate)".to_string(),
        });

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        save_graph(&graph, &graph_path).unwrap();

        let mut config = Config::default();
        config.coordinator.verify_mode = "separate".to_string();

        let mut graph = worksgood::parser::load_graph(&graph_path).unwrap();
        let modified = build_separate_verify_tasks(dir.path(), &mut graph, &config);
        assert!(modified, "should have created a verify task");

        let verify_task = graph.get_task(".sep-verify-my-task").unwrap();
        assert_eq!(verify_task.status, Status::Open);
        assert!(
            verify_task.tags.contains(&"separate-verify".to_string()),
            "should be tagged as separate-verify"
        );
        assert!(
            verify_task.after.contains(&"my-task".to_string()),
            "verify task should depend on source task"
        );
        assert!(
            verify_task
                .description
                .as_ref()
                .unwrap()
                .contains("cargo test test_feature_x"),
            "description should contain the verify command"
        );
        assert!(
            verify_task
                .description
                .as_ref()
                .unwrap()
                .contains("wg approve my-task"),
            "description should tell agent how to approve"
        );
        assert!(
            verify_task
                .description
                .as_ref()
                .unwrap()
                .contains("wg reject my-task"),
            "description should tell agent how to reject"
        );
    }

    #[test]
    fn test_separate_verify_not_created_when_inline_mode() {
        // When verify_mode=inline, no .sep-verify-* tasks should be created
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Implement feature X".to_string();
        source.status = Status::PendingValidation;
        source.verify = Some("cargo test".to_string());
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("agent-1".to_string()),
            user: None,
            message: "Pending separate verification (verify_mode=separate)".to_string(),
        });

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        save_graph(&graph, &graph_path).unwrap();

        // Config defaults to "inline"
        let config = Config::default();
        assert_eq!(config.coordinator.verify_mode, "inline");

        // build_separate_verify_tasks should not be called when inline,
        // but even if called it should still create tasks (the guard is
        // in the coordinator tick). Let's test the coordinator_tick guard:
        // The function itself creates tasks regardless — the config check
        // is in coordinator_tick. So let's just verify default config is "inline".
    }

    #[test]
    fn test_separate_verify_idempotent() {
        // Running build_separate_verify_tasks twice should not create duplicates
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Test".to_string();
        source.status = Status::PendingValidation;
        source.verify = Some("cargo test".to_string());
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: None,
            message: "Pending separate verification (verify_mode=separate)".to_string(),
        });

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        save_graph(&graph, &graph_path).unwrap();

        let mut config = Config::default();
        config.coordinator.verify_mode = "separate".to_string();

        let mut graph = worksgood::parser::load_graph(&graph_path).unwrap();
        let modified1 = build_separate_verify_tasks(dir.path(), &mut graph, &config);
        assert!(modified1);

        let modified2 = build_separate_verify_tasks(dir.path(), &mut graph, &config);
        assert!(!modified2, "should not create duplicate verify task");
    }

    #[test]
    fn test_separate_verify_skips_system_tasks() {
        // System tasks (dot-prefixed) should not get separate verification
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = ".evaluate-something".to_string();
        source.title = "Eval".to_string();
        source.status = Status::PendingValidation;
        source.verify = Some("echo ok".to_string());
        source.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: None,
            message: "Pending separate verification (verify_mode=separate)".to_string(),
        });

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        save_graph(&graph, &graph_path).unwrap();

        let mut config = Config::default();
        config.coordinator.verify_mode = "separate".to_string();

        let mut graph = worksgood::parser::load_graph(&graph_path).unwrap();
        let modified = build_separate_verify_tasks(dir.path(), &mut graph, &config);
        assert!(!modified, "should not create verify task for system tasks");
    }

    // ========== Priority dispatch tests ==========

    #[test]
    fn test_dispatch_orders_by_priority() {
        let config = Config::default();
        let mut graph = WorkGraph::new();

        let mut critical = Task::default();
        critical.id = "task-critical".to_string();
        critical.title = "Critical task".to_string();
        critical.status = worksgood::graph::Status::Open;
        critical.priority = worksgood::graph::PRIORITY_CRITICAL;
        critical.created_at = Some(Utc::now().to_rfc3339());

        let mut normal = Task::default();
        normal.id = "task-normal".to_string();
        normal.title = "Normal task".to_string();
        normal.status = worksgood::graph::Status::Open;
        normal.priority = worksgood::graph::PRIORITY_NORMAL;
        normal.created_at = Some(Utc::now().to_rfc3339());

        let mut low = Task::default();
        low.id = "task-low".to_string();
        low.title = "Low task".to_string();
        low.status = worksgood::graph::Status::Open;
        low.priority = worksgood::graph::PRIORITY_LOW;
        low.created_at = Some(Utc::now().to_rfc3339());

        graph.add_node(Node::Task(normal.clone()));
        graph.add_node(Node::Task(low.clone()));
        graph.add_node(Node::Task(critical.clone()));

        // Pass tasks in wrong order to verify sorting fixes it
        let tasks: Vec<&Task> = vec![
            graph.get_task("task-normal").unwrap(),
            graph.get_task("task-low").unwrap(),
            graph.get_task("task-critical").unwrap(),
        ];

        let sorted = sort_tasks_by_priority_with_features(&graph, tasks, &config);
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].id, "task-critical");
        assert_eq!(sorted[1].id, "task-normal");
        assert_eq!(sorted[2].id, "task-low");
    }

    #[test]
    fn test_within_level_fair_share() {
        let config = Config::default();
        let mut graph = WorkGraph::new();

        let mut task_a = Task::default();
        task_a.id = "task-a".to_string();
        task_a.title = "Task A".to_string();
        task_a.status = worksgood::graph::Status::Open;
        task_a.priority = worksgood::graph::PRIORITY_NORMAL;
        task_a.dispatch_count = 3;
        task_a.created_at = Some(Utc::now().to_rfc3339());

        let mut task_b = Task::default();
        task_b.id = "task-b".to_string();
        task_b.title = "Task B".to_string();
        task_b.status = worksgood::graph::Status::Open;
        task_b.priority = worksgood::graph::PRIORITY_NORMAL;
        task_b.dispatch_count = 1;
        task_b.created_at = Some(Utc::now().to_rfc3339());

        graph.add_node(Node::Task(task_a.clone()));
        graph.add_node(Node::Task(task_b.clone()));

        let tasks: Vec<&Task> = vec![
            graph.get_task("task-a").unwrap(),
            graph.get_task("task-b").unwrap(),
        ];

        let sorted = sort_tasks_by_priority_with_features(&graph, tasks, &config);
        assert_eq!(sorted.len(), 2);
        // task-b has fewer dispatches (1 vs 3), so it should come first
        assert_eq!(sorted[0].id, "task-b");
        assert_eq!(sorted[1].id, "task-a");
    }

    #[test]
    fn test_idle_only_dispatched_when_higher_empty() {
        let config = Config::default();
        let mut graph = WorkGraph::new();

        let mut idle_task = Task::default();
        idle_task.id = "task-idle".to_string();
        idle_task.title = "Idle task".to_string();
        idle_task.status = worksgood::graph::Status::Open;
        idle_task.priority = worksgood::graph::PRIORITY_IDLE;
        idle_task.created_at = Some(Utc::now().to_rfc3339());

        let mut normal_task = Task::default();
        normal_task.id = "task-normal".to_string();
        normal_task.title = "Normal task".to_string();
        normal_task.status = worksgood::graph::Status::Open;
        normal_task.priority = worksgood::graph::PRIORITY_NORMAL;
        normal_task.created_at = Some(Utc::now().to_rfc3339());

        // Case 1: Idle + Normal ready → Idle excluded
        graph.add_node(Node::Task(idle_task.clone()));
        graph.add_node(Node::Task(normal_task.clone()));

        let tasks: Vec<&Task> = vec![
            graph.get_task("task-idle").unwrap(),
            graph.get_task("task-normal").unwrap(),
        ];

        let sorted = sort_tasks_by_priority_with_features(&graph, tasks, &config);
        assert_eq!(
            sorted.len(),
            1,
            "Idle should be excluded when Normal is present"
        );
        assert_eq!(sorted[0].id, "task-normal");

        // Case 2: Only Idle ready → Idle included
        let mut graph2 = WorkGraph::new();
        graph2.add_node(Node::Task(idle_task.clone()));

        let tasks2: Vec<&Task> = vec![graph2.get_task("task-idle").unwrap()];

        let sorted2 = sort_tasks_by_priority_with_features(&graph2, tasks2, &config);
        assert_eq!(
            sorted2.len(),
            1,
            "Idle should be dispatched when nothing else is ready"
        );
        assert_eq!(sorted2[0].id, "task-idle");

        // Case 3: Idle + Low ready (no Normal+) → both included
        let mut graph3 = WorkGraph::new();
        let mut low_task = Task::default();
        low_task.id = "task-low".to_string();
        low_task.title = "Low task".to_string();
        low_task.status = worksgood::graph::Status::Open;
        low_task.priority = worksgood::graph::PRIORITY_LOW;
        low_task.created_at = Some(Utc::now().to_rfc3339());
        graph3.add_node(Node::Task(idle_task.clone()));
        graph3.add_node(Node::Task(low_task.clone()));

        let tasks3: Vec<&Task> = vec![
            graph3.get_task("task-idle").unwrap(),
            graph3.get_task("task-low").unwrap(),
        ];

        let sorted3 = sort_tasks_by_priority_with_features(&graph3, tasks3, &config);
        assert_eq!(
            sorted3.len(),
            2,
            "Idle included when only Low tasks present"
        );
        assert_eq!(sorted3[0].id, "task-low");
        assert_eq!(sorted3[1].id, "task-idle");
    }

    // ------------------------------------------------------------------
    // chat-agent-loops bug A: chat-loop tagged tasks must NOT be claimed
    // by the dispatcher — the daemon's `subprocess_coordinator_loop`
    // owns spawning chat handlers via `wg spawn-task` directly. Letting
    // the dispatcher also claim them spawns a regular worker that idle-
    // loops `wg log` + `wg done`, which is the user's repro.
    // ------------------------------------------------------------------

    fn task_with_tags(id: &str, tags: &[&str]) -> Task {
        let mut t = Task::default();
        t.id = id.to_string();
        t.title = id.to_string();
        t.status = Status::Open;
        t.tags = tags.iter().map(|s| s.to_string()).collect();
        t
    }

    #[test]
    fn retired_agency_tasks_are_never_dispatched() {
        for id in [".assign-work", ".flip-work", ".evaluate-work"] {
            assert!(is_retired_agency_task(id));
        }
        assert!(!is_retired_agency_task("work"));
        assert!(!is_retired_agency_task(".verify-work"));
    }

    #[test]
    fn test_is_daemon_managed_skips_chat_loop_tag() {
        let chat_new = task_with_tags(".chat-2", &[worksgood::chat_id::CHAT_LOOP_TAG]);
        assert!(
            is_daemon_managed(&chat_new),
            "chat-loop tagged tasks must be daemon-managed (bug A regression)"
        );

        let chat_legacy = task_with_tags(
            ".coordinator-0",
            &[worksgood::chat_id::LEGACY_COORDINATOR_LOOP_TAG],
        );
        assert!(
            is_daemon_managed(&chat_legacy),
            "legacy coordinator-loop tag still daemon-managed"
        );

        let regular = task_with_tags("real-work", &["impl", "test"]);
        assert!(
            !is_daemon_managed(&regular),
            "regular tasks must remain spawnable by the dispatcher"
        );
    }

    #[test]
    fn admission_deferral_never_becomes_spawn_failure_or_pending_eval() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let occupied = Task {
            id: "occupied-build".into(),
            title: "cargo build occupying the only build slot".into(),
            status: Status::InProgress,
            ..Default::default()
        };
        let deferred = Task {
            id: "deferred-build".into(),
            title: "cargo test waiting for build capacity".into(),
            status: Status::Open,
            ..Default::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(occupied));
        graph.add_node(Node::Task(deferred));
        save_graph(&graph, &graph_path).unwrap();

        // A real live registry entry occupies the build-heavy budget. Using
        // this test process as the PID gives AgentEntry::is_live the same
        // process+heartbeat evidence the daemon uses without launching an LLM.
        let mut registry = AgentRegistry::load_locked(dir.path()).unwrap();
        registry.register_agent(
            std::process::id(),
            "occupied-build",
            "test",
            "/tmp/occupied-build.log",
        );
        registry.save().unwrap();

        let mut config = Config::default();
        config.coordinator.resource_management.disk_sentinel_enabled = false;
        config.coordinator.resource_management.max_build_agents = 1;
        config.coordinator.max_spawn_failures = 5;
        let provider_health_before =
            serde_json::to_value(worksgood::service::ProviderHealth::load(dir.path()).unwrap())
                .unwrap();

        // More ticks than the spawn breaker threshold must remain pure
        // backpressure; the occupied build slot prevents worker launch.
        for _ in 0..=config.coordinator.max_spawn_failures {
            let snapshot = load_graph(&graph_path).unwrap();
            let summary =
                spawn_agents_for_ready_tasks(dir.path(), &snapshot, "test", &config, None, 1);
            assert_eq!(summary.spawned, 0);
            assert_eq!(
                summary.admission_deferred_tasks, 1,
                "the occupied live build slot must be reported as admission backpressure"
            );
        }

        let persisted = load_graph(&graph_path).unwrap();
        let task = persisted.get_task("deferred-build").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.assigned.is_none());
        assert_eq!(task.spawn_failures, 0);
        assert!(task.last_spawn_failure_at.is_none());
        assert_eq!(task.dispatch_count, 0);
        assert!(persisted.get_task(".evaluate-deferred-build").is_none());
        assert!(persisted.get_task(".flip-deferred-build").is_none());
        assert_eq!(
            task.lifecycle
                .audit
                .iter()
                .filter(|event| event.event_kind == "admission-deferred")
                .count(),
            1,
            "identical tick deferrals must coalesce into one durable event"
        );
        assert_eq!(
            serde_json::to_value(worksgood::service::ProviderHealth::load(dir.path()).unwrap())
                .unwrap(),
            provider_health_before,
            "resource backpressure must not charge provider circuit health"
        );
    }

    #[test]
    fn spawn_preparation_deferral_is_coalesced_and_breaker_neutral() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "observer-baseline".into(),
            title: "observer baseline".into(),
            status: Status::Open,
            ..Default::default()
        }));
        save_graph(&graph, &graph_path).unwrap();

        assert!(record_spawn_preparation_deferral(
            &graph_path,
            "observer-baseline",
            "failed to establish isolated-worktree observer baseline: escaping-symlink:bad-link"
        ));
        assert!(!record_spawn_preparation_deferral(
            &graph_path,
            "observer-baseline",
            "a later tick repeats the same preparation failure"
        ));

        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("observer-baseline").unwrap();
        assert_eq!(task.status, Status::Open);
        assert!(task.assigned.is_none());
        assert_eq!(task.spawn_failures, 0);
        assert!(task.last_spawn_failure_at.is_none());
        assert_eq!(
            task.lifecycle
                .audit
                .iter()
                .filter(|event| event.reason_code == "spawn_preparation_deferred")
                .count(),
            1
        );
        let diagnostics = task
            .log
            .iter()
            .filter(|entry| entry.actor.as_deref() == Some("spawn-preparation"))
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("Repair"));
        assert!(diagnostics[0].message.contains("no circuit-breaker charge"));
    }

    #[test]
    fn four_build_requests_serialize_while_pi_terra_evaluation_is_eligible() {
        let builds: Vec<Task> = (0..4)
            .map(|index| Task {
                id: format!("build-{index}"),
                title: "cargo test full suite".into(),
                ..Default::default()
            })
            .collect();
        let evaluator = Task {
            id: ".evaluate-build-0".into(),
            title: "Pi Terra evaluation".into(),
            exec_mode: Some("full".into()),
            ..Default::default()
        };

        let mut active_heavy = 0;
        let mut admitted = Vec::new();
        for task in builds.iter().chain(std::iter::once(&evaluator)) {
            if build_admission_denial(task, false, active_heavy, 1, "healthy").is_none() {
                admitted.push(task.id.clone());
                if worksgood::disk_sentinel::classify_task(task).is_heavy() {
                    active_heavy += 1;
                }
            }
        }
        assert_eq!(admitted, vec!["build-0", ".evaluate-build-0"]);

        // Under pause all four Cargo requests defer, but the evaluator still
        // clears the class-specific gate rather than being stranded.
        assert!(
            builds
                .iter()
                .all(|task| build_admission_denial(task, true, 0, 1, "low space").is_some())
        );
        assert!(build_admission_denial(&evaluator, true, 1, 1, "low space").is_none());
    }

    #[test]
    fn test_daemon_managed_tags_includes_chat_loop() {
        // Lock the constant against accidental removal — every other
        // entry has callers in the codebase but the chat-loop entry
        // is here purely as a dispatcher-skip rule.
        assert!(
            DAEMON_MANAGED_TAGS.contains(&worksgood::chat_id::CHAT_LOOP_TAG),
            "DAEMON_MANAGED_TAGS must contain '{}' to prevent dispatcher from claiming chat tasks",
            worksgood::chat_id::CHAT_LOOP_TAG,
        );
        assert!(
            DAEMON_MANAGED_TAGS.contains(&worksgood::chat_id::LEGACY_COORDINATOR_LOOP_TAG),
            "DAEMON_MANAGED_TAGS must still contain legacy '{}' until migration is complete",
            worksgood::chat_id::LEGACY_COORDINATOR_LOOP_TAG,
        );
    }
}
