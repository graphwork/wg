use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use workgraph::config::Config;
use workgraph::graph::{LogEntry, Status};
use workgraph::parser::modify_graph;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use workgraph::parser::load_graph;

pub fn run(dir: &Path, id: &str, reason: &str) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("Workgraph not initialized. Run 'wg init' first.");
    }

    let config = Config::load_or_default(dir);
    let max_triage = config.guardrails.max_triage_attempts;

    let mut error: Option<anyhow::Error> = None;
    let mut triage_count: u32 = 0;

    modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        if task.status != Status::InProgress {
            error = Some(anyhow::anyhow!(
                "Task '{}' is not in-progress (status: {:?}). Only in-progress tasks can be requeued.",
                id,
                task.status
            ));
            return false;
        }

        if task.triage_count >= max_triage {
            error = Some(anyhow::anyhow!(
                "Triage budget exhausted for '{}' ({}/{}). Use `wg fail` instead.",
                id,
                task.triage_count,
                max_triage
            ));
            return false;
        }

        task.triage_count += 1;
        triage_count = task.triage_count;

        task.status = Status::Open;
        task.assigned = None;
        task.started_at = None;
        task.session_id = None;
        // Preserve: loop_iteration, cycle_config, tags, retry_count, agent

        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: None,
            user: Some(workgraph::current_user()),
            message: format!(
                "Requeued (triage {}/{}): {}",
                triage_count, max_triage, reason
            ),
        });

        true
    })
    .context("Failed to modify graph")?;

    if let Some(e) = error {
        return Err(e);
    }

    super::notify_graph_changed(dir);

    // Record operation
    let _ = workgraph::provenance::record(
        dir,
        "requeue",
        Some(id),
        None,
        serde_json::json!({
            "triage_count": triage_count,
            "max_triage_attempts": max_triage,
            "reason": reason,
        }),
        config.log.rotation_threshold,
    );

    println!(
        "Requeued '{}' for triage (attempt {}/{}): {}",
        id, triage_count, max_triage, reason
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use workgraph::graph::{Node, Task, WorkGraph};
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

    #[test]
    fn test_requeue_in_progress_task_transitions_to_open() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.assigned = Some("agent-1".to_string());
        task.started_at = Some("2026-01-01T00:00:00Z".to_string());
        task.session_id = Some("session-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", "Created fix task for failed dep");
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Open);
        assert_eq!(task.assigned, None);
        assert_eq!(task.started_at, None);
        assert_eq!(task.session_id, None);
        assert_eq!(task.triage_count, 1);
    }

    #[test]
    fn test_requeue_increments_triage_count() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.triage_count = 1;
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", "Second triage").unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.triage_count, 2);
    }

    #[test]
    fn test_requeue_non_in_progress_errors() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", "reason");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in-progress"));
    }

    #[test]
    fn test_requeue_failed_task_errors() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::Failed)],
        );

        let result = run(dir_path, "t1", "reason");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in-progress"));
    }

    #[test]
    fn test_requeue_budget_exhausted() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.triage_count = 3; // default max is 3
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", "reason");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Triage budget exhausted"),
            "Expected budget exhausted error, got: {}",
            err
        );
    }

    #[test]
    fn test_requeue_preserves_loop_iteration_and_agent() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.loop_iteration = 2;
        task.agent = Some("agent-hash-123".to_string());
        task.retry_count = 1;
        task.tags = vec!["implementation".to_string()];
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", "triage reason").unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.loop_iteration, 2);
        assert_eq!(task.agent, Some("agent-hash-123".to_string()));
        assert_eq!(task.retry_count, 1);
        assert_eq!(task.tags, vec!["implementation".to_string()]);
    }

    #[test]
    fn test_requeue_adds_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::InProgress)],
        );

        run(dir_path, "t1", "Created fix for dep-a").unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.log.is_empty());
        let last_log = task.log.last().unwrap();
        assert!(
            last_log.message.contains("Requeued"),
            "Log should contain 'Requeued', got: {}",
            last_log.message
        );
        assert!(
            last_log.message.contains("Created fix for dep-a"),
            "Log should contain reason, got: {}",
            last_log.message
        );
    }

    #[test]
    fn test_requeue_task_not_found() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::InProgress)],
        );

        let result = run(dir_path, "nonexistent", "reason");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
