use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use workgraph::graph::{LogEntry, Status};
use workgraph::parser::modify_graph;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use workgraph::parser::{load_graph, save_graph};

/// Approve a task that is pending validation, transitioning it to Done.
pub fn run(dir: &Path, id: &str) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    let mut error: Option<anyhow::Error> = None;

    let _graph = modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        if !matches!(task.status, Status::PendingValidation | Status::PendingEval) {
            error = Some(anyhow::anyhow!(
                "Task '{}' is not awaiting approval (status: {:?}). Only pending-validation \
                 and pending-eval tasks can be approved.",
                id,
                task.status
            ));
            return false;
        }

        task.status = Status::Done;
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: std::env::var("WG_AGENT_ID").ok(),
            user: Some(workgraph::current_user()),
            message: "Task approved by validator".to_string(),
        });

        true
    })
    .context("Failed to save graph")?;

    if let Some(e) = error {
        return Err(e);
    }

    super::notify_graph_changed(dir);

    // Record operation
    let config = workgraph::config::Config::load_or_default(dir);
    let _ = workgraph::provenance::record(
        dir,
        "approve",
        Some(id),
        std::env::var("WG_AGENT_ID").ok().as_deref(),
        serde_json::json!({ "prev_status": "PendingValidation/PendingEval" }),
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
    use workgraph::graph::{Node, Task, WorkGraph};

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
    fn test_approve_pending_eval_transitions_to_done() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::PendingEval)],
        );

        let result = run(dir_path, "t1");
        assert!(result.is_ok(), "approve should accept PendingEval");

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
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
