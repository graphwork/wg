use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use worksgood::WorkGraph;
use worksgood::graph::{Status, Task};

/// Information about a blocking chain node
#[derive(Debug, Clone)]
struct BlockingNode {
    id: String,
    status: Status,
    is_phantom: bool,
    failure_reason: Option<String>,
    evaluation_health: Option<worksgood::eval_lifecycle::EvaluationHealth>,
    eval_bypasses: Vec<(String, Status)>,
    children: Vec<usize>,
}

#[derive(Debug, Clone)]
struct BlockingTree {
    nodes: Vec<BlockingNode>,
    root: usize,
}

/// Root blocker information
#[derive(Debug, Clone)]
struct RootBlocker<'a> {
    task: &'a Task,
    is_ready: bool,
}

#[derive(Debug, Clone)]
struct PendingConvergence {
    action: String,
    deadline: String,
    rank: Option<String>,
}

pub fn run(dir: &Path, id: &str, json: bool) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;

    let task = graph.get_task_or_err(id)?;

    // Build the blocking chain tree (resolves remote deps via federation)
    let mut visited = HashSet::new();
    let blocking_tree = build_blocking_tree(&graph, id, &mut visited, dir);

    // Find root blockers (tasks with no blockers of their own, and not done)
    let mut root_blocker_ids = HashSet::new();
    collect_root_blockers(&graph, &blocking_tree, &mut root_blocker_ids);

    let root_blockers: Vec<RootBlocker> = root_blocker_ids
        .iter()
        .filter_map(|rid| {
            // For remote refs, we can't get a &Task, but the blocking tree already
            // has the status. Root blockers from remote peers are only shown in the
            // tree; they won't appear here (since graph.get_task won't find them).
            graph.get_task(rid).map(|t| {
                let is_ready = is_task_ready(&graph, t, dir);
                RootBlocker { task: t, is_ready }
            })
        })
        .collect();

    // Collect phantom root blocker IDs (not in graph, so not in root_blockers)
    let phantom_root_ids: Vec<String> = root_blocker_ids
        .iter()
        .filter(|rid| {
            graph.get_task(rid).is_none()
                && graph.get_archived_boundary(rid).is_none()
                && worksgood::federation::parse_remote_ref(rid).is_none()
        })
        .cloned()
        .collect();

    // Count total blocking tasks
    let total_blockers = count_blockers(&blocking_tree);
    let pending_convergence = pending_convergence(dir, task);

    if json {
        print_json(
            &graph,
            task,
            &blocking_tree,
            &root_blockers,
            &phantom_root_ids,
            total_blockers,
            pending_convergence.as_ref(),
        )?;
    } else {
        print_human(
            &graph,
            task,
            &blocking_tree,
            &root_blockers,
            &phantom_root_ids,
            total_blockers,
            pending_convergence.as_ref(),
        );
    }

    Ok(())
}

fn blocking_node(graph: &WorkGraph, task_id: &str) -> BlockingNode {
    let task = graph.get_task(task_id);
    let boundary = graph.get_archived_boundary(task_id);
    BlockingNode {
        id: task_id.to_string(),
        status: task
            .map(|task| task.status)
            .or_else(|| boundary.map(|boundary| boundary.status))
            .unwrap_or(Status::Open),
        is_phantom: task.is_none()
            && boundary.is_none()
            && worksgood::federation::parse_remote_ref(task_id).is_none(),
        failure_reason: task.and_then(|task| task.failure_reason.clone()),
        evaluation_health: task
            .and_then(|task| worksgood::eval_lifecycle::evaluation_health(graph, &task.id)),
        eval_bypasses: Vec::new(),
        children: Vec::new(),
    }
}

fn build_blocking_tree(
    graph: &WorkGraph,
    task_id: &str,
    visited: &mut HashSet<String>,
    dir: &Path,
) -> BlockingTree {
    let mut tree = BlockingTree {
        nodes: vec![blocking_node(graph, task_id)],
        root: 0,
    };
    let mut work = vec![0usize];
    while let Some(index) = work.pop() {
        let id = tree.nodes[index].id.clone();
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(task) = graph.get_task(&id) else {
            continue;
        };
        let mut recurse = Vec::new();
        for blocker_id in &task.after {
            if visited.contains(blocker_id) {
                continue;
            }
            if let Some((peer_name, remote_task_id)) =
                worksgood::federation::parse_remote_ref(blocker_id)
            {
                let remote = worksgood::federation::resolve_remote_task_status(
                    peer_name,
                    remote_task_id,
                    dir,
                );
                if remote.status != Status::Done {
                    let child = tree.nodes.len();
                    tree.nodes.push(BlockingNode {
                        id: blocker_id.clone(),
                        status: remote.status,
                        is_phantom: false,
                        failure_reason: None,
                        evaluation_health: None,
                        eval_bypasses: Vec::new(),
                        children: Vec::new(),
                    });
                    tree.nodes[index].children.push(child);
                }
            } else if graph.get_task(blocker_id).is_some()
                || graph.get_archived_boundary(blocker_id).is_some()
            {
                match worksgood::query::dependency_disposition(
                    blocker_id,
                    &task.id,
                    graph,
                    Some(dir),
                ) {
                    worksgood::query::DependencyDisposition::Satisfied => {}
                    worksgood::query::DependencyDisposition::EvalSystemBypass {
                        blocker_status,
                    } => tree.nodes[index]
                        .eval_bypasses
                        .push((blocker_id.clone(), blocker_status)),
                    worksgood::query::DependencyDisposition::Blocked { .. } => {
                        let child = tree.nodes.len();
                        tree.nodes.push(blocking_node(graph, blocker_id));
                        tree.nodes[index].children.push(child);
                        recurse.push(child);
                    }
                }
            } else {
                let child = tree.nodes.len();
                tree.nodes.push(blocking_node(graph, blocker_id));
                tree.nodes[index].children.push(child);
            }
        }
        work.extend(recurse.into_iter().rev());
    }
    tree
}

fn collect_root_blockers(graph: &WorkGraph, tree: &BlockingTree, roots: &mut HashSet<String>) {
    for node in &tree.nodes {
        if !node.children.is_empty() {
            continue;
        }
        if node.is_phantom {
            roots.insert(node.id.clone());
        } else if graph
            .get_task(&node.id)
            .is_some_and(|task| !task.status.is_dep_satisfied())
            || graph
                .get_archived_boundary(&node.id)
                .is_some_and(|boundary| boundary.status != Status::Done)
        {
            roots.insert(node.id.clone());
        }
    }
}

fn is_task_ready(graph: &WorkGraph, task: &Task, dir: &Path) -> bool {
    if task.status != Status::Open {
        return false;
    }
    task.after.iter().all(|blocker_id| {
        worksgood::query::dependency_disposition(blocker_id, &task.id, graph, Some(dir))
            .is_satisfied()
    })
}

fn count_blockers(tree: &BlockingTree) -> usize {
    tree.nodes.len().saturating_sub(1)
}

fn pending_convergence(dir: &Path, task: &Task) -> Option<PendingConvergence> {
    let tx = worksgood::finalization::FinalizationStore::open(dir)
        .ok()
        .and_then(|store| store.load_task(&task.id).ok().flatten());
    let record = worksgood::service::ConvergenceState::load(dir)
        .ok()
        .and_then(|state| {
            state
                .goals
                .get(&format!("{}#{}", task.id, task.lifecycle.generation))
                .cloned()
        });
    let deadline = record
        .as_ref()
        .map(|record| record.next_wake_at.clone())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let projected_action = record
        .as_ref()
        .and_then(|record| record.pending_convergence_action)
        .map(|action| action.description().to_string());
    let projected_rank = record
        .as_ref()
        .and_then(|record| record.finish_convergence_rank)
        .and_then(|rank| serde_json::to_value(rank).ok())
        .and_then(|value| value.as_str().map(str::to_owned));
    if let Some(intent) = task.lifecycle.reopen_intent.as_ref() {
        return Some(PendingConvergence {
            action: projected_action.unwrap_or_else(|| {
                format!(
                    "release exact dead owner once, then resume session/worktree ({})",
                    intent.operation
                )
            }),
            deadline,
            rank: projected_rank,
        });
    }
    if let Some(tx) = tx
        && tx.cleanup_receipt.is_none()
    {
        return Some(PendingConvergence {
            action: projected_action.unwrap_or(tx.safe_next_command),
            deadline,
            rank: projected_rank,
        });
    }
    let pi_exit_pending = task.status == Status::InProgress
        && (task.lifecycle.pi_terminal_reservation.is_some()
            || task.lifecycle.audit.iter().any(|event| {
                event.generation == task.lifecycle.generation
                    && event.event_kind == "pi-process-epoch-exited"
            }));
    pi_exit_pending.then(|| PendingConvergence {
        action: projected_action.unwrap_or_else(|| {
            "finish exact durable receipt, or fence dead owner and resume the same session/worktree"
                .into()
        }),
        deadline,
        rank: projected_rank,
    })
}

fn print_human(
    graph: &WorkGraph,
    task: &Task,
    tree: &BlockingTree,
    root_blockers: &[RootBlocker],
    phantom_roots: &[String],
    total: usize,
    pending_convergence: Option<&PendingConvergence>,
) {
    println!("Task: {}", task.id);
    let root = &tree.nodes[tree.root];

    if root.children.is_empty() {
        println!("Status: {:?}", task.status);
        println!();
        if let Some(pending) = pending_convergence {
            println!("{} is waiting on lifecycle convergence.", task.id);
            println!("Pending action: {}", pending.action);
            if let Some(rank) = pending.rank.as_deref() {
                println!("Convergence rank: {rank}");
            }
            println!("Deadline: {}", pending.deadline);
        } else if root.eval_bypasses.is_empty() {
            println!("{} has no blockers.", task.id);
        } else {
            println!(
                "{} is dispatcher-ready via evaluation-system bypass.",
                task.id
            );
            for (blocker, status) in &root.eval_bypasses {
                println!(
                    "  {}: {} — evaluation-system bypass (this satellite is part of {}'s gate)",
                    blocker, status, blocker
                );
            }
            if let Some(reason) = task.failure_reason.as_deref() {
                println!("Lifecycle health: {}", reason);
            }
        }
        if let Some(health) = worksgood::eval_lifecycle::evaluation_health(graph, &task.id) {
            println!(
                "Evaluation health: {} (pipeline={}, source_attempt={})",
                health.state, health.pipeline_id, health.source_attempt
            );
            println!("  {}", health.diagnostic);
        }
        return;
    }

    println!("Status: blocked (transitively)");
    for node in tree.nodes.iter().skip(1) {
        if node.status == Status::Abandoned {
            println!("blocked: prerequisite {} was abandoned", node.id);
            if let Some(abandoned) = graph.get_task(&node.id)
                && !abandoned.superseded_by.is_empty()
            {
                println!(
                    "  Superseded by: {} (provenance only; the edge remains blocked)",
                    abandoned.superseded_by.join(", ")
                );
            }
            println!(
                "  Repair: `wg retry {0}`, relink to a completed replacement, or explicitly remove the edge with `wg rm-dep {1} {0}`.",
                node.id, task.id
            );
        }
    }
    println!();
    println!("Blocking chain:");
    println!();
    print_tree(tree);

    if !root_blockers.is_empty() || !phantom_roots.is_empty() {
        println!();
        println!("Root blockers (actionable now):");
        for rb in root_blockers {
            let assigned = rb
                .task
                .assigned
                .as_ref()
                .map(|a| format!(", assigned to {}", a))
                .unwrap_or_else(|| ", unassigned".to_string());
            let ready_str = if rb.is_ready { ", ready to start" } else { "" };
            println!(
                "  - {}: {:?}{}{}",
                rb.task.id, rb.task.status, assigned, ready_str
            );
        }
        for phantom_id in phantom_roots {
            println!(
                "  - {}: DOES NOT EXIST (phantom dependency — fix with: wg edit {} --remove-after {})",
                phantom_id, task.id, phantom_id
            );
        }
    }

    println!();
    if root_blockers.len() == 1 {
        println!(
            "Summary: {} is blocked by {} task{}; unblock {} to make progress.",
            task.id,
            total,
            if total == 1 { "" } else { "s" },
            root_blockers[0].task.id
        );
    } else if root_blockers.is_empty() {
        println!(
            "Summary: {} is blocked by {} task{}.",
            task.id,
            total,
            if total == 1 { "" } else { "s" }
        );
    } else {
        let ids: Vec<&str> = root_blockers.iter().map(|rb| rb.task.id.as_str()).collect();
        println!(
            "Summary: {} is blocked by {} task{}; unblock {} to make progress.",
            task.id,
            total,
            if total == 1 { "" } else { "s" },
            ids.join(" or ")
        );
    }
}

fn print_tree(tree: &BlockingTree) {
    const MAX_VISUAL_INDENT: usize = 32;
    let mut work = vec![(tree.root, 0usize)];
    while let Some((index, depth)) = work.pop() {
        let node = &tree.nodes[index];
        let prefix = if depth <= 1 {
            String::new()
        } else if depth > MAX_VISUAL_INDENT {
            "… ".to_string()
        } else {
            "     ".repeat(depth - 1)
        };
        if depth == 0 {
            println!("{}", node.id);
        } else if node.is_phantom {
            println!(
                "{} \\-- blocked by: {} (DOES NOT EXIST — phantom dependency) <-- ROOT CAUSE",
                prefix, node.id
            );
        } else {
            let root_marker = if node.children.is_empty() && !node.status.is_terminal() {
                " <-- ROOT CAUSE"
            } else {
                ""
            };
            println!(
                "{} \\-- blocked by: {} (status: {:?}){}",
                prefix, node.id, node.status, root_marker
            );
            if let Some(reason) = node.failure_reason.as_deref() {
                println!("{}     lifecycle health: {}", prefix, reason);
            }
            if let Some(health) = node.evaluation_health.as_ref() {
                println!(
                    "{}     evaluation health: {} — {}",
                    prefix, health.state, health.diagnostic
                );
            }
        }
        work.extend(node.children.iter().rev().map(|&child| (child, depth + 1)));
    }
}

fn print_json(
    graph: &WorkGraph,
    task: &Task,
    tree: &BlockingTree,
    root_blockers: &[RootBlocker],
    phantom_roots: &[String],
    total: usize,
    pending_convergence: Option<&PendingConvergence>,
) -> Result<()> {
    let mut all_root_blockers: Vec<serde_json::Value> = root_blockers
        .iter()
        .map(|rb| {
            serde_json::json!({
                "id": rb.task.id,
                "title": rb.task.title,
                "status": rb.task.status,
                "assigned": rb.task.assigned,
                "is_ready": rb.is_ready,
            })
        })
        .collect();
    for phantom_id in phantom_roots {
        all_root_blockers.push(serde_json::json!({
            "id": phantom_id,
            "phantom": true,
            "status": "DOES NOT EXIST",
        }));
    }
    let root = &tree.nodes[tree.root];
    let blocking_chain = if tree.nodes.len() <= 256 {
        tree_to_json(tree, tree.root)
    } else {
        // serde_json values are recursively represented. For a very deep
        // chain, preserve every node in a flat parent-indexed form rather than
        // risking serializer/drop stack overflow or truncating graph truth.
        let mut parent: Vec<Option<usize>> = vec![None; tree.nodes.len()];
        for (index, node) in tree.nodes.iter().enumerate() {
            for &child in &node.children {
                parent[child] = Some(index);
            }
        }
        serde_json::json!({
            "format": "flat-deep-chain",
            "root": tree.root,
            "nodes": tree.nodes.iter().enumerate().map(|(index, node)| serde_json::json!({
                "index": index,
                "parent": parent[index],
                "id": node.id,
                "status": format!("{:?}", node.status),
                "phantom": node.is_phantom,
                "failure_reason": node.failure_reason,
                "evaluation_health": node.evaluation_health,
                "evaluation_system_bypasses": node.eval_bypasses,
            })).collect::<Vec<_>>(),
        })
    };
    let blocking_reasons: Vec<serde_json::Value> = tree
        .nodes
        .iter()
        .skip(1)
        .map(|node| {
            let reason = if node.status == Status::Abandoned {
                format!("prerequisite {} was abandoned", node.id)
            } else if node.is_phantom {
                format!("prerequisite {} does not exist", node.id)
            } else {
                format!("prerequisite {} status is {}", node.id, node.status)
            };
            let superseded_by = graph
                .get_task(&node.id)
                .map(|task| task.superseded_by.clone())
                .unwrap_or_default();
            serde_json::json!({
                "id": node.id,
                "status": node.status,
                "reason": reason,
                "superseded_by": superseded_by,
                "repair_commands": [
                    format!("wg retry {}", node.id),
                    format!("wg rm-dep {} {}", task.id, node.id),
                ],
            })
        })
        .collect();
    let output = serde_json::json!({
        "task": {
            "id": task.id,
            "title": task.title,
            "status": task.status,
        },
        "dispatcher_ready_via_evaluation_system_bypass": root.children.is_empty() && !root.eval_bypasses.is_empty(),
        "is_blocked": !root.children.is_empty() || pending_convergence.is_some(),
        "pending_convergence": pending_convergence.map(|pending| serde_json::json!({
            "action": pending.action,
            "deadline": pending.deadline,
            "rank": pending.rank,
        })),
        "blocking_chain": blocking_chain,
        "root_blockers": all_root_blockers,
        "total_blockers": total,
        "blocking_reasons": blocking_reasons,
        "evaluation_health": worksgood::eval_lifecycle::evaluation_health(graph, &task.id),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn tree_to_json(tree: &BlockingTree, index: usize) -> serde_json::Value {
    let node = &tree.nodes[index];
    let mut obj = serde_json::json!({
        "id": node.id,
        "status": format!("{:?}", node.status),
        "after": node.children.iter().map(|&child| tree_to_json(tree, child)).collect::<Vec<_>>(),
    });
    if node.is_phantom {
        obj["phantom"] = serde_json::Value::Bool(true);
    }
    if let Some(reason) = node.failure_reason.as_deref() {
        obj["failure_reason"] = serde_json::Value::String(reason.to_string());
    }
    if let Some(health) = node.evaluation_health.as_ref() {
        obj["evaluation_health"] = serde_json::to_value(health).unwrap_or_default();
    }
    obj["evaluation_system_bypasses"] = serde_json::Value::Array(
        node.eval_bypasses
            .iter()
            .map(|(id, status)| serde_json::json!({"id": id, "status": status}))
            .collect(),
    );
    obj
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::graph::{Node, Task};

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    #[test]
    fn test_build_blocking_tree_no_blockers() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Task 1")));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "t1", &mut visited, dir);

        assert_eq!(tree.nodes[tree.root].id, "t1");
        assert!(tree.nodes[tree.root].children.is_empty());
    }

    #[test]
    fn test_build_blocking_tree_single_blocker() {
        let mut graph = WorkGraph::new();

        let blocker = make_task("blocker", "Blocker");
        let mut blocked = make_task("blocked", "Blocked");
        blocked.after = vec!["blocker".to_string()];

        graph.add_node(Node::Task(blocker));
        graph.add_node(Node::Task(blocked));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "blocked", &mut visited, dir);

        let root = &tree.nodes[tree.root];
        assert_eq!(root.id, "blocked");
        assert_eq!(root.children.len(), 1);
        assert_eq!(tree.nodes[root.children[0]].id, "blocker");
    }

    #[test]
    fn test_build_blocking_tree_chain() {
        let mut graph = WorkGraph::new();

        let t1 = make_task("t1", "Task 1");
        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];
        let mut t3 = make_task("t3", "Task 3");
        t3.after = vec!["t2".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "t3", &mut visited, dir);

        let root = &tree.nodes[tree.root];
        assert_eq!(root.id, "t3");
        assert_eq!(root.children.len(), 1);
        let middle = &tree.nodes[root.children[0]];
        assert_eq!(middle.id, "t2");
        assert_eq!(middle.children.len(), 1);
        assert_eq!(tree.nodes[middle.children[0]].id, "t1");
    }

    #[test]
    fn test_build_blocking_tree_excludes_done() {
        let mut graph = WorkGraph::new();

        let mut blocker = make_task("blocker", "Blocker");
        blocker.status = Status::Done;

        let mut blocked = make_task("blocked", "Blocked");
        blocked.after = vec!["blocker".to_string()];

        graph.add_node(Node::Task(blocker));
        graph.add_node(Node::Task(blocked));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "blocked", &mut visited, dir);

        assert_eq!(tree.nodes[tree.root].id, "blocked");
        assert!(tree.nodes[tree.root].children.is_empty()); // Done blocker excluded
    }

    #[test]
    fn test_build_blocking_tree_handles_cycles() {
        let mut graph = WorkGraph::new();

        let mut t1 = make_task("t1", "Task 1");
        t1.after = vec!["t2".to_string()];

        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "t1", &mut visited, dir);

        // Should not infinite loop - t2 will be a child but t1 won't be repeated
        let root = &tree.nodes[tree.root];
        assert_eq!(root.id, "t1");
        assert_eq!(root.children.len(), 1);
        let child = &tree.nodes[root.children[0]];
        assert_eq!(child.id, "t2");
        // t2's children should be empty because t1 was already visited
        assert!(child.children.is_empty());
    }

    #[test]
    fn test_collect_root_blockers() {
        let mut graph = WorkGraph::new();

        let root = make_task("root", "Root");
        let mut mid = make_task("mid", "Middle");
        mid.after = vec!["root".to_string()];
        let mut leaf = make_task("leaf", "Leaf");
        leaf.after = vec!["mid".to_string()];

        graph.add_node(Node::Task(root));
        graph.add_node(Node::Task(mid));
        graph.add_node(Node::Task(leaf));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "leaf", &mut visited, dir);

        let mut roots = HashSet::new();
        collect_root_blockers(&graph, &tree, &mut roots);

        assert_eq!(roots.len(), 1);
        assert!(roots.contains("root"));
    }

    #[test]
    fn test_count_blockers() {
        let mut graph = WorkGraph::new();

        let t1 = make_task("t1", "Task 1");
        let mut t2 = make_task("t2", "Task 2");
        t2.after = vec!["t1".to_string()];
        let mut t3 = make_task("t3", "Task 3");
        t3.after = vec!["t2".to_string()];

        graph.add_node(Node::Task(t1));
        graph.add_node(Node::Task(t2));
        graph.add_node(Node::Task(t3));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "t3", &mut visited, dir);

        assert_eq!(count_blockers(&tree), 2);
    }

    #[test]
    fn test_is_task_ready() {
        let mut graph = WorkGraph::new();

        let mut blocker = make_task("blocker", "Blocker");
        blocker.status = Status::Done;

        let mut blocked = make_task("blocked", "Blocked");
        blocked.after = vec!["blocker".to_string()];

        graph.add_node(Node::Task(blocker));
        graph.add_node(Node::Task(blocked.clone()));

        let dir = Path::new("/tmp");

        // blocked task is ready because blocker is done
        assert!(is_task_ready(&graph, &blocked, dir));

        // Now test with an open blocker
        let mut graph2 = WorkGraph::new();
        let blocker2 = make_task("blocker", "Blocker");
        let mut blocked2 = make_task("blocked", "Blocked");
        blocked2.after = vec!["blocker".to_string()];

        graph2.add_node(Node::Task(blocker2));
        graph2.add_node(Node::Task(blocked2.clone()));

        assert!(!is_task_ready(&graph2, &blocked2, dir));
    }

    #[test]
    fn test_eval_satellite_reports_dispatcher_bypass_not_root_blocker() {
        for status in [Status::PendingEval, Status::FailedPendingEval] {
            let mut graph = WorkGraph::new();
            let mut source = make_task("source", "Source");
            source.status = status;
            let mut flip = make_task(".flip-source", "FLIP");
            flip.after = vec!["source".to_string()];
            graph.add_node(Node::Task(source));
            graph.add_node(Node::Task(flip.clone()));

            let mut visited = HashSet::new();
            let tree = build_blocking_tree(&graph, ".flip-source", &mut visited, Path::new("/tmp"));
            assert!(tree.nodes[tree.root].children.is_empty());
            assert_eq!(
                tree.nodes[tree.root].eval_bypasses,
                vec![("source".to_string(), status)]
            );
            assert!(is_task_ready(&graph, &flip, Path::new("/tmp")));
        }
    }

    #[test]
    fn test_unrelated_system_rows_do_not_inherit_eval_bypass() {
        for id in [".assign-source", ".verify-source", ".other"] {
            let mut graph = WorkGraph::new();
            let mut source = make_task("source", "Source");
            source.status = Status::FailedPendingEval;
            let mut dependent = make_task(id, id);
            dependent.after = vec!["source".to_string()];
            graph.add_node(Node::Task(source));
            graph.add_node(Node::Task(dependent.clone()));
            assert!(!is_task_ready(&graph, &dependent, Path::new("/tmp")));
        }
    }

    #[test]
    fn deep_blocking_chain_uses_iterative_arena() {
        let mut graph = WorkGraph::new();
        for index in 0..2_000usize {
            let mut task = make_task(&format!("deep-{index:04}"), "Deep blocker");
            if index > 0 {
                task.after = vec![format!("deep-{:04}", index - 1)];
            }
            graph.add_node(Node::Task(task));
        }
        let mut visited = HashSet::new();
        let tree = build_blocking_tree(&graph, "deep-1999", &mut visited, Path::new("/tmp"));
        assert_eq!(tree.nodes.len(), 2_000);
        assert_eq!(count_blockers(&tree), 1_999);
        let mut roots = HashSet::new();
        collect_root_blockers(&graph, &tree, &mut roots);
        assert_eq!(roots, HashSet::from(["deep-0000".to_string()]));
    }

    #[test]
    fn test_collect_root_blockers_includes_in_progress() {
        let mut graph = WorkGraph::new();

        let mut root = make_task("root", "Root");
        root.status = Status::InProgress;
        let mut leaf = make_task("leaf", "Leaf");
        leaf.after = vec!["root".to_string()];

        graph.add_node(Node::Task(root));
        graph.add_node(Node::Task(leaf));

        let mut visited = HashSet::new();
        let dir = Path::new("/tmp");
        let tree = build_blocking_tree(&graph, "leaf", &mut visited, dir);

        let mut roots = HashSet::new();
        collect_root_blockers(&graph, &tree, &mut roots);

        assert_eq!(roots.len(), 1);
        assert!(roots.contains("root"));
    }
}
