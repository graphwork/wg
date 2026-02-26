//! Mock executor integration tests.
//!
//! Tests the coordinator lifecycle with a mock shell executor that returns
//! canned responses from fixture files. Validates task state transitions,
//! agent registry operations, and error handling paths.
//!
//! These tests exercise the coordinator's data structures and task state
//! machine without spawning real LLM agents.

use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

use workgraph::graph::{Node, Status, Task, WorkGraph};
use workgraph::parser::{load_graph, save_graph};
use workgraph::query::ready_tasks;
use workgraph::service::registry::{AgentRegistry, AgentStatus};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_task(id: &str, title: &str, status: Status) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status,
        ..Task::default()
    }
}

fn setup_workgraph(tmp: &TempDir) -> (PathBuf, PathBuf) {
    let wg_dir = tmp.path().join(".workgraph");
    fs::create_dir_all(&wg_dir).unwrap();
    fs::create_dir_all(wg_dir.join("service")).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = WorkGraph::new();
    save_graph(&graph, &graph_path).unwrap();
    (wg_dir, graph_path)
}

fn save_test_graph(wg_dir: &Path, graph: &WorkGraph) -> PathBuf {
    let graph_path = wg_dir.join("graph.jsonl");
    save_graph(graph, &graph_path).unwrap();
    graph_path
}

// ============================================================================
// 1. Task state transitions
// ============================================================================

#[test]
fn test_task_open_to_in_progress() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(make_task("task-1", "Test Task", Status::Open)));
    save_test_graph(&wg_dir, &graph);

    // Load and verify it's ready
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "task-1");

    // Simulate coordinator claiming the task
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task_mut("task-1").unwrap();
    task.status = Status::InProgress;
    task.assigned = Some("mock-agent-1".to_string());
    save_test_graph(&wg_dir, &graph);

    // Verify state changed
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = loaded.tasks().find(|t| t.id == "task-1").unwrap();
    assert_eq!(task.status, Status::InProgress);
    assert_eq!(task.assigned.as_deref(), Some("mock-agent-1"));
}

#[test]
fn test_task_in_progress_to_done() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    let mut task = make_task("task-2", "Complete Me", Status::InProgress);
    task.assigned = Some("mock-agent-2".to_string());
    graph.add_node(Node::Task(task));
    save_test_graph(&wg_dir, &graph);

    // Simulate agent completing the task
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("task-2").unwrap().status = Status::Done;
    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = loaded.tasks().find(|t| t.id == "task-2").unwrap();
    assert_eq!(task.status, Status::Done);
}

#[test]
fn test_task_in_progress_to_failed() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    let mut task = make_task("task-3", "Fail Me", Status::InProgress);
    task.assigned = Some("mock-agent-3".to_string());
    graph.add_node(Node::Task(task));
    save_test_graph(&wg_dir, &graph);

    // Simulate agent failing the task
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("task-3").unwrap().status = Status::Failed;
    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = loaded.tasks().find(|t| t.id == "task-3").unwrap();
    assert_eq!(task.status, Status::Failed);
}

#[test]
fn test_task_blocked_becomes_ready_when_dependency_done() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(make_task("blocker", "Blocker", Status::Open)));
    let mut downstream = make_task("downstream", "Downstream", Status::Open);
    downstream.after = vec!["blocker".to_string()];
    graph.add_node(Node::Task(downstream));
    save_test_graph(&wg_dir, &graph);

    // Initially, only blocker is ready
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "blocker");

    // Complete the blocker
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("blocker").unwrap().status = Status::Done;
    save_test_graph(&wg_dir, &graph);

    // Now downstream should be ready
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "downstream");
}

// ============================================================================
// 2. Agent registry operations
// ============================================================================

#[test]
fn test_agent_registry_register_and_query() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(registry.list_agents().len(), 0);

    let id = registry.register_agent(12345, "task-1", "mock", "/tmp/test.log");
    registry.save(&wg_dir).unwrap();

    // Reload and verify
    let loaded = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(loaded.list_agents().len(), 1);
    let agent = loaded.get_agent(&id).unwrap();
    assert_eq!(agent.task_id, "task-1");
    assert_eq!(agent.executor, "mock");
    assert_eq!(agent.status, AgentStatus::Working);
}

#[test]
fn test_agent_registry_multiple_agents() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    let _id1 = registry.register_agent(100, "task-1", "mock", "/tmp/1.log");
    let _id2 = registry.register_agent(200, "task-2", "mock", "/tmp/2.log");
    let id3 = registry.register_agent(300, "task-3", "mock", "/tmp/3.log");
    // id1 and id2 stay Working (default), id3 is Done
    registry.set_status(&id3, AgentStatus::Done);
    registry.save(&wg_dir).unwrap();

    let loaded = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(loaded.list_agents().len(), 3);

    let alive: Vec<_> = loaded.list_alive_agents();
    assert_eq!(alive.len(), 2);
}

#[test]
fn test_agent_registry_update_status() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    let id = registry.register_agent(12345, "task-1", "mock", "/tmp/test.log");
    registry.save(&wg_dir).unwrap();

    // Update status
    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    registry.set_status(&id, AgentStatus::Done);
    registry.save(&wg_dir).unwrap();

    let loaded = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(loaded.get_agent(&id).unwrap().status, AgentStatus::Done);
}

#[test]
fn test_agent_registry_get_by_task() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    let _id = registry.register_agent(555, "my-task", "mock", "/tmp/my.log");
    registry.save(&wg_dir).unwrap();

    let loaded = AgentRegistry::load(&wg_dir).unwrap();
    let agent = loaded.get_agent_by_task("my-task").unwrap();
    assert_eq!(agent.task_id, "my-task");
    assert!(loaded.get_agent_by_task("no-such-task").is_none());
}

// ============================================================================
// 3. Diamond pattern: fan-out / fan-in
// ============================================================================

#[test]
fn test_diamond_pattern_fan_out() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();

    // Root task (done)
    graph.add_node(Node::Task(make_task("root", "Root Task", Status::Done)));

    // Fan-out: 3 parallel tasks depending on root
    for i in 1..=3 {
        let mut task = make_task(&format!("worker-{}", i), &format!("Worker {}", i), Status::Open);
        task.after = vec!["root".to_string()];
        graph.add_node(Node::Task(task));
    }

    // Fan-in: join task depending on all workers
    let mut join = make_task("join", "Join Task", Status::Open);
    join.after = vec![
        "worker-1".to_string(),
        "worker-2".to_string(),
        "worker-3".to_string(),
    ];
    graph.add_node(Node::Task(join));

    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    let ready_ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ready.len(), 3);
    assert!(ready_ids.contains(&"worker-1"));
    assert!(ready_ids.contains(&"worker-2"));
    assert!(ready_ids.contains(&"worker-3"));
}

#[test]
fn test_diamond_pattern_fan_in() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(make_task("root", "Root Task", Status::Done)));

    for i in 1..=3 {
        let mut task = make_task(&format!("worker-{}", i), &format!("Worker {}", i), Status::Done);
        task.after = vec!["root".to_string()];
        graph.add_node(Node::Task(task));
    }

    let mut join = make_task("join", "Join Task", Status::Open);
    join.after = vec![
        "worker-1".to_string(),
        "worker-2".to_string(),
        "worker-3".to_string(),
    ];
    graph.add_node(Node::Task(join));

    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "join");
}

#[test]
fn test_diamond_pattern_partial_completion_blocks_join() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(make_task("root", "Root", Status::Done)));

    let mut w1 = make_task("worker-1", "W1", Status::Done);
    w1.after = vec!["root".to_string()];
    graph.add_node(Node::Task(w1));

    let mut w2 = make_task("worker-2", "W2", Status::InProgress);
    w2.after = vec!["root".to_string()];
    graph.add_node(Node::Task(w2));

    let mut w3 = make_task("worker-3", "W3", Status::Done);
    w3.after = vec!["root".to_string()];
    graph.add_node(Node::Task(w3));

    let mut join = make_task("join", "Join", Status::Open);
    join.after = vec![
        "worker-1".to_string(),
        "worker-2".to_string(),
        "worker-3".to_string(),
    ];
    graph.add_node(Node::Task(join));

    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert!(
        !ready.iter().any(|t| t.id == "join"),
        "Join should not be ready while worker-2 is in progress"
    );
}

// ============================================================================
// 4. Full lifecycle simulation
// ============================================================================

#[test]
fn test_full_lifecycle_open_to_done() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    // Phase 1: Create task
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(make_task("lifecycle-task", "Lifecycle Test", Status::Open)));
    save_test_graph(&wg_dir, &graph);

    // Phase 2: Coordinator picks it up (claim)
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&graph);
    assert_eq!(ready.len(), 1);
    let task = graph.get_task_mut("lifecycle-task").unwrap();
    task.status = Status::InProgress;
    task.assigned = Some("mock-agent".to_string());
    save_test_graph(&wg_dir, &graph);

    // Phase 3: Agent registers in registry
    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    let agent_id = registry.register_agent(999, "lifecycle-task", "mock", "/tmp/lifecycle.log");
    registry.save(&wg_dir).unwrap();

    // Phase 4: Agent completes the task
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("lifecycle-task").unwrap().status = Status::Done;
    save_test_graph(&wg_dir, &graph);

    // Phase 5: Registry updated
    let mut registry = AgentRegistry::load(&wg_dir).unwrap();
    registry.set_status(&agent_id, AgentStatus::Done);
    registry.save(&wg_dir).unwrap();

    // Verify final state
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = loaded.tasks().find(|t| t.id == "lifecycle-task").unwrap();
    assert_eq!(task.status, Status::Done);

    let loaded_reg = AgentRegistry::load(&wg_dir).unwrap();
    assert_eq!(loaded_reg.get_agent(&agent_id).unwrap().status, AgentStatus::Done);
}

#[test]
fn test_full_lifecycle_with_failure_and_retry() {
    let tmp = TempDir::new().unwrap();
    let (wg_dir, _) = setup_workgraph(&tmp);

    // Phase 1: Create and claim task
    let mut graph = WorkGraph::new();
    let mut task = make_task("retry-task", "Retry Test", Status::InProgress);
    task.assigned = Some("agent-fail".to_string());
    graph.add_node(Node::Task(task));
    save_test_graph(&wg_dir, &graph);

    // Phase 2: Agent fails
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("retry-task").unwrap().status = Status::Failed;
    save_test_graph(&wg_dir, &graph);

    // Phase 3: Retry — reset to open, clear assignment
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task_mut("retry-task").unwrap();
    task.status = Status::Open;
    task.assigned = None;
    save_test_graph(&wg_dir, &graph);

    // Phase 4: Task is ready again
    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let ready = ready_tasks(&loaded);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "retry-task");
    assert!(ready[0].assigned.is_none());

    // Phase 5: Second attempt succeeds
    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task_mut("retry-task").unwrap();
    task.status = Status::InProgress;
    task.assigned = Some("agent-success".to_string());
    save_test_graph(&wg_dir, &graph);

    let mut graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    graph.get_task_mut("retry-task").unwrap().status = Status::Done;
    save_test_graph(&wg_dir, &graph);

    let loaded = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = loaded.tasks().find(|t| t.id == "retry-task").unwrap();
    assert_eq!(task.status, Status::Done);
}

// ============================================================================
// 5. Mock executor shell script tests
// ============================================================================

#[test]
#[serial]
fn test_mock_executor_script_with_fixture() {
    let tmp = TempDir::new().unwrap();
    let fixture_dir = tmp.path().join("fixtures");
    fs::create_dir_all(&fixture_dir).unwrap();

    // Create a simple response fixture
    fs::write(
        fixture_dir.join("test-task.response"),
        "Mock executor response for test-task\n",
    )
    .unwrap();

    // Run the mock executor script
    let mock_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock-executor.sh");
    let output = Command::new("bash")
        .arg(&mock_script)
        .env("WG_TASK_ID", "test-task")
        .env("WG_FIXTURES_DIR", fixture_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Mock executor response for test-task"));
}

#[test]
#[serial]
fn test_mock_executor_script_no_fixture_completes_task() {
    let tmp = TempDir::new().unwrap();
    let fixture_dir = tmp.path().join("empty-fixtures");
    fs::create_dir_all(&fixture_dir).unwrap();

    let mock_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock-executor.sh");
    let output = Command::new("bash")
        .arg(&mock_script)
        .env("WG_TASK_ID", "no-such-task")
        .env("WG_FIXTURES_DIR", fixture_dir.to_str().unwrap())
        .env("WG_DIR", tmp.path().join(".workgraph").to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Mock executor: completing task"));
}

#[test]
#[serial]
fn test_mock_executor_with_custom_script_fixture() {
    let tmp = TempDir::new().unwrap();
    let fixture_dir = tmp.path().join("fixtures");
    fs::create_dir_all(&fixture_dir).unwrap();

    fs::write(
        fixture_dir.join("custom-task.sh"),
        "#!/bin/bash\necho 'Custom script executed successfully'\nexit 0\n",
    )
    .unwrap();

    let mock_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock-executor.sh");
    let output = Command::new("bash")
        .arg(&mock_script)
        .env("WG_TASK_ID", "custom-task")
        .env("WG_FIXTURES_DIR", fixture_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Custom script executed successfully"));
}

#[test]
#[serial]
fn test_mock_executor_with_failing_script() {
    let tmp = TempDir::new().unwrap();
    let fixture_dir = tmp.path().join("fixtures");
    fs::create_dir_all(&fixture_dir).unwrap();

    fs::write(
        fixture_dir.join("fail-task.sh"),
        "#!/bin/bash\necho 'Error: something went wrong' >&2\nexit 1\n",
    )
    .unwrap();

    let mock_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock-executor.sh");
    let output = Command::new("bash")
        .arg(&mock_script)
        .env("WG_TASK_ID", "fail-task")
        .env("WG_FIXTURES_DIR", fixture_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("something went wrong"));
}
