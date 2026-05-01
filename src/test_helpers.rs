use crate::graph::{Node, Status, Task, WorkGraph};
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
    for task in tasks {
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, &path).unwrap();
    path
}
