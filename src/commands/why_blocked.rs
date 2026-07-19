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
    eval_bypasses: Vec<(String, Status)>,
    children: Vec<BlockingNode>,
}

/// Root blocker information
#[derive(Debug, Clone)]
struct RootBlocker<'a> {
    task: &'a Task,
    is_ready: bool,
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
            graph.get_task(rid).is_none() && worksgood::federation::parse_remote_ref(rid).is_none()
        })
        .cloned()
        .collect();

    // Count total blocking tasks
    let total_blockers = count_blockers(&blocking_tree);

    if json {
        print_json(
            task,
            &blocking_tree,
            &root_blockers,
            &phantom_root_ids,
            total_blockers,
        )?;
    } else {
        print_human(
            task,
            &blocking_tree,
            &root_blockers,
            &phantom_root_ids,
            total_blockers,
        );
    }

    Ok(())
}

fn build_blocking_tree(
    graph: &WorkGraph,
    task_id: &str,
    visited: &mut HashSet<String>,
    dir: &Path,
) -> BlockingNode {
    let task = graph.get_task(task_id);
    let is_phantom = task.is_none() && worksgood::federation::parse_remote_ref(task_id).is_none();
    let status = task.map(|t| t.status).unwrap_or(Status::Open);

    let mut node = BlockingNode {
        id: task_id.to_string(),
        status,
        is_phantom,
        failure_reason: task.and_then(|task| task.failure_reason.clone()),
        eval_bypasses: vec![],
        children: vec![],
    };

    if visited.contains(task_id) {
        return node; // Avoid cycles
    }
    visited.insert(task_id.to_string());

    if let Some(task) = task {
        for blocker_id in &task.after {
            // Skip if already visited (cycle detection)
            if visited.contains(blocker_id) {
                continue;
            }

            if let Some((_peer_name, _remote_task_id)) =
                worksgood::federation::parse_remote_ref(blocker_id)
            {
                // Cross-repo dependency — resolve remote status
                let remote = worksgood::federation::resolve_remote_task_status(
                    _peer_name,
                    _remote_task_id,
                    dir,
                );
                if !remote.status.is_terminal() {
                    let child = BlockingNode {
                        id: blocker_id.clone(),
                        status: remote.status,
                        is_phantom: false,
                        failure_reason: None,
                        eval_bypasses: vec![],
                        children: vec![], // Don't recurse into remote graphs
                    };
                    node.children.push(child);
                }
            } else if graph.get_task(blocker_id).is_some() {
                match worksgood::query::dependency_disposition(
                    blocker_id,
                    &task.id,
                    graph,
                    Some(dir),
                ) {
                    worksgood::query::DependencyDisposition::Satisfied => {}
                    worksgood::query::DependencyDisposition::EvalSystemBypass {
                        blocker_status,
                    } => node
                        .eval_bypasses
                        .push((blocker_id.clone(), blocker_status)),
                    worksgood::query::DependencyDisposition::Blocked { .. } => {
                        let child = build_blocking_tree(graph, blocker_id, visited, dir);
                        node.children.push(child);
                    }
                }
            } else {
                // Phantom dependency — task doesn't exist in the graph
                let child = BlockingNode {
                    id: blocker_id.clone(),
                    status: Status::Open,
                    is_phantom: true,
                    failure_reason: None,
                    eval_bypasses: vec![],
                    children: vec![],
                };
                node.children.push(child);
            }
        }
    }

    node
}

fn collect_root_blockers(graph: &WorkGraph, node: &BlockingNode, roots: &mut HashSet<String>) {
    if node.children.is_empty() {
        if node.is_phantom {
            // Phantom dependency is always a root blocker
            roots.insert(node.id.clone());
        } else if let Some(task) = graph.get_task(&node.id) {
            // It's a root blocker if it's not terminal (still open, in-progress, or blocked)
            if !task.status.is_terminal() {
                roots.insert(node.id.clone());
            }
        }
    } else {
        for child in &node.children {
            collect_root_blockers(graph, child, roots);
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

fn count_blockers(node: &BlockingNode) -> usize {
    let mut count = 0;
    let mut visited = HashSet::new();
    count_blockers_recursive(node, &mut count, &mut visited);
    count
}

fn count_blockers_recursive(node: &BlockingNode, count: &mut usize, visited: &mut HashSet<String>) {
    for child in &node.children {
        if !visited.contains(&child.id) {
            visited.insert(child.id.clone());
            *count += 1;
            count_blockers_recursive(child, count, visited);
        }
    }
}

fn print_human(
    task: &Task,
    tree: &BlockingNode,
    root_blockers: &[RootBlocker],
    phantom_roots: &[String],
    total: usize,
) {
    println!("Task: {}", task.id);

    if tree.children.is_empty() {
        println!("Status: {:?}", task.status);
        println!();
        if tree.eval_bypasses.is_empty() {
            println!("{} has no blockers.", task.id);
        } else {
            println!(
                "{} is dispatcher-ready via evaluation-system bypass.",
                task.id
            );
            for (blocker, status) in &tree.eval_bypasses {
                println!(
                    "  {}: {} — evaluation-system bypass (this satellite is part of {}'s gate)",
                    blocker, status, blocker
                );
            }
            if let Some(reason) = task.failure_reason.as_deref() {
                println!("Lifecycle health: {}", reason);
            }
        }
        return;
    }

    println!("Status: blocked (transitively)");
    println!();
    println!("Blocking chain:");
    println!();
    print_tree(tree, "", 0);

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

fn print_tree(node: &BlockingNode, prefix: &str, depth: usize) {
    if depth == 0 {
        // Root node - just print the ID
        println!("{}", node.id);
    } else if node.is_phantom {
        // Phantom dependency — clearly label as non-existent
        println!(
            "{} \\-- blocked by: {} (DOES NOT EXIST — phantom dependency) <-- ROOT CAUSE",
            prefix, node.id
        );
    } else {
        // Child node - print with tree connector and status
        let status_str = format!("(status: {:?})", node.status);
        let root_marker = if node.children.is_empty() && !node.status.is_terminal() {
            " <-- ROOT CAUSE"
        } else {
            ""
        };
        println!(
            "{} \\-- blocked by: {} {}{}",
            prefix, node.id, status_str, root_marker
        );
        if let Some(reason) = node.failure_reason.as_deref() {
            println!("{}     lifecycle health: {}", prefix, reason);
        }
    }

    // Calculate the prefix for children
    let child_prefix = if depth == 0 {
        "".to_string()
    } else {
        format!("{}     ", prefix)
    };

    for child in &node.children {
        print_tree(child, &child_prefix, depth + 1);
    }
}

fn print_json(
    task: &Task,
    tree: &BlockingNode,
    root_blockers: &[RootBlocker],
    phantom_roots: &[String],
    total: usize,
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
    let output = serde_json::json!({
        "task": {
            "id": task.id,
            "title": task.title,
            "status": task.status,
        },
        "dispatcher_ready_via_evaluation_system_bypass": tree.children.is_empty() && !tree.eval_bypasses.is_empty(),
        "is_blocked": !tree.children.is_empty(),
        "blocking_chain": tree_to_json(tree),
        "root_blockers": all_root_blockers,
        "total_blockers": total,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn tree_to_json(node: &BlockingNode) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "id": node.id,
        "status": format!("{:?}", node.status),
        "after": node.children.iter().map(tree_to_json).collect::<Vec<_>>(),
    });
    if node.is_phantom {
        obj["phantom"] = serde_json::Value::Bool(true);
    }
    if let Some(reason) = node.failure_reason.as_deref() {
        obj["failure_reason"] = serde_json::Value::String(reason.to_string());
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

        assert_eq!(tree.id, "t1");
        assert!(tree.children.is_empty());
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

        assert_eq!(tree.id, "blocked");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].id, "blocker");
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

        assert_eq!(tree.id, "t3");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].id, "t2");
        assert_eq!(tree.children[0].children.len(), 1);
        assert_eq!(tree.children[0].children[0].id, "t1");
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

        assert_eq!(tree.id, "blocked");
        assert!(tree.children.is_empty()); // Done blocker excluded
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
        assert_eq!(tree.id, "t1");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].id, "t2");
        // t2's children should be empty because t1 was already visited
        assert!(tree.children[0].children.is_empty());
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
            assert!(tree.children.is_empty());
            assert_eq!(tree.eval_bypasses, vec![("source".to_string(), status)]);
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
