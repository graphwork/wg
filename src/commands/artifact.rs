use anyhow::{Context, Result};
use std::path::Path;
use worksgood::parser::modify_graph;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use worksgood::parser::load_graph;

/// Register an artifact (produced output) for a task
pub fn run_add(dir: &Path, task_id: &str, artifact_path: &str) -> Result<()> {
    let path = super::graph_path(dir);
    let mut error: Option<anyhow::Error> = None;
    let mut already_registered = false;
    let artifact_str = artifact_path.to_string();
    let task_id_owned = task_id.to_string();
    modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(&task_id_owned) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id_owned));
                return false;
            }
        };
        if task.artifacts.contains(&artifact_str) {
            already_registered = true;
            return false;
        }
        task.artifacts.push(artifact_str.clone());
        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }
    if already_registered {
        println!(
            "Artifact '{}' already registered for task '{}'",
            artifact_path, task_id
        );
        return Ok(());
    }
    super::notify_graph_changed(dir);

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "artifact_add",
        Some(task_id),
        None,
        serde_json::json!({ "path": artifact_path }),
        config.log.rotation_threshold,
    );

    println!(
        "Registered artifact '{}' for task '{}'",
        artifact_path, task_id
    );
    Ok(())
}

/// Remove an artifact from a task
pub fn run_remove(dir: &Path, task_id: &str, artifact_path: &str) -> Result<()> {
    let path = super::graph_path(dir);
    let mut error: Option<anyhow::Error> = None;
    let artifact_str = artifact_path.to_string();
    let task_id_owned = task_id.to_string();
    modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(&task_id_owned) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id_owned));
                return false;
            }
        };
        let original_len = task.artifacts.len();
        task.artifacts.retain(|a| a != &artifact_str);
        if task.artifacts.len() == original_len {
            error = Some(anyhow::anyhow!(
                "Artifact '{}' not found on task '{}'",
                artifact_str,
                task_id_owned
            ));
            return false;
        }
        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }
    super::notify_graph_changed(dir);

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "artifact_rm",
        Some(task_id),
        None,
        serde_json::json!({ "path": artifact_path }),
        config.log.rotation_threshold,
    );

    println!(
        "Removed artifact '{}' from task '{}'",
        artifact_path, task_id
    );
    Ok(())
}

/// List artifacts for a task
pub fn run_list(dir: &Path, task_id: &str, json: bool) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;

    let task = graph.get_task_or_err(task_id)?;

    if json {
        let output = serde_json::json!({
            "task_id": task_id,
            "deliverables": task.deliverables,
            "artifacts": task.artifacts,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Task: {} - {}", task.id, task.title);
        println!();

        if !task.deliverables.is_empty() {
            println!("Expected deliverables:");
            for d in &task.deliverables {
                let produced = if task.artifacts.contains(d) {
                    " [produced]"
                } else {
                    ""
                };
                println!("  {}{}", d, produced);
            }
            println!();
        }

        if !task.artifacts.is_empty() {
            println!("Produced artifacts:");
            for a in &task.artifacts {
                let expected = if task.deliverables.contains(a) {
                    ""
                } else {
                    " [extra]"
                };
                println!("  {}{}", a, expected);
            }
        } else {
            println!("No artifacts produced yet.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn setup_graph() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("graph.jsonl");

        let mut graph = WorkGraph::new();
        let mut task = make_task("t1", "Test Task");
        task.deliverables = vec!["output.txt".to_string()];
        graph.add_node(Node::Task(task));
        save_graph(&graph, &path).unwrap();

        temp_dir
    }

    #[test]
    fn test_add_artifact() {
        let temp_dir = setup_graph();

        let result = run_add(temp_dir.path(), "t1", "output.txt");
        assert!(result.is_ok());

        let graph = load_graph(graph_path(temp_dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(task.artifacts.contains(&"output.txt".to_string()));
    }

    #[test]
    fn test_add_artifact_duplicate() {
        let temp_dir = setup_graph();

        run_add(temp_dir.path(), "t1", "output.txt").unwrap();
        let result = run_add(temp_dir.path(), "t1", "output.txt");
        assert!(result.is_ok()); // Should succeed but not duplicate

        let graph = load_graph(graph_path(temp_dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.artifacts.len(), 1);
    }

    #[test]
    fn test_remove_artifact() {
        let temp_dir = setup_graph();

        run_add(temp_dir.path(), "t1", "output.txt").unwrap();
        let result = run_remove(temp_dir.path(), "t1", "output.txt");
        assert!(result.is_ok());

        let graph = load_graph(graph_path(temp_dir.path())).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(task.artifacts.is_empty());
    }

    #[test]
    fn test_remove_nonexistent_artifact() {
        let temp_dir = setup_graph();

        let result = run_remove(temp_dir.path(), "t1", "nonexistent.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_artifacts() {
        let temp_dir = setup_graph();

        run_add(temp_dir.path(), "t1", "output.txt").unwrap();
        let result = run_list(temp_dir.path(), "t1", false);
        assert!(result.is_ok());
    }
}
