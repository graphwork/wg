use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::graph::{LogEntry, Status};
use worksgood::parser::{load_graph, modify_graph};

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use worksgood::parser::save_graph;

/// Approve a task that is pending validation, transitioning it to Done.
pub fn run(dir: &Path, id: &str) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    let graph = load_graph(&path).context("Failed to load graph")?;
    let task = graph
        .get_task(id)
        .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", id))?;
    if task.status == Status::PendingEval {
        anyhow::bail!(
            "Task '{}' is under a required evaluation gate. `wg approve` cannot bypass exact attempt-bound verdict thresholds; wait for reconciliation or use `wg retry`.",
            id
        );
    }
    if task.status != Status::PendingValidation {
        anyhow::bail!(
            "Task '{}' is not awaiting approval (status: {:?}). Only pending-validation tasks can be approved.",
            id,
            task.status
        );
    }
    drop(graph);
    super::finalize::commit_terminal_success(
        dir,
        id,
        std::env::var("WG_AGENT_ID").ok().as_deref(),
        "validator_approved_graphsave",
    )?;
    modify_graph(&path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: std::env::var("WG_AGENT_ID").ok(),
            user: Some(worksgood::current_user()),
            message: "Task approved by validator".into(),
        });
        true
    })?;

    super::notify_graph_changed(dir);

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "approve",
        Some(id),
        std::env::var("WG_AGENT_ID").ok().as_deref(),
        serde_json::json!({ "prev_status": "PendingValidation" }),
        config.log.rotation_threshold,
    );

    println!("Approved '{}' — task is now done", id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};

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

    #[test]
    fn test_approve_pending_validation_transitions_to_done() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::PendingValidation)],
        );

        let result = run(dir_path, "t1");
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
    }

    #[test]
    fn test_approve_creates_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::PendingValidation)],
        );

        run(dir_path, "t1").unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.message, "Task approved by validator");
    }

    #[test]
    fn test_approve_non_pending_task_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not awaiting approval"));
    }

    #[test]
    fn test_approve_pending_eval_cannot_bypass_required_gate() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::PendingEval)],
        );

        let result = run(dir_path, "t1");
        assert!(result.is_err(), "approve must not bypass PendingEval");
        assert!(result.unwrap_err().to_string().contains("cannot bypass"));

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::PendingEval);
    }

    #[test]
    fn test_approve_done_task_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Done)]);

        let result = run(dir_path, "t1");
        assert!(result.is_err());
    }

    #[test]
    fn test_approve_nonexistent_task_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![]);

        let result = run(dir_path, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
