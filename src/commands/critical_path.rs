use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use worksgood::format_hours;
use worksgood::graph::{Status, WorkGraph};

/// Information about a task on the critical path
#[derive(Debug, Clone, Serialize)]
struct CriticalTask {
    id: String,
    title: String,
    status: Status,
    hours: Option<f64>,
    after: Option<String>,
}

/// Slack information for non-critical tasks
#[derive(Debug, Clone, Serialize)]
struct SlackInfo {
    id: String,
    title: String,
    slack_hours: f64,
    note: String,
}

/// JSON output structure
#[derive(Debug, Serialize)]
struct CriticalPathOutput {
    critical_path: Vec<CriticalTask>,
    task_count: usize,
    total_hours: f64,
    slack_analysis: Vec<SlackInfo>,
    cycles_skipped: Vec<Vec<String>>,
}

pub fn run(dir: &Path, json: bool) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;

    // Get active tasks only (exclude terminal states: done, failed, abandoned)
    let active_tasks: Vec<_> = graph.tasks().filter(|t| !t.status.is_terminal()).collect();

    if active_tasks.is_empty() {
        if json {
            let output = CriticalPathOutput {
                critical_path: vec![],
                task_count: 0,
                total_hours: 0.0,
                slack_analysis: vec![],
                cycles_skipped: vec![],
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("No active tasks found.");
        }
        return Ok(());
    }

    // Build set of active task IDs for filtering
    let active_ids: HashSet<&str> = active_tasks.iter().map(|t| t.id.as_str()).collect();

    // Detect cycles among active tasks
    let cycles = detect_cycles_among_active(&graph, &active_ids);
    let cycle_nodes: HashSet<&str> = cycles.iter().flatten().map(String::as_str).collect();

    // Build dependency graph (task_id -> list of tasks it blocks)
    // This is the "forward" direction for finding paths
    let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);

    // Find tasks with no active blockers (entry points)
    let entry_points: Vec<&str> = active_tasks
        .iter()
        .filter(|t| !cycle_nodes.contains(t.id.as_str()))
        .filter(|t| {
            t.after.iter().all(|blocker_id| {
                // Not blocked by any active non-terminal task
                !active_ids.contains(blocker_id.as_str())
                    || cycle_nodes.contains(blocker_id.as_str())
                    || graph
                        .get_task(blocker_id)
                        .map(|bt| bt.status.is_terminal())
                        .unwrap_or(true)
            })
        })
        .map(|t| t.id.as_str())
        .collect();

    // Calculate longest paths with an iterative reverse-topological pass.
    // Memo entries hold only a next pointer, avoiding O(N²) path-prefix clones
    // on a deep linear graph.
    let mut memo: HashMap<&str, PathState<'_>> = HashMap::new();

    for entry in &entry_points {
        calculate_longest_path(entry, &graph, &forward_index, &mut memo, &cycle_nodes);
    }

    // Find the overall longest path and reconstruct it once.
    let (critical_path, total_hours) = memo
        .iter()
        .max_by(|a, b| compare_path_state(a.1, b.1))
        .map(|(&id, state)| (reconstruct_path(id, &memo), state.hours))
        .unwrap_or_default();

    // Build critical task info
    let critical_set: HashSet<&str> = critical_path.iter().map(String::as_str).collect();
    let critical_tasks: Vec<CriticalTask> = critical_path
        .iter()
        .enumerate()
        .filter_map(|(i, task_id)| {
            graph.get_task(task_id).map(|t| {
                let after = if i == 0 {
                    None
                } else {
                    Some(critical_path[i - 1].clone())
                };
                CriticalTask {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    status: t.status,
                    hours: t.estimate.as_ref().and_then(|e| e.hours),
                    after,
                }
            })
        })
        .collect();

    // Calculate slack for non-critical tasks
    let slack_analysis: Vec<SlackInfo> = active_tasks
        .iter()
        .filter(|t| !critical_set.contains(t.id.as_str()) && !cycle_nodes.contains(t.id.as_str()))
        .filter_map(|t| {
            // Slack = critical path hours - hours if this task were on the path
            // Simplified: just show the difference from total hours for this task's path
            let task_hours = t.estimate.as_ref().and_then(|e| e.hours).unwrap_or(0.0);
            let slack = total_hours - task_hours;
            if slack > 0.0 {
                Some(SlackInfo {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    slack_hours: slack,
                    note: "can delay without affecting deadline".to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    if json {
        let output = CriticalPathOutput {
            critical_path: critical_tasks,
            task_count: critical_path.len(),
            total_hours,
            slack_analysis,
            cycles_skipped: cycles,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        if critical_path.is_empty() {
            println!("No critical path found (no active dependency chains).");
            if !cycles.is_empty() {
                println!("\nNote: {} cycle(s) were skipped.", cycles.len());
            }
            return Ok(());
        }

        println!(
            "Critical path ({} tasks, estimated {} hours):\n",
            critical_path.len(),
            format_hours(total_hours)
        );

        for (i, task_id) in critical_path.iter().enumerate() {
            if let Some(task) = graph.get_task(task_id) {
                let status_str = match task.status {
                    Status::Open | Status::InProgress => "ready",
                    Status::Blocked => "blocked",
                    Status::Done => "done",
                    Status::Failed => "failed",
                    Status::Abandoned => "abandoned",
                    Status::Waiting => "waiting",
                    Status::PendingValidation => "pending-validation",
                    Status::PendingEval => "pending-eval",
                    Status::FailedPendingEval => "failed-pending-eval",
                    Status::Incomplete => "incomplete",
                };

                let hours_str = task
                    .estimate
                    .as_ref()
                    .and_then(|e| e.hours)
                    .map(|h| format!(" ({}h)", h))
                    .unwrap_or_default();

                let blocked_str = if i == 0 {
                    String::new()
                } else {
                    format!(" <- blocked by {}", critical_path[i - 1])
                };

                println!(
                    "{}. [{}] {}{}{}",
                    i + 1,
                    status_str,
                    task.id,
                    hours_str,
                    blocked_str
                );
            }
        }

        if !slack_analysis.is_empty() {
            println!("\nSlack analysis:");
            for slack in &slack_analysis {
                println!(
                    "  {}: {}h slack ({})",
                    slack.id,
                    format_hours(slack.slack_hours),
                    slack.note
                );
            }
        }

        if !cycles.is_empty() {
            println!(
                "\nNote: {} cycle(s) were skipped in analysis.",
                cycles.len()
            );
        }
    }

    Ok(())
}

/// Build forward index: task_id -> tasks that it blocks (among active non-cycle tasks)
fn build_forward_index<'a>(
    graph: &'a WorkGraph,
    active_ids: &HashSet<&str>,
    cycle_nodes: &HashSet<&str>,
) -> HashMap<&'a str, Vec<&'a str>> {
    let mut index: HashMap<&str, Vec<&str>> = HashMap::new();

    for task in graph.tasks() {
        if !active_ids.contains(task.id.as_str()) || cycle_nodes.contains(task.id.as_str()) {
            continue;
        }

        // For each blocker, add this task to its forward list
        for blocker_id in &task.after {
            if active_ids.contains(blocker_id.as_str())
                && !cycle_nodes.contains(blocker_id.as_str())
            {
                index
                    .entry(blocker_id.as_str())
                    .or_default()
                    .push(task.id.as_str());
            }
        }
    }

    index
}

#[derive(Clone, Copy, Debug)]
struct PathState<'a> {
    hours: f64,
    hops: usize,
    next: Option<&'a str>,
}

fn compare_path_state(a: &PathState<'_>, b: &PathState<'_>) -> std::cmp::Ordering {
    a.hours
        .partial_cmp(&b.hours)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.hops.cmp(&b.hops))
}

fn reconstruct_path(start: &str, memo: &HashMap<&str, PathState<'_>>) -> Vec<String> {
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(start);
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        path.push(id.to_string());
        current = memo.get(id).and_then(|state| state.next);
    }
    path
}

/// Calculate the longest path starting at `task_id` with no recursive calls.
fn calculate_longest_path<'a>(
    task_id: &'a str,
    graph: &'a WorkGraph,
    forward_index: &HashMap<&'a str, Vec<&'a str>>,
    memo: &mut HashMap<&'a str, PathState<'a>>,
    cycle_nodes: &HashSet<&str>,
) -> (f64, Vec<String>) {
    if cycle_nodes.contains(task_id) || graph.get_task(task_id).is_none() {
        return (0.0, vec![]);
    }
    if let Some(state) = memo.get(task_id) {
        return (state.hours, reconstruct_path(task_id, memo));
    }

    // Discover the reachable active DAG.
    let mut reachable = HashSet::new();
    let mut work = vec![task_id];
    while let Some(id) = work.pop() {
        if cycle_nodes.contains(id) || !reachable.insert(id) {
            continue;
        }
        for &child in forward_index.get(id).map(Vec::as_slice).unwrap_or(&[]) {
            if !cycle_nodes.contains(child) {
                work.push(child);
            }
        }
    }

    let mut remaining_children: HashMap<&str, usize> = HashMap::new();
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for &id in &reachable {
        let children: Vec<&str> = forward_index
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|child| reachable.contains(child) && !cycle_nodes.contains(child))
            .collect();
        remaining_children.insert(id, children.len());
        for child in children {
            parents.entry(child).or_default().push(id);
        }
    }

    let mut queue: std::collections::VecDeque<&str> = remaining_children
        .iter()
        .filter_map(|(&id, &remaining)| (remaining == 0).then_some(id))
        .collect();
    let mut best_child: HashMap<&str, PathState<'a>> = HashMap::new();

    while let Some(id) = queue.pop_front() {
        let own_hours = graph
            .get_task(id)
            .and_then(|task| task.estimate.as_ref())
            .and_then(|estimate| estimate.hours)
            .unwrap_or(1.0);
        let own_hours = if own_hours.is_finite() {
            own_hours.max(0.0)
        } else {
            0.0
        };
        let child = best_child.get(id).copied();
        // `best_child.next` names the child chosen by this node. Leaf nodes
        // have no next pointer.
        let state = PathState {
            hours: own_hours + child.map(|value| value.hours).unwrap_or(0.0),
            hops: 1 + child.map(|value| value.hops).unwrap_or(0),
            next: child.and_then(|value| value.next),
        };
        memo.insert(id, state);

        for &parent in parents.get(id).map(Vec::as_slice).unwrap_or(&[]) {
            let candidate = PathState {
                hours: state.hours,
                hops: state.hops,
                next: Some(id),
            };
            match best_child.get(parent) {
                Some(current)
                    if compare_path_state(current, &candidate) != std::cmp::Ordering::Less => {}
                _ => {
                    best_child.insert(parent, candidate);
                }
            }
            let remaining = remaining_children
                .get_mut(parent)
                .expect("reachable parent has child count");
            *remaining -= 1;
            if *remaining == 0 {
                queue.push_back(parent);
            }
        }
    }

    memo.get(task_id)
        .map(|state| (state.hours, reconstruct_path(task_id, memo)))
        .unwrap_or((0.0, vec![]))
}

/// Detect SCCs in the active induced subgraph using the shared iterative
/// Tarjan implementation.
fn detect_cycles_among_active(graph: &WorkGraph, active_ids: &HashSet<&str>) -> Vec<Vec<String>> {
    let mut named = worksgood::cycle::NamedGraph::new();
    let mut ids: Vec<&str> = active_ids.iter().copied().collect();
    ids.sort_unstable();
    for id in &ids {
        named.add_node(id);
    }
    for id in &ids {
        if let Some(task) = graph.get_task(id) {
            for dep in &task.after {
                if active_ids.contains(dep.as_str()) {
                    named.add_edge(dep, id);
                }
            }
        }
    }
    named
        .analyze_cycles()
        .into_iter()
        .map(|cycle| {
            cycle
                .members
                .into_iter()
                .map(|id| named.get_name(id).to_string())
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::graph::{Estimate, Node, PRIORITY_DEFAULT, Task};

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn make_task_with_hours(id: &str, title: &str, hours: f64) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            description: None,
            status: Status::Open,
            priority: PRIORITY_DEFAULT,
            assigned: None,
            estimate: Some(Estimate {
                hours: Some(hours),
                cost: None,
            }),
            before: vec![],
            after: vec![],
            requires: vec![],
            tags: vec![],
            skills: vec![],
            inputs: vec![],
            deliverables: vec![],
            artifacts: vec![],
            exec: None,
            timeout: None,
            not_before: None,
            created_at: None,
            started_at: None,
            completed_at: None,
            last_interaction_at: None,
            log: vec![],
            retry_count: 0,
            max_retries: None,
            failure_reason: None,
            failure_class: None,
            model: None,
            reasoning: None,
            provider: None,
            endpoint: None,
            remote_provider: None,
            profile: None,
            command_argv: vec![],
            working_dir: None,
            executor_preset_name: None,
            verify: None,
            verify_timeout: None,
            agent: None,
            loop_iteration: 0,
            last_iteration_completed_at: None,
            cycle_failure_restarts: 0,
            ready_after: None,
            paused: false,
            visibility: "internal".to_string(),
            context_scope: None,
            exec_mode: None,
            cycle_config: None,
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
            last_spawn_failure_at: None,
            dispatch_count: 0,
            tier: None,
            no_tier_escalation: false,
            tried_models: vec![],
            superseded_by: vec![],
            supersedes: None,
            unplaced: false,
            place_near: vec![],
            place_before: vec![],
            independent: false,
            iteration_anchor: None,
            iteration_config: None,
            iteration_round: 0,
            iteration_parent: None,
            cron_schedule: None,
            cron_enabled: false,
            last_cron_fire: None,
            next_cron_fire: None,
        }
    }

    #[test]
    fn test_format_hours_whole() {
        assert_eq!(format_hours(8.0), "8");
        assert_eq!(format_hours(47.0), "47");
    }

    #[test]
    fn test_format_hours_decimal() {
        assert_eq!(format_hours(8.5), "8.5");
        assert_eq!(format_hours(47.25), "47.2");
    }

    #[test]
    fn test_empty_graph_has_no_critical_path() {
        let graph = WorkGraph::new();
        let active_ids: HashSet<&str> = HashSet::new();
        let cycles = detect_cycles_among_active(&graph, &active_ids);
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_single_task_is_critical_path() {
        let mut graph = WorkGraph::new();
        let task = make_task_with_hours("t1", "Task 1", 8.0);
        graph.add_node(Node::Task(task));

        let active_ids: HashSet<&str> = vec!["t1"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        assert_eq!(hours, 8.0);
        assert_eq!(path, vec!["t1".to_string()]);
    }

    #[test]
    fn test_linear_chain_critical_path() {
        let mut graph = WorkGraph::new();

        // t1 (8h) -> t2 (16h) -> t3 (4h)
        let t1 = make_task_with_hours("t1", "Task 1", 8.0);
        let mut t2 = make_task_with_hours("t2", "Task 2", 16.0);
        t2.after = vec!["t1".to_string()];
        let mut t3 = make_task_with_hours("t3", "Task 3", 4.0);
        t3.after = vec!["t2".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));

        let active_ids: HashSet<&str> = vec!["t1", "t2", "t3"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        assert_eq!(hours, 28.0);
        assert_eq!(
            path,
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()]
        );
    }

    #[test]
    fn test_parallel_paths_picks_longest() {
        let mut graph = WorkGraph::new();

        // t1 (8h) -> t2 (16h) -> t4 (4h)
        // t1 (8h) -> t3 (2h) -> t4 (4h)
        // Longest: t1 -> t2 -> t4 = 28h
        let t1 = make_task_with_hours("t1", "Task 1", 8.0);
        let mut t2 = make_task_with_hours("t2", "Task 2", 16.0);
        t2.after = vec!["t1".to_string()];
        let mut t3 = make_task_with_hours("t3", "Task 3", 2.0);
        t3.after = vec!["t1".to_string()];
        let mut t4 = make_task_with_hours("t4", "Task 4", 4.0);
        t4.after = vec!["t2".to_string(), "t3".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));
        graph.add_node(Node::Task(t4));

        let active_ids: HashSet<&str> = vec!["t1", "t2", "t3", "t4"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        assert_eq!(hours, 28.0);
        assert_eq!(
            path,
            vec!["t1".to_string(), "t2".to_string(), "t4".to_string()]
        );
    }

    #[test]
    fn test_done_tasks_excluded() {
        let mut graph = WorkGraph::new();

        let mut t1 = make_task_with_hours("t1", "Task 1", 8.0);
        t1.status = Status::Done;
        let mut t2 = make_task_with_hours("t2", "Task 2", 16.0);
        t2.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));

        // Only t2 is active
        let active_ids: HashSet<&str> = vec!["t2"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, path) =
            calculate_longest_path("t2", &graph, &forward_index, &mut memo, &cycle_nodes);

        assert_eq!(hours, 16.0);
        assert_eq!(path, vec!["t2".to_string()]);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = WorkGraph::new();

        // t1 -> t2 -> t1 (cycle)
        let mut t1 = make_task("t1", "Task 1");
        t1.after = vec!["t2".to_string()];
        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));

        let active_ids: HashSet<&str> = vec!["t1", "t2"].into_iter().collect();
        let cycles = detect_cycles_among_active(&graph, &active_ids);

        assert!(!cycles.is_empty());
    }

    #[test]
    fn test_tasks_without_hours_default_to_one() {
        let mut graph = WorkGraph::new();

        let t1 = make_task("t1", "Task 1");
        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));

        let active_ids: HashSet<&str> = vec!["t1", "t2"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        // Each task defaults to 1 hour
        assert_eq!(hours, 2.0);
        assert_eq!(path, vec!["t1".to_string(), "t2".to_string()]);
    }

    #[test]
    fn test_build_forward_index() {
        let mut graph = WorkGraph::new();

        // t1 -> t2, t1 -> t3
        let t1 = make_task("t1", "Task 1");
        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];
        let mut t3 = make_task("t3", "Task 3");
        t3.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));

        let active_ids: HashSet<&str> = vec!["t1", "t2", "t3"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);

        let t1_blocks = forward_index.get("t1").unwrap();
        assert_eq!(t1_blocks.len(), 2);
        assert!(t1_blocks.contains(&"t2"));
        assert!(t1_blocks.contains(&"t3"));
    }

    #[test]
    fn test_nan_estimate_does_not_panic() {
        let mut graph = WorkGraph::new();

        // Create tasks where one has NaN hours (simulates corrupt estimate)
        let t1 = make_task_with_hours("t1", "Task 1", f64::NAN);
        let t2 = make_task_with_hours("t2", "Task 2", 4.0);
        let mut t3 = make_task_with_hours("t3", "Task 3", 2.0);
        t3.after = vec!["t1".to_string(), "t2".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));

        let active_ids: HashSet<&str> = vec!["t1", "t2", "t3"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        // Should not panic — NaN comparison falls back to Equal
        for entry in &["t1", "t2"] {
            calculate_longest_path(entry, &graph, &forward_index, &mut memo, &cycle_nodes);
        }

        let result = memo.iter().max_by(|a, b| compare_path_state(a.1, b.1));
        // Just verify we don't crash — the exact result with NaN is implementation-defined
        assert!(result.is_some());
    }

    #[test]
    fn test_orphan_blocker_in_critical_path() {
        // A task references a blocker that doesn't exist in the graph
        let mut graph = WorkGraph::new();

        let mut t1 = make_task_with_hours("t1", "Task 1", 8.0);
        t1.after = vec!["ghost".to_string()]; // orphan reference

        graph.add_node(Node::Task(t1));

        let active_ids: HashSet<&str> = vec!["t1"].into_iter().collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        // Should not panic even with orphan blocker references
        let (hours, path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        assert_eq!(hours, 8.0);
        assert_eq!(path, vec!["t1".to_string()]);
    }

    #[test]
    fn test_negative_estimate_clamped_to_zero() {
        // Negative estimates should be clamped to 0 and not corrupt path calculations
        let mut graph = WorkGraph::new();

        let t1 = make_task_with_hours("t1", "Task 1", 10.0);
        let mut t2 = make_task_with_hours("t2", "Task 2", -5.0);
        t2.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));

        let active_ids: HashSet<&str> = graph.tasks().map(|t| t.id.as_str()).collect();
        let cycle_nodes: HashSet<&str> = HashSet::new();
        let forward_index = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();

        let (hours, _path) =
            calculate_longest_path("t1", &graph, &forward_index, &mut memo, &cycle_nodes);

        // t1 = 10h, t2 = 0h (clamped from -5), total path should be 10
        assert!(
            hours >= 10.0,
            "negative estimate should not reduce path length, got {}",
            hours
        );
    }

    #[test]
    fn deep_chain_critical_path_is_stack_safe_and_linear_storage() {
        let mut graph = WorkGraph::new();
        for index in 0..2_000usize {
            let mut task = make_task_with_hours(&format!("deep-{index:04}"), "deep", 1.0);
            if index > 0 {
                task.after = vec![format!("deep-{:04}", index - 1)];
            }
            graph.add_node(Node::Task(task));
        }
        let active_ids: HashSet<&str> = graph.tasks().map(|task| task.id.as_str()).collect();
        let cycle_nodes = HashSet::new();
        let forward = build_forward_index(&graph, &active_ids, &cycle_nodes);
        let mut memo = HashMap::new();
        let (hours, path) =
            calculate_longest_path("deep-0000", &graph, &forward, &mut memo, &cycle_nodes);
        assert_eq!(hours, 2_000.0);
        assert_eq!(path.len(), 2_000);
        assert_eq!(memo.len(), 2_000);
    }

    #[test]
    fn test_format_hours_nan_and_infinity() {
        assert_eq!(format_hours(f64::NAN), "?");
        assert_eq!(format_hours(f64::INFINITY), "?");
        assert_eq!(format_hours(f64::NEG_INFINITY), "?");
        assert_eq!(format_hours(5.0), "5");
        assert_eq!(format_hours(2.5), "2.5");
    }
}
