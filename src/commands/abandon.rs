use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use workgraph::graph::{LogEntry, Status};
use workgraph::parser::modify_graph;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use workgraph::parser::load_graph;

pub fn run(dir: &Path, id: &str, reason: Option<&str>, superseded_by: &[String]) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("Workgraph not initialized. Run 'wg init' first.");
    }

    let mut error: Option<anyhow::Error> = None;
    let mut already_abandoned = false;
    let mut prev_assigned: Option<String> = None;
    let mut cascade_targets: Vec<String> = Vec::new();

    let _graph = modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        if task.status == Status::Done {
            error = Some(anyhow::anyhow!(
                "Task '{}' is already done and cannot be abandoned",
                id
            ));
            return false;
        }

        if task.status == Status::Abandoned {
            already_abandoned = true;
            return false;
        }

        prev_assigned = task.assigned.clone();
        task.status = Status::Abandoned;
        task.failure_reason = reason.map(String::from);
        if !superseded_by.is_empty() {
            task.superseded_by = superseded_by.to_vec();
        }

        let log_message = match reason {
            Some(r) => format!("Task abandoned: {}", r),
            None => "Task abandoned".to_string(),
        };
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: task.assigned.clone(),
            user: Some(workgraph::current_user()),
            message: log_message,
        });

        // Cascade abandon to system tasks that depend on this task.
        // We match both by direct dependency (dot-prefixed tasks listing `id` in their `after`)
        // AND by naming convention (.evaluate-X, .flip-X, .verify-X) to handle transitive cases
        // — e.g. when FLIP is enabled, .evaluate-X depends on .flip-X, not X directly.
        let assign_id = format!(".assign-{}", id);
        let eval_id = format!(".evaluate-{}", id);
        let flip_id = format!(".flip-{}", id);
        let verify_id = format!(".verify-{}", id);
        let verify_deferred_id = format!(".verify-deferred-{}", id);
        cascade_targets = graph
            .tasks()
            .filter(|t| {
                if t.status.is_terminal() {
                    return false;
                }
                let is_system_dep = t.id.starts_with('.') && t.after.contains(&id.to_string());
                let is_eval_scaffold = t.id == assign_id
                    || t.id == eval_id
                    || t.id == flip_id
                    || t.id == verify_id
                    || t.id == verify_deferred_id;
                is_system_dep || is_eval_scaffold
            })
            .map(|t| t.id.clone())
            .collect();

        for target_id in &cascade_targets {
            if let Some(t) = graph.get_task_mut(target_id) {
                t.status = Status::Abandoned;
                t.failure_reason = Some(format!("Parent task '{}' was abandoned", id));
                t.log.push(LogEntry {
                    timestamp: Utc::now().to_rfc3339(),
                    actor: None,
                    user: Some(workgraph::current_user()),
                    message: format!("Auto-abandoned: parent '{}' was abandoned", id),
                });
            }
        }

        true
    })
    .context("Failed to save graph")?;

    if let Some(e) = error {
        return Err(e);
    }

    if already_abandoned {
        println!("Task '{}' is already abandoned", id);
        return Ok(());
    }

    super::notify_graph_changed(dir);

    let config = workgraph::config::Config::load_or_default(dir);
    let _ = workgraph::provenance::record(
        dir,
        "abandon",
        Some(id),
        prev_assigned.as_deref(),
        serde_json::json!({
            "reason": reason,
            "prev_assigned": prev_assigned,
            "cascaded": cascade_targets,
            "superseded_by": superseded_by,
        }),
        config.log.rotation_threshold,
    );

    let reason_msg = reason.map(|r| format!(" ({})", r)).unwrap_or_default();
    println!("Marked '{}' as abandoned{}", id, reason_msg);
    for target in &cascade_targets {
        println!("  Auto-abandoned: {}", target);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use workgraph::graph::{Node, Status, Task, WorkGraph};
    use workgraph::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn setup_graph(dir: &Path, graph: &WorkGraph) {
        std::fs::create_dir_all(dir).unwrap();
        save_graph(graph, &graph_path(dir)).unwrap();
    }

    #[test]
    fn test_abandon_open_task() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Open task")));
        setup_graph(&dir, &graph);
        let result = run(&dir, "t1", Some("no longer needed"), &[]);
        assert!(result.is_ok());
        let task = load_graph(graph_path(&dir))
            .unwrap()
            .get_task("t1")
            .unwrap()
            .clone();
        assert_eq!(task.status, Status::Abandoned);
        assert_eq!(task.failure_reason.as_deref(), Some("no longer needed"));
    }

    #[test]
    fn test_abandon_done_task_errors() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        let mut t = make_task("t1", "Done");
        t.status = Status::Done;
        graph.add_node(Node::Task(t));
        setup_graph(&dir, &graph);
        assert!(run(&dir, "t1", None, &[]).is_err());
    }

    #[test]
    fn test_abandon_already_abandoned_is_noop() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        let mut t = make_task("t1", "Abandoned");
        t.status = Status::Abandoned;
        graph.add_node(Node::Task(t));
        setup_graph(&dir, &graph);
        assert!(run(&dir, "t1", None, &[]).is_ok());
    }

    #[test]
    fn test_abandon_cascades_to_system_tasks() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Main task")));
        let mut eval = make_task(".evaluate-t1", "Eval t1");
        eval.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(eval));
        let mut verify = make_task(".verify-t1", "Verify t1");
        verify.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(verify));
        let mut dep = make_task("t2", "Depends on t1");
        dep.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(dep));
        setup_graph(&dir, &graph);

        assert!(run(&dir, "t1", Some("decomposed"), &[]).is_ok());
        let g = load_graph(graph_path(&dir)).unwrap();
        assert_eq!(g.get_task("t1").unwrap().status, Status::Abandoned);
        assert_eq!(
            g.get_task(".evaluate-t1").unwrap().status,
            Status::Abandoned
        );
        assert_eq!(g.get_task(".verify-t1").unwrap().status, Status::Abandoned);
        assert_eq!(g.get_task("t2").unwrap().status, Status::Open);
    }

    #[test]
    fn test_abandon_does_not_cascade_to_terminal() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Main")));
        let mut eval = make_task(".evaluate-t1", "Eval t1");
        eval.after = vec!["t1".to_string()];
        eval.status = Status::Done;
        graph.add_node(Node::Task(eval));
        setup_graph(&dir, &graph);
        run(&dir, "t1", None, &[]).unwrap();
        assert_eq!(
            load_graph(graph_path(&dir))
                .unwrap()
                .get_task(".evaluate-t1")
                .unwrap()
                .status,
            Status::Done
        );
    }

    #[test]
    fn test_abandon_cascades_eval_scaffold_with_flip() {
        // When FLIP is enabled, the chain is: task → .flip-task → .evaluate-task
        // .evaluate-task depends on .flip-task, NOT on task directly.
        // The cascade must still abandon .evaluate-task.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Main task")));

        let mut flip = make_task(".flip-t1", "FLIP t1");
        flip.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(flip));

        let mut eval = make_task(".evaluate-t1", "Eval t1");
        eval.after = vec![".flip-t1".to_string()]; // depends on .flip-t1, not t1
        graph.add_node(Node::Task(eval));

        // Normal dependent should NOT be cascaded
        let mut dep = make_task("t2", "Depends on t1");
        dep.after = vec!["t1".to_string()];
        graph.add_node(Node::Task(dep));

        setup_graph(&dir, &graph);
        assert!(run(&dir, "t1", Some("decomposed"), &[]).is_ok());

        let g = load_graph(graph_path(&dir)).unwrap();
        assert_eq!(g.get_task("t1").unwrap().status, Status::Abandoned);
        assert_eq!(g.get_task(".flip-t1").unwrap().status, Status::Abandoned);
        assert_eq!(
            g.get_task(".evaluate-t1").unwrap().status,
            Status::Abandoned
        );
        // Normal dependent unaffected
        assert_eq!(g.get_task("t2").unwrap().status, Status::Open);
    }

    #[test]
    fn test_abandon_cascades_to_assign_task() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();

        let mut main = make_task("t1", "Main task");
        main.after = vec![".assign-t1".to_string()];
        graph.add_node(Node::Task(main));

        // .assign-t1 uses `before` (not `after`) so is NOT caught by the
        // dependency-based check — must be caught by naming convention.
        let mut assign = make_task(".assign-t1", "Assign t1");
        assign.before = vec!["t1".to_string()];
        graph.add_node(Node::Task(assign));

        setup_graph(&dir, &graph);
        run(&dir, "t1", Some("no longer needed"), &[]).unwrap();

        let g = load_graph(graph_path(&dir)).unwrap();
        assert_eq!(g.get_task("t1").unwrap().status, Status::Abandoned);
        assert_eq!(g.get_task(".assign-t1").unwrap().status, Status::Abandoned);
    }

    #[test]
    fn test_abandon_does_not_cascade_done_assign() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();

        let mut main = make_task("t1", "Main task");
        main.after = vec![".assign-t1".to_string()];
        graph.add_node(Node::Task(main));

        let mut assign = make_task(".assign-t1", "Assign t1");
        assign.before = vec!["t1".to_string()];
        assign.status = Status::Done;
        graph.add_node(Node::Task(assign));

        setup_graph(&dir, &graph);
        run(&dir, "t1", None, &[]).unwrap();

        let g = load_graph(graph_path(&dir)).unwrap();
        assert_eq!(g.get_task(".assign-t1").unwrap().status, Status::Done);
    }

    #[test]
    fn test_abandon_with_superseded_by() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".wg");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Original")));
        setup_graph(&dir, &graph);
        let r = vec!["t2".to_string(), "t3".to_string()];
        run(&dir, "t1", Some("decomposed"), &r).unwrap();
        let task = load_graph(graph_path(&dir))
            .unwrap()
            .get_task("t1")
            .unwrap()
            .clone();
        assert_eq!(task.superseded_by, vec!["t2", "t3"]);
    }
}
