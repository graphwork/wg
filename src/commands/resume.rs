use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use workgraph::graph::{LogEntry, WorkGraph};
use workgraph::parser::modify_graph;

use super::eval_scaffold;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use workgraph::parser::load_graph;

pub fn run(dir: &Path, id: &str, only: bool) -> Result<()> {
    run_inner(dir, id, Mode::Subgraph(only), false)
}

/// Publish a draft task (alias for resume with validation messaging).
///
/// `only` and `wcc` are mutually exclusive at the CLI layer; here `wcc`
/// wins if the caller passes both.
pub fn publish(dir: &Path, id: &str, only: bool, wcc: bool) -> Result<()> {
    let mode = if wcc { Mode::Wcc } else { Mode::Subgraph(only) };
    run_inner(dir, id, mode, true)
}

/// What to release relative to the seed task.
///
/// * `Subgraph(true)`  — only the seed task (caller passed `--only`).
/// * `Subgraph(false)` — seed + downstream consumers (default).
/// * `Wcc`             — every task in the seed's weakly-connected component
///                       (caller passed `--wcc`). Released in topological
///                       order so a task being unpaused already has all of
///                       its dependencies unpaused.
#[derive(Clone, Copy)]
enum Mode {
    Subgraph(bool),
    Wcc,
}

fn run_inner(dir: &Path, id: &str, mode: Mode, is_publish: bool) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Use modify_graph for atomic load-modify-save under a single exclusive
    // lock.  This prevents the coordinator's own modify_graph from
    // interleaving and overwriting our paused-flag change with a stale
    // snapshot (the root cause of the "publish doesn't clear paused" bug).
    let mut error: Option<anyhow::Error> = None;
    let mut unpaused: Vec<String> = Vec::new();

    let _graph = modify_graph(&path, |graph| {
        // Verify seed task exists and is paused
        let task = match graph.get_task(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };
        if !task.paused {
            error = Some(anyhow::anyhow!("Task '{}' is not paused", id));
            return false;
        }

        match mode {
            Mode::Subgraph(true) => {
                // Single-task mode: validate just this task's deps, then unpause
                if let Err(e) = validate_task_deps(graph, id, is_publish) {
                    error = Some(e);
                    return false;
                }
                let action = if is_publish { "published" } else { "resumed" };
                unpause_task(graph, id, action);
                unpaused.push(id.to_string());

                // Eagerly scaffold agency pipeline (idempotent — skips if already scaffolded)
                scaffold_eval_for_unpaused(dir, graph, &[id.to_string()], action);
            }
            Mode::Subgraph(false) => {
                // Propagating mode: discover subgraph, validate all, unpause all
                let subgraph = discover_downstream(graph, id);

                // Validate the entire subgraph structure
                if let Err(e) = validate_subgraph(graph, &subgraph, is_publish) {
                    error = Some(e);
                    return false;
                }

                // Atomic unpause: all paused tasks in the subgraph
                let action = if is_publish { "published" } else { "resumed" };
                for task_id in &subgraph {
                    let t = graph.get_task(task_id).unwrap();
                    if t.paused {
                        unpaused.push(task_id.clone());
                    }
                }
                for task_id in &unpaused {
                    unpause_task(graph, task_id, action);
                }

                // Eagerly scaffold agency pipeline (idempotent — skips if already scaffolded)
                scaffold_eval_for_unpaused(dir, graph, &unpaused, action);
            }
            Mode::Wcc => {
                // WCC mode: discover the entire weakly-connected component,
                // validate it as a whole, then release in topological order
                // (deps before dependents) so a task being unpaused already
                // has all of its dependencies unpaused.
                let component = discover_wcc(graph, id);

                if let Err(e) = validate_subgraph(graph, &component, is_publish) {
                    error = Some(e);
                    return false;
                }

                let ordered = topo_sort_subset(graph, &component);
                let action = if is_publish { "published" } else { "resumed" };
                for task_id in &ordered {
                    let t = graph.get_task(task_id).unwrap();
                    if t.paused {
                        unpause_task(graph, task_id, action);
                        unpaused.push(task_id.clone());
                    }
                }

                // Eagerly scaffold agency pipeline for every newly-unpaused task.
                scaffold_eval_for_unpaused(dir, graph, &unpaused, action);
            }
        }

        true
    })
    .context("Failed to save graph")?;

    // Propagate any validation/logic error that occurred inside the closure
    if let Some(e) = error {
        return Err(e);
    }

    // Kick the dispatcher: bypass settling delay so the user sees agent
    // activity within sub-second after publish/resume succeeds.
    super::notify_kick(dir);
    record_provenance(dir, id, is_publish);

    match mode {
        Mode::Subgraph(true) => {
            if is_publish {
                println!("Published '{}' — task is now available for dispatch", id);
            } else {
                println!("Resumed '{}'", id);
            }
        }
        Mode::Subgraph(false) => {
            let verb = if is_publish { "Published" } else { "Resumed" };
            println!(
                "{} '{}' and {} downstream task(s)",
                verb,
                id,
                unpaused.len().saturating_sub(1)
            );
        }
        Mode::Wcc => {
            let verb = if is_publish { "Published" } else { "Resumed" };
            println!(
                "{} '{}' and {} task(s) in the weakly-connected component",
                verb,
                id,
                unpaused.len().saturating_sub(1)
            );
        }
    }

    Ok(())
}

/// Discover all tasks reachable downstream from the seed task.
/// "Downstream" means: tasks whose `after` list includes a member of the subgraph,
/// plus tasks reachable via `before` edges from the subgraph.
fn discover_downstream(graph: &WorkGraph, seed_id: &str) -> Vec<String> {
    // Build a reverse index: for each task, which tasks depend on it (have it in `after`)?
    let mut dependents: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for task in graph.tasks() {
        for dep_id in &task.after {
            dependents
                .entry(dep_id.clone())
                .or_default()
                .push(task.id.clone());
        }
    }

    // Also include `before` edges: if A has B in `before`, B depends on A,
    // so B is downstream of A.
    for task in graph.tasks() {
        for downstream_id in &task.before {
            dependents
                .entry(task.id.clone())
                .or_default()
                .push(downstream_id.clone());
        }
    }

    // BFS from seed
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(seed_id.to_string());
    queue.push_back(seed_id.to_string());

    while let Some(current) = queue.pop_front() {
        if let Some(deps) = dependents.get(&current) {
            for dep in deps {
                // Only include actual tasks (not resources, not missing)
                if graph.get_task(dep).is_some() && visited.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    let mut result: Vec<String> = visited.into_iter().collect();
    result.sort(); // deterministic order
    result
}

/// Discover the entire weakly-connected component containing `seed_id`.
///
/// Treats both `after` and `before` edges as undirected so multi-root
/// fan-outs (5 audits → 1 synthesis, 5 audits ← 1 setup) collapse to a
/// single component when any node is the publish seed. Edges to
/// non-existent nodes (resources, federation refs) are ignored — the
/// component contains only real, local tasks.
fn discover_wcc(graph: &WorkGraph, seed_id: &str) -> Vec<String> {
    let mut adjacency: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for task in graph.tasks() {
        for dep_id in &task.after {
            if graph.get_task(dep_id).is_some() {
                adjacency
                    .entry(task.id.clone())
                    .or_default()
                    .push(dep_id.clone());
                adjacency
                    .entry(dep_id.clone())
                    .or_default()
                    .push(task.id.clone());
            }
        }
        for downstream_id in &task.before {
            if graph.get_task(downstream_id).is_some() {
                adjacency
                    .entry(task.id.clone())
                    .or_default()
                    .push(downstream_id.clone());
                adjacency
                    .entry(downstream_id.clone())
                    .or_default()
                    .push(task.id.clone());
            }
        }
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(seed_id.to_string());
    queue.push_back(seed_id.to_string());

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adjacency.get(&current) {
            for n in neighbors {
                if visited.insert(n.clone()) {
                    queue.push_back(n.clone());
                }
            }
        }
    }

    let mut result: Vec<String> = visited.into_iter().collect();
    result.sort();
    result
}

/// Topologically order a subset of task ids: dependencies first, dependents
/// last. Tie-breaks lexicographically so the order is deterministic.
///
/// If a cycle is present (already permitted by `validate_subgraph` when the
/// cycle has `cycle_config`), unsorted members are appended at the end —
/// the WCC release path still unpauses every task, just without a strict
/// ordering guarantee inside the cycle.
fn topo_sort_subset(graph: &WorkGraph, subset: &[String]) -> Vec<String> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    let in_subset: HashSet<&str> = subset.iter().map(|s| s.as_str()).collect();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for id in subset {
        in_degree.insert(id.clone(), 0);
        adjacency.insert(id.clone(), Vec::new());
    }

    for id in subset {
        let task = graph.get_task(id).unwrap();
        for dep_id in &task.after {
            if in_subset.contains(dep_id.as_str()) {
                *in_degree.get_mut(id).unwrap() += 1;
                adjacency.get_mut(dep_id).unwrap().push(id.clone());
            }
        }
        for downstream in &task.before {
            if in_subset.contains(downstream.as_str()) {
                *in_degree.get_mut(downstream).unwrap() += 1;
                adjacency.get_mut(id).unwrap().push(downstream.clone());
            }
        }
    }

    let mut queue: BinaryHeap<Reverse<String>> = BinaryHeap::new();
    for (id, deg) in &in_degree {
        if *deg == 0 {
            queue.push(Reverse(id.clone()));
        }
    }
    let mut result = Vec::with_capacity(subset.len());
    while let Some(Reverse(id)) = queue.pop() {
        if let Some(neighbors) = adjacency.get(&id).cloned() {
            for n in neighbors {
                let entry = in_degree.get_mut(&n).unwrap();
                *entry -= 1;
                if *entry == 0 {
                    queue.push(Reverse(n));
                }
            }
        }
        result.push(id);
    }

    if result.len() < subset.len() {
        let placed: HashSet<String> = result.iter().cloned().collect();
        for id in subset {
            if !placed.contains(id) {
                result.push(id.clone());
            }
        }
    }
    result
}

/// Validate a single task's `after` dependencies exist.
fn validate_task_deps(graph: &WorkGraph, task_id: &str, is_publish: bool) -> Result<()> {
    let task = graph.get_task_or_err(task_id)?;
    let mut missing = Vec::new();
    for dep_id in &task.after {
        if workgraph::federation::parse_remote_ref(dep_id).is_some() {
            continue;
        }
        if graph.get_node(dep_id).is_none() {
            let mut msg = format!("'{}'", dep_id);
            let all_ids: Vec<&str> = graph.tasks().map(|t| t.id.as_str()).collect();
            if let Some((suggestion, _)) =
                workgraph::check::fuzzy_match_task_id(dep_id, all_ids.iter().copied(), 3)
            {
                msg.push_str(&format!(" (did you mean '{}'?)", suggestion));
            }
            missing.push(msg);
        }
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "Cannot {} task '{}': dangling dependencies:\n  {}",
            if is_publish { "publish" } else { "resume" },
            task_id,
            missing.join("\n  ")
        );
    }
    Ok(())
}

/// Validate the entire subgraph structure before unpausing.
fn validate_subgraph(graph: &WorkGraph, subgraph: &[String], is_publish: bool) -> Result<()> {
    let action = if is_publish { "publish" } else { "resume" };
    let mut errors = Vec::new();

    for task_id in subgraph {
        let task = graph.get_task(task_id).unwrap();

        // Check for dangling after-dependencies
        for dep_id in &task.after {
            if workgraph::federation::parse_remote_ref(dep_id).is_some() {
                continue;
            }
            if graph.get_node(dep_id).is_none() {
                let mut msg = format!("Task '{}': dangling dependency '{}'", task_id, dep_id);
                let all_ids: Vec<&str> = graph.tasks().map(|t| t.id.as_str()).collect();
                if let Some((suggestion, _)) =
                    workgraph::check::fuzzy_match_task_id(dep_id, all_ids.iter().copied(), 3)
                {
                    msg.push_str(&format!(" (did you mean '{}'?)", suggestion));
                }
                errors.push(msg);
            }
        }
    }

    // Check cycle validity: any cycle in the subgraph must have max_iterations configured
    let subgraph_set: HashSet<&str> = subgraph.iter().map(|s| s.as_str()).collect();
    let cycle_analysis = workgraph::graph::CycleAnalysis::from_graph(graph);
    for cycle in &cycle_analysis.cycles {
        // Check if this cycle intersects with our subgraph
        let members_in_subgraph: Vec<&str> = cycle
            .members
            .iter()
            .filter(|id| subgraph_set.contains(id.as_str()))
            .map(|s| s.as_str())
            .collect();
        if members_in_subgraph.len() > 1 {
            // This is a real cycle — check if any task has cycle_config
            let has_config = members_in_subgraph.iter().any(|id| {
                graph
                    .get_task(id)
                    .map(|t| t.cycle_config.is_some())
                    .unwrap_or(false)
            });
            if !has_config {
                errors.push(format!(
                    "Cycle without --max-iterations: [{}]",
                    members_in_subgraph.join(", ")
                ));
            }
        }
    }

    if !errors.is_empty() {
        anyhow::bail!(
            "Cannot {} subgraph — structural errors:\n  {}",
            action,
            errors.join("\n  ")
        );
    }

    Ok(())
}

fn unpause_task(graph: &mut WorkGraph, task_id: &str, action: &str) {
    let task = graph.get_task_mut(task_id).unwrap();
    task.paused = false;
    task.log.push(LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        actor: None,
        user: Some(workgraph::current_user()),
        message: format!("Task {}", action),
    });
}

/// Create the full agency pipeline (`.assign-*`, `.flip-*`,
/// `.evaluate-*`) for each unpaused task in one atomic pass.
///
/// All tasks and their edges are written together into the same graph
/// object before the caller saves — guaranteeing a single, atomic write.
/// Idempotent: skips tasks that already have scaffold siblings.
fn scaffold_eval_for_unpaused(
    dir: &Path,
    graph: &mut WorkGraph,
    task_ids: &[String],
    action: &str,
) {
    let config = workgraph::config::Config::load_or_default(dir);

    // Collect (id, title) pairs, filtering out system tasks
    let candidates: Vec<(String, String)> = task_ids
        .iter()
        .filter(|id| !workgraph::graph::is_system_task(id))
        .filter_map(|id| graph.get_task(id).map(|t| (id.clone(), t.title.clone())))
        .collect();

    // Scaffold the full pipeline (.place → .assign → task → .flip → .evaluate)
    let count = eval_scaffold::scaffold_full_pipeline_batch(dir, graph, &candidates, &config);
    if count > 0 {
        eprintln!(
            "[{}] Eagerly scaffolded full agency pipeline for {} task(s)",
            action, count
        );
    }
}

fn record_provenance(dir: &Path, id: &str, is_publish: bool) {
    let config = workgraph::config::Config::load_or_default(dir);
    let op = if is_publish { "publish" } else { "resume" };
    let _ = workgraph::provenance::record(
        dir,
        op,
        Some(id),
        None,
        serde_json::json!({}),
        config.log.rotation_threshold,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use workgraph::graph::{CycleConfig, Node, Status, Task, WorkGraph};
    use workgraph::parser::save_graph;

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

    // --- Single-task (--only) tests ---

    #[test]
    fn test_resume_paused_task_only() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        let result = run(dir.path(), "t1", true);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.paused);
    }

    #[test]
    fn test_resume_not_paused_fails() {
        let dir = tempdir().unwrap();
        setup_workgraph(dir.path(), vec![make_task("t1", "Test", Status::Open)]);

        let result = run(dir.path(), "t1", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not paused"));
    }

    #[test]
    fn test_resume_nonexistent_task_fails() {
        let dir = tempdir().unwrap();
        setup_workgraph(dir.path(), vec![]);

        let result = run(dir.path(), "nonexistent", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_resume_only_adds_log_entry() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        run(dir.path(), "t1", true).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.log.len(), 1);
        assert!(task.log[0].message.contains("resumed"));
    }

    #[test]
    fn test_resume_only_with_dangling_dep_fails() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        task.after = vec!["nonexistent-dep".to_string()];
        setup_workgraph(dir.path(), vec![task]);

        let result = run(dir.path(), "t1", true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("dangling dependencies"), "got: {msg}");
        assert!(msg.contains("nonexistent-dep"), "got: {msg}");
    }

    #[test]
    fn test_resume_only_with_valid_deps_succeeds() {
        let dir = tempdir().unwrap();
        let dep = make_task("dep1", "Dependency", Status::Open);
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        task.after = vec!["dep1".to_string()];
        setup_workgraph(dir.path(), vec![dep, task]);

        let result = run(dir.path(), "t1", true);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.paused);
    }

    // --- Propagating resume tests ---

    #[test]
    fn test_propagating_resume_unpauses_chain() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("research", "Research X", Status::Open);
        t1.paused = true;
        let mut t2 = make_task("implement", "Implement X", Status::Open);
        t2.paused = true;
        t2.after = vec!["research".to_string()];
        let mut t3 = make_task("test-x", "Test X", Status::Open);
        t3.paused = true;
        t3.after = vec!["implement".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2, t3]);

        let result = run(dir.path(), "research", false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("research").unwrap().paused);
        assert!(!graph.get_task("implement").unwrap().paused);
        assert!(!graph.get_task("test-x").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_only_flag_unpauses_single() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("research", "Research X", Status::Open);
        t1.paused = true;
        let mut t2 = make_task("implement", "Implement X", Status::Open);
        t2.paused = true;
        t2.after = vec!["research".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2]);

        let result = run(dir.path(), "research", true);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("research").unwrap().paused);
        // Downstream task should still be paused
        assert!(graph.get_task("implement").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_dangling_dep_in_subgraph_fails() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("research", "Research X", Status::Open);
        t1.paused = true;
        let mut t2 = make_task("implement", "Implement X", Status::Open);
        t2.paused = true;
        t2.after = vec!["research".to_string(), "missing-task".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2]);

        let result = run(dir.path(), "research", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("structural errors"), "got: {msg}");
        assert!(msg.contains("missing-task"), "got: {msg}");

        // Nothing should have been unpaused (atomic)
        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(graph.get_task("research").unwrap().paused);
        assert!(graph.get_task("implement").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_does_not_affect_unrelated_tasks() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("a", "Task A", Status::Open);
        t1.paused = true;
        let mut t2 = make_task("b", "Task B", Status::Open);
        t2.paused = true;
        t2.after = vec!["a".to_string()];
        let mut t3 = make_task("unrelated", "Unrelated", Status::Open);
        t3.paused = true;
        setup_workgraph(dir.path(), vec![t1, t2, t3]);

        run(dir.path(), "a", false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("a").unwrap().paused);
        assert!(!graph.get_task("b").unwrap().paused);
        // Unrelated task should still be paused
        assert!(graph.get_task("unrelated").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_skips_already_unpaused() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("a", "Task A", Status::Open);
        t1.paused = true;
        let mut t2 = make_task("b", "Task B", Status::Open);
        // t2 is NOT paused, but is downstream
        t2.after = vec!["a".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2]);

        let result = run(dir.path(), "a", false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("a").unwrap().paused);
        assert!(!graph.get_task("b").unwrap().paused);
        // b should have no log entry since it wasn't paused
        assert!(graph.get_task("b").unwrap().log.is_empty());
    }

    #[test]
    fn test_propagating_resume_diamond_shape() {
        let dir = tempdir().unwrap();
        let mut root = make_task("root", "Root", Status::Open);
        root.paused = true;
        let mut left = make_task("left", "Left", Status::Open);
        left.paused = true;
        left.after = vec!["root".to_string()];
        let mut right = make_task("right", "Right", Status::Open);
        right.paused = true;
        right.after = vec!["root".to_string()];
        let mut join = make_task("join", "Join", Status::Open);
        join.paused = true;
        join.after = vec!["left".to_string(), "right".to_string()];
        setup_workgraph(dir.path(), vec![root, left, right, join]);

        run(dir.path(), "root", false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        for id in &["root", "left", "right", "join"] {
            assert!(
                !graph.get_task(id).unwrap().paused,
                "{} should be unpaused",
                id
            );
        }
    }

    // --- Publish tests ---

    #[test]
    fn test_publish_with_dangling_dep_fails() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        task.after = vec!["missing-task".to_string()];
        setup_workgraph(dir.path(), vec![task]);

        let result = publish(dir.path(), "t1", false, false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("structural errors"), "got: {msg}");
        assert!(msg.contains("dangling"), "got: {msg}");
    }

    #[test]
    fn test_publish_with_valid_deps_succeeds() {
        let dir = tempdir().unwrap();
        let dep = make_task("dep1", "Dependency", Status::Open);
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        task.after = vec!["dep1".to_string()];
        setup_workgraph(dir.path(), vec![dep, task]);

        let result = publish(dir.path(), "t1", false, false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.paused);
        assert!(task.log.last().unwrap().message.contains("published"));
    }

    #[test]
    fn test_publish_no_deps_succeeds() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        let result = publish(dir.path(), "t1", false, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resume_with_multiple_dangling_deps_lists_all() {
        let dir = tempdir().unwrap();
        let mut task = make_task("t1", "Test", Status::Open);
        task.paused = true;
        task.after = vec!["missing-a".to_string(), "missing-b".to_string()];
        setup_workgraph(dir.path(), vec![task]);

        let result = run(dir.path(), "t1", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("missing-a"), "got: {msg}");
        assert!(msg.contains("missing-b"), "got: {msg}");
    }

    #[test]
    fn test_propagating_resume_with_before_edges() {
        // Test that `before` edges are followed for downstream discovery
        let dir = tempdir().unwrap();
        let mut t1 = make_task("seed", "Seed", Status::Open);
        t1.paused = true;
        t1.before = vec!["downstream".to_string()];
        let mut t2 = make_task("downstream", "Downstream", Status::Open);
        t2.paused = true;
        setup_workgraph(dir.path(), vec![t1, t2]);

        run(dir.path(), "seed", false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("seed").unwrap().paused);
        assert!(!graph.get_task("downstream").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_cycle_without_max_iterations_fails() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("a", "Task A", Status::Open);
        t1.paused = true;
        t1.after = vec!["b".to_string()];
        let mut t2 = make_task("b", "Task B", Status::Open);
        t2.paused = true;
        t2.after = vec!["a".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2]);

        let result = run(dir.path(), "a", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Cycle without --max-iterations"), "got: {msg}");

        // Atomic: nothing unpaused
        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(graph.get_task("a").unwrap().paused);
        assert!(graph.get_task("b").unwrap().paused);
    }

    #[test]
    fn test_propagating_resume_cycle_with_max_iterations_succeeds() {
        let dir = tempdir().unwrap();
        let mut t1 = make_task("a", "Task A", Status::Open);
        t1.paused = true;
        t1.after = vec!["b".to_string()];
        t1.cycle_config = Some(CycleConfig {
            max_iterations: 3,
            guard: None,
            delay: None,
            no_converge: false,
            restart_on_failure: true,
            max_failure_restarts: None,
        });
        let mut t2 = make_task("b", "Task B", Status::Open);
        t2.paused = true;
        t2.after = vec!["a".to_string()];
        setup_workgraph(dir.path(), vec![t1, t2]);

        let result = run(dir.path(), "a", false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("a").unwrap().paused);
        assert!(!graph.get_task("b").unwrap().paused);
    }

    #[test]
    fn test_publish_creates_pipeline_tasks_with_auto_place() {
        // Verify that publish creates .assign-* and .evaluate-* tasks
        // (placement is handled by the assignment step, no separate .place-* tasks).
        let dir = tempdir().unwrap();
        let mut task = make_task("my-task", "My Task", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        // Enable auto_place in config (dir.path() IS the .wg dir)
        fs::write(
            dir.path().join("config.toml"),
            "[agency]\nauto_place = true\nauto_assign = true\nauto_evaluate = true\n",
        )
        .unwrap();

        let result = publish(dir.path(), "my-task", false, false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(
            graph.get_task(".assign-my-task").is_some(),
            ".assign-my-task must be created at publish time"
        );
        assert!(
            graph.get_task(".evaluate-my-task").is_some(),
            ".evaluate-my-task must be created at publish time"
        );
    }

    // --- Resume scaffolding tests ---

    #[test]
    fn test_resume_scaffolds_agency_pipeline_for_draft_task() {
        let dir = tempdir().unwrap();
        let mut task = make_task("my-task", "My Task", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        fs::write(
            dir.path().join("config.toml"),
            "[agency]\nauto_place = true\nauto_assign = true\nauto_evaluate = true\n",
        )
        .unwrap();

        let result = run(dir.path(), "my-task", false);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(
            graph.get_task(".assign-my-task").is_some(),
            ".assign-my-task must be created at resume time"
        );
        assert!(
            graph.get_task(".evaluate-my-task").is_some(),
            ".evaluate-my-task must be created at resume time"
        );
    }

    #[test]
    fn test_resume_scaffolds_agency_pipeline_only_mode() {
        let dir = tempdir().unwrap();
        let mut task = make_task("my-task", "My Task", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        fs::write(
            dir.path().join("config.toml"),
            "[agency]\nauto_assign = true\nauto_evaluate = true\n",
        )
        .unwrap();

        let result = run(dir.path(), "my-task", true);
        assert!(result.is_ok());

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(
            graph.get_task(".assign-my-task").is_some(),
            ".assign-my-task must be created at resume time (--only)"
        );
        assert!(
            graph.get_task(".evaluate-my-task").is_some(),
            ".evaluate-my-task must be created at resume time (--only)"
        );
    }

    #[test]
    fn test_resume_does_not_double_scaffold_already_published_task() {
        let dir = tempdir().unwrap();
        let mut task = make_task("my-task", "My Task", Status::Open);
        task.paused = true;
        setup_workgraph(dir.path(), vec![task]);

        fs::write(
            dir.path().join("config.toml"),
            "[agency]\nauto_assign = true\nauto_evaluate = true\n",
        )
        .unwrap();

        // First: publish (creates scaffold tasks)
        publish(dir.path(), "my-task", false, false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let assign = graph.get_task(".assign-my-task").unwrap();
        let assign_created_at = assign.created_at.clone();
        let eval = graph.get_task(".evaluate-my-task").unwrap();
        let eval_created_at = eval.created_at.clone();

        // Now pause and resume the task
        {
            let path = graph_path(dir.path());
            workgraph::parser::modify_graph(&path, |g| {
                let t = g.get_task_mut("my-task").unwrap();
                t.paused = true;
                true
            })
            .unwrap();
        }

        run(dir.path(), "my-task", false).unwrap();

        // Scaffold tasks should still be the same (not recreated)
        let graph = load_graph(graph_path(dir.path())).unwrap();
        let assign = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(assign.created_at, assign_created_at);
        let eval = graph.get_task(".evaluate-my-task").unwrap();
        assert_eq!(eval.created_at, eval_created_at);
    }

    // ── --wcc: weakly-connected component release ────────────────────────

    #[test]
    fn test_publish_wcc_releases_linear_chain_from_leaf() {
        // Validation row from the task spec: build a 5-node paused linear
        // chain, call `wg publish leaf --wcc`, ASSERT all 5 are open.
        let dir = tempdir().unwrap();
        let mut tasks = Vec::new();
        for i in 0..5 {
            let id = format!("n{}", i);
            let mut t = make_task(&id, &format!("Node {}", i), Status::Open);
            t.paused = true;
            if i > 0 {
                t.after = vec![format!("n{}", i - 1)];
            }
            tasks.push(t);
        }
        setup_workgraph(dir.path(), tasks);

        // Publish from the LEAF (downstream-most) node — the existing
        // default would only unpause the leaf because nothing is downstream
        // of it. WCC must still pull every node in the component.
        publish(dir.path(), "n4", false, true).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        for i in 0..5 {
            let id = format!("n{}", i);
            assert!(
                !graph.get_task(&id).unwrap().paused,
                "node {} should be unpaused after publish --wcc from leaf",
                id
            );
        }
    }

    #[test]
    fn test_publish_wcc_releases_diamond_from_join() {
        // Validation row: A → B; A → C; B → D; C → D (diamond).
        // `wg publish D --wcc` releases A, B, C, D.
        let dir = tempdir().unwrap();
        let mut a = make_task("a", "A", Status::Open);
        a.paused = true;
        let mut b = make_task("b", "B", Status::Open);
        b.paused = true;
        b.after = vec!["a".to_string()];
        let mut c = make_task("c", "C", Status::Open);
        c.paused = true;
        c.after = vec!["a".to_string()];
        let mut d = make_task("d", "D", Status::Open);
        d.paused = true;
        d.after = vec!["b".to_string(), "c".to_string()];
        setup_workgraph(dir.path(), vec![a, b, c, d]);

        publish(dir.path(), "d", false, true).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        for id in &["a", "b", "c", "d"] {
            assert!(
                !graph.get_task(id).unwrap().paused,
                "{} should be unpaused after publish d --wcc",
                id
            );
        }
    }

    #[test]
    fn test_publish_wcc_multi_root_fanout_synthesis() {
        // The motivating use case from the task description: 5 audit tasks
        // depend on a setup task; 1 synthesis task depends on all 5 audits.
        // `wg publish synthesis --wcc` (or `wg publish setup --wcc`, or
        // `wg publish audit-2 --wcc`) must release the whole shape with
        // ONE command.
        let dir = tempdir().unwrap();
        let mut setup = make_task("setup", "Setup", Status::Open);
        setup.paused = true;
        let mut audits = Vec::new();
        let mut audit_ids = Vec::new();
        for i in 0..5 {
            let id = format!("audit-{}", i);
            let mut t = make_task(&id, &format!("Audit {}", i), Status::Open);
            t.paused = true;
            t.after = vec!["setup".to_string()];
            audit_ids.push(id);
            audits.push(t);
        }
        let mut synth = make_task("synthesis", "Synthesis", Status::Open);
        synth.paused = true;
        synth.after = audit_ids.clone();

        let mut all = vec![setup];
        all.extend(audits);
        all.push(synth);
        setup_workgraph(dir.path(), all);

        // From the synthesis end.
        publish(dir.path(), "synthesis", false, true).unwrap();
        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("setup").unwrap().paused);
        for id in &audit_ids {
            assert!(
                !graph.get_task(id).unwrap().paused,
                "{} must be unpaused — WCC publish from synthesis",
                id
            );
        }
        assert!(!graph.get_task("synthesis").unwrap().paused);
    }

    #[test]
    fn test_publish_wcc_topological_release_order() {
        // Validation row: topological order at release time, verifiable
        // via the per-task log-entry timestamps. A task being unpaused
        // must have all of its `after` deps already unpaused.
        let dir = tempdir().unwrap();
        let mut a = make_task("root", "Root", Status::Open);
        a.paused = true;
        let mut b = make_task("mid", "Mid", Status::Open);
        b.paused = true;
        b.after = vec!["root".to_string()];
        let mut c = make_task("leaf", "Leaf", Status::Open);
        c.paused = true;
        c.after = vec!["mid".to_string()];
        setup_workgraph(dir.path(), vec![a, b, c]);

        publish(dir.path(), "leaf", false, true).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        // Each task got exactly one log entry from the unpause; we compare
        // those timestamps to assert root < mid < leaf.
        let ts = |id: &str| -> String {
            let t = graph.get_task(id).unwrap();
            assert_eq!(t.log.len(), 1, "{} should have 1 log entry", id);
            t.log[0].timestamp.clone()
        };
        let t_root = ts("root");
        let t_mid = ts("mid");
        let t_leaf = ts("leaf");
        assert!(
            t_root <= t_mid,
            "root ({}) should be unpaused at-or-before mid ({})",
            t_root,
            t_mid
        );
        assert!(
            t_mid <= t_leaf,
            "mid ({}) should be unpaused at-or-before leaf ({})",
            t_mid,
            t_leaf
        );
    }

    #[test]
    fn test_publish_wcc_does_not_touch_other_components() {
        // Two disjoint subgraphs. Publishing one component via --wcc must
        // leave the unrelated component fully paused.
        let dir = tempdir().unwrap();
        let mut a = make_task("a", "A", Status::Open);
        a.paused = true;
        let mut b = make_task("b", "B", Status::Open);
        b.paused = true;
        b.after = vec!["a".to_string()];
        let mut x = make_task("x", "X", Status::Open);
        x.paused = true;
        let mut y = make_task("y", "Y", Status::Open);
        y.paused = true;
        y.after = vec!["x".to_string()];
        setup_workgraph(dir.path(), vec![a, b, x, y]);

        publish(dir.path(), "b", false, true).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(!graph.get_task("a").unwrap().paused);
        assert!(!graph.get_task("b").unwrap().paused);
        // Unrelated component must stay paused.
        assert!(graph.get_task("x").unwrap().paused);
        assert!(graph.get_task("y").unwrap().paused);
    }

    #[test]
    fn test_publish_wcc_is_idempotent_on_already_unpaused() {
        // Mid-component task already unpaused should NOT cause an error;
        // remaining paused tasks in the WCC should still get released.
        let dir = tempdir().unwrap();
        let mut a = make_task("a", "A", Status::Open);
        a.paused = true;
        let b = make_task("b", "B", Status::Open); // not paused
        let mut c = make_task("c", "C", Status::Open);
        c.paused = true;
        c.after = vec!["b".to_string()];
        setup_workgraph(dir.path(), vec![a, b, c]); // a, b, c — note: a is isolated from b,c

        // Build a connected line a → b → c instead.
        let dir2 = tempdir().unwrap();
        let mut a2 = make_task("a", "A", Status::Open);
        a2.paused = true;
        let mut b2 = make_task("b", "B", Status::Open);
        b2.after = vec!["a".to_string()]; // unpaused mid-task
        let mut c2 = make_task("c", "C", Status::Open);
        c2.paused = true;
        c2.after = vec!["b".to_string()];
        setup_workgraph(dir2.path(), vec![a2, b2, c2]);

        publish(dir2.path(), "c", false, true).unwrap();
        let graph = load_graph(graph_path(dir2.path())).unwrap();
        assert!(!graph.get_task("a").unwrap().paused);
        assert!(!graph.get_task("b").unwrap().paused);
        assert!(!graph.get_task("c").unwrap().paused);
        // b was already unpaused; it should NOT have a log entry from publish.
        assert!(
            graph.get_task("b").unwrap().log.is_empty(),
            "b was not paused; publish must not log a re-unpause"
        );
    }

    #[test]
    fn test_publish_wcc_with_dangling_dep_fails_atomically() {
        // A WCC member with a dangling `after` dep must abort the whole
        // release (no partial unpause).
        let dir = tempdir().unwrap();
        let mut a = make_task("a", "A", Status::Open);
        a.paused = true;
        let mut b = make_task("b", "B", Status::Open);
        b.paused = true;
        b.after = vec!["a".to_string(), "missing".to_string()];
        setup_workgraph(dir.path(), vec![a, b]);

        let res = publish(dir.path(), "a", false, true);
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("missing"), "got: {msg}");

        // Atomic: nothing was unpaused.
        let graph = load_graph(graph_path(dir.path())).unwrap();
        assert!(graph.get_task("a").unwrap().paused);
        assert!(graph.get_task("b").unwrap().paused);
    }

    #[test]
    fn test_publish_wcc_follows_before_edges() {
        // `before` edges describe the inverse of `after` and should also
        // contribute to component connectivity.
        let dir = tempdir().unwrap();
        let mut a = make_task("a", "A", Status::Open);
        a.paused = true;
        a.before = vec!["b".to_string()]; // a → b via before
        let mut b = make_task("b", "B", Status::Open);
        b.paused = true;
        let mut c = make_task("c", "C", Status::Open);
        c.paused = true;
        c.after = vec!["b".to_string()];
        setup_workgraph(dir.path(), vec![a, b, c]);

        publish(dir.path(), "c", false, true).unwrap();
        let graph = load_graph(graph_path(dir.path())).unwrap();
        for id in &["a", "b", "c"] {
            assert!(
                !graph.get_task(id).unwrap().paused,
                "{} should be unpaused (WCC must follow before-edges too)",
                id
            );
        }
    }

    #[test]
    fn test_topo_sort_subset_orders_deps_before_dependents() {
        // Direct unit test on the helper: deps come strictly before
        // dependents in the result order.
        let a = make_task("a", "A", Status::Open);
        let mut b = make_task("b", "B", Status::Open);
        b.after = vec!["a".to_string()];
        let mut c = make_task("c", "C", Status::Open);
        c.after = vec!["b".to_string()];
        let mut graph = WorkGraph::new();
        for t in [a.clone(), b.clone(), c.clone()] {
            graph.add_node(workgraph::graph::Node::Task(t));
        }
        let sorted = topo_sort_subset(&graph, &["c".to_string(), "b".to_string(), "a".to_string()]);
        assert_eq!(
            sorted,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn test_discover_wcc_isolates_components() {
        let a = make_task("a", "A", Status::Open);
        let mut b = make_task("b", "B", Status::Open);
        b.after = vec!["a".to_string()];
        let x = make_task("x", "X", Status::Open);
        let mut y = make_task("y", "Y", Status::Open);
        y.after = vec!["x".to_string()];
        let mut graph = WorkGraph::new();
        for t in [a.clone(), b.clone(), x.clone(), y.clone()] {
            graph.add_node(workgraph::graph::Node::Task(t));
        }
        let _ = (a, b, x, y);

        let comp_a = discover_wcc(&graph, "a");
        assert_eq!(comp_a, vec!["a".to_string(), "b".to_string()]);
        let comp_y = discover_wcc(&graph, "y");
        assert_eq!(comp_y, vec!["x".to_string(), "y".to_string()]);
    }
}
