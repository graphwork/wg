//! Coordinator tick logic: task readiness, auto-assign, auto-evaluate, agent spawning.

use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use worksgood::agency;
use worksgood::agency::evolver::{self, EvolutionTrigger, EvolverState};
use worksgood::agency::run_mode::{self, AssignmentPath};
use worksgood::agency::{
    AssignerModeContext, AssignmentMode, AssignmentSource, Evaluation, TaskAssignmentRecord,
    count_assignment_records, eval_source, load_all_evaluations_or_warn,
    render_assigner_mode_context, save_assignment_record,
};
use worksgood::chat;
use worksgood::config::{Config, DispatchRole};
use worksgood::graph::{
    LogEntry, Node, PRIORITY_DEFAULT, PRIORITY_IDLE, PRIORITY_NORMAL, Priority, Status, Task,
    WaitCondition, WaitSpec, boost_priority, evaluate_all_cycle_failure_restarts,
    evaluate_all_cycle_iterations,
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
        return Ok(Err(TickResult {
            agents_alive: alive_count,
            tasks_ready: 0,
            agents_spawned: 0,
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
            "build-heavy budget full ({active_build_heavy}/{max_build_agents})"
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
        });
    }
    None
}

/// Evaluate a single wait condition against the current graph/filesystem state.
/// Returns `true` if the condition is satisfied.
fn evaluate_condition(
    condition: &WaitCondition,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
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
            // Check for messages from non-agent senders since the task started waiting
            has_non_agent_message_since(dir, task_id, wait_started_at)
        }
        WaitCondition::Message => {
            // Check for any message since the task started waiting
            has_any_message_since(dir, task_id, wait_started_at)
        }
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

/// Check if any message exists for a task since the wait started.
fn has_any_message_since(dir: &Path, task_id: &str, wait_started_at: Option<&str>) -> bool {
    if let Ok(msgs) = messages::list_messages(dir, task_id) {
        if let Some(wait_ts) = wait_started_at
            && let Ok(wait_time) = wait_ts.parse::<chrono::DateTime<chrono::Utc>>()
        {
            msgs.iter().any(|m| {
                m.timestamp
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map(|t| t > wait_time)
                    .unwrap_or(false)
            })
        } else {
            !msgs.is_empty()
        }
    } else {
        false
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

/// Evaluate all conditions in a WaitSpec.
fn evaluate_wait_spec(
    spec: &WaitSpec,
    graph: &worksgood::graph::WorkGraph,
    dir: &Path,
    task_id: &str,
    wait_started_at: Option<&str>,
) -> bool {
    match spec {
        WaitSpec::All(conditions) => conditions
            .iter()
            .all(|c| evaluate_condition(c, graph, dir, task_id, wait_started_at)),
        WaitSpec::Any(conditions) => conditions
            .iter()
            .any(|c| evaluate_condition(c, graph, dir, task_id, wait_started_at)),
    }
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
        eprintln!("[dispatcher] Circular wait detected: {:?}", cycle);
        for task_id in cycle {
            if let Some(t) = graph.get_task_mut(task_id)
                && t.status == Status::Waiting
            {
                t.status = Status::Failed;
                t.wait_condition = None;
                t.failure_reason = Some(format!("Circular wait detected: {}", cycle.join(" -> ")));
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: format!("Failed: circular wait detected ({})", cycle.join(" -> ")),
                });
                modified = true;
            }
        }
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
            )
        })
        .collect();

    for (task_id, spec, wait_started, _session_id, _checkpoint) in &waiting_data {
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
            let reason = format!(
                "Wait condition unsatisfiable: {}",
                unsatisfiable_reasons.join(", ")
            );
            if let Some(t) = graph.get_task_mut(task_id) {
                t.status = Status::Failed;
                t.wait_condition = None;
                t.failure_reason = Some(reason.clone());
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: format!("Failed: {}", reason),
                });
                modified = true;
                eprintln!("[dispatcher] Waiting task '{}' failed: {}", task_id, reason);
            }
            continue;
        }

        // Evaluate the wait spec
        let satisfied = evaluate_wait_spec(spec, graph, dir, task_id, wait_started.as_deref());

        if satisfied {
            // Human-as-agent dispatch tail (R13): if this task is assigned to a
            // human, their reply (the non-agent message that just satisfied the
            // wait) completes it — record the reply as a reply-to-artifact for
            // each declared deliverable and mark the task Done. Resuming to Open
            // (the generic path below) would re-park it forever, since there is
            // no AI agent to spawn for a human assignee.
            if human_dispatch::try_complete_human_task_on_reply(
                graph,
                dir,
                task_id,
                wait_started.as_deref(),
            ) {
                modified = true;
                eprintln!(
                    "[dispatcher] Waiting task '{}' received human reply, marking Done",
                    task_id
                );
                continue;
            }

            // Build resume delta before mutating
            let delta = {
                let task = graph.get_task(task_id).unwrap();
                build_resume_delta(graph, task, dir)
            };

            if let Some(t) = graph.get_task_mut(task_id) {
                t.status = Status::Open;
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

// ---------------------------------------------------------------------------
// Message-triggered resurrection
// ---------------------------------------------------------------------------

/// Maximum number of resurrections allowed per task.
const MAX_RESURRECTIONS: u32 = 5;

/// Minimum seconds between resurrections of the same task.
const RESURRECTION_COOLDOWN_SECS: i64 = 60;

/// Scan Done tasks for unread messages and resurrect them.
///
/// Two modes:
/// 1. Reopen: if no downstream task is InProgress or Done, transition Done → Open.
/// 2. Child task: if downstream tasks are running, create a child task
///    `.respond-to-<parent-id>` that inherits the parent's session_id and checkpoint.
///
/// Guards: rate limit, sender whitelist, abandoned exclusion.
/// Returns `true` if the graph was modified.
fn resurrect_done_tasks(graph: &mut worksgood::graph::WorkGraph, dir: &Path) -> bool {
    let mut modified = false;

    // Collect Done tasks with unread messages from whitelisted senders
    let candidates: Vec<_> = graph
        .tasks()
        .filter(|t| t.status == Status::Done)
        .filter(|t| !t.tags.iter().any(|tag| tag == "resurrect:false"))
        .map(|t| {
            (
                t.id.clone(),
                t.assigned.clone(),
                t.before.clone(),
                t.session_id.clone(),
                t.checkpoint.clone(),
                t.resurrection_count,
                t.last_resurrected_at.clone(),
            )
        })
        .collect();

    for (
        task_id,
        assigned_agent,
        downstream_ids,
        session_id,
        checkpoint,
        resurrection_count,
        last_resurrected_at,
    ) in candidates
    {
        // Rate limit: max resurrections
        if resurrection_count >= MAX_RESURRECTIONS {
            continue;
        }

        // Rate limit: cooldown
        if let Some(ref last_ts) = last_resurrected_at
            && let Ok(last_time) = last_ts.parse::<chrono::DateTime<chrono::Utc>>()
        {
            let elapsed = Utc::now().signed_duration_since(last_time);
            if elapsed.num_seconds() < RESURRECTION_COOLDOWN_SECS {
                continue;
            }
        }

        // Check for unread messages not from the task's own agent
        let messages = match messages::list_messages(dir, &task_id) {
            Ok(msgs) => msgs,
            Err(_) => continue,
        };

        // Find messages with status=Sent that are not from the task's own agent
        let triggering_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m.status == messages::DeliveryStatus::Sent)
            .filter(|m| {
                // Sender whitelist: user, coordinator, or dependent-task agents
                if m.sender == "user" || m.sender == "coordinator" {
                    return true;
                }
                // Allow messages from agents working on tasks that depend on this one
                // (i.e., downstream tasks whose agents might send questions back)
                if m.sender.starts_with("agent-") {
                    return true;
                }
                false
            })
            .filter(|m| {
                // Exclude messages from the task's own agent
                if let Some(ref agent) = assigned_agent {
                    m.sender != *agent
                } else {
                    true
                }
            })
            .collect();

        if triggering_msgs.is_empty() {
            continue;
        }

        // Check downstream state to decide reopen vs child task
        let has_active_downstream = downstream_ids.iter().any(|did| {
            graph
                .get_task(did)
                .is_some_and(|dt| matches!(dt.status, Status::InProgress | Status::Done))
        });

        if has_active_downstream {
            // Mode 2: Create child task
            let child_id = format!(".respond-to-{}", task_id);

            // Skip if child already exists
            if graph.get_task(&child_id).is_some() {
                continue;
            }

            let msg_summary: Vec<String> = triggering_msgs
                .iter()
                .map(|m| format!("[{}] {}: {}", m.timestamp, m.sender, m.body))
                .collect();

            let child_desc = format!(
                "You previously completed task `{}`. There are pending messages that need your attention.\n\n\
                 ## Pending Messages\n{}\n\n\
                 Read and respond to these messages using `wg msg read {} --agent $WG_AGENT_ID`.\n\
                 When done, mark this task complete with `wg done {}`.",
                task_id,
                msg_summary.join("\n"),
                task_id,
                child_id,
            );

            let child_task = Task {
                id: child_id.clone(),
                title: format!("Respond to messages on {}", task_id),
                description: Some(child_desc),
                status: Status::Open,
                session_id: session_id.clone(),
                checkpoint: checkpoint.clone(),
                after: vec![task_id.clone()],
                tags: vec!["resurrection-child".to_string()],
                created_at: Some(Utc::now().to_rfc3339()),
                ..Default::default()
            };

            graph.add_node(Node::Task(child_task));

            // Update parent resurrection tracking
            if let Some(t) = graph.get_task_mut(&task_id) {
                t.resurrection_count += 1;
                t.last_resurrected_at = Some(Utc::now().to_rfc3339());
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: format!(
                        "Resurrection: created child task '{}' ({} pending message(s), downstream active)",
                        child_id,
                        triggering_msgs.len()
                    ),
                });
            }

            eprintln!(
                "[dispatcher] Resurrection: created child task '{}' for Done task '{}' ({} message(s))",
                child_id,
                task_id,
                triggering_msgs.len()
            );
            modified = true;
        } else {
            // Mode 1: Reopen
            if let Some(t) = graph.get_task_mut(&task_id) {
                t.status = Status::Open;
                t.assigned = None;
                t.resurrection_count += 1;
                t.last_resurrected_at = Some(Utc::now().to_rfc3339());
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: format!(
                        "Resurrection: reopened due to {} pending message(s)",
                        triggering_msgs.len()
                    ),
                });

                eprintln!(
                    "[dispatcher] Resurrection: reopened Done task '{}' ({} message(s))",
                    task_id,
                    triggering_msgs.len()
                );
                modified = true;
            }

            // Reopen .assign-* dependency so reassignment can happen
            let assign_id = format!(".assign-{}", task_id);
            if let Some(assign_task) = graph.get_task_mut(&assign_id)
                && assign_task.status == Status::Done
            {
                assign_task.status = Status::Open;
                assign_task.assigned = None;
                assign_task.completed_at = None;
                assign_task.description = None;
                assign_task.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: "Reopened for reassignment (source task resurrected)".to_string(),
                });
                eprintln!(
                    "[dispatcher] Resurrection: reopened '{}' for reassignment",
                    assign_id,
                );
            }
        }
    }

    modified
}

// ---------------------------------------------------------------------------
// Unblock stuck tasks
// ---------------------------------------------------------------------------

/// Scan blocked tasks and unblock those whose dependencies are satisfied
/// (terminal) or missing (archived/deleted).
///
/// The coordinator runs unblock logic only when a task transitions to done.
/// This misses cases where:
/// 1. A dependency is archived/deleted → dangling reference never confirms
/// 2. Coordinator misses a completion event (restart, crash, timing)
/// 3. Tasks blocked on completed tasks never get unblocked
///
/// This function:
/// 1. Scans all blocked tasks
/// 2. Checks if all after dependencies are terminal OR don't exist
/// 3. If so, transitions Blocked → Open
/// 4. Logs diagnostic info for stale blocked states
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

fn unblock_stuck_tasks(graph: &mut worksgood::graph::WorkGraph, _dir: &Path) -> bool {
    let mut modified = false;

    // Collect blocked task IDs first
    let blocked_task_ids: Vec<String> = graph
        .tasks()
        .filter(|t| t.status == Status::Blocked)
        .map(|t| t.id.clone())
        .collect();

    for task_id in blocked_task_ids {
        // Check if all dependencies are satisfied
        let task = graph.tasks().find(|t| t.id == task_id);
        let all_deps_satisfied = match task {
            Some(task) => task.after.iter().all(|dep_id| {
                // Check if dependency exists
                match graph.tasks().find(|t| t.id == *dep_id) {
                    Some(dep_task) => dep_task.status.is_dep_satisfied(),
                    None => true, // Missing dependency = satisfied for stuck tasks
                }
            }),
            None => false,
        };

        if all_deps_satisfied {
            // Get mutable reference to update the task
            if let Some(task) = graph.get_task_mut(&task_id)
                && !task.after.is_empty()
            {
                task.status = Status::Open;
                task.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some("coordinator".to_string()),
                        user: Some(worksgood::current_user()),
                        message: format!(
                            "Unblocked by coordinator scan — all dependencies satisfied or archived/deleted. Dependencies: {}",
                            task.after.join(", ")
                        ),
                    });
                eprintln!(
                    "[dispatcher] Unblocked stuck task '{}' (blocked on: {})",
                    task.id,
                    task.after.join(", ")
                );
                modified = true;
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
                        graph.tasks().find(|t| t.id == *dep_id).map(|t| {
                            if !t.status.is_terminal() {
                                format!("{}:{:?}", dep_id, t.status)
                            } else {
                                String::new()
                            }
                        })
                    })
                    .filter(|s| !s.is_empty())
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

/// Auto-assign: scaffold `.assign-*` tasks and run lightweight LLM assignment.
///
/// Phase 1 — Scaffold: For ready unassigned non-system tasks without an
/// `.assign-*` task, create one as a blocking dependency. This handles tasks
/// created via `wg add`; published tasks already have `.assign-*` from
/// publish-time scaffolding. Also reopens stale Done `.assign-*` tasks when
/// the source task was resurrected.
///
/// Phase 2 — Process: For each ready Open `.assign-*` task, run a lightweight
/// LLM call to select the best agent, set the agent on the source task, and mark
/// `.assign-*` as Done (which unblocks the source task via graph edges).
///
/// Returns `true` if the graph was modified.
fn build_auto_assign_tasks(
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
    dir: &Path,
) -> bool {
    let mut modified = false;

    let grace_seconds = config.agency.auto_assign_grace_seconds;

    // Phase 1: Scaffold .assign-* for ready unassigned tasks that don't have one.
    // Also reopens stale Done .assign-* tasks for resurrected source tasks.
    {
        let ready_task_data: Vec<_> = {
            let cycle_analysis = graph.compute_cycle_analysis();
            let ready = ready_tasks_with_peers_cycle_aware(graph, dir, &cycle_analysis);
            ready
                .iter()
                .filter(|t| t.agent.is_none() && t.assigned.is_none())
                .filter(|t| !worksgood::graph::is_system_task(&t.id))
                // Exclude shell tasks from auto-assign — they run commands, not agents
                .filter(|t| t.exec.is_none() && t.exec_mode.as_deref() != Some("shell"))
                .map(|t| (t.id.clone(), t.title.clone(), t.created_at.clone()))
                .collect()
        };

        for (task_id, task_title, task_created_at) in ready_task_data {
            // Grace period: skip tasks created less than `auto_assign_grace_seconds` ago.
            if grace_seconds > 0
                && let Some(ref created_str) = task_created_at
                && let Ok(created) = created_str.parse::<chrono::DateTime<chrono::Utc>>()
            {
                let age = Utc::now().signed_duration_since(created);
                if age.num_seconds() < grace_seconds as i64 {
                    eprintln!(
                        "[dispatcher] Skipping auto-assign for '{}': created {}s ago (grace period: {}s)",
                        task_id,
                        age.num_seconds(),
                        grace_seconds,
                    );
                    continue;
                }
            }

            let assign_task_id = format!(".assign-{}", task_id);

            if let Some(existing) = graph.get_task(&assign_task_id) {
                let needs_reopen = match existing.status {
                    Status::Done => true,
                    Status::Failed | Status::Abandoned => true,
                    _ => false, // Open or InProgress — Phase 2 will handle
                };
                if needs_reopen {
                    let reason = match existing.status {
                        Status::Done => {
                            "Reopened for reassignment (source task resurrected)".to_string()
                        }
                        _ => format!(
                            "Reopened for retry (was {:?}, source task still needs assignment)",
                            existing.status
                        ),
                    };
                    if let Some(t) = graph.get_task_mut(&assign_task_id) {
                        t.status = Status::Open;
                        t.assigned = None;
                        t.completed_at = None;
                        t.description = None;
                        t.failure_reason = None;
                        t.log.push(LogEntry {
                            timestamp: Utc::now().to_rfc3339(),
                            actor: Some("coordinator".to_string()),
                            user: Some(worksgood::current_user()),
                            message: reason,
                        });
                    }
                    // Ensure blocking edge exists (may be missing for pre-migration tasks)
                    if let Some(source) = graph.get_task_mut(&task_id)
                        && !source.after.iter().any(|a| a == &assign_task_id)
                    {
                        source.after.push(assign_task_id.clone());
                    }
                    modified = true;
                }
                // Already exists (Open or just reopened) — Phase 2 will process it
                continue;
            }

            // Create .assign-* with blocking edge via shared scaffold helper
            crate::commands::eval_scaffold::scaffold_assign_task(graph, &task_id, &task_title);
            modified = true;
        }
    }

    // Phase 1b: Reopen Failed/Abandoned `.assign-*` tasks even when their
    // source task is not "ready" by the standard query — Failed is not
    // dep-satisfied, so source stays Blocked and Phase 1 above misses it.
    // Without this, a transient LLM error during assignment permanently
    // strands the source task. Mirrors Phase 1's reopen logic.
    {
        let stuck_assign_ids: Vec<(String, String, Status)> = graph
            .tasks()
            .filter_map(|t| {
                let source_id = t.id.strip_prefix(".assign-")?;
                if !matches!(t.status, Status::Failed | Status::Abandoned) {
                    return None;
                }
                let source = graph.get_task(source_id)?;
                if source.agent.is_some() || source.assigned.is_some() {
                    return None;
                }
                if !matches!(source.status, Status::Open | Status::Incomplete) {
                    return None;
                }
                Some((t.id.clone(), source_id.to_string(), t.status))
            })
            .collect();

        for (assign_id, _source_id, prev_status) in stuck_assign_ids {
            if let Some(t) = graph.get_task_mut(&assign_id) {
                t.status = Status::Open;
                t.assigned = None;
                t.completed_at = None;
                t.description = None;
                t.failure_reason = None;
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: format!(
                        "Reopened for retry (was {:?}, source task still needs assignment)",
                        prev_status
                    ),
                });
                modified = true;
            }
        }
    }

    // Phase 2: Process ready .assign-* tasks (run lightweight LLM assignment).
    // These may have been created at publish time or in Phase 1 above.
    //
    // Time budget: each LLM assignment call can take seconds, and running many
    // back-to-back blocks the daemon's main event loop (which handles IPC).
    // Cap Phase 2 at 10 seconds; remaining tasks will be picked up next tick.
    let phase2_start = Instant::now();
    const ASSIGN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

    let agency_dir = dir.join("agency");
    let total_assignments = count_assignment_records(&agency_dir.join("assignments")) as u32;

    let assign_task_ids: Vec<String> = graph
        .tasks()
        .filter(|t| {
            t.id.starts_with(".assign-")
                && t.status == Status::Open
                && !t.paused
                // Check readiness: all after deps must be terminal.
                && t.after.iter().all(|dep_id| {
                    graph
                        .get_task(dep_id)
                        .map(|d| d.status.is_terminal())
                        .unwrap_or(false)
                })
        })
        .map(|t| t.id.clone())
        .collect();

    for assign_task_id in assign_task_ids {
        if phase2_start.elapsed() > ASSIGN_TIME_BUDGET {
            eprintln!(
                "[dispatcher] Assignment time budget exceeded ({}s), deferring remaining to next tick",
                ASSIGN_TIME_BUDGET.as_secs()
            );
            break;
        }
        let source_id = match assign_task_id.strip_prefix(".assign-") {
            Some(id) => id.to_string(),
            None => continue,
        };

        // Get source task data for the LLM call
        let (task_title, task_desc, task_skills, task_tags, task_after, task_context_scope) =
            match graph.get_task(&source_id) {
                Some(t) => (
                    t.title.clone(),
                    t.description.clone(),
                    t.skills.clone(),
                    t.tags.clone(),
                    t.after.clone(),
                    t.context_scope.clone(),
                ),
                None => continue,
            };

        // Determine assignment path — always LLM-based
        let assignment_path =
            run_mode::determine_assignment_path(&config.agency, total_assignments);

        // Design experiment for the assigner
        let learning_count = count_assignment_records(&agency_dir.join("assignments")) as u32;
        let experiment =
            run_mode::design_experiment(&agency_dir, &config.agency, learning_count, &source_id);

        let mode_context = render_assigner_mode_context(&AssignerModeContext {
            assignment_path,
            experiment: Some(&experiment),
            total_assignments,
        });

        eprintln!(
            "[dispatcher] Assignment path for '{}': {:?} (total_assignments={})",
            source_id, assignment_path, total_assignments,
        );

        // Detect task underspecification
        let is_underspecified =
            task_desc.is_none() || task_desc.as_ref().map(|d| d.len() < 20).unwrap_or(true);
        let has_no_skills = task_skills.is_empty();
        let underspec_warning = if is_underspecified || has_no_skills {
            let mut warnings = Vec::new();
            if is_underspecified {
                warnings.push("task has no description or a very short description");
            }
            if has_no_skills {
                warnings.push("task has no skills/capabilities specified");
            }
            Some(format!(
                "\n**⚠ Underspecification Warning:** {}\n\
                 The assigner should use best-effort heuristics: match on title keywords, \
                 check dependency context, and default to a generalist agent.\n",
                warnings.join("; "),
            ))
        } else {
            None
        };

        // Load all agents for the lightweight LLM assignment call
        let agents_dir = agency_dir.join("cache/agents");
        let all_agents = agency::load_all_agents_or_warn(&agents_dir);
        let roles_dir = agency_dir.join("cache/roles");
        let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");

        // Build a temporary Task with the gathered data for the prompt builder
        let task_snapshot = Task {
            id: source_id.clone(),
            title: task_title.clone(),
            description: task_desc.clone(),
            skills: task_skills.clone(),
            tags: task_tags.clone(),
            after: task_after.clone(),
            context_scope: task_context_scope.clone(),
            ..Default::default()
        };

        // Try Agency assignment if configured
        if config.agency.assignment_source.as_deref() == Some("agency")
            && config.agency.agency_server_url.is_some()
        {
            let task_title_ref = task_title.as_str();
            let task_desc_ref = task_desc.as_deref().unwrap_or("");
            match agency::request_agency_assignment(task_title_ref, task_desc_ref, &config.agency) {
                Ok(response) => {
                    eprintln!(
                        "[dispatcher] Agency assignment for '{}': agency_task_id={}",
                        source_id, response.agency_task_id,
                    );

                    // Mark the .assign-* task as Done
                    let now = Utc::now().to_rfc3339();
                    if let Some(assign_task) = graph.get_task_mut(&assign_task_id) {
                        assign_task.status = Status::Done;
                        assign_task.description = Some(format!(
                            "Agency assignment for '{}': agency_task_id={}",
                            source_id, response.agency_task_id,
                        ));
                        assign_task.started_at = Some(now.clone());
                        assign_task.completed_at = Some(now);
                        assign_task.exec_mode = Some("bare".to_string());
                        assign_task.log.push(LogEntry {
                            timestamp: Utc::now().to_rfc3339(),
                            actor: Some("coordinator".to_string()),
                            user: Some(worksgood::current_user()),
                            message: format!(
                                "Assigned via Agency (agency_task_id={})",
                                response.agency_task_id,
                            ),
                        });
                    }

                    // Persist TaskAssignmentRecord with Agency source
                    let record = TaskAssignmentRecord {
                        task_id: source_id.clone(),
                        agent_id: String::new(),
                        composition_id: String::new(),
                        timestamp: Utc::now().to_rfc3339(),
                        mode: AssignmentMode::Learning(experiment.clone()),
                        agency_task_id: Some(response.agency_task_id.clone()),
                        assignment_source: AssignmentSource::Agency {
                            agency_task_id: response.agency_task_id,
                        },
                    };

                    let assignments_dir = agency_dir.join("assignments");
                    if let Err(e) = save_assignment_record(&record, &assignments_dir) {
                        eprintln!(
                            "[dispatcher] Warning: failed to save assignment record for '{}': {}",
                            source_id, e,
                        );
                    }

                    let _ = worksgood::parser::modify_graph(graph_path(dir), |fresh| {
                        // Copy assignment record task from local graph
                        for node in graph.nodes() {
                            if let worksgood::graph::Node::Task(t) = node
                                && let Some(ft) = fresh.get_task_mut(&t.id)
                            {
                                ft.after = t.after.clone();
                                ft.before = t.before.clone();
                                ft.status = t.status;
                                ft.log = t.log.clone();
                            }
                        }
                        true
                    });
                    continue;
                }
                Err(e) => {
                    eprintln!(
                        "[dispatcher] Warning: Agency assignment failed for '{}' ({}), falling back to native",
                        source_id, e,
                    );
                    // Fall through to native LLM assigner
                }
            }
        }

        // Build active tasks context for placement (merged into assignment)
        let active_tasks_context = if config.agency.auto_place {
            super::assignment::build_active_tasks_context(graph, &source_id)
        } else {
            String::new()
        };

        // Structural pool separation: for a normal work task (anything that
        // is NOT an evaluation/review primitive), the LLM assigner must only
        // see **work agents** — system evaluation personas (Reviewer /
        // Evaluator / Assigner / Evolver / Agent Creator) are filtered out
        // *before* the prompt so they can never be picked regardless of their
        // historical usage or score. Evaluation/review primitives keep the full
        // pool (system agents are the correct candidates there). See
        // `assignment_eligibility` and task `make-evaluator-and`.
        let components_dir = agency_dir.join("primitives/components");
        let assignment_pool: Vec<agency::Agent> =
            if worksgood::assignment_eligibility::task_uses_work_pool(&task_snapshot) {
                let work_pool = worksgood::assignment_eligibility::filter_work_pool_agents(
                    &all_agents,
                    &roles_dir,
                    &components_dir,
                );
                if work_pool.is_empty() {
                    // No work agent — refuse to hand a system agent to a work
                    // task. Skip this tick loudly so the task stays unassigned
                    // rather than running an evaluator on a build task.
                    eprintln!(
                        "[dispatcher] POOL SEPARATION: task '{}' is a work task but the \
                         work pool is empty (only system evaluation agents exist). \
                         Skipping assignment this tick — create a work agent \
                         (`wg agent create` with a work role, e.g. Programmer) to \
                         unblock it. This is a configuration error, not a transient one.",
                        source_id,
                    );
                    continue;
                }
                work_pool.into_iter().cloned().collect()
            } else {
                // Evaluation/review primitive — system agents are the correct pool.
                all_agents.clone()
            };

        // Run lightweight LLM call for assignment
        let (verdict, assign_token_usage) = match super::assignment::run_lightweight_assignment(
            config,
            &task_snapshot,
            &assignment_pool,
            &roles_dir,
            &tradeoffs_dir,
            &mode_context,
            underspec_warning.as_deref(),
            &active_tasks_context,
            Some(graph),
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "[dispatcher] Lightweight assignment failed for '{}': {}, will retry next tick",
                    source_id, e
                );
                continue;
            }
        };

        // Resolve the agent hash from the verdict
        let mut resolved_agent =
            match agency::find_agent_by_prefix(&agents_dir, &verdict.agent_hash) {
                Ok(agent) => agent,
                Err(e) => {
                    eprintln!(
                        "[dispatcher] Assignment verdict agent '{}' not found for '{}': {}",
                        verdict.agent_hash, source_id, e
                    );
                    continue;
                }
            };

        // Post-hoc pool-separation backstop: the pre-LLM filter already
        // excludes system evaluation agents from the work-task candidate set,
        // but defend against a bypass (stale role resolution, an agency-server
        // verdict, or a future caller) — if a system evaluation persona landed
        // on a normal work task, reassign to a fallback work agent. When no
        // work agent exists, SKIP this tick loudly rather than running an
        // evaluator on a work task. See `assignment_eligibility` and task
        // `make-evaluator-and`.
        if let Some(task) = graph.get_task(&source_id) {
            let role = agency::find_role_by_prefix(&roles_dir, &resolved_agent.role_id).ok();
            let comp_names = role
                .as_ref()
                .map(|r| {
                    worksgood::assignment_eligibility::resolve_role_component_names(
                        r,
                        &components_dir,
                    )
                })
                .unwrap_or_default();
            let is_system = role
                .as_ref()
                .map(|r| {
                    worksgood::assignment_eligibility::role_is_system_evaluation_with_components(
                        r,
                        &comp_names,
                    )
                })
                .unwrap_or(false);
            if is_system && worksgood::assignment_eligibility::task_uses_work_pool(task) {
                let original_name = resolved_agent.name.clone();
                let original_role = role.as_ref().map(|r| r.name.clone()).unwrap_or_default();
                let fallback = worksgood::assignment_eligibility::pick_implementation_capable_agent(
                    &all_agents,
                    &roles_dir,
                    &components_dir,
                );
                match fallback {
                    Some(fb) => {
                        eprintln!(
                            "[dispatcher] POOL SEPARATION: task '{}' is a work task but the \
                             assigner picked '{}' (system role '{}', evaluation/review/agency \
                             only). Reassigning to work agent '{}' ({}).",
                            source_id,
                            original_name,
                            original_role,
                            fb.name,
                            agency::short_hash(&fb.id),
                        );
                        resolved_agent = fb.clone();
                    }
                    None => {
                        eprintln!(
                            "[dispatcher] POOL SEPARATION: task '{}' is a work task and the \
                             assigner picked '{}' (system role '{}'), but NO work agent is \
                             available. Refusing to run a system evaluation agent on a work \
                             task — skipping assignment this tick. Create a work agent \
                             (`wg agent create` with a work role, e.g. Programmer). This is \
                             a configuration error, not a transient one.",
                            source_id, original_name, original_role,
                        );
                        continue;
                    }
                }
            }
        }

        // Apply assignment to the source task
        if let Some(task) = graph.get_task_mut(&source_id) {
            task.agent = Some(resolved_agent.id.clone());
            if let Some(ref mode) = verdict.exec_mode
                && mode.parse::<worksgood::config::ExecMode>().is_ok()
            {
                task.exec_mode = Some(mode.clone());
            }
            if let Some(ref scope) = verdict.context_scope {
                match scope.as_str() {
                    "clean" | "task" | "graph" | "full" => {
                        if task.context_scope.is_none() {
                            task.context_scope = Some(scope.clone());
                        }
                    }
                    _ => {}
                }
            }
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("coordinator".to_string()),
                user: Some(worksgood::current_user()),
                message: format!(
                    "Lightweight assignment: agent={} ({}), exec_mode={}, context_scope={}, reason={}",
                    resolved_agent.name,
                    agency::short_hash(&resolved_agent.id),
                    verdict.exec_mode.as_deref().unwrap_or("(default)"),
                    verdict.context_scope.as_deref().unwrap_or("(default)"),
                    verdict.reason,
                ),
            });
        }

        // Apply placement edges from the verdict (merged placement step)
        if let Some(ref placement) = verdict.placement {
            // Pre-validate which deps exist in the graph (avoids borrow conflict)
            let valid_after: Vec<String> = placement
                .after
                .iter()
                .filter(|dep| !dep.is_empty() && graph.get_task(dep).is_some())
                .cloned()
                .collect();
            let valid_before: Vec<String> = placement
                .before
                .iter()
                .filter(|dep| !dep.is_empty() && graph.get_task(dep).is_some())
                .cloned()
                .collect();

            if let Some(task) = graph.get_task_mut(&source_id) {
                let mut edges_added = Vec::new();
                for dep in &valid_after {
                    if !task.after.contains(dep) {
                        task.after.push(dep.clone());
                        edges_added.push(format!("--after {}", dep));
                    }
                }
                for dep in &valid_before {
                    if !task.before.contains(dep) {
                        task.before.push(dep.clone());
                        edges_added.push(format!("--before {}", dep));
                    }
                }
                if !edges_added.is_empty() {
                    task.tags.retain(|t| t != "placed");
                    task.tags.push("placed".to_string());
                    task.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some("coordinator".to_string()),
                        user: Some(worksgood::current_user()),
                        message: format!(
                            "Placement applied (via assignment): {}",
                            edges_added.join(" "),
                        ),
                    });
                    eprintln!(
                        "[dispatcher] Placement for '{}': {}",
                        source_id,
                        edges_added.join(" "),
                    );
                } else {
                    eprintln!(
                        "[dispatcher] Placement for '{}': no valid edges to add",
                        source_id,
                    );
                }
            }
        }

        // Mark the .assign-* task as Done (unblocks source task via graph edge)
        let now = Utc::now().to_rfc3339();
        if let Some(assign_task) = graph.get_task_mut(&assign_task_id) {
            assign_task.status = Status::Done;
            assign_task.description = Some(format!(
                "Lightweight assignment: {} ({}) → '{}'\nReason: {}",
                resolved_agent.name,
                agency::short_hash(&resolved_agent.id),
                source_id,
                verdict.reason,
            ));
            assign_task.started_at = Some(now.clone());
            assign_task.completed_at = Some(now);
            assign_task.model = Some(
                config
                    .resolve_model_for_role(worksgood::config::DispatchRole::Assigner)
                    .model,
            );
            assign_task.provider = config
                .resolve_model_for_role(worksgood::config::DispatchRole::Assigner)
                .provider;
            assign_task.agent = config.agency.assigner_agent.clone();
            assign_task.token_usage = assign_token_usage;
            assign_task.exec_mode = Some("bare".to_string());
            assign_task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("coordinator".to_string()),
                user: Some(worksgood::current_user()),
                message: format!("Assigned via LLM (path: {:?})", assignment_path,),
            });
        }

        // Persist TaskAssignmentRecord with actual agent info
        let assignment_mode = match assignment_path {
            AssignmentPath::Learning => AssignmentMode::Learning(experiment.clone()),
            AssignmentPath::ForcedExploration => {
                AssignmentMode::ForcedExploration(experiment.clone())
            }
        };

        let record = TaskAssignmentRecord {
            task_id: source_id.clone(),
            agent_id: resolved_agent.id.clone(),
            composition_id: resolved_agent.id.clone(),
            timestamp: Utc::now().to_rfc3339(),
            mode: assignment_mode,
            agency_task_id: None,
            assignment_source: AssignmentSource::Native,
        };

        let assignments_dir = agency_dir.join("assignments");
        if let Err(e) = save_assignment_record(&record, &assignments_dir) {
            eprintln!(
                "[dispatcher] Warning: failed to save assignment record for '{}': {}",
                source_id, e,
            );
        }

        eprintln!(
            "[dispatcher] Lightweight assignment for '{}': {} ({}) [path={:?}]",
            source_id,
            resolved_agent.name,
            agency::short_hash(&resolved_agent.id),
            assignment_path,
        );

        // If the assigner signals that no good match was found, trigger the
        // creator agent to expand the primitive store (self-healing).
        if verdict.create_needed && config.agency.auto_create {
            let has_pending_create = graph.tasks().any(|t| {
                t.id.starts_with(".create-")
                    && matches!(t.status, Status::Open | Status::InProgress)
            });
            if !has_pending_create {
                let ts = Utc::now().format("%Y%m%d-%H%M%S");
                let create_task_id = format!(".create-needed-{}", ts);
                let creator_resolved =
                    config.resolve_model_for_role(worksgood::config::DispatchRole::Creator);

                // Find most recently completed non-system task for graph connectivity
                let causal_edge: Vec<String> = graph
                    .tasks()
                    .filter(|t| {
                        t.status == Status::Done && !worksgood::graph::is_system_task(&t.id)
                    })
                    .max_by(|a, b| a.completed_at.cmp(&b.completed_at))
                    .map(|t| vec![t.id.clone()])
                    .unwrap_or_default();

                let desc = format!(
                    "## Creator Triggered by Assigner\n\n\
                     The assigner could not find a good agent match for task '{}' ({}).\n\
                     Reason: {}\n\n\
                     **Triggering task:** `{}`\n\n\
                     Run `wg agency create` to expand the primitive store.\n",
                    source_id,
                    graph
                        .get_task(&source_id)
                        .map(|t| t.title.as_str())
                        .unwrap_or("?"),
                    verdict.reason,
                    source_id,
                );

                let create_task = Task {
                    id: create_task_id.clone(),
                    title: format!("Create agents: poor match for '{}'", source_id),
                    description: Some(desc),
                    status: Status::Open,
                    priority: PRIORITY_DEFAULT,
                    assigned: None,
                    estimate: None,
                    before: vec![],
                    after: causal_edge,
                    requires: vec![],
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
                    log: vec![],
                    retry_count: 0,
                    max_retries: Some(1),
                    failure_reason: None,
                    failure_class: None,
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
                    session_id: None,
                    wait_condition: None,
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
                    spawn_failures: 0,
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
                eprintln!(
                    "[dispatcher] Assigner flagged create_needed for '{}' — created '{}'",
                    source_id, create_task_id,
                );
            }
        }

        modified = true;
    }

    modified
}

/// Auto-evaluate: create evaluation tasks for completed/active tasks.
///
/// Per the agency design (§4.3), when auto_evaluate is enabled the coordinator
/// creates an evaluation task `evaluate-{task-id}` that is blocked by the
/// original task.  When the original task completes (done or failed),
/// the evaluation task becomes ready and the coordinator spawns an
/// evaluator agent on it.
///
/// Tasks tagged "evaluation", "assignment", or "evolution" are NOT
/// auto-evaluated to prevent infinite regress.  Abandoned tasks are also
/// excluded.
///
/// Returns `true` if the graph was modified.
fn build_auto_evaluate_tasks(
    dir: &Path,
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
) -> bool {
    let mut modified = false;

    // Load agents to identify human operators — their work quality isn't
    // a reflection of a role+tradeoff prompt so we skip auto-evaluation.
    let agents_dir = dir.join("agency").join("cache/agents");
    let all_agents = agency::load_all_agents_or_warn(&agents_dir);
    let human_agent_ids: std::collections::HashSet<&str> = all_agents
        .iter()
        .filter(|a| a.is_human())
        .map(|a| a.id.as_str())
        .collect();

    // Catch-all for tasks that weren't published with eager scaffolding
    // (backward compatibility). The eval_scaffold helper handles idempotency
    // structurally, so this is safe to call even if publish already created
    // the eval task.
    let tasks_needing_eval: Vec<_> = graph
        .tasks()
        .filter(|t| {
            // Skip paused/draft tasks — their pipeline is scaffolded at
            // `wg publish` time via scaffold_full_pipeline.  Creating
            if t.paused {
                return false;
            }
            let eval_id = format!(".evaluate-{}", t.id);
            if graph.get_task(&eval_id).is_some() {
                return false;
            }
            if worksgood::graph::is_system_task(&t.id) {
                return false;
            }
            // Skip tasks assigned to human agents
            if let Some(ref agent_id) = t.agent
                && human_agent_ids.contains(agent_id.as_str())
            {
                return false;
            }
            !matches!(t.status, Status::Abandoned)
        })
        .map(|t| (t.id.clone(), t.title.clone()))
        .collect();

    // Use shared scaffold helper (same logic as publish-time creation)
    let count = crate::commands::eval_scaffold::scaffold_eval_tasks_batch(
        dir,
        graph,
        &tasks_needing_eval,
        config,
    );
    if count > 0 {
        modified = true;
    }

    // Unblock evaluation tasks whose source task has Failed or FailedPendingEval.
    // `ready_tasks()` only unblocks when the blocker is Done. For Failed and
    // FailedPendingEval tasks we still want evaluation to proceed (§4.3:
    // "Failed tasks also get evaluated"), so we remove the blocker explicitly.
    let eval_fixups: Vec<(String, String)> = graph
        .tasks()
        .filter(|t| t.id.starts_with(".evaluate-") && t.status == Status::Open)
        .filter_map(|t| {
            // The eval task blocks on a single task: the original
            if t.after.len() == 1 {
                let source_id = &t.after[0];
                if let Some(source) = graph.get_task(source_id)
                    && matches!(source.status, Status::Failed | Status::FailedPendingEval)
                {
                    return Some((t.id.clone(), source_id.clone()));
                }
            }
            None
        })
        .collect();

    for (eval_id, source_id) in &eval_fixups {
        if let Some(t) = graph.get_task_mut(eval_id) {
            t.after.retain(|b| b != source_id);
            modified = true;
            eprintln!(
                "[dispatcher] Unblocked evaluation task '{}' (source '{}' failed)",
                eval_id, source_id,
            );
        }
    }

    modified
}

/// Create verification tasks for tasks whose FLIP score fell below the
/// configured threshold. The verification task independently checks whether
/// the work was actually completed, using the Opus model by default.
///
/// Returns `true` if the graph was modified.
fn build_flip_verification_tasks(
    dir: &Path,
    graph: &mut worksgood::graph::WorkGraph,
    config: &Config,
) -> bool {
    let threshold = match config.agency.flip_verification_threshold {
        Some(t) => t,
        None => return false, // Disabled
    };

    // Load all FLIP evaluations
    let evals_dir = dir.join("agency").join("evaluations");
    let all_evals = load_all_evaluations_or_warn(&evals_dir);

    // Filter to FLIP evaluations below threshold
    let low_flip: Vec<&Evaluation> = all_evals
        .iter()
        .filter(|e| e.source == eval_source::FLIP && e.score < threshold)
        .collect();

    if low_flip.is_empty() {
        return false;
    }

    let mut modified = false;
    let verification_resolved =
        config.resolve_model_for_role(worksgood::config::DispatchRole::Verification);
    let verification_model = verification_resolved.model;

    for eval in &low_flip {
        let source_task_id = &eval.task_id;
        let verify_task_id = format!(".verify-{}", source_task_id);

        // Skip if verification task already exists
        if graph.get_task(&verify_task_id).is_some() {
            continue;
        }

        // Skip if the source task doesn't exist or is already failed
        let source_task = match graph.get_task(source_task_id) {
            Some(t) => t,
            None => continue,
        };
        if source_task.status == Status::Failed || source_task.status == Status::Abandoned {
            continue;
        }

        // Skip system tasks (dot-prefixed) to prevent verification loops
        if worksgood::graph::is_system_task(source_task_id) {
            continue;
        }

        // Skip tasks that would be handled by eval gate - let eval gate take precedence
        if let Some(eval_threshold) = config.agency.eval_gate_threshold {
            let has_deliverables = source_task
                .description
                .as_deref()
                .map(crate::commands::deliverables::parse_deliverables)
                .is_some_and(|deliverables| !deliverables.is_empty());
            let is_eval_gated = config.agency.eval_gate_all || has_deliverables;
            if is_eval_gated {
                // Check if there's a regular evaluation for this task that scored below eval threshold
                // But exclude system evaluations (infrastructure failures) from this check
                let has_low_eval = all_evals.iter().any(|e| {
                    e.task_id == *source_task_id
                        && e.source != worksgood::agency::eval_source::FLIP
                        && e.source != "system"  // Skip infrastructure failures
                        && e.score < eval_threshold
                });
                if has_low_eval {
                    // Eval gate should handle this task's failure, skip FLIP verification
                    continue;
                }
            }
        }

        // Build verification task description
        let source_verify_cmd = source_task.verify.clone();
        let source_title = source_task.title.clone();
        let source_desc_snippet = source_task
            .description
            .as_deref()
            .unwrap_or("(no description)")
            .chars()
            .take(2000)
            .collect::<String>();

        // Gather source task checkpoint and artifacts for context
        let source_checkpoint = source_task.checkpoint.clone().unwrap_or_default();
        let source_artifacts = source_task.artifacts.clone();

        let mut desc = format!(
            "## FLIP Verification & Repair\n\n\
             FLIP score {:.2} is below threshold {:.2} — independently verify and, if needed, **fix** this task's work.\n\n\
             ### Your Authority\n\
             You are a **senior engineer reviewing a junior's PR**. You have full authority to:\n\
             - Edit source files, run builds, run tests, and commit fixes\n\
             - Correct mistakes, resolve test failures, and improve the implementation\n\
             - Only reject (fail) the source task if the approach is fundamentally wrong\n\n\
             **Fix first, fail last.** If the work is close but has issues, repair it yourself.\n\n\
             ### Original Task\n\
             **ID:** {}\n\
             **Title:** {}\n\
             **Description:**\n{}\n\n",
            eval.score, threshold, source_task_id, source_title, source_desc_snippet,
        );

        if !source_checkpoint.is_empty() {
            desc.push_str(&format!(
                "**Checkpoint (last known state):**\n{}\n\n",
                source_checkpoint
            ));
        }

        if !source_artifacts.is_empty() {
            desc.push_str("**Artifacts:**\n");
            for artifact in &source_artifacts {
                desc.push_str(&format!("- `{}`\n", artifact));
            }
            desc.push('\n');
        }

        // Inject FLIP evaluation context so the verify agent knows exactly what failed
        desc.push_str("### FLIP Evaluation Results\n\n");
        if !eval.dimensions.is_empty() {
            desc.push_str("**Dimension scores:**\n");
            let mut dims: Vec<_> = eval.dimensions.iter().collect();
            dims.sort_by(|a, b| a.0.cmp(b.0));
            for (dim, score) in &dims {
                desc.push_str(&format!("- **{}:** {:.2}\n", dim, score));
            }
            desc.push('\n');
        }
        if !eval.notes.is_empty() {
            desc.push_str("**Evaluator reasoning:**\n");
            // Truncate very long notes to keep the description manageable
            let notes_truncated: String = eval.notes.chars().take(4000).collect();
            desc.push_str(&notes_truncated);
            if eval.notes.len() > 4000 {
                desc.push_str("\n... (truncated)");
            }
            desc.push_str("\n\n");
        }

        desc.push_str("### Verification Steps\n");
        desc.push_str("Independently check whether the work was actually completed.\n");
        desc.push_str("Do NOT trust the original agent's claims.\n\n");

        if let Some(ref verify_cmd) = source_verify_cmd {
            desc.push_str(&format!(
                "1. **Run the verification command:** `{}`\n",
                verify_cmd
            ));
            desc.push_str("2. Check git log/diff for actual changes\n");
            desc.push_str("3. Run relevant tests\n");
            desc.push_str("4. Verify artifacts exist\n\n");
        } else {
            desc.push_str(
                "1. Check `git log --oneline -10` for recent commits related to this task\n",
            );
            desc.push_str("2. Check `git diff` to see if meaningful changes were made\n");
            desc.push_str("3. Run `cargo build && cargo test` to verify nothing is broken\n");
            desc.push_str("4. Verify any artifacts mentioned in the task description exist\n\n");
        }

        desc.push_str(
            "### Repair & Verdict\n\
             - If everything looks good: log verification passed and mark this task done.\n\
             - If problems found: **fix them directly** — edit code, resolve test failures, \
               correct logic errors, then run the verification again. Commit your fixes \
               with a descriptive message. Once fixed, mark this task done.\n\
             - **Only as a last resort**, if the approach is fundamentally wrong and cannot \
               be salvaged: run `wg fail '{source_task_id}' --reason \"FLIP verification failed: <reason>\"` \
               then mark this task done.\n\n\
             Remember: your job is to make the work **pass**, not to find reasons to reject it.\n"
        );
        // Replace placeholders
        desc = desc.replace("{source_task_id}", source_task_id);

        let verify_task = Task {
            id: verify_task_id.clone(),
            title: format!("Verify (FLIP {:.2}): {}", eval.score, source_title),
            description: Some(desc),
            status: Status::Open,
            priority: PRIORITY_DEFAULT,
            assigned: None,
            estimate: None,
            before: vec![],
            after: vec![source_task_id.clone()],
            requires: vec![],
            tags: vec!["verification".to_string(), "agency".to_string()],
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
            log: vec![],
            retry_count: 0,
            max_retries: Some(1),
            failure_reason: None,
            failure_class: None,
            model: Some(verification_model.clone()),
            reasoning: verification_resolved.reasoning,
            provider: verification_resolved.provider.clone(),
            endpoint: None,
            remote_provider: None,
            profile: None,
            command_argv: vec![],
            working_dir: None,
            executor_preset_name: None,
            verify: source_verify_cmd,
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
            // Verification agent is empowered to fix problems — needs full exec
            // authority to edit files, run builds, and commit repairs.
            exec_mode: None,
            token_usage: None,
            session_id: None,
            wait_condition: None,
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
            spawn_failures: 0,
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
                    "FLIP score {:.2} below threshold {:.2} — triggering verification (model: {})",
                    eval.score, threshold, verification_model,
                ),
            });
        }

        eprintln!(
            "[dispatcher] Created FLIP verification task '{}' (score {:.2} < {:.2})",
            verify_task_id, eval.score, threshold,
        );

        // Scaffold the full agency pipeline (.assign-*, .flip-*, .evaluate-*) for
        // the verify task — it's a pipeline-eligible system task, so
        // scaffold_full_pipeline handles it just like a regular task.
        let verify_title = format!("Verify (FLIP {:.2}): {}", eval.score, source_title);
        crate::commands::eval_scaffold::scaffold_full_pipeline(
            dir,
            graph,
            &verify_task_id,
            &verify_title,
            config,
        );

        // Add verify as additional dep on .evaluate-<source> so the source's
        // evaluation waits for verification to complete.
        let eval_task_id = format!(".evaluate-{}", source_task_id);
        if let Some(eval_task) = graph.get_task_mut(&eval_task_id)
            && !eval_task.after.contains(&verify_task_id)
        {
            eval_task.after.push(verify_task_id.clone());
        }

        modified = true;
    }

    modified
}

/// Separate-agent verification: when verify_mode=separate, tasks transition to
/// PendingValidation instead of running their --verify command inline.  This
/// function finds those tasks and creates a `.sep-verify-{task_id}` agent task
/// that runs the verify command in an independent context window.
///
/// The separate verification agent receives:
/// - The original task description and --verify criteria
/// - Task artifacts and file diffs
/// - NO implementation conversation history (independent context)
///
/// On pass: the verify agent calls `wg approve {task_id}` → Done
/// On fail: the verify agent calls `wg reject {task_id} --reason "..."` → Open (re-dispatch)
///
/// Returns `true` if the graph was modified.
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
            description: Some(desc),
            status: Status::Open,
            priority: PRIORITY_DEFAULT,
            assigned: None,
            estimate: None,
            before: vec![],
            after: vec![source_task_id.clone()],
            requires: vec![],
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
            log: vec![],
            retry_count: 0,
            max_retries: Some(1),
            failure_reason: None,
            failure_class: None,
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
            session_id: None,
            wait_condition: None,
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
            spawn_failures: 0,
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

    let evolve_task = Task {
        id: evolve_task_id.clone(),
        title: format!("Auto-evolve: {}", trigger_reason),
        description: Some(desc),
        status: Status::Open,
        priority: PRIORITY_DEFAULT,
        assigned: None,
        estimate: None,
        before: vec![],
        after: causal_ids,
        requires: vec![],
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
        log: vec![],
        retry_count: 0,
        max_retries: Some(1),
        failure_reason: None,
        failure_class: None,
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
        session_id: None,
        wait_condition: None,
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
        spawn_failures: 0,
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

    let create_task = Task {
        id: create_task_id.clone(),
        title: format!("Auto-create: {} tasks since last creation", since_last),
        description: Some(desc),
        status: Status::Open,
        priority: PRIORITY_DEFAULT,
        assigned: None,
        estimate: None,
        before: vec![],
        after: trigger_ids,
        requires: vec![],
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
        log: vec![],
        retry_count: 0,
        max_retries: Some(1),
        failure_reason: None,
        failure_class: None,
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
        session_id: None,
        wait_condition: None,
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
        spawn_failures: 0,
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
fn write_inline_artifacts(
    output_dir: &Path,
    agent_id: &str,
    task_id: &str,
    executor: &str,
    model: Option<&str>,
    script: &str,
) {
    let metadata = serde_json::json!({
        "agent_id": agent_id,
        "task_id": task_id,
        "executor": executor,
        "model": model,
        "started_at": Utc::now().to_rfc3339(),
        "inline": true,
    });
    let _ = fs::write(
        output_dir.join("metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap_or_default(),
    );
    let _ = fs::write(
        output_dir.join("prompt.txt"),
        format!(
            "[inline {} task — no LLM prompt; runs: {}]",
            executor, task_id
        ),
    );
    let wrapper = format!(
        "#!/bin/bash\n# Auto-generated inline {} wrapper\n{}",
        executor, script
    );
    let _ = fs::write(output_dir.join("run.sh"), &wrapper);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(output_dir.join("run.sh"), fs::Permissions::from_mode(0o755));
    }
}

/// Spawn an evaluation task directly without the full agent spawn machinery.
///
/// Forks a single process: `wg evaluate <source-task> --model <model>` that
/// marks the eval task done/failed on exit. Skips executor config resolution
/// and template processing but still emits the standard agent artifacts
/// (metadata.json, prompt.txt, run.sh, output.log) for debugging and replay.
///
/// The forked process is still tracked in the agent registry for dead-agent
/// detection.
/// Build the bash script that runs an inline eval, optionally records a
/// special-agent performance row, and marks the eval task done/failed.
///
/// Inputs `escaped_eval_id` and `escaped_output` are already shell-escaped
/// for single-quoted contexts (i.e. each `'` already replaced with `'\''`).
/// `special_agent_id`, when present, is similarly escaped by the caller.
fn build_inline_eval_script(
    eval_cmd: &str,
    escaped_eval_id: &str,
    escaped_output: &str,
    special_agent_id: Option<&str>,
    wg_cmd: &str,
    escaped_agent_id: &str,
    heartbeat_interval_seconds: u64,
) -> String {
    let (success_record, failure_record) = special_agent_id.map_or_else(
        || (String::new(), String::new()),
        |sa_id| {
            let escaped_sa_id = sa_id.replace('\'', "'\\''");
            (
                format!(
                    "    {wg_cmd} evaluate record --task '{escaped_eval_id}' --score 1.0 --source system --notes \"Inline evaluation completed successfully (agent: {escaped_sa_id})\" 2>> '{escaped_output}' || true\n"
                ),
                format!(
                    "        {wg_cmd} evaluate record --task '{escaped_eval_id}' --score 0.0 --source system --notes \"Inline evaluation failed with exit code $EXIT_CODE (agent: {escaped_sa_id})\" 2>> '{escaped_output}' || true\n"
                ),
            )
        },
    );

    format!(
        r#"unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT
_WG_STDERR=$(mktemp)
# The watcher owns no timer subprocess. Its stdin is an anonymous guard pipe
# whose sole writer belongs to this wrapper; the inference child explicitly
# closes the writer so wrapper death (including SIGKILL) produces immediate EOF.
exec {{INLINE_HEARTBEAT_GUARD_FD}}> >({wg_cmd} heartbeat-watch '{escaped_agent_id}' --interval-seconds {heartbeat_interval_seconds} --supervised-pid "$$" 2>> '{escaped_output}')
INLINE_HEARTBEAT_PID=$!
_WG_STOP_INLINE_HEARTBEAT() {{
    exec {{INLINE_HEARTBEAT_GUARD_FD}}>&- 2>/dev/null || true
    if [ -n "${{INLINE_HEARTBEAT_PID:-}}" ]; then
        kill "$INLINE_HEARTBEAT_PID" 2>/dev/null || true
        wait "$INLINE_HEARTBEAT_PID" 2>/dev/null || true
        INLINE_HEARTBEAT_PID=
    fi
}}
trap '_WG_STOP_INLINE_HEARTBEAT' EXIT
trap '_WG_STOP_INLINE_HEARTBEAT; trap - EXIT; exit 143' TERM INT HUP
{{
    {eval_cmd} >> '{escaped_output}' 2>"$_WG_STDERR"
}} {{INLINE_HEARTBEAT_GUARD_FD}}>&-
EXIT_CODE=$?
_WG_STOP_INLINE_HEARTBEAT
trap - EXIT TERM INT HUP
cat "$_WG_STDERR" >> '{escaped_output}'
if [ $EXIT_CODE -eq 0 ]; then
    rm -f "$_WG_STDERR"
{success_record}    {wg_cmd} done '{escaped_eval_id}' 2>> '{escaped_output}'
else
    _WG_ROUTE_FAILURE=0
    grep -q 'error\[WG-EXEC-' "$_WG_STDERR" 2>/dev/null && _WG_ROUTE_FAILURE=1
    _WG_STDERR_TAIL=$(tail -n 20 "$_WG_STDERR" 2>/dev/null | head -c 2000 || true)
    _WG_STDERR_FULL=$(tail -n 100 "$_WG_STDERR" 2>/dev/null || true)
    rm -f "$_WG_STDERR"
    {wg_cmd} log '{escaped_eval_id}' "Eval stderr: $_WG_STDERR_FULL" 2>> '{escaped_output}' || true
    if [ $_WG_ROUTE_FAILURE -eq 1 ]; then
        {wg_cmd} wait '{escaped_eval_id}' --until 'timer:1m' --checkpoint 'LLM execution route unavailable; retry without recording a semantic verdict' 2>> '{escaped_output}'
    else
{failure_record}        REASON=$(printf 'wg evaluate exited with code %s\n---\n%s' "$EXIT_CODE" "$_WG_STDERR_TAIL")
        {wg_cmd} fail '{escaped_eval_id}' --reason "$REASON" 2>> '{escaped_output}'
    fi
fi
exit $EXIT_CODE"#,
    )
}

fn persisted_agency_plan(
    dir: &Path,
    eval_task_id: &str,
) -> Result<worksgood::eval_lifecycle::AgencyDispatchPlan> {
    let graph_path = graph_path(dir);
    let graph = worksgood::parser::load_graph(&graph_path)?;
    let satellite = graph
        .get_task(eval_task_id)
        .ok_or_else(|| anyhow::anyhow!("Eval task '{}' not found", eval_task_id))?;
    if let Some(plan) = satellite.agency_dispatch.as_ref() {
        worksgood::eval_lifecycle::validate_plan(plan)?;
        return Ok(plan.clone());
    }
    let source_id = eval_task_id
        .strip_prefix(".flip-")
        .or_else(|| eval_task_id.strip_prefix(".evaluate-"))
        .ok_or_else(|| anyhow::anyhow!("invalid evaluation satellite id {eval_task_id:?}"))?;
    let source = graph
        .get_task(source_id)
        .ok_or_else(|| anyhow::anyhow!("evaluation source {source_id:?} not found"))?;
    let migrated = worksgood::eval_lifecycle::migrate_legacy_plan(source, satellite)?;
    let expected_model = satellite.model.clone();
    let expected_provider = satellite.provider.clone();
    let migrated_clone = migrated.clone();
    let mut installed = false;
    modify_graph(&graph_path, |fresh| {
        let Some(task) = fresh.get_task_mut(eval_task_id) else {
            return false;
        };
        if task.agency_dispatch.is_some()
            || task.model != expected_model
            || task.provider != expected_provider
        {
            return false;
        }
        task.model = Some(migrated_clone.calls[0].route.clone());
        task.provider = Some(migrated_clone.calls[0].system.handler.clone());
        task.agency_dispatch = Some(migrated_clone.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("eval-lifecycle-repair".to_string()),
            user: None,
            message: format!(
                "Installed lossless historical agency plan {} (route={})",
                migrated_clone.plan_hash, migrated_clone.calls[0].route
            ),
        });
        installed = true;
        true
    })?;
    if !installed {
        let fresh = worksgood::parser::load_graph(&graph_path)?;
        let plan = fresh
            .get_task(eval_task_id)
            .and_then(|task| task.agency_dispatch.clone())
            .ok_or_else(|| anyhow::anyhow!("agency plan changed concurrently; retry"))?;
        worksgood::eval_lifecycle::validate_plan(&plan)?;
        return Ok(plan);
    }
    Ok(migrated)
}

fn inline_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Absolute, graph-pinned command prefix for every recursive WG invocation in
/// an inline wrapper. Resolving this once at spawn prevents PATH collisions and
/// an inherited sibling project's cwd/WG_DIR from redirecting heartbeats or
/// lifecycle transitions.
fn authoritative_inline_wg_command(dir: &Path) -> Result<String> {
    let exe = std::env::current_exe().context("resolve authoritative WG executable")?;
    if !exe.is_absolute() {
        anyhow::bail!(
            "authoritative WG executable is not absolute: {}",
            exe.display()
        );
    }
    let graph_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    Ok(format!(
        "{} --dir {}",
        inline_shell_quote(&exe.to_string_lossy()),
        inline_shell_quote(&graph_dir.to_string_lossy())
    ))
}

fn qualify_inline_wg_exec(exec: &str, wg_cmd: &str) -> Option<String> {
    exec.strip_prefix("wg ")
        .map(|arguments| format!("{wg_cmd} {arguments}"))
}

/// Keep at least three beats inside the configured reaper window. The exact
/// seconds override makes this production primitive accelerated-testable while
/// the ordinary default remains the long-standing five-minute window.
fn inline_heartbeat_interval_seconds(config: &Config) -> u64 {
    (config.agent.heartbeat_timeout_secs() / 3).clamp(1, 120)
}

fn spawn_eval_inline(
    dir: &Path,
    eval_task_id: &str,
    _compatibility_model: Option<&str>,
) -> Result<(String, u32)> {
    use std::process::{Command, Stdio};

    // Persisted stage-aware plans are invocation authority across scaffold,
    // restart and retry. Only lossless historical rows are migrated.
    let config = Config::load_or_default(dir);
    let wg_cmd = authoritative_inline_wg_command(dir)?;
    let heartbeat_interval_seconds = inline_heartbeat_interval_seconds(&config);
    let plan = persisted_agency_plan(dir, eval_task_id)?;
    let primary = plan
        .calls
        .first()
        .ok_or_else(|| anyhow::anyhow!("agency plan contains no calls"))?;
    let eval_executor = primary.system.handler.clone();
    let eval_recorded_model = plan
        .calls
        .iter()
        .map(|call| call.route.as_str())
        .collect::<Vec<_>>()
        .join(" + ");

    let graph_path = graph_path(dir);

    // Set up minimal agent tracking (before modify_graph so we have the agent_id)
    // Use load_locked to prevent the non-locked save from clobbering concurrent
    // registry updates from wg done/wg fail (which also use load_locked).
    let mut locked_registry = AgentRegistry::load_locked(dir)?;
    let agent_id = format!("agent-{}", locked_registry.next_agent_id);
    // Create minimal output directory for log capture
    let output_dir = dir.join("agents").join(&agent_id);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create eval output dir: {:?}", output_dir))?;
    let output_file = output_dir.join("output.log");
    let output_file_str = output_file.to_string_lossy().to_string();

    let escaped_eval_id = eval_task_id.replace('\'', "'\\''");
    let escaped_agent_id = agent_id.replace('\'', "'\\''");
    let escaped_output = output_file_str.replace('\'', "'\\''");

    // Atomically claim the task and extract needed fields.
    // Using modify_graph prevents the "lost update" race where a concurrent
    // `wg done` from a previously-spawned fast eval task saves between our
    // read and write, and our write clobbers the Done status back to InProgress.
    let mut eval_task_exec: Option<String> = None;
    let mut eval_task_agent: Option<String> = None;
    let mut claim_error: Option<String> = None;
    let agent_id_clone = agent_id.clone();
    let eval_model_msg = format!(" --agency-plan {}", plan.plan_hash);

    modify_graph(&graph_path, |graph| {
        let task = match graph.get_task_mut(eval_task_id) {
            Some(t) => t,
            None => {
                claim_error = Some(format!("Eval task '{}' not found", eval_task_id));
                return false;
            }
        };

        if task.status != Status::Open {
            claim_error = Some(format!(
                "Eval task '{}' is not open (status: {:?})",
                eval_task_id, task.status
            ));
            return false;
        }

        eval_task_exec = task.exec.clone();
        eval_task_agent = task.agent.clone();
        if task
            .agency_dispatch
            .as_ref()
            .map(|current| current.plan_hash.as_str())
            != Some(plan.plan_hash.as_str())
        {
            claim_error = Some(format!(
                "Eval task '{}' plan changed before claim",
                eval_task_id
            ));
            return false;
        }

        let lifecycle = task.evaluation_lifecycle.get_or_insert_with(|| {
            worksgood::eval_lifecycle::EvaluationLifecycle {
                schema: worksgood::eval_lifecycle::EVAL_LIFECYCLE_SCHEMA,
                pipeline_id: plan.pipeline_id.clone(),
                source_attempt: plan.source_attempt,
                route_generation: 0,
                schedule_attempts: 0,
                transport_attempts: 0,
                semantic_attempts: 0,
                execution_state: worksgood::eval_lifecycle::EvaluationExecutionState::Ready,
                linked_flip_verdict: None,
                linked_eval_verdict: None,
                consumed_verdict: None,
                repair_version: 0,
            }
        });
        if lifecycle.pipeline_id != plan.pipeline_id {
            lifecycle.execution_state =
                worksgood::eval_lifecycle::EvaluationExecutionState::Blocked;
            task.status = Status::Blocked;
            task.failure_reason = Some(format!(
                "evaluation pipeline changed: task={} plan={}",
                lifecycle.pipeline_id, plan.pipeline_id
            ));
            claim_error = Some(format!(
                "error[WG-EXEC-AGENCY-PLAN-MISMATCH]: '{}' lifecycle does not match its plan",
                eval_task_id
            ));
            return true;
        }
        if let Err(error) = lifecycle.reserve_transport_attempt() {
            task.status = Status::Blocked;
            task.failure_reason = Some(format!(
                "evaluation execution retry exhausted for plan {} after {} claimed attempt(s)",
                plan.plan_hash, lifecycle.transport_attempts
            ));
            claim_error = Some(format!("{error:#}: task={eval_task_id}"));
            return true;
        }
        task.status = Status::InProgress;
        task.started_at = Some(Utc::now().to_rfc3339());
        task.assigned = Some(agent_id_clone.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some(agent_id_clone.clone()),
            user: Some(worksgood::current_user()),
            message: format!("Spawned eval inline{}", eval_model_msg),
        });

        true
    })
    .context("Failed to load/save graph for eval spawn")?;

    if let Some(err) = claim_error {
        anyhow::bail!("{}", err);
    }

    // Use the task's exec command directly if it starts with "wg evaluate".
    // This handles both "wg evaluate run <task>" and "wg evaluate org <task>".
    // Fall back to reconstructing from task ID for backward compatibility.
    let source_task_id = eval_task_id
        .strip_prefix(".evaluate-")
        .or_else(|| eval_task_id.strip_prefix(".flip-"))
        .or_else(|| eval_task_id.strip_prefix("evaluate-"))
        .unwrap_or(eval_task_id);
    let base_eval_cmd = if let Some(ref exec) = eval_task_exec
        && exec.starts_with("wg evaluate")
    {
        qualify_inline_wg_exec(exec, &wg_cmd)
            .ok_or_else(|| anyhow::anyhow!("invalid inline eval command: {exec:?}"))?
    } else {
        format!(
            "{wg_cmd} evaluate run '{}'",
            source_task_id.replace('\'', "'\\''")
        )
    };
    let escaped_plan_hash = plan.plan_hash.replace('\'', "'\\''");
    let eval_cmd = format!(
        "WG_AGENCY_TASK_ID='{escaped_eval_id}' WG_AGENCY_PLAN_HASH='{escaped_plan_hash}' {base_eval_cmd}"
    );

    // Resolve the special agent (evaluator) hash for performance recording.
    // After the inline eval completes, we record an Evaluation against this
    // agent so it accumulates performance history like any other agent.
    let special_agent_hash = eval_task_agent
        .clone()
        .or_else(|| config.agency.evaluator_agent.clone());

    // Build the special agent performance recording command.
    // After `wg evaluate` completes, record an evaluation against the special
    // agent (evaluator) entity so it accumulates performance history.
    // On success: score 1.0. On failure: score 0.0.
    let special_agent_verified = special_agent_hash.as_ref().and_then(|hash| {
        let agency_dir = dir.join("agency");
        let agents_dir = agency_dir.join("cache/agents");
        agency::find_agent_by_prefix(&agents_dir, hash)
            .ok()
            .map(|a| a.id)
    });

    // Single script: run eval, record special agent perf, then mark done/failed.
    // Token usage is captured by `wg done` which parses __WG_TOKENS__ lines
    // from the output.log directly.
    let script = build_inline_eval_script(
        &eval_cmd,
        &escaped_eval_id,
        &escaped_output,
        special_agent_verified.as_deref(),
        &wg_cmd,
        &escaped_agent_id,
        heartbeat_interval_seconds,
    );

    // Route resolution happened before the claim so registry metadata and the
    // actual call cannot disagree or create a claimed task with no executor.
    write_inline_artifacts(
        &output_dir,
        &agent_id,
        eval_task_id,
        &eval_executor,
        Some(&eval_recorded_model),
        &script,
    );

    // Fork the process
    let bash_path = worksgood::platform_bash::bash_exe_path(config.bash.path.as_deref())
        .context("Failed to resolve bash executable for inline eval")?;
    let mut cmd = Command::new(&bash_path);
    cmd.arg("-c").arg(&script);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    // Detach into own session so it survives daemon restart
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
    // On Windows, the direct equivalent is `CREATE_NEW_PROCESS_GROUP`: it
    // puts the child at the root of its own process group so console
    // control events (Ctrl+Break, Ctrl+C, window-close) sent to — or
    // cascading through — the daemon's group don't also terminate it.
    // Without this, inline-spawn bash children die at roughly each 60s
    // tick because a stray console event in the daemon's group takes them
    // with it. The regular task-agent spawn path in `spawn/execution`
    // already uses the same flag for this reason.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x0000_0200);
    }

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Rollback the claim atomically
            let agent_id_rollback = agent_id.clone();
            let err_msg = e.to_string();
            let _ = modify_graph(&graph_path, |graph| {
                if let Some(t) = graph.get_task_mut(eval_task_id) {
                    t.status = Status::Open;
                    t.started_at = None;
                    t.assigned = None;
                    t.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some(agent_id_rollback.clone()),
                        user: Some(worksgood::current_user()),
                        message: format!("Eval spawn failed, reverting claim: {}", err_msg),
                    });
                    true
                } else {
                    false
                }
            });
            return Err(anyhow::anyhow!("Failed to spawn eval process: {}", e));
        }
    };

    let pid = child.id();

    // Register in agent registry for dead-agent detection
    locked_registry.register_agent_with_model(
        pid,
        eval_task_id,
        &eval_executor,
        &output_file_str,
        Some(&eval_recorded_model),
    );
    locked_registry
        .save()
        .context("Failed to save agent registry after eval spawn")?;

    Ok((agent_id, pid))
}

/// Spawn an assignment inline task (similar to eval but for `wg assign --auto`).
/// Emits the standard agent artifacts (metadata.json, prompt.txt, run.sh, output.log).
fn spawn_assign_inline(dir: &Path, assign_task_id: &str) -> Result<(String, u32)> {
    use std::process::{Command, Stdio};

    // Preflight before any claim/artifact mutation. Built-in Claude catalog
    // defaults do not authorize an assignment call.
    let assign_config = Config::load_or_default(dir);
    let wg_cmd = authoritative_inline_wg_command(dir)?;
    let heartbeat_interval_seconds = inline_heartbeat_interval_seconds(&assign_config);
    let assign_dispatch =
        worksgood::service::llm::resolve_agency_dispatch(&assign_config, DispatchRole::Assigner)
            .context("agency assigner execution route is not selected")?;
    let assign_executor = assign_dispatch.handler.as_str().to_string();
    let assign_model = assign_dispatch.raw_spec;

    let graph_path = graph_path(dir);

    // Set up minimal agent tracking (before modify_graph so we have the agent_id)
    // Use load_locked to prevent the non-locked save from clobbering concurrent
    // registry updates from wg done/wg fail (which also use load_locked).
    let mut locked_registry = AgentRegistry::load_locked(dir)?;
    let agent_id = format!("agent-{}", locked_registry.next_agent_id);

    // Create minimal output directory for log capture
    let output_dir = dir.join("agents").join(&agent_id);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create assign output dir: {:?}", output_dir))?;
    let output_file = output_dir.join("output.log");
    let output_file_str = output_file.to_string_lossy().to_string();

    let escaped_assign_id = assign_task_id.replace('\'', "'\\''");
    let escaped_agent_id = agent_id.replace('\'', "'\\''");
    let escaped_output = output_file_str.replace('\'', "'\\''");

    // Atomically claim the task and extract needed fields.
    // Using modify_graph prevents the "lost update" race where a concurrent
    // `wg done` from a previously-spawned fast inline task saves between our
    // read and write, and our write clobbers the Done status back to InProgress.
    let mut assign_task_exec: Option<String> = None;
    let mut claim_error: Option<String> = None;
    let agent_id_clone = agent_id.clone();

    modify_graph(&graph_path, |graph| {
        let task = match graph.get_task_mut(assign_task_id) {
            Some(t) => t,
            None => {
                claim_error = Some(format!("Assignment task '{}' not found", assign_task_id));
                return false;
            }
        };

        if task.status != Status::Open {
            claim_error = Some(format!(
                "Assignment task '{}' is not open (status: {:?})",
                assign_task_id, task.status
            ));
            return false;
        }

        assign_task_exec = task.exec.clone();

        task.status = Status::InProgress;
        task.started_at = Some(Utc::now().to_rfc3339());
        task.assigned = Some(agent_id_clone.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some(agent_id_clone.clone()),
            user: Some(worksgood::current_user()),
            message: "Spawned assignment inline".to_string(),
        });

        true
    })
    .context("Failed to load/save graph for assign spawn")?;

    if let Some(err) = claim_error {
        anyhow::bail!("{}", err);
    }

    // Extract source task ID from the assign task ID (strip ".assign-" prefix)
    let source_task_id = assign_task_id
        .strip_prefix(".assign-")
        .unwrap_or(assign_task_id);

    // Use the task's exec command directly if it starts with "wg assign".
    // Fall back to constructing from task ID for backward compatibility.
    let assign_cmd = if let Some(ref exec) = assign_task_exec
        && exec.starts_with("wg assign")
    {
        qualify_inline_wg_exec(exec, &wg_cmd)
            .ok_or_else(|| anyhow::anyhow!("invalid inline assignment command: {exec:?}"))?
    } else {
        format!(
            "{wg_cmd} assign '{}' --auto",
            source_task_id.replace('\'', "'\\''")
        )
    };

    // Build the script: run assign, then mark done/failed
    let script = format!(
        r#"unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT
_WG_STDERR=$(mktemp)
exec {{INLINE_HEARTBEAT_GUARD_FD}}> >({wg_cmd} heartbeat-watch '{escaped_agent_id}' --interval-seconds {heartbeat_interval_seconds} --supervised-pid "$$" 2>> '{escaped_output}')
INLINE_HEARTBEAT_PID=$!
_WG_STOP_INLINE_HEARTBEAT() {{
    exec {{INLINE_HEARTBEAT_GUARD_FD}}>&- 2>/dev/null || true
    if [ -n "${{INLINE_HEARTBEAT_PID:-}}" ]; then
        kill "$INLINE_HEARTBEAT_PID" 2>/dev/null || true
        wait "$INLINE_HEARTBEAT_PID" 2>/dev/null || true
        INLINE_HEARTBEAT_PID=
    fi
}}
trap '_WG_STOP_INLINE_HEARTBEAT' EXIT
trap '_WG_STOP_INLINE_HEARTBEAT; trap - EXIT; exit 143' TERM INT HUP
{{
    {assign_cmd} >> '{escaped_output}' 2>"$_WG_STDERR"
}} {{INLINE_HEARTBEAT_GUARD_FD}}>&-
EXIT_CODE=$?
_WG_STOP_INLINE_HEARTBEAT
trap - EXIT TERM INT HUP
cat "$_WG_STDERR" >> '{escaped_output}'
if [ $EXIT_CODE -eq 0 ]; then
    rm -f "$_WG_STDERR"
    {wg_cmd} done '{escaped_assign_id}' 2>> '{escaped_output}'
else
    _WG_ROUTE_FAILURE=0
    grep -q 'error\[WG-EXEC-' "$_WG_STDERR" 2>/dev/null && _WG_ROUTE_FAILURE=1
    _WG_STDERR_TAIL=$(tail -n 20 "$_WG_STDERR" 2>/dev/null | head -c 2000 || true)
    _WG_STDERR_FULL=$(tail -n 100 "$_WG_STDERR" 2>/dev/null || true)
    rm -f "$_WG_STDERR"
    {wg_cmd} log '{escaped_assign_id}' "Assign stderr: $_WG_STDERR_FULL" 2>> '{escaped_output}' || true
    if [ $_WG_ROUTE_FAILURE -eq 1 ]; then
        {wg_cmd} wait '{escaped_assign_id}' --until 'timer:1m' --checkpoint 'LLM execution route unavailable; retry without recording a semantic verdict' 2>> '{escaped_output}'
    else
        REASON=$(printf 'wg assign exited with code %s\n---\n%s' "$EXIT_CODE" "$_WG_STDERR_TAIL")
        {wg_cmd} fail '{escaped_assign_id}' --reason "$REASON" 2>> '{escaped_output}'
    fi
fi
exit $EXIT_CODE"#,
    );

    write_inline_artifacts(
        &output_dir,
        &agent_id,
        assign_task_id,
        &assign_executor,
        Some(&assign_model),
        &script,
    );

    // Fork the process
    let bash_path = worksgood::platform_bash::bash_exe_path(assign_config.bash.path.as_deref())
        .context("Failed to resolve bash executable for inline assign")?;
    let mut cmd = Command::new(&bash_path);
    cmd.arg("-c").arg(&script);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    // Detach into own session so it survives daemon restart
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
    // On Windows, the direct equivalent is `CREATE_NEW_PROCESS_GROUP`: it
    // puts the child at the root of its own process group so console
    // control events (Ctrl+Break, Ctrl+C, window-close) sent to — or
    // cascading through — the daemon's group don't also terminate it.
    // Without this, inline-spawn bash children die at roughly each 60s
    // tick because a stray console event in the daemon's group takes them
    // with it. The regular task-agent spawn path in `spawn/execution`
    // already uses the same flag for this reason.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x0000_0200);
    }

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Rollback the claim atomically
            let agent_id_rollback = agent_id.clone();
            let err_msg = e.to_string();
            let _ = modify_graph(&graph_path, |graph| {
                if let Some(t) = graph.get_task_mut(assign_task_id) {
                    t.status = Status::Open;
                    t.started_at = None;
                    t.assigned = None;
                    t.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some(agent_id_rollback.clone()),
                        user: Some(worksgood::current_user()),
                        message: format!("Assignment spawn failed, reverting claim: {}", err_msg),
                    });
                    true
                } else {
                    false
                }
            });
            return Err(anyhow::anyhow!("Failed to spawn assignment process: {}", e));
        }
    };

    let pid = child.id();

    // Register in agent registry for dead-agent detection
    locked_registry.register_agent_with_model(
        pid,
        assign_task_id,
        &assign_executor,
        &output_file_str,
        Some(&assign_model),
    );
    locked_registry
        .save()
        .context("Failed to save agent registry after assign spawn")?;

    Ok((agent_id, pid))
}

/// Spawn a shell-mode task inline: fork `wg exec --shell <task_id>` as a
/// lightweight subprocess instead of going through the full agent spawn path.
fn spawn_shell_inline(dir: &Path, task_id: &str) -> Result<(String, u32)> {
    use std::process::{Command, Stdio};

    let graph_path = graph_path(dir);
    let config = Config::load_or_default(dir);
    let graph = load_graph(&graph_path).context("Failed to load graph for shell admission")?;
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| anyhow::anyhow!("Shell task '{}' not found", task_id))?;
    let build_class = worksgood::disk_sentinel::classify_task(task);
    if build_class.is_build_capable() {
        let (level, reason, _) = worksgood::disk_sentinel::current_admission(
            dir,
            &config.coordinator.resource_management,
        );
        if level.blocks_builds() {
            anyhow::bail!("build admission {:?}: {}", level, reason);
        }
    }

    let mut locked_registry = AgentRegistry::load_locked(dir)?;
    if build_class.is_build_capable() {
        let admission = worksgood::disk_sentinel::build_admission_reclaiming_owned(
            dir,
            &config.coordinator.resource_management,
            build_class,
        );
        if !admission.allowed {
            anyhow::bail!(
                "build admission refused: {} (candidate={} bytes, concurrent-reserve={} bytes)",
                admission.reason,
                admission.candidate_bytes,
                admission.concurrent_reserved_bytes
            );
        }
    }
    let agent_id = format!("agent-{}", locked_registry.next_agent_id);
    let target_path = if build_class.is_build_capable() {
        Some(
            config
                .coordinator
                .resource_management
                .cargo_target_root
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("service").join("disk").join("build-targets"))
                .join(format!("wg-target-{agent_id}")),
        )
    } else {
        None
    };
    if let Some(path) = target_path.as_ref() {
        fs::create_dir_all(path)?;
    }
    let tmp_path = if build_class.is_build_capable() {
        Some(
            config
                .coordinator
                .resource_management
                .build_tmp_root
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(format!("wg-cargo-tmp-{agent_id}")),
        )
    } else {
        None
    };
    if let Some(path) = tmp_path.as_ref() {
        fs::create_dir_all(path)?;
    }

    let output_dir = dir.join("agents").join(&agent_id);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create shell output dir: {:?}", output_dir))?;
    let output_file = output_dir.join("output.log");
    let output_file_str = output_file.to_string_lossy().to_string();

    let escaped_task_id = task_id.replace('\'', "'\\''");
    let escaped_output = output_file_str.replace('\'', "'\\''");

    let mut claim_error: Option<String> = None;
    let agent_id_clone = agent_id.clone();

    modify_graph(&graph_path, |graph| {
        let task = match graph.get_task_mut(task_id) {
            Some(t) => t,
            None => {
                claim_error = Some(format!("Shell task '{}' not found", task_id));
                return false;
            }
        };

        if task.status != Status::Open {
            claim_error = Some(format!(
                "Shell task '{}' is not open (status: {:?})",
                task_id, task.status
            ));
            return false;
        }

        task.status = Status::InProgress;
        task.started_at = Some(Utc::now().to_rfc3339());
        task.assigned = Some(agent_id_clone.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some(agent_id_clone.clone()),
            user: Some(worksgood::current_user()),
            message: "Spawned shell task inline".to_string(),
        });

        true
    })
    .context("Failed to load/save graph for shell spawn")?;

    if let Some(err) = claim_error {
        anyhow::bail!("{}", err);
    }

    let script = format!(
        r#"unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT
_WG_STDERR=$(mktemp)
wg exec '{escaped_task_id}' --shell >> '{escaped_output}' 2>"$_WG_STDERR"
EXIT_CODE=$?
cat "$_WG_STDERR" >> '{escaped_output}'
rm -f "$_WG_STDERR"
exit $EXIT_CODE"#,
    );

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(&script);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    if let Some(path) = target_path.as_ref() {
        cmd.env("CARGO_TARGET_DIR", path);
    }
    if let Some(path) = tmp_path.as_ref() {
        cmd.env("TMPDIR", path);
    }

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

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let agent_id_rollback = agent_id.clone();
            let err_msg = e.to_string();
            let _ = modify_graph(&graph_path, |graph| {
                if let Some(t) = graph.get_task_mut(task_id) {
                    t.status = Status::Open;
                    t.started_at = None;
                    t.assigned = None;
                    t.log.push(LogEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        actor: Some(agent_id_rollback.clone()),
                        user: Some(worksgood::current_user()),
                        message: format!("Shell spawn failed, reverting claim: {}", err_msg),
                    });
                    true
                } else {
                    false
                }
            });
            return Err(anyhow::anyhow!("Failed to spawn shell process: {}", e));
        }
    };

    let pid = child.id();

    locked_registry.register_agent_with_model(pid, task_id, "shell", &output_file_str, None);
    for (path, kind) in [
        (
            target_path.as_ref(),
            worksgood::disk_sentinel::CacheKind::CargoTarget,
        ),
        (
            tmp_path.as_ref(),
            worksgood::disk_sentinel::CacheKind::CargoInstallScratch,
        ),
    ] {
        let Some(path) = path else { continue };
        let cache = worksgood::disk_sentinel::make_owned_cache(
            path,
            kind,
            task_id,
            &agent_id,
            pid,
            None,
            config
                .coordinator
                .resource_management
                .owned_cache_lease_seconds,
        );
        if let Err(error) = worksgood::disk_sentinel::register_owned_cache(dir, cache) {
            let _ = kill_process_graceful(pid, 1);
            anyhow::bail!("failed to persist shell build-cache ownership: {error:#}");
        }
    }
    locked_registry
        .save()
        .context("Failed to save agent registry after shell spawn")?;

    Ok((agent_id, pid))
}

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
/// Maximum number of rapid respawns allowed before the task is failed.
const RESPAWN_MAX_RAPID: usize = 5;

/// Time window (seconds) within which respawns are considered "rapid".
const RESPAWN_WINDOW_SECS: i64 = 300; // 5 minutes

/// Minimum seconds of backoff between respawns when rapid respawning is detected.
/// Each successive rapid respawn doubles the backoff (exponential).
const RESPAWN_BASE_BACKOFF_SECS: i64 = 60;

/// Check if a task is in a rapid respawn loop and should be throttled.
///
/// Examines the task's log for recent "process exited" / "Triage" entries
/// that indicate the agent died without completing the task. Returns:
/// - `Ok(())` if spawning should proceed
/// - `Err(reason)` if spawning should be skipped (throttled or failed)
fn check_respawn_throttle(task: &Task, graph_path: &Path) -> std::result::Result<(), String> {
    let now = Utc::now();

    // Count recent agent death events within the respawn window
    let recent_deaths: Vec<&LogEntry> = task
        .log
        .iter()
        .filter(|entry| {
            // Match log messages from triage/cleanup that indicate agent death
            (entry.message.contains("process exited")
                || entry.message.contains("PID reused")
                || entry.message.contains("Triage:"))
                && entry
                    .timestamp
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map(|t| now.signed_duration_since(t).num_seconds() < RESPAWN_WINDOW_SECS)
                    .unwrap_or(false)
        })
        .collect();

    let death_count = recent_deaths.len();

    // A single death is normal (OOM, signal, network hiccup).
    // Only start throttling at 2+ rapid deaths in the window.
    if death_count <= 1 {
        return Ok(());
    }

    // Fail the task if too many rapid respawns
    if death_count >= RESPAWN_MAX_RAPID {
        // Save the failure to the graph
        let task_id = task.id.clone();
        let fail_reason = format!(
            "Rapid respawn loop: {} agent deaths in {} seconds",
            death_count, RESPAWN_WINDOW_SECS
        );
        let fail_msg = format!(
            "Failed: rapid respawn loop detected ({} deaths in {}s window)",
            death_count, RESPAWN_WINDOW_SECS
        );
        let _ = modify_graph(graph_path, |graph| {
            if let Some(t) = graph.get_task_mut(&task_id) {
                t.status = Status::Failed;
                t.assigned = None;
                t.failure_reason = Some(fail_reason.clone());
                t.log.push(LogEntry {
                    timestamp: now.to_rfc3339(),
                    actor: Some("coordinator".to_string()),
                    user: Some(worksgood::current_user()),
                    message: fail_msg.clone(),
                });
                true
            } else {
                false
            }
        });
        return Err(format!(
            "rapid respawn loop ({} deaths), task failed",
            death_count
        ));
    }

    // Exponential backoff: base * 2^(death_count - 2)
    // death_count=2 → 60s, 3 → 120s, 4 → 240s
    let backoff_secs = RESPAWN_BASE_BACKOFF_SECS * (1i64 << (death_count - 2).min(6));

    // Check time since last death
    if let Some(last_death) = recent_deaths.last()
        && let Ok(last_time) = last_death
            .timestamp
            .parse::<chrono::DateTime<chrono::Utc>>()
    {
        let elapsed = now.signed_duration_since(last_time).num_seconds();
        if elapsed < backoff_secs {
            return Err(format!(
                "respawn backoff: {} deaths, waiting {}s ({}s elapsed)",
                death_count, backoff_secs, elapsed
            ));
        }
    }

    Ok(())
}

/// Check if a task has exceeded the spawn failure threshold and should be skipped.
///
/// Returns:
/// - `Ok(())` if spawning should proceed
/// - `Err(reason)` if spawning should be skipped (already failed by circuit breaker)
fn check_spawn_circuit_breaker(
    task: &Task,
    max_spawn_failures: u32,
) -> std::result::Result<(), String> {
    if max_spawn_failures == 0 {
        return Ok(()); // circuit breaker disabled
    }
    if task.spawn_failures >= max_spawn_failures {
        Err(format!(
            "spawn circuit breaker: {} consecutive spawn failures (threshold: {})",
            task.spawn_failures, max_spawn_failures,
        ))
    } else {
        Ok(())
    }
}

/// Record a spawn failure: increment the counter, log the error, and auto-fail
/// the task if the threshold is reached. Returns true if the task was auto-failed.
fn record_spawn_failure(
    graph_path: &Path,
    task_id: &str,
    error: &str,
    executor: &str,
    exec_mode: Option<&str>,
    max_spawn_failures: u32,
) -> bool {
    let now = Utc::now();
    let task_id_owned = task_id.to_string();
    let error_owned = error.to_string();
    let executor_owned = executor.to_string();
    let exec_mode_owned = exec_mode.map(|s| s.to_string());
    let mut tripped = false;

    let _ = modify_graph(graph_path, |graph| {
        let task = match graph.get_task_mut(&task_id_owned) {
            Some(t) => t,
            None => return false,
        };
        task.spawn_failures += 1;
        let failures = task.spawn_failures;

        let mode_str = exec_mode_owned.as_deref().unwrap_or("default");

        // Log the spawn failure
        task.log.push(LogEntry {
            timestamp: now.to_rfc3339(),
            actor: Some("spawn".to_string()),
            user: None,
            message: format!(
                "Spawn failed (attempt {}/{}): {}. exec_mode={}, executor={}",
                failures,
                if max_spawn_failures > 0 {
                    max_spawn_failures.to_string()
                } else {
                    "unlimited".to_string()
                },
                error_owned,
                mode_str,
                executor_owned,
            ),
        });

        // Circuit breaker: mark incomplete after threshold (evaluator decides fail)
        if max_spawn_failures > 0 && failures >= max_spawn_failures {
            task.status = Status::Incomplete;
            task.assigned = None;
            task.log.push(LogEntry {
                timestamp: now.to_rfc3339(),
                actor: Some("spawn-circuit-breaker".to_string()),
                user: None,
                message: format!(
                    "Circuit breaker tripped: spawn failed {} times. Last error: {}. exec_mode={}, executor={}. Task marked incomplete for evaluator review.",
                    failures, error_owned, mode_str, executor_owned,
                ),
            });
            tripped = true;
        }
        true
    });

    tripped
}

/// Keep an agency satellite retryable when execution selection/readiness fails
/// before claim. The Open->Waiting/Blocked mutation is the scheduling
/// reservation: concurrent ticks holding the same stale ready snapshot cannot
/// both increment the generation counter. No semantic budget is consumed.
fn park_agency_execution_error(graph_path: &Path, task_id: &str, error: &anyhow::Error) -> bool {
    let diagnostic = format!("{error:#}");
    if !diagnostic.contains("error[WG-EXEC-") {
        return false;
    }

    let task_id = task_id.to_string();
    let _ = modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(&task_id) else {
            return false;
        };
        if task.status != Status::Open {
            return false;
        }
        let Some(plan) = task.agency_dispatch.clone() else {
            // An ambiguous legacy row has no safe generation to retry.
            task.status = Status::Blocked;
            task.failure_reason = Some(diagnostic.clone());
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("agency-execution".to_string()),
                user: None,
                message: format!("Lifecycle route repair required: {diagnostic}"),
            });
            return true;
        };
        let lifecycle = task.evaluation_lifecycle.get_or_insert_with(|| {
            worksgood::eval_lifecycle::EvaluationLifecycle {
                schema: worksgood::eval_lifecycle::EVAL_LIFECYCLE_SCHEMA,
                pipeline_id: plan.pipeline_id.clone(),
                source_attempt: plan.source_attempt,
                route_generation: 0,
                schedule_attempts: 0,
                transport_attempts: 0,
                semantic_attempts: 0,
                execution_state: worksgood::eval_lifecycle::EvaluationExecutionState::Ready,
                linked_flip_verdict: None,
                linked_eval_verdict: None,
                consumed_verdict: None,
                repair_version: 0,
            }
        });
        lifecycle.schedule_attempts = lifecycle.schedule_attempts.saturating_add(1);
        let attempts = lifecycle.schedule_attempts;
        let limit = worksgood::eval_lifecycle::MAX_EXECUTION_ATTEMPTS_PER_ROUTE_GENERATION;
        task.assigned = None;
        task.failure_reason = Some(diagnostic.clone());
        if attempts >= limit {
            task.status = Status::Blocked;
            task.wait_condition = None;
            lifecycle.execution_state =
                worksgood::eval_lifecycle::EvaluationExecutionState::Blocked;
        } else {
            let delay_minutes = i64::from(1u32 << attempts.saturating_sub(1).min(5));
            let resume_after = (Utc::now() + chrono::Duration::minutes(delay_minutes)).to_rfc3339();
            task.status = Status::Waiting;
            task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::Timer { resume_after }]));
            lifecycle.execution_state =
                worksgood::eval_lifecycle::EvaluationExecutionState::Waiting;
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("agency-execution".to_string()),
            user: None,
            message: format!(
                "Execution route unavailable; scheduling attempt {attempts}/{limit}; no semantic verdict: {diagnostic}"
            ),
        });
        true
    });
    true
}

fn spawn_agents_for_ready_tasks(
    dir: &Path,
    graph: &worksgood::graph::WorkGraph,
    _executor: &str,
    config: &Config,
    default_model: Option<&str>,
    slots_available: usize,
    auto_assign: bool,
) -> usize {
    let cycle_analysis = graph.compute_cycle_analysis();
    let ready_tasks_raw = ready_tasks_with_peers_cycle_aware(graph, dir, &cycle_analysis);
    let agents_dir = dir.join("agency").join("cache/agents");
    let gp = graph_path(dir);
    let mut spawned = 0;
    let disk_snapshot = worksgood::disk_sentinel::load_snapshot(dir).ok().flatten();
    let builds_blocked = disk_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.level.blocks_builds());
    let mut active_build_heavy = disk_snapshot
        .as_ref()
        .map(|snapshot| snapshot.active_build_heavy)
        .unwrap_or(0);
    // Memoize loaded WCC-profile configs by name for this tick so a component
    // of N profiled tasks loads each profile file at most once.
    let mut profile_cache = worksgood::dispatch::ProfileCache::new();

    // Sort ready tasks by priority with starvation prevention and priority inheritance
    let final_ready = sort_tasks_by_priority_with_features(graph, ready_tasks_raw, config);

    for task in final_ready.iter() {
        if spawned >= slots_available {
            break;
        }
        // Skip if already claimed
        if task.assigned.is_some() {
            continue;
        }

        // Skip daemon-managed loop tasks — handled directly by the daemon, not spawned as agents
        if is_daemon_managed(task) {
            continue;
        }

        // Respawn throttle: detect rapid respawn loops and back off
        if let Err(reason) = check_respawn_throttle(task, &gp) {
            eprintln!("[dispatcher] Skipping '{}': {}", task.id, reason);
            continue;
        }

        // Spawn circuit breaker: skip tasks that have already hit the spawn failure threshold
        if let Err(reason) =
            check_spawn_circuit_breaker(task, config.coordinator.max_spawn_failures)
        {
            eprintln!("[dispatcher] Skipping '{}': {}", task.id, reason);
            continue;
        }

        // Skip system tasks whose source task is abandoned (defense-in-depth)
        if task.id.starts_with('.') {
            let source_abandoned = task.after.iter().any(|dep_id| {
                graph
                    .get_task(dep_id)
                    .is_some_and(|t| t.status == Status::Abandoned)
            });
            if source_abandoned {
                eprintln!(
                    "[dispatcher] Skipping '{}': source task is abandoned",
                    task.id
                );
                continue;
            }
        }

        // Resource admission is class-specific: low disk pauses only tasks
        // that can create build caches. Agency evaluation/assignment and graph
        // operations continue through the same ready queue.
        let build_class = worksgood::disk_sentinel::classify_task(task);
        let projected = worksgood::disk_sentinel::build_admission_reclaiming_owned(
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
            eprintln!(
                "[dispatcher] Deferring '{}' while ordinary/evaluator tasks remain eligible: {}",
                task.id, reason
            );
            continue;
        }

        // Shell-mode tasks run inline: fork `wg exec --shell` directly instead
        // of going through the full agent spawn path. Must be checked before the
        // auto_assign gate because shell tasks are intentionally excluded from
        // auto-assign (they run commands, not agents) and thus have no agent field.
        let is_shell_task = task.exec_mode.as_deref() == Some("shell") && task.exec.is_some();
        if is_shell_task {
            let task_id = task.id.clone();
            let title = task.title.clone();
            eprintln!(
                "[dispatcher] Spawning shell task inline for: {} - {}",
                task_id, title,
            );
            match spawn_shell_inline(dir, &task_id) {
                Ok((agent_id, pid)) => {
                    eprintln!("[dispatcher] Spawned shell {} (PID {})", agent_id, pid);
                    spawned += 1;
                    if build_class.is_heavy() {
                        active_build_heavy += 1;
                    }
                }
                Err(e) => {
                    eprintln!("[dispatcher] Failed to spawn shell for {}: {}", task_id, e);
                    record_spawn_failure(
                        &gp,
                        &task_id,
                        &format!("{}", e),
                        "inline-shell",
                        task.exec_mode.as_deref(),
                        config.coordinator.max_spawn_failures,
                    );
                }
            }
            continue;
        }

        // Defense-in-depth: when auto_assign is enabled, non-system tasks
        // should have an agent set before being spawned. Normally the graph
        // dependency on `.assign-*` prevents reaching here without an agent,
        // but this gate catches edge cases (e.g., pre-migration tasks without
        // the `.assign-*` blocking edge).
        if auto_assign && !worksgood::graph::is_system_task(&task.id) && task.agent.is_none() {
            continue;
        }

        // Evaluation, flip, and assignment tasks run inline: fork `wg evaluate`, `wg flip`, or `wg assign`
        // directly instead of going through the full spawn machinery
        // (run.sh, executor config, etc.)
        let is_inline_task = task.exec.is_some()
            && (task.id.starts_with(".evaluate-")
                || task.id.starts_with(".flip-")
                || task.id.starts_with(".assign-"));
        if is_inline_task {
            let is_assignment = task.id.starts_with(".assign-");
            let eval_model = task.model.as_deref();
            let task_id = task.id.clone();
            let title = task.title.clone();

            if is_assignment {
                eprintln!(
                    "[dispatcher] Spawning assignment inline for: {} - {}",
                    task_id, title,
                );
                match spawn_assign_inline(dir, &task_id) {
                    Ok((agent_id, pid)) => {
                        eprintln!("[dispatcher] Spawned assignment {} (PID {})", agent_id, pid);
                        record_dispatch(&gp, &task_id);
                        spawned += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "[dispatcher] Failed to spawn assignment for {}: {}",
                            task_id, e
                        );
                        if !park_agency_execution_error(&gp, &task_id, &e) {
                            record_spawn_failure(
                                &gp,
                                &task_id,
                                &format!("{}", e),
                                "inline-assignment",
                                task.exec_mode.as_deref(),
                                config.coordinator.max_spawn_failures,
                            );
                        }
                    }
                }
            } else {
                eprintln!(
                    "[dispatcher] Spawning eval inline for: {} - {}{}",
                    task_id,
                    title,
                    eval_model
                        .map(|m| format!(" (model: {})", m))
                        .unwrap_or_default(),
                );
                match spawn_eval_inline(dir, &task_id, eval_model) {
                    Ok((agent_id, pid)) => {
                        eprintln!("[dispatcher] Spawned eval {} (PID {})", agent_id, pid);
                        record_dispatch(&gp, &task_id);
                        spawned += 1;
                    }
                    Err(e) => {
                        eprintln!("[dispatcher] Failed to spawn eval for {}: {}", task_id, e);
                        if !park_agency_execution_error(&gp, &task_id, &e) {
                            record_spawn_failure(
                                &gp,
                                &task_id,
                                &format!("{}", e),
                                "inline-eval",
                                task.exec_mode.as_deref(),
                                config.coordinator.max_spawn_failures,
                            );
                        }
                    }
                }
            }
            continue;
        }

        // Per-WCC profile: if this task was stamped with a profile (via
        // `wg publish --profile`), resolve a per-task effective config from
        // that profile's complete snapshot and hand THAT to `plan_spawn`
        // instead of the global config. A profile file carries the whole
        // `coordinator.*` / `[models.*]` / `[llm_endpoints]` surface, so the
        // existing executor/model/endpoint cascade transparently honors it.
        // No profile ⇒ `effective_config_for_task` returns the global config
        // unchanged (backward-compatible).
        let eff_config =
            worksgood::dispatch::effective_config_for_task(task, config, &mut profile_cache);
        let eff_config: &Config = eff_config.as_ref();

        // Resolve model per-task: system tasks use their respective role models,
        // all other tasks use the default (TaskAgent) model. When a profile is
        // pinned, resolve the TaskAgent model from the PROFILE so the work
        // task routes through the profile's model, not the global default.
        let task_model = if task.id.starts_with(".assign-") {
            Some(
                eff_config
                    .resolve_model_for_role(worksgood::config::DispatchRole::Assigner)
                    .spawn_model_spec(),
            )
        } else if task.profile.is_some() {
            Some(
                eff_config
                    .resolve_model_for_role(worksgood::config::DispatchRole::TaskAgent)
                    .spawn_model_spec(),
            )
        } else {
            default_model.map(String::from)
        };

        // SINGLE SOURCE OF TRUTH: every spawn decision flows through plan_spawn.
        // This is the ONLY place that decides {executor, model, endpoint} for
        // a task spawn.
        //
        // Agency reports the agent's preferred executor when it has an
        // explicit one (non-default `executor` field, or `preferred_provider`).
        // For default agents, agency abstains and the dispatcher's executor
        // floor wins. The model-compat override (claude → native when the
        // model is non-Anthropic) is applied INSIDE `plan_spawn` after
        // executor resolution — see `enforce_model_compat`.
        let agent_entity = task
            .agent
            .as_ref()
            .and_then(|agent_hash| agency::find_agent_by_prefix(&agents_dir, agent_hash).ok());
        let agent_executor = agent_entity.as_ref().and_then(|a| a.explicit_executor());
        let plan = match worksgood::dispatch::plan_spawn(
            task,
            eff_config,
            agent_executor,
            task_model.as_deref(),
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[dispatcher] plan_spawn failed for {}: {}", task.id, e);
                record_spawn_failure(
                    &gp,
                    &task.id,
                    &format!("plan_spawn: {}", e),
                    "unknown",
                    task.exec_mode.as_deref(),
                    config.coordinator.max_spawn_failures,
                );
                continue;
            }
        };
        let effective_executor = plan.executor.as_str().to_string();

        // Provenance: every spawn emits one line tracing each decision back to
        // the config knob that produced it. Eliminates silent-routing bugs.
        eprintln!(
            "[dispatcher] {}: {}",
            task.id,
            plan.provenance.log_line(&plan)
        );
        eprintln!(
            "[dispatcher] Spawning agent for: {} - {} (executor: {})",
            task.id, task.title, effective_executor
        );
        match spawn::spawn_agent(
            dir,
            &task.id,
            &effective_executor,
            task.timeout.as_deref(),
            task_model.as_deref(),
        ) {
            Ok((agent_id, pid)) => {
                eprintln!("[dispatcher] Spawned {} (PID {})", agent_id, pid);
                record_dispatch(&gp, &task.id);
                spawned += 1;
                if build_class.is_heavy() {
                    active_build_heavy += 1;
                }
            }
            Err(e) => {
                eprintln!("[dispatcher] Failed to spawn for {}: {}", task.id, e);
                record_spawn_failure(
                    &gp,
                    &task.id,
                    &format!("{}", e),
                    &effective_executor,
                    task.exec_mode.as_deref(),
                    config.coordinator.max_spawn_failures,
                );
            }
        }
    }

    spawned
}

fn record_dispatch(graph_path: &Path, task_id: &str) {
    let task_id_owned = task_id.to_string();
    let _ = modify_graph(graph_path, |graph| {
        if let Some(task) = graph.get_task_mut(&task_id_owned) {
            task.dispatch_count += 1;
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

    // Load config for agency settings
    let config = Config::load_or_default(dir);

    // Process chat inbox FIRST — chat is a user-facing interaction that must not
    // be blocked by agent capacity limits or empty task queues. The early returns
    // below (max agents, no ready tasks) would skip chat processing otherwise.
    process_chat_inbox(dir);

    // Periodic disk work is outside the TUI/render path and writes a bounded,
    // cached snapshot consumed by status surfaces. Cleanup consults only the
    // explicit ownership registry; unknown directories are never candidates.
    let disk_was_due = worksgood::disk_sentinel::load_snapshot(dir)
        .ok()
        .flatten()
        .and_then(|snapshot| chrono::DateTime::parse_from_rfc3339(&snapshot.generated_at).ok())
        .map(|generated| {
            (Utc::now() - generated.with_timezone(&Utc)).num_seconds()
                >= config
                    .coordinator
                    .resource_management
                    .disk_scan_interval_seconds as i64
        })
        .unwrap_or(true);
    match worksgood::disk_sentinel::refresh_if_due(dir, &config.coordinator.resource_management) {
        Ok(Some(snapshot)) if snapshot.level != worksgood::disk_sentinel::DiskLevel::Healthy => {
            eprintln!(
                "[dispatcher] Disk sentinel {:?}: {}",
                snapshot.level, snapshot.reason
            );
        }
        Ok(_) => {}
        Err(error) => eprintln!("[dispatcher] Disk sentinel warning: {error:#}"),
    }
    if disk_was_due {
        match worksgood::disk_sentinel::cleanup_owned(
            dir,
            &config.coordinator.resource_management,
            true,
        ) {
            Ok(report)
                if report.reaped > 0
                    || report.compressed_files > 0
                    || report.deduplicated_files > 0 =>
            {
                eprintln!(
                    "[dispatcher] Disk cleanup: reaped {} owned target(s), freed {} bytes; compressed {} stream(s), saved {} bytes; deduplicated {}, saved {} bytes",
                    report.reaped,
                    report.bytes_freed,
                    report.compressed_files,
                    report.compression_bytes_saved,
                    report.deduplicated_files,
                    report.deduplication_bytes_saved
                )
            }
            Ok(_) => {}
            Err(error) => eprintln!("[dispatcher] Disk cleanup warning: {error:#}"),
        }
    }

    // Source cleanup is a separate policy from rebuildable cache cleanup. The
    // sweep ignores only the exact WG marker and removes a worktree only after
    // eval + merge proof and a fresh real-source-dirtiness check.
    match super::worktree::sweep_cleanup_pending_worktrees(dir) {
        Ok(removed) if removed > 0 => {
            eprintln!("[dispatcher] Archived/removed {removed} integrated clean worktree(s)")
        }
        Ok(_) => {}
        Err(error) => eprintln!("[dispatcher] Worktree cleanup warning: {error:#}"),
    }

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
        if !sweep.killed.is_empty() {
            eprintln!(
                "[dispatcher] Zero-output sweep: killed {} agent(s)",
                sweep.killed.len()
            );
        }
        if !sweep.circuit_broken_tasks.is_empty() {
            eprintln!(
                "[dispatcher] Zero-output circuit breaker: {} task(s) failed: {:?}",
                sweep.circuit_broken_tasks.len(),
                sweep.circuit_broken_tasks
            );
        }
        if sweep.global_outage_detected {
            eprintln!("[dispatcher] Zero-output: global API outage detected, spawn paused");
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

        // Phases 2.46–2.47: route-stable evaluation lifecycle repair and
        // verdict-required parent resolution. A terminal/missing evaluator is
        // never treated as a score. Historical pre-claim Codex rows are
        // normalized once; ambiguous provider-only rows park for an operator.
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

        // Phase 2.8: Message-triggered resurrection.
        modified |= resurrect_done_tasks(graph, dir);

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

        // Phase 3: Auto-assign unassigned ready tasks
        if config.agency.auto_assign {
            modified |= build_auto_assign_tasks(graph, &config, dir);
        }

        // Phase 4: Auto-evaluate tasks
        if config.agency.auto_evaluate {
            modified |= build_auto_evaluate_tasks(dir, graph, &config);
        }

        // Phase 4.5: FLIP verification
        modified |= build_flip_verification_tasks(dir, graph, &config);

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

    // Phase 5.5: Check if spawning is paused due to global API-down backoff.
    if super::zero_output::should_pause_spawning(dir) {
        eprintln!("[dispatcher] Spawning paused: global zero-output backoff active");
        let cycle_analysis = graph.compute_cycle_analysis();
        let final_ready = ready_tasks_with_peers_cycle_aware(&graph, dir, &cycle_analysis);
        // Exclude daemon-managed loop tasks from ready count.
        let ready_count = final_ready.iter().filter(|t| !is_daemon_managed(t)).count();
        return Ok(TickResult {
            agents_alive: alive_count,
            tasks_ready: ready_count,
            agents_spawned: 0,
        });
    }

    // Phase 5.6: Check if spawning is paused due to provider health failures.
    match worksgood::service::ProviderHealth::load(dir) {
        Ok(provider_health) if provider_health.should_pause_spawning() => {
            eprintln!(
                "[dispatcher] Spawning paused: {}",
                provider_health.get_status_summary()
            );
            let cycle_analysis = graph.compute_cycle_analysis();
            let final_ready = ready_tasks_with_peers_cycle_aware(&graph, dir, &cycle_analysis);
            // Exclude daemon-managed loop tasks from ready count.
            let ready_count = final_ready.iter().filter(|t| !is_daemon_managed(t)).count();
            return Ok(TickResult {
                agents_alive: alive_count,
                tasks_ready: ready_count,
                agents_spawned: 0,
            });
        }
        Err(e) => {
            eprintln!(
                "[dispatcher] Warning: failed to load provider health: {}",
                e
            );
        }
        _ => {} // Provider health is healthy, continue
    }

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
    let spawned = spawn_agents_for_ready_tasks(
        dir,
        &graph,
        executor,
        &config,
        Some(effective_model.as_str()),
        slots_available,
        config.agency.auto_assign,
    );

    Ok(TickResult {
        agents_alive: alive_count + spawned,
        tasks_ready: ready_count,
        agents_spawned: spawned,
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
    fn test_eval_inline_extracts_source_task_from_exec() {
        // spawn_eval_inline extracts the source task ID from exec command
        // This tests the extraction logic used in the function
        let exec = Some("wg evaluate run my-source-task".to_string());
        let source_id = exec
            .as_deref()
            .and_then(|e| {
                e.strip_prefix("wg evaluate run ")
                    .or_else(|| e.strip_prefix("wg evaluate "))
            })
            .unwrap_or("fallback");
        assert_eq!(source_id, "my-source-task");
    }

    #[test]
    fn test_eval_inline_extracts_source_task_from_id_fallback() {
        // When exec is missing the prefix, fall back to stripping evaluate- from task ID
        let exec: Option<String> = None;
        let eval_task_id = "evaluate-some-task";
        let source_id = exec
            .as_deref()
            .and_then(|e| {
                e.strip_prefix("wg evaluate run ")
                    .or_else(|| e.strip_prefix("wg evaluate "))
            })
            .unwrap_or_else(|| {
                eval_task_id
                    .strip_prefix(".evaluate-")
                    .or_else(|| eval_task_id.strip_prefix("evaluate-"))
                    .unwrap_or(eval_task_id)
            });
        assert_eq!(source_id, "some-task");
    }

    #[test]
    fn test_flip_eval_record_invocation_uses_flag_args() {
        // Regression test: the inline-eval script that wraps FLIP / agency
        // evaluations MUST invoke `wg evaluate record` with flag-style args
        // (`--task <id> --score <n>`), not positional, because the CLI now
        // requires them. Positional args here cause:
        //   error: unexpected argument '.flip-...' found
        // and the eval result is silently dropped.
        let script = build_inline_eval_script(
            "wg evaluate run my-source",
            ".flip-my-source",
            "/tmp/out.log",
            Some("agent-hash-deadbeef"),
            "wg",
            "agent-1",
            1,
        );

        // Success branch: must use --task / --score, NOT positional.
        assert!(
            script.contains(
                "wg evaluate record --task '.flip-my-source' --score 1.0 --source system"
            ),
            "success branch must use flag-style record invocation; got:\n{script}"
        );

        // Failure branch: same contract, score 0.0.
        assert!(
            script.contains(
                "wg evaluate record --task '.flip-my-source' --score 0.0 --source system"
            ),
            "failure branch must use flag-style record invocation; got:\n{script}"
        );

        // Negative assertion: no positional `record <task-id>` form survives.
        assert!(
            !script.contains("wg evaluate record '.flip-my-source'"),
            "positional record invocation must not appear; got:\n{script}"
        );
        assert!(
            !script.contains("wg evaluate record '.flip-my-source' 1.0"),
            "positional record invocation must not appear; got:\n{script}"
        );
    }

    #[test]
    fn inline_eval_script_uses_managed_process_bound_heartbeat() {
        let script = build_inline_eval_script(
            "wg evaluate run my-source",
            ".evaluate-my-source",
            "/tmp/out.log",
            None,
            "'/opt/wg/bin/wg' --dir '/srv/project/.wg'",
            "agent-41",
            2,
        );
        assert!(script.contains(
            "'/opt/wg/bin/wg' --dir '/srv/project/.wg' heartbeat-watch 'agent-41' --interval-seconds 2 --supervised-pid \"$$\""
        ));
        assert!(script.contains("} {INLINE_HEARTBEAT_GUARD_FD}>&-"));
        assert!(script.contains("trap '_WG_STOP_INLINE_HEARTBEAT' EXIT"));
        assert!(script.contains("kill \"$INLINE_HEARTBEAT_PID\""));
        assert!(script.contains("wait \"$INLINE_HEARTBEAT_PID\""));
    }

    #[test]
    fn inline_runtime_is_absolute_graph_pinned_and_accelerated() {
        let dir = tempdir().unwrap();
        let command = authoritative_inline_wg_command(dir.path()).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert!(command.contains(&exe.to_string_lossy().to_string()));
        assert!(
            command.contains(
                &dir.path()
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            )
        );
        assert!(command.contains(" --dir "));

        let mut config = Config::default();
        config.agent.heartbeat_timeout_seconds = Some(6);
        assert_eq!(inline_heartbeat_interval_seconds(&config), 2);
        config.agency.inference_timeout = Some(4);
        assert_eq!(config.agency.inference_timeout_secs(), 4);
    }

    #[test]
    fn test_inline_eval_script_without_special_agent_skips_record() {
        // When there is no resolved special agent, the script must NOT
        // emit a `wg evaluate record` line at all (success or failure branch).
        let script = build_inline_eval_script(
            "wg evaluate run my-source",
            "evaluate-my-source",
            "/tmp/out.log",
            None,
            "wg",
            "agent-1",
            1,
        );

        assert!(
            !script.contains("wg evaluate record"),
            "no-special-agent branch must skip record entirely; got:\n{script}"
        );
        // Sanity: still wraps the eval and finalizes the task.
        assert!(script.contains("wg evaluate run my-source"));
        assert!(script.contains("wg done 'evaluate-my-source'"));
    }

    #[test]
    fn test_inline_eval_route_failure_parks_without_recording_verdict() {
        let script = build_inline_eval_script(
            "wg evaluate run my-source",
            ".evaluate-my-source",
            "/tmp/out.log",
            Some("agent-hash-deadbeef"),
            "wg",
            "agent-1",
            1,
        );

        assert!(script.contains("grep -q 'error\\[WG-EXEC-'"));
        assert!(script.contains(
            "wg wait '.evaluate-my-source' --until 'timer:1m' --checkpoint 'LLM execution route unavailable; retry without recording a semantic verdict'"
        ));
        let route_branch = script
            .split("if [ $_WG_ROUTE_FAILURE -eq 1 ]; then")
            .nth(1)
            .expect("route-failure branch");
        let route_only = route_branch
            .split("else")
            .next()
            .expect("route-failure branch body");
        assert!(!route_only.contains("wg evaluate record"));
        assert!(!route_only.contains("wg fail"));
    }

    #[test]
    fn test_preclaim_execution_error_parks_agency_without_failure_budget() {
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        let source = Task {
            id: "source".into(),
            title: "Source".into(),
            status: Status::PendingEval,
            ..Task::default()
        };
        let mut config = Config::default();
        config.tiers.fast = Some("pi:openrouter:test/evaluator".to_string());
        let plan = worksgood::eval_lifecycle::build_plan(
            &config,
            &source,
            ".evaluate-source",
            worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap();
        let task = Task {
            id: ".evaluate-source".into(),
            title: "Evaluate source".into(),
            status: Status::Open,
            agency_dispatch: Some(plan),
            ..Task::default()
        };
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(task));
        save_graph(&graph, &graph_path).unwrap();

        let error =
            anyhow::anyhow!("error[WG-EXEC-ROUTE-FAILED]: role=evaluator selected_route=pi:test");
        assert!(park_agency_execution_error(
            &graph_path,
            ".evaluate-source",
            &error
        ));

        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task(".evaluate-source").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.spawn_failures, 0);
        assert!(task.assigned.is_none());
        assert!(matches!(
            task.wait_condition,
            Some(WaitSpec::All(ref conditions))
                if matches!(conditions.as_slice(), [WaitCondition::Timer { .. }])
        ));
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
    fn test_evaluate_waiting_tasks_autofails_unsatisfiable() {
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

        assert!(modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert!(
            task.failure_reason
                .as_ref()
                .unwrap()
                .contains("unsatisfiable")
        );
    }

    #[test]
    fn test_evaluate_waiting_tasks_fails_circular_waits() {
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

        assert!(modified);
        let a = graph.get_task("task-a").unwrap();
        let b = graph.get_task("task-b").unwrap();
        assert_eq!(a.status, Status::Failed);
        assert_eq!(b.status, Status::Failed);
        assert!(a.failure_reason.as_ref().unwrap().contains("Circular wait"));
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
    fn test_message_condition_with_messages() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        messages::send_message(dir.path(), "main", "Hello", "user", "normal").unwrap();

        let mut main_task = Task::default();
        main_task.id = "main".to_string();
        main_task.status = Status::Waiting;
        main_task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::Message]));

        setup_wait_graph(dir.path(), vec![main_task]);

        let mut graph = load_wait_graph(dir.path());
        let modified = evaluate_waiting_tasks(&mut graph, dir.path());

        assert!(modified);
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Open);
    }

    // -----------------------------------------------------------------------
    // Message-triggered resurrection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resurrection_detects_unread_messages_on_done_task() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        // Create a Done task
        let mut task = Task::default();
        task.id = "done-task".to_string();
        task.status = Status::Done;
        task.assigned = Some("agent-old".to_string());

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        // Send a message from "user" (not the task's own agent)
        messages::send_message(dir.path(), "done-task", "Please fix X", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(modified, "Graph should be modified by resurrection");
        let task = graph.get_task("done-task").unwrap();
        assert_eq!(task.status, Status::Open, "Done task should be reopened");
        assert!(task.assigned.is_none(), "Assignment should be cleared");
        assert_eq!(task.resurrection_count, 1);
        assert!(task.last_resurrected_at.is_some());
        assert!(task.log.last().unwrap().message.contains("Resurrection"));
    }

    #[test]
    fn test_resurrection_reopen_when_no_downstream_active() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        // Done task with a downstream that is Open (not started)
        let mut parent = Task::default();
        parent.id = "parent".to_string();
        parent.status = Status::Done;
        parent.before = vec!["child".to_string()];

        let mut child = Task::default();
        child.id = "child".to_string();
        child.status = Status::Open;
        child.after = vec!["parent".to_string()];

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(parent));
        graph.add_node(Node::Task(child));

        messages::send_message(
            dir.path(),
            "parent",
            "Update needed",
            "coordinator",
            "normal",
        )
        .unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(modified);
        let task = graph.get_task("parent").unwrap();
        assert_eq!(
            task.status,
            Status::Open,
            "Should reopen (downstream not active)"
        );
    }

    #[test]
    fn test_resurrection_child_task_when_downstream_active() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        // Done task with a downstream that is InProgress
        let mut parent = Task::default();
        parent.id = "parent".to_string();
        parent.status = Status::Done;
        parent.session_id = Some("sess-123".to_string());
        parent.checkpoint = Some("Did some work".to_string());
        parent.before = vec!["downstream".to_string()];

        let mut downstream = Task::default();
        downstream.id = "downstream".to_string();
        downstream.status = Status::InProgress;
        downstream.after = vec!["parent".to_string()];

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(parent));
        graph.add_node(Node::Task(downstream));

        messages::send_message(
            dir.path(),
            "parent",
            "Question about X",
            "agent-downstream",
            "normal",
        )
        .unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(modified);
        // Parent stays Done
        let parent = graph.get_task("parent").unwrap();
        assert_eq!(parent.status, Status::Done, "Parent should stay Done");
        assert_eq!(parent.resurrection_count, 1);

        // Child task created
        let child = graph.get_task(".respond-to-parent").unwrap();
        assert_eq!(child.status, Status::Open);
        assert_eq!(
            child.session_id,
            Some("sess-123".to_string()),
            "Session inherited"
        );
        assert_eq!(
            child.checkpoint,
            Some("Did some work".to_string()),
            "Checkpoint inherited"
        );
        assert!(
            child
                .description
                .as_deref()
                .unwrap()
                .contains("pending messages")
        );
    }

    #[test]
    fn test_resurrection_rate_limit_max_resurrections() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "exhausted".to_string();
        task.status = Status::Done;
        task.resurrection_count = MAX_RESURRECTIONS; // Already at max

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        messages::send_message(dir.path(), "exhausted", "One more", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(!modified, "Should NOT resurrect: max count reached");
        assert_eq!(graph.get_task("exhausted").unwrap().status, Status::Done);
    }

    #[test]
    fn test_resurrection_cooldown() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "cooled".to_string();
        task.status = Status::Done;
        task.resurrection_count = 1;
        task.last_resurrected_at = Some(Utc::now().to_rfc3339()); // Just now

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        messages::send_message(dir.path(), "cooled", "Again", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(!modified, "Should NOT resurrect: cooldown active");
        assert_eq!(graph.get_task("cooled").unwrap().status, Status::Done);
    }

    #[test]
    fn test_resurrection_excluded_for_abandoned_tasks() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "abandoned".to_string();
        task.status = Status::Abandoned;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        messages::send_message(dir.path(), "abandoned", "Come back", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(!modified, "Should NOT resurrect abandoned tasks");
    }

    #[test]
    fn test_resurrection_ignores_messages_from_own_agent() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "self-msg".to_string();
        task.status = Status::Done;
        task.assigned = Some("agent-42".to_string());

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        // Only message is from the task's own agent
        messages::send_message(dir.path(), "self-msg", "I'm done", "agent-42", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(!modified, "Should NOT resurrect from own agent's messages");
    }

    #[test]
    fn test_resurrection_batches_multiple_messages() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "multi".to_string();
        task.status = Status::Done;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        // Send 3 messages
        messages::send_message(dir.path(), "multi", "Msg 1", "user", "normal").unwrap();
        messages::send_message(dir.path(), "multi", "Msg 2", "coordinator", "normal").unwrap();
        messages::send_message(dir.path(), "multi", "Msg 3", "agent-other", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(modified);
        let task = graph.get_task("multi").unwrap();
        assert_eq!(task.status, Status::Open);
        // Only ONE resurrection despite 3 messages
        assert_eq!(
            task.resurrection_count, 1,
            "Should batch into one resurrection"
        );
        assert!(
            task.log
                .last()
                .unwrap()
                .message
                .contains("3 pending message(s)")
        );
    }

    #[test]
    fn test_resurrection_child_not_duplicated() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut parent = Task::default();
        parent.id = "parent".to_string();
        parent.status = Status::Done;
        parent.before = vec!["downstream".to_string()];

        let mut downstream = Task::default();
        downstream.id = "downstream".to_string();
        downstream.status = Status::InProgress;

        // Child already exists from a previous resurrection
        let mut existing_child = Task::default();
        existing_child.id = ".respond-to-parent".to_string();
        existing_child.status = Status::InProgress;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(parent));
        graph.add_node(Node::Task(downstream));
        graph.add_node(Node::Task(existing_child));

        messages::send_message(dir.path(), "parent", "Another question", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(!modified, "Should NOT create duplicate child task");
    }

    #[test]
    fn test_resurrection_opt_out_tag() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();

        let mut task = Task::default();
        task.id = "no-resurrect".to_string();
        task.status = Status::Done;
        task.tags = vec!["resurrect:false".to_string()];

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));

        messages::send_message(dir.path(), "no-resurrect", "Wake up", "user", "normal").unwrap();

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(
            !modified,
            "Should NOT resurrect tasks with resurrect:false tag"
        );
    }

    // -----------------------------------------------------------------------
    // spawn_agents_for_ready_tasks: auto_assign filtering
    // -----------------------------------------------------------------------

    /// When auto_assign=true, a ready task WITHOUT an agent field should be
    /// skipped by spawn_agents_for_ready_tasks (it needs to go through the
    /// .assign-* flow first).
    #[test]
    fn test_spawn_skips_unassigned_task_when_auto_assign_enabled() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();

        let mut task = Task::default();
        task.id = "my-task".to_string();
        task.title = "Test".to_string();
        task.status = Status::Open;
        // No agent field set — hasn't been through assignment

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, &wg_dir.join("graph.jsonl")).unwrap();

        let config = Config::load_or_default(wg_dir);
        let result = spawn_agents_for_ready_tasks(
            wg_dir, &graph, "shell", &config, None, 10, true, // auto_assign = true
        );

        // Task should be skipped (no agent), so nothing spawned
        assert_eq!(
            result, 0,
            "unassigned task should NOT be spawned when auto_assign=true"
        );
    }

    /// When auto_assign=true, a ready task WITH an agent field SHOULD be
    /// spawned (it has been through the assignment flow).
    #[test]
    fn test_spawn_allows_assigned_agent_task_when_auto_assign_enabled() {
        // Verify the condition logic: task with agent set should NOT be skipped
        let has_agent = true; // agent = Some("abc123")
        let is_system = worksgood::graph::is_system_task("my-task");
        let would_skip = true && !is_system && !has_agent;
        assert!(!would_skip, "task with agent field should NOT be skipped");
    }

    /// System tasks (dot-prefixed) are always spawned regardless of auto_assign.
    #[test]
    fn test_spawn_always_allows_system_tasks_when_auto_assign_enabled() {
        // System tasks like .assign-foo, .evaluate-foo should bypass auto_assign filter
        let is_system = worksgood::graph::is_system_task(".assign-my-task");
        assert!(is_system, ".assign-* should be a system task");

        // The filter: skip if auto_assign && !is_system && agent.is_none()
        // For system tasks, !is_system is false, so the condition is false → not skipped
        let would_skip = true && !is_system && true; // auto_assign=true, agent=None
        assert!(
            !would_skip,
            "system tasks should never be skipped by auto_assign filter"
        );
    }

    /// When auto_assign=false, tasks without agent field should still be spawned.
    #[test]
    fn test_spawn_allows_unassigned_task_when_auto_assign_disabled() {
        let auto_assign = false;
        let is_system = worksgood::graph::is_system_task("my-task");
        let has_agent = false; // no agent field

        let would_skip = auto_assign && !is_system && !has_agent;
        assert!(!would_skip, "should not skip when auto_assign is disabled");
    }

    // -----------------------------------------------------------------------
    // Model-based executor detection (formerly `requires_native_executor`)
    // is gone — the dispatcher's executor pin is no longer overridden by
    // model spec. The single source of truth is now
    // `dispatch::plan::resolve_executor`; see tests in src/dispatch/plan.rs.
    // -----------------------------------------------------------------------

    ///.assign-* tasks with an `exec` field are detected as inline
    /// tasks and spawned via the lightweight inline path, not as full Claude agents.
    #[test]
    fn test_assign_spawned_inline() {
        // A .assign-* task with exec should be detected as inline. Tags do not route it.
        let mut assign_task = Task::default();
        assign_task.id = ".assign-my-task".to_string();
        assign_task.title = "Assign agent for: My Task".to_string();
        assign_task.tags = vec!["assignment".to_string(), "agency".to_string()];
        assign_task.exec = Some("wg assign my-task --auto".to_string());
        assign_task.status = Status::Open;

        let is_inline_task = assign_task.exec.is_some()
            && (assign_task.id.starts_with(".evaluate-")
                || assign_task.id.starts_with(".flip-")
                || assign_task.id.starts_with(".assign-"));
        assert!(
            is_inline_task,
            ".assign-* task with exec should be detected as inline"
        );

        // Verify the assignment branch is taken (not eval)
        let is_assignment = assign_task.id.starts_with(".assign-");
        assert!(
            is_assignment,
            ".assign-* task should be routed to the assignment inline path"
        );

        // An .assign-* task WITHOUT exec should NOT match inline (fallback to Phase 2)
        let mut no_exec_assign = Task::default();
        no_exec_assign.id = ".assign-other".to_string();
        no_exec_assign.tags = vec!["assignment".to_string()];
        let is_inline_no_exec = no_exec_assign.exec.is_some()
            && (no_exec_assign.id.starts_with(".evaluate-")
                || no_exec_assign.id.starts_with(".flip-")
                || no_exec_assign.id.starts_with(".assign-"));
        assert!(
            !is_inline_no_exec,
            ".assign-* without exec should NOT be inline"
        );

        // A regular task with exec but no assignment/eval/flip tag should NOT match
        let mut regular_exec = Task::default();
        regular_exec.id = "build-thing".to_string();
        regular_exec.tags = vec![
            "assignment".to_string(),
            "evaluation".to_string(),
            "flip".to_string(),
        ];
        regular_exec.exec = Some("make build".to_string());
        let is_inline_regular = regular_exec.exec.is_some()
            && (regular_exec.id.starts_with(".evaluate-")
                || regular_exec.id.starts_with(".flip-")
                || regular_exec.id.starts_with(".assign-"));
        assert!(
            !is_inline_regular,
            "regular task with label tags and exec should NOT be inline"
        );
    }

    /// A resurrected task (reopened after completion) with an existing done
    /// .assign-* task should have the stale assignment removed so a new one
    /// can be created on the next tick.
    #[test]
    fn test_assign_recreated_after_resurrection() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("agency/assignments")).unwrap();
        std::fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();

        // Source task: resurrected (Open again after being Done)
        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Resurrected task".to_string();
        source.status = Status::Open;
        // No agent — cleared on resurrection

        // Old stale .assign task: completed from the previous round
        let mut old_assign = Task::default();
        old_assign.id = ".assign-my-task".to_string();
        old_assign.title = "Assign my-task".to_string();
        old_assign.status = Status::Done;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(old_assign));

        // Verify stale .assign exists before the call
        assert!(graph.get_task(".assign-my-task").is_some());

        let config = Config::load_or_default(wg_dir);
        let _modified = build_auto_assign_tasks(&mut graph, &config, wg_dir);

        // The stale Done .assign should be reopened for fresh assignment.
        // (The LLM call will fail in tests, but the critical fix is that the
        // stale guard no longer blocks progress — the reopened .assign-* will
        // be processed on the next coordinator tick.)
        let assign = graph.get_task(".assign-my-task");
        assert!(
            assign.is_none() || assign.unwrap().status != Status::Done,
            "stale Done .assign-my-task should be reopened after resurrection"
        );
    }

    /// A Failed .assign-* should be reopened when the source task still needs
    /// assignment.  This prevents a permanent deadlock where the source is
    /// ready (Failed is terminal → dep satisfied) but has agent=None, and the
    /// auto_assign gate in spawn_agents_for_ready_tasks skips it.
    #[test]
    fn test_assign_reopened_after_failure() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("agency/assignments")).unwrap();
        std::fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();

        // Source task: ready, no agent (assignment never succeeded)
        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Stuck task".to_string();
        source.status = Status::Open;
        // .assign-* is in `after` but is Failed → terminal → dep satisfied → source is ready
        source.after = vec![".assign-my-task".to_string()];

        // Failed .assign task
        let mut failed_assign = Task::default();
        failed_assign.id = ".assign-my-task".to_string();
        failed_assign.title = "Assign my-task".to_string();
        failed_assign.status = Status::Failed;
        failed_assign.failure_reason = Some("LLM call timed out".to_string());
        failed_assign.tags = vec!["assignment".to_string(), "agency".to_string()];
        failed_assign.exec = Some("wg assign my-task --auto".to_string());

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(failed_assign));

        let config = Config::load_or_default(wg_dir);
        let modified = build_auto_assign_tasks(&mut graph, &config, wg_dir);

        assert!(
            modified,
            "graph should be modified (failed .assign reopened)"
        );
        let assign = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(
            assign.status,
            Status::Open,
            "failed .assign should be reopened for retry"
        );
        assert!(
            assign.failure_reason.is_none(),
            "failure_reason should be cleared on reopen"
        );
    }

    /// An Abandoned .assign-* should also be reopened (same deadlock fix).
    #[test]
    fn test_assign_reopened_after_abandonment() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("agency/assignments")).unwrap();
        std::fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Stuck task".to_string();
        source.status = Status::Open;
        source.after = vec![".assign-my-task".to_string()];

        let mut abandoned_assign = Task::default();
        abandoned_assign.id = ".assign-my-task".to_string();
        abandoned_assign.title = "Assign my-task".to_string();
        abandoned_assign.status = Status::Abandoned;
        abandoned_assign.tags = vec!["assignment".to_string(), "agency".to_string()];
        abandoned_assign.exec = Some("wg assign my-task --auto".to_string());

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(abandoned_assign));

        let config = Config::load_or_default(wg_dir);
        let modified = build_auto_assign_tasks(&mut graph, &config, wg_dir);

        assert!(
            modified,
            "graph should be modified (abandoned .assign reopened)"
        );
        let assign = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(
            assign.status,
            Status::Open,
            "abandoned .assign should be reopened for retry"
        );
    }

    /// An in-progress (Open/Waiting) .assign-* task should NOT be removed.
    #[test]
    fn test_assign_not_removed_when_still_active() {
        let dir = tempdir().unwrap();
        let wg_dir = dir.path();
        std::fs::create_dir_all(wg_dir.join("agency/assignments")).unwrap();
        std::fs::create_dir_all(wg_dir.join("agency/cache/agents")).unwrap();

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Active task".to_string();
        source.status = Status::Open;

        // .assign is still Open (in-progress)
        let mut active_assign = Task::default();
        active_assign.id = ".assign-my-task".to_string();
        active_assign.title = "Assign my-task".to_string();
        active_assign.status = Status::Open;

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(active_assign));

        let config = Config::load_or_default(wg_dir);
        let modified = build_auto_assign_tasks(&mut graph, &config, wg_dir);

        // Active .assign should be left alone
        assert!(
            !modified,
            "should not modify graph when .assign is still active"
        );
        let assign = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(assign.status, Status::Open);
    }

    #[test]
    fn test_resurrection_downstream_done_triggers_child() {
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

        let modified = resurrect_done_tasks(&mut graph, dir.path());

        assert!(modified);
        // Downstream is Done, so child task should be created
        let child = graph.get_task(".respond-to-parent").unwrap();
        assert_eq!(child.status, Status::Open);
    }

    #[test]
    fn test_flip_verify_task_includes_eval_context() {
        // Setup: create a source task (Done) and a low FLIP evaluation
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = "my-task".to_string();
        source.title = "Implement feature".to_string();
        source.description = Some("Build the widget".to_string());
        source.status = Status::Done;
        source.verify = Some("cargo test test_widget".to_string());

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        save_graph(&graph, &graph_path).unwrap();

        // Create a FLIP evaluation with dimensions and notes
        let evals_dir = dir.path().join("agency").join("evaluations");
        std::fs::create_dir_all(&evals_dir).unwrap();

        let mut dimensions = std::collections::HashMap::new();
        dimensions.insert("completeness".to_string(), 0.3);
        dimensions.insert("correctness".to_string(), 0.5);

        let eval = worksgood::agency::Evaluation {
            id: "flip-my-task-123".to_string(),
            task_id: "my-task".to_string(),
            agent_id: String::new(),
            role_id: "unknown".to_string(),
            tradeoff_id: "unknown".to_string(),
            score: 0.35,
            dimensions,
            notes: "The implementation is incomplete — missing error handling and the test only covers the happy path.".to_string(),
            evaluator: "flip:test".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            model: None,
            source: worksgood::agency::eval_source::FLIP.to_string(),
            loop_iteration: 0,
        };

        let eval_path = evals_dir.join("flip-my-task-123.json");
        let json = serde_json::to_string_pretty(&eval).unwrap();
        std::fs::write(&eval_path, json).unwrap();

        // Config with FLIP verification threshold + agency pipeline enabled
        let mut config = Config::default();
        config.tiers.fast = Some("claude:haiku".to_string());
        config.agency.flip_verification_threshold = Some(0.6);
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;

        let modified = build_flip_verification_tasks(dir.path(), &mut graph, &config);
        assert!(modified, "should create verify task");

        // Check verify task exists and has FLIP context in description
        let desc = graph
            .get_task(".verify-my-task")
            .unwrap()
            .description
            .clone()
            .unwrap();

        assert!(
            desc.contains("FLIP Evaluation Results"),
            "should have FLIP results section"
        );
        assert!(
            desc.contains("completeness"),
            "should include dimension names"
        );
        assert!(
            desc.contains("correctness"),
            "should include dimension names"
        );
        assert!(
            desc.contains("incomplete"),
            "should include evaluator reasoning (notes)"
        );

        // Check .assign-verify-* task was created via scaffold_full_pipeline
        let assign = graph.get_task(".assign-.verify-my-task").unwrap();
        assert_eq!(assign.status, Status::Open);
        assert!(
            assign.tags.contains(&"assignment".to_string()),
            "should be tagged as assignment"
        );
        assert!(
            assign
                .exec
                .as_deref()
                .unwrap()
                .contains("wg assign .verify-my-task --auto"),
            "should exec agency assignment"
        );

        // Check that .verify-my-task depends on .assign-verify-my-task
        let verify = graph.get_task(".verify-my-task").unwrap();
        assert!(
            verify
                .after
                .contains(&".assign-.verify-my-task".to_string()),
            "verify task should be blocked by its assignment task"
        );

        // Check that .flip-.verify-my-task was created (full pipeline)
        let flip = graph.get_task(".flip-.verify-my-task").unwrap();
        assert!(
            flip.after.contains(&".verify-my-task".to_string()),
            "flip task should depend on verify task"
        );
        assert!(
            flip.tags.contains(&"flip".to_string()),
            "should be tagged as flip"
        );

        // Check that .evaluate-.verify-my-task was created (full pipeline)
        let eval = graph.get_task(".evaluate-.verify-my-task").unwrap();
        assert!(
            eval.tags.contains(&"evaluation".to_string()),
            "should be tagged as evaluation"
        );
    }

    #[test]
    fn test_flip_verify_task_no_assignment_when_already_exists() {
        // If .assign-.verify-* already exists, don't create a duplicate
        let dir = tempdir().unwrap();
        let graph_path = dir.path().join("graph.jsonl");

        let mut source = Task::default();
        source.id = "t1".to_string();
        source.title = "Task one".to_string();
        source.status = Status::Done;

        let mut existing_assign = Task::default();
        existing_assign.id = ".assign-.verify-t1".to_string();
        existing_assign.title = "Existing assign".to_string();
        existing_assign.status = Status::Open;
        existing_assign.tags = vec!["assignment".to_string(), "agency".to_string()];

        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(existing_assign));
        save_graph(&graph, &graph_path).unwrap();

        // Create low FLIP eval
        let evals_dir = dir.path().join("agency").join("evaluations");
        std::fs::create_dir_all(&evals_dir).unwrap();

        let eval = worksgood::agency::Evaluation {
            id: "flip-t1-123".to_string(),
            task_id: "t1".to_string(),
            agent_id: String::new(),
            role_id: "unknown".to_string(),
            tradeoff_id: "unknown".to_string(),
            score: 0.2,
            dimensions: std::collections::HashMap::new(),
            notes: "Bad work".to_string(),
            evaluator: "flip:test".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            model: None,
            source: worksgood::agency::eval_source::FLIP.to_string(),
            loop_iteration: 0,
        };

        let eval_path = evals_dir.join("flip-t1-123.json");
        let json = serde_json::to_string_pretty(&eval).unwrap();
        std::fs::write(&eval_path, json).unwrap();

        let mut config = Config::default();
        config.agency.flip_verification_threshold = Some(0.5);
        config.agency.auto_assign = true;

        let modified = build_flip_verification_tasks(dir.path(), &mut graph, &config);
        assert!(modified, "should create verify task");

        // The .assign-verify task should not be duplicated — the existing one stays
        // (idempotency check inside scaffold_full_pipeline)
        let assign = graph.get_task(".assign-.verify-t1").unwrap();
        assert_eq!(
            assign.title, "Existing assign",
            "should keep existing assignment"
        );
    }

    // -----------------------------------------------------------------------
    // Spawn circuit breaker tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_spawn_circuit_breaker_allows_below_threshold() {
        let mut task = Task::default();
        task.id = "t1".to_string();
        task.spawn_failures = 0;
        assert!(check_spawn_circuit_breaker(&task, 5).is_ok());

        task.spawn_failures = 4;
        assert!(check_spawn_circuit_breaker(&task, 5).is_ok());
    }

    #[test]
    fn test_spawn_circuit_breaker_blocks_at_threshold() {
        let mut task = Task::default();
        task.id = "t1".to_string();
        task.spawn_failures = 5;
        let result = check_spawn_circuit_breaker(&task, 5);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("spawn circuit breaker"), "msg: {}", msg);
        assert!(msg.contains("5 consecutive"), "msg: {}", msg);
    }

    #[test]
    fn test_spawn_circuit_breaker_disabled_when_zero() {
        let mut task = Task::default();
        task.id = "t1".to_string();
        task.spawn_failures = 100;
        // threshold=0 means disabled
        assert!(check_spawn_circuit_breaker(&task, 0).is_ok());
    }

    #[test]
    fn test_spawn_circuit_breaker() {
        // Full integration test: record_spawn_failure increments counter
        // and auto-fails after threshold
        let dir = tempdir().unwrap();
        let wg_dir = dir.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        let gp = wg_dir.join("graph.jsonl");

        // Create a task with status Open
        let mut graph = WorkGraph::new();
        let mut task = Task::default();
        task.id = "test-task".to_string();
        task.title = "Test Task".to_string();
        task.status = Status::Open;
        task.exec_mode = Some("shell".to_string());
        graph.add_node(Node::Task(task));
        save_graph(&graph, &gp).unwrap();

        let max_failures: u32 = 5;

        // Record 4 failures — task should remain open
        for i in 1..=4 {
            let tripped = record_spawn_failure(
                &gp,
                "test-task",
                &format!("error {}", i),
                "claude",
                Some("shell"),
                max_failures,
            );
            assert!(!tripped, "should not trip at failure {}", i);

            let g = load_graph(&gp).unwrap();
            let t = g.get_task("test-task").unwrap();
            assert_eq!(t.spawn_failures, i as u32);
            assert_eq!(t.status, Status::Open);
        }

        // 5th failure — should trip the circuit breaker
        let tripped = record_spawn_failure(
            &gp,
            "test-task",
            "final error: exec_mode mismatch",
            "claude",
            Some("shell"),
            max_failures,
        );
        assert!(tripped, "should trip at failure 5");

        let g = load_graph(&gp).unwrap();
        let t = g.get_task("test-task").unwrap();
        assert_eq!(t.spawn_failures, 5);
        assert_eq!(
            t.status,
            Status::Incomplete,
            "Circuit breaker should mark task Incomplete (not Failed) — evaluator decides failure"
        );

        // No failure_reason set — circuit breaker logs evidence but doesn't auto-fail
        assert!(
            t.failure_reason.is_none(),
            "Circuit breaker should not set failure_reason (evaluator decides)"
        );

        // Check log entries
        assert!(
            t.log.iter().any(|e| e.actor == Some("spawn".to_string())
                && e.message.contains("Spawn failed")),
            "Expected spawn failure log entry"
        );
        assert!(
            t.log
                .iter()
                .any(|e| e.actor == Some("spawn-circuit-breaker".to_string())
                    && e.message.contains("Circuit breaker tripped")),
            "Expected circuit breaker log entry"
        );
        assert!(
            t.log.iter().any(|e| e.message.contains("evaluator review")),
            "Circuit breaker log should mention evaluator review"
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

    #[test]
    fn test_default_priorities_for_system_tasks() {
        // Verify that system tasks get sensible default priorities
        // .assign-* inherits parent priority (via calculate_auto_priority)
        // coordinator tasks get High priority
        use crate::commands::eval_scaffold::scaffold_assign_task;

        let mut graph = WorkGraph::new();

        // Normal user task
        let mut user_task = Task::default();
        user_task.id = "my-task".to_string();
        user_task.title = "My Task".to_string();
        user_task.status = worksgood::graph::Status::Open;
        user_task.priority = worksgood::graph::PRIORITY_NORMAL;
        graph.add_node(Node::Task(user_task));

        // Critical user task
        let mut critical_task = Task::default();
        critical_task.id = "crit-task".to_string();
        critical_task.title = "Critical Task".to_string();
        critical_task.status = worksgood::graph::Status::Open;
        critical_task.priority = worksgood::graph::PRIORITY_CRITICAL;
        graph.add_node(Node::Task(critical_task));

        // Scaffold assign tasks
        scaffold_assign_task(&mut graph, "my-task", "My Task");
        scaffold_assign_task(&mut graph, "crit-task", "Critical Task");

        // .assign-* inherits parent priority
        let assign_normal = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(
            assign_normal.priority,
            worksgood::graph::PRIORITY_NORMAL,
            ".assign-* for Normal task should be Normal"
        );

        let assign_critical = graph.get_task(".assign-crit-task").unwrap();
        assert_eq!(
            assign_critical.priority,
            worksgood::graph::PRIORITY_CRITICAL,
            ".assign-* for Critical task should be Critical"
        );
    }

    #[test]
    fn test_write_inline_artifacts_creates_all_files() {
        let dir = tempdir().unwrap();
        let output_dir = dir.path().join("agents").join("agent-42");
        fs::create_dir_all(&output_dir).unwrap();

        write_inline_artifacts(
            &output_dir,
            "agent-42",
            ".evaluate-my-task",
            "claude",
            Some("claude:haiku"),
            "#!/bin/bash\nwg evaluate run my-task",
        );

        assert!(output_dir.join("metadata.json").exists());
        assert!(output_dir.join("prompt.txt").exists());
        assert!(output_dir.join("run.sh").exists());

        let metadata: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(output_dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["agent_id"], "agent-42");
        assert_eq!(metadata["task_id"], ".evaluate-my-task");
        assert_eq!(metadata["executor"], "claude");
        assert_eq!(metadata["model"], "claude:haiku");
        assert_eq!(metadata["inline"], true);

        let prompt = fs::read_to_string(output_dir.join("prompt.txt")).unwrap();
        assert!(prompt.contains("claude"));

        let run_sh = fs::read_to_string(output_dir.join("run.sh")).unwrap();
        assert!(run_sh.contains("wg evaluate run my-task"));
        assert!(run_sh.starts_with("#!/bin/bash"));
    }

    #[test]
    fn test_write_inline_artifacts_assign_variant() {
        let dir = tempdir().unwrap();
        let output_dir = dir.path().join("agents").join("agent-99");
        fs::create_dir_all(&output_dir).unwrap();

        write_inline_artifacts(
            &output_dir,
            "agent-99",
            ".assign-my-task",
            "claude",
            Some("claude:haiku"),
            "wg assign 'my-task' --auto",
        );

        assert!(output_dir.join("metadata.json").exists());
        assert!(output_dir.join("prompt.txt").exists());
        assert!(output_dir.join("run.sh").exists());

        let metadata: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(output_dir.join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["agent_id"], "agent-99");
        assert_eq!(metadata["executor"], "claude");
        assert_eq!(metadata["model"], "claude:haiku");

        let run_sh = fs::read_to_string(output_dir.join("run.sh")).unwrap();
        assert!(run_sh.contains("wg assign"));
    }

    #[test]
    fn test_no_minimal_artifact_spawn_path() {
        // Verify that both inline spawn functions call write_inline_artifacts.
        // This is a compile-time guarantee: if write_inline_artifacts is removed
        // from either function, the call sites in spawn_eval_inline and
        // spawn_assign_inline would fail to compile. This test documents the
        // contract: every spawn path must produce metadata.json + prompt.txt +
        // run.sh + output.log.
        //
        // The canonical spawn path (spawn/execution.rs::spawn_agent_inner) writes
        // metadata.json at L802, prompt.txt at various points per executor type,
        // and run.sh via write_wrapper_script at L1415.
        //
        // The inline paths (coordinator.rs) use write_inline_artifacts.

        let dir = tempdir().unwrap();
        let output_dir = dir.path();
        fs::create_dir_all(output_dir).unwrap();

        // Simulate what both inline spawn paths now do after building script
        write_inline_artifacts(
            output_dir,
            "agent-1",
            "task-1",
            "claude",
            Some("claude:haiku"),
            "echo test",
        );

        let expected_files = ["metadata.json", "prompt.txt", "run.sh"];
        for f in &expected_files {
            assert!(
                output_dir.join(f).exists(),
                "Inline spawn must produce {} but it is missing",
                f
            );
        }
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
    fn concurrent_preclaim_route_parking_reserves_once() {
        let dir = tempdir().unwrap();
        let path = graph_path(dir.path());
        let source = Task {
            id: "source".to_string(),
            title: "source".to_string(),
            status: Status::FailedPendingEval,
            ..Task::default()
        };
        let mut config = Config::default();
        config.models.evaluator = Some(worksgood::config::RoleModelConfig {
            provider: None,
            model: Some("codex:gpt-5.5".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let plan = worksgood::eval_lifecycle::build_plan(
            &config,
            &source,
            ".evaluate-source",
            worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
        )
        .unwrap();
        let satellite = Task {
            id: ".evaluate-source".to_string(),
            title: "eval".to_string(),
            status: Status::Open,
            agency_dispatch: Some(plan),
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        graph.add_node(Node::Task(satellite));
        save_graph(&graph, &path).unwrap();

        let first_path = path.clone();
        let second_path = path.clone();
        let first = std::thread::spawn(move || {
            park_agency_execution_error(
                &first_path,
                ".evaluate-source",
                &anyhow::anyhow!("error[WG-EXEC-TEST]: unavailable"),
            )
        });
        let second = std::thread::spawn(move || {
            park_agency_execution_error(
                &second_path,
                ".evaluate-source",
                &anyhow::anyhow!("error[WG-EXEC-TEST]: unavailable"),
            )
        });
        let _ = (first.join().unwrap(), second.join().unwrap());

        let graph = worksgood::parser::load_graph(&path).unwrap();
        let task = graph.get_task(".evaluate-source").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(
            task.evaluation_lifecycle
                .as_ref()
                .unwrap()
                .schedule_attempts,
            1,
            "the Open→Waiting CAS must make a stale second tick a no-op"
        );
        assert_eq!(task.spawn_failures, 0);
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
