use crate::graph::{Node, Status, Task, WorkGraph};
use crate::lifecycle::AttemptRef;
use crate::parser::save_graph;
use std::path::{Path, PathBuf};

/// Create a task with the given id and title, with all other fields defaulted.
pub fn make_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        ..Task::default()
    }
}

/// Create a task with the given id, title, and status.
pub fn make_task_with_status(id: &str, title: &str, status: Status) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status,
        ..Task::default()
    }
}

/// Create a `.wg` directory structure at `dir`, populate it with the
/// given tasks, and return the path to the graph file.
pub fn setup_workgraph(dir: &Path, tasks: Vec<Task>) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("graph.jsonl");
    let mut graph = WorkGraph::new();
    for mut task in tasks {
        // In-progress fixtures represent an already-started generation and must
        // carry the exact source attempt required by terminal lifecycle calls.
        if task.status == Status::InProgress && task.lifecycle.current_attempt.is_none() {
            let actor_id = task.assigned.clone().unwrap_or_else(|| "test".to_string());
            task.lifecycle.fence = 1;
            task.lifecycle.attempt_sequence = 1;
            task.lifecycle.current_attempt = Some(AttemptRef {
                id: format!("test-attempt:{}:0:1", task.id),
                generation: 0,
                fence: 1,
                actor_id,
                disposition: None,
            });
        }
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &path).unwrap();
    path
}
