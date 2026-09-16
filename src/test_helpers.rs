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

/// Process-global lock serializing every test that mutates environment
/// variables production path-resolution reads (`HOME`, `WG_GLOBAL_DIR`,
/// `WG_PROFILE_USAGE_PATH`, …).
///
/// Env vars are process-global state and the lib test harness runs all unit
/// tests as threads of one process. Before this lock, each module that mutated
/// `HOME`/`WG_GLOBAL_DIR` either used its own module-local mutex or none at
/// all, so a test in module A could observe module B's temp dir (or have its
/// own `HOME` clobbered mid-test) — e.g. a `profile::named` test reading
/// another module's `WG_GLOBAL_DIR` tempdir, or a `migrate_project_local_pi`
/// rollback racing a concurrent `HOME` reset. Every env-mutating test guard
/// (the `EnvRestore` / `GlobalDirGuard` / `with_home`-style helpers) must hold
/// this lock for the whole mutation window; the guard drops it when it
/// restores the previous value.
///
/// Test-only: this module is compiled under
/// `#[cfg(any(test, feature = "test-support"))]`.
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
    // A panicked test must not poison every later env-mutating test.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
