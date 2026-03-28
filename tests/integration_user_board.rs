//! Integration tests for the .user-NAME user board system.
//!
//! Tests: creation, alias resolution, auto-increment on archive,
//! non-claimability (user-board tag in DAEMON_MANAGED_TAGS).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use workgraph::graph::{
    Node, Status, WorkGraph, create_user_board_task, is_user_board,
    resolve_user_board_alias,
};
use workgraph::parser::{load_graph, modify_graph, save_graph};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(
        path.exists(),
        "wg binary not found at {:?}. Run `cargo build` first.",
        path
    );
    path
}

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run wg")
}

fn setup_wg_dir() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir).unwrap();
    let graph = WorkGraph::new();
    let path = dir.join("graph.jsonl");
    save_graph(&graph, &path).unwrap();
    tmp
}

fn graph_at(dir: &Path) -> WorkGraph {
    load_graph(&dir.join("graph.jsonl")).unwrap()
}

// ---------------------------------------------------------------------------
// Unit-style tests (using the library directly)
// ---------------------------------------------------------------------------

#[test]
fn test_user_board_creation_via_api() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    // Create a user board task
    let task = create_user_board_task("erik", 0);
    assert_eq!(task.id, ".user-erik-0");
    assert_eq!(task.status, Status::InProgress);
    assert!(task.tags.contains(&"user-board".to_string()));
    assert!(task.assigned.is_none());
    assert!(task.agent.is_none());

    // Save to graph and reload
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &path).unwrap();

    let graph = load_graph(&path).unwrap();
    let loaded = graph.get_task(".user-erik-0").unwrap();
    assert_eq!(loaded.status, Status::InProgress);
    assert!(loaded.tags.contains(&"user-board".to_string()));
    assert!(loaded.assigned.is_none());
}

#[test]
fn test_user_board_auto_increment_on_archive() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    // Create initial user board
    let task = create_user_board_task("erik", 0);
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &path).unwrap();

    // Mark it as done via the done command
    let out = wg_cmd(dir, &["done", ".user-erik-0"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg done failed: stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Created successor board '.user-erik-1'"),
        "Expected successor creation message, got: {}",
        stdout
    );

    // Verify the graph state
    let graph = graph_at(dir);

    // Original board should be Done with 'archived' tag
    let old = graph.get_task(".user-erik-0").unwrap();
    assert_eq!(old.status, Status::Done);
    assert!(old.tags.contains(&"archived".to_string()));

    // Successor should exist and be InProgress
    let new = graph.get_task(".user-erik-1").unwrap();
    assert_eq!(new.status, Status::InProgress);
    assert!(new.tags.contains(&"user-board".to_string()));
    assert!(new.assigned.is_none());
}

#[test]
fn test_user_board_not_claimable_by_agents() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    // Create a user board
    let task = create_user_board_task("erik", 0);
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &path).unwrap();

    // User boards are tagged 'user-board' which is in DAEMON_MANAGED_TAGS.
    // The coordinator skips these during dispatch. Verify the tag is correct.
    let graph = graph_at(dir);
    let board = graph.get_task(".user-erik-0").unwrap();
    assert!(board.tags.contains(&"user-board".to_string()));
    // User boards are system tasks (dot-prefix)
    assert!(is_user_board(&board.id));
    // No agent assignment
    assert!(board.assigned.is_none());
    assert!(board.agent.is_none());
}

#[test]
fn test_user_board_msg_send_auto_creates() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();

    // Send a message to a user board that doesn't exist yet
    let out = wg_cmd(
        dir,
        &["msg", "send", ".user-testuser-0", "Hello from test", "--from", "test-agent"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg msg send failed: stdout={}, stderr={}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("Auto-created user board"),
        "Expected auto-creation message, got stderr: {}",
        stderr
    );

    // Verify the board was created
    let graph = graph_at(dir);
    let board = graph.get_task(".user-testuser-0").unwrap();
    assert_eq!(board.status, Status::InProgress);
    assert!(board.tags.contains(&"user-board".to_string()));
}

#[test]
fn test_user_board_alias_resolution_in_msg_send() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    // Pre-create a user board
    let task = create_user_board_task("alice", 0);
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &path).unwrap();

    // Send a message using the alias (.user-alice, no -N suffix)
    let out = wg_cmd(
        dir,
        &["msg", "send", ".user-alice", "Hello Alice", "--from", "bob"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "wg msg send via alias failed: stdout={}, stderr={}",
        stdout,
        stderr
    );
    // Should resolve to .user-alice-0
    assert!(
        stdout.contains(".user-alice-0"),
        "Expected resolved task ID in output, got: {}",
        stdout
    );
}

#[test]
fn test_user_board_chained_archive_creates_sequence() {
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    // Create initial board
    let task = create_user_board_task("multi", 0);
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &path).unwrap();

    // Archive .user-multi-0 → creates .user-multi-1
    let out = wg_cmd(dir, &["done", ".user-multi-0"]);
    assert!(out.status.success());

    // Archive .user-multi-1 → creates .user-multi-2
    let out = wg_cmd(dir, &["done", ".user-multi-1"]);
    assert!(out.status.success());

    let graph = graph_at(dir);
    assert_eq!(graph.get_task(".user-multi-0").unwrap().status, Status::Done);
    assert_eq!(graph.get_task(".user-multi-1").unwrap().status, Status::Done);
    let board2 = graph.get_task(".user-multi-2").unwrap();
    assert_eq!(board2.status, Status::InProgress);
    assert!(board2.tags.contains(&"user-board".to_string()));
}

#[test]
fn test_user_board_externally_linked_properties() {
    // User boards should have the same "externally-linked" pattern as coordinators:
    // InProgress status, no agent assignment, not dispatched by coordinator
    let task = create_user_board_task("erik", 0);

    // InProgress on creation (externally-linked)
    assert_eq!(task.status, Status::InProgress);
    // No cycle config (unlike coordinators which have one)
    assert!(task.cycle_config.is_none());
    // No assignment
    assert!(task.assigned.is_none());
    assert!(task.agent.is_none());
    // Has user-board tag
    assert!(task.tags.contains(&"user-board".to_string()));
    // Is a system task (dot-prefix)
    assert!(task.id.starts_with('.'));
}

// ---------------------------------------------------------------------------
// Coordinator integration tests
// ---------------------------------------------------------------------------

#[test]
fn test_ensure_user_board_creates_on_empty_graph() {
    // Simulates what ensure_user_board does at coordinator startup:
    // If no active user board exists, create one.
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    let handle = workgraph::current_user();

    // Graph starts empty — no user boards
    let graph = graph_at(dir);
    let alias = format!(".user-{}", handle);
    let resolved = resolve_user_board_alias(&graph, &alias);
    assert_eq!(resolved, alias, "No board should exist yet");

    // Simulate ensure_user_board logic: check + create
    let prefix = format!(".user-{}-", handle);
    let has_active = graph
        .tasks()
        .any(|t| is_user_board(&t.id) && t.id.starts_with(&prefix) && !t.status.is_terminal());
    assert!(!has_active);

    let seq = workgraph::graph::next_user_board_seq(&graph, &handle);
    assert_eq!(seq, 0);

    let task = create_user_board_task(&handle, seq);
    let task_id = task.id.clone();
    modify_graph(&path, |fresh| {
        fresh.add_node(Node::Task(task.clone()));
        true
    })
    .unwrap();

    // Verify it was created
    let graph = graph_at(dir);
    let board = graph.get_task(&task_id).unwrap();
    assert_eq!(board.status, Status::InProgress);
    assert!(board.tags.contains(&"user-board".to_string()));
}

#[test]
fn test_ensure_user_board_idempotent_when_active() {
    // If an active user board exists, ensure_user_board should not create another.
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    let handle = workgraph::current_user();

    // Pre-create a user board
    let task = create_user_board_task(&handle, 0);
    modify_graph(&path, |fresh| {
        fresh.add_node(Node::Task(task.clone()));
        true
    })
    .unwrap();

    // Check that the active board is found
    let graph = graph_at(dir);
    let prefix = format!(".user-{}-", handle);
    let has_active = graph
        .tasks()
        .any(|t| is_user_board(&t.id) && t.id.starts_with(&prefix) && !t.status.is_terminal());
    assert!(has_active, "Should find existing active board");

    // Count user boards before
    let count_before = graph.tasks().filter(|t| is_user_board(&t.id)).count();
    assert_eq!(count_before, 1);
}

#[test]
fn test_chat_message_forwarded_to_user_board() {
    // Test that messages sent via send_message appear on the user board.
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    let handle = workgraph::current_user();

    // Create user board
    let task = create_user_board_task(&handle, 0);
    let board_id = task.id.clone();
    modify_graph(&path, |fresh| {
        fresh.add_node(Node::Task(task.clone()));
        true
    })
    .unwrap();

    // Send a message to the user board (simulating what forward_chat_to_user_board does)
    workgraph::messages::send_message(dir, &board_id, "Hello from chat!", "user", "normal")
        .unwrap();

    // Verify message was stored
    let msgs = workgraph::messages::list_messages(dir, &board_id).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].body, "Hello from chat!");
    assert_eq!(msgs[0].sender, "user");
}

#[test]
fn test_user_board_persists_across_restarts() {
    // User boards are stored in graph.jsonl, so they persist across
    // coordinator restarts. This test verifies the data survives save/load.
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    let handle = workgraph::current_user();

    // Create user board and send messages
    let task = create_user_board_task(&handle, 0);
    let board_id = task.id.clone();
    modify_graph(&path, |fresh| {
        fresh.add_node(Node::Task(task.clone()));
        true
    })
    .unwrap();

    workgraph::messages::send_message(dir, &board_id, "Message 1", "user", "normal").unwrap();
    workgraph::messages::send_message(dir, &board_id, "Message 2", "coordinator", "normal")
        .unwrap();

    // Simulate restart: reload graph from disk
    let graph = load_graph(&path).unwrap();
    let board = graph.get_task(&board_id).unwrap();
    assert_eq!(board.status, Status::InProgress);
    assert!(board.tags.contains(&"user-board".to_string()));

    // Messages persist (stored in separate .workgraph/messages/ files)
    let msgs = workgraph::messages::list_messages(dir, &board_id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].body, "Message 1");
    assert_eq!(msgs[1].body, "Message 2");

    // After "restart", ensure_user_board would see the active board and skip
    let prefix = format!(".user-{}-", handle);
    let has_active = graph
        .tasks()
        .any(|t| is_user_board(&t.id) && t.id.starts_with(&prefix) && !t.status.is_terminal());
    assert!(has_active, "Active board should persist across restarts");
}

#[test]
fn test_full_lifecycle_start_chat_stop_start_see_history() {
    // Integration test: full lifecycle
    // 1. Start — create user board
    // 2. Chat — send messages
    // 3. Stop — board persists
    // 4. Start — board found, messages visible
    let tmp = setup_wg_dir();
    let dir = tmp.path();
    let path = dir.join("graph.jsonl");

    let handle = workgraph::current_user();

    // Phase 1: "Start" — auto-create user board (simulating ensure_user_board)
    let seq = workgraph::graph::next_user_board_seq(&graph_at(dir), &handle);
    let task = create_user_board_task(&handle, seq);
    let board_id = task.id.clone();
    modify_graph(&path, |fresh| {
        fresh.add_node(Node::Task(task.clone()));
        true
    })
    .unwrap();

    // Phase 2: "Chat" — send several messages
    workgraph::messages::send_message(dir, &board_id, "What tasks are running?", "user", "normal")
        .unwrap();
    workgraph::messages::send_message(
        dir,
        &board_id,
        "There are 3 tasks in progress.",
        "coordinator",
        "normal",
    )
    .unwrap();
    workgraph::messages::send_message(dir, &board_id, "Add a new test task", "user", "normal")
        .unwrap();

    // Phase 3: "Stop" — verify board state before simulated shutdown
    let graph = graph_at(dir);
    let board = graph.get_task(&board_id).unwrap();
    assert_eq!(board.status, Status::InProgress);

    // Phase 4: "Restart" — reload and check board + history
    let graph = load_graph(&path).unwrap();
    let alias = format!(".user-{}", handle);
    let resolved = resolve_user_board_alias(&graph, &alias);
    assert_eq!(resolved, board_id, "Alias should resolve to active board");

    // Messages preserved
    let msgs = workgraph::messages::list_messages(dir, &board_id).unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].body, "What tasks are running?");
    assert_eq!(msgs[1].body, "There are 3 tasks in progress.");
    assert_eq!(msgs[2].body, "Add a new test task");

    // ensure_user_board would skip because active board exists
    let prefix = format!(".user-{}-", handle);
    let has_active = graph
        .tasks()
        .any(|t| is_user_board(&t.id) && t.id.starts_with(&prefix) && !t.status.is_terminal());
    assert!(has_active);
}
