//! Tests for the `last_interaction_at` primitive (task `revert-redo-fix`).
//!
//! Locks the regression behind the bad ship of fix-tui-graph (commit
//! 73f2f5c11): every substantive task mutation must bump
//! `last_interaction_at`, but the field must NOT be touched by side-channel
//! reads or by serde alone. Migration: tasks predating the field default to
//! their `created_at` timestamp.

use std::io::Write;
use tempfile::{NamedTempFile, tempdir};
use workgraph::graph::{Node, Status, Task, WorkGraph};
use workgraph::parser::{load_graph, modify_graph, save_graph};
use workgraph::{chat, messages};

fn make_task(id: &str, title: &str) -> Task {
    Task {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::Open,
        created_at: Some("2026-04-30T00:00:00+00:00".to_string()),
        ..Task::default()
    }
}

#[test]
fn migration_defaults_last_interaction_at_to_created_at() {
    let mut file = NamedTempFile::new().unwrap();
    // Old-format task on disk: no `last_interaction_at` field at all.
    writeln!(
        file,
        r#"{{"id":"old","kind":"task","title":"Old","status":"open","created_at":"2026-04-30T00:00:00+00:00"}}"#
    )
    .unwrap();

    let graph = load_graph(file.path()).unwrap();
    let task = graph.get_task("old").unwrap();
    assert_eq!(
        task.last_interaction_at.as_deref(),
        Some("2026-04-30T00:00:00+00:00"),
        "tasks predating the field must default to created_at on read"
    );
}

#[test]
fn migration_leaves_last_interaction_at_alone_when_present() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(
        file,
        r#"{{"id":"new","kind":"task","title":"New","status":"open","created_at":"2026-04-30T00:00:00+00:00","last_interaction_at":"2026-05-01T12:00:00+00:00"}}"#
    )
    .unwrap();

    let graph = load_graph(file.path()).unwrap();
    let task = graph.get_task("new").unwrap();
    assert_eq!(
        task.last_interaction_at.as_deref(),
        Some("2026-05-01T12:00:00+00:00")
    );
}

#[test]
fn modify_graph_bumps_on_substantive_change() {
    let file = NamedTempFile::new().unwrap();
    let mut graph = WorkGraph::default();
    graph.add_node(Node::Task(make_task("t1", "First")));
    save_graph(&graph, file.path()).unwrap();

    // Sleep briefly so any new timestamp is strictly after the saved one.
    std::thread::sleep(std::time::Duration::from_millis(20));

    let pre = load_graph(file.path()).unwrap();
    let pre_ts = pre
        .get_task("t1")
        .unwrap()
        .last_interaction_at
        .clone()
        .unwrap_or_default();

    modify_graph(file.path(), |g| {
        let t = g.get_task_mut("t1").unwrap();
        t.status = Status::InProgress;
        true
    })
    .unwrap();

    let post = load_graph(file.path()).unwrap();
    let post_ts = post.get_task("t1").unwrap().last_interaction_at.clone();
    assert!(
        post_ts.is_some(),
        "modify_graph must bump last_interaction_at on substantive changes"
    );
    assert_ne!(
        post_ts.as_deref().unwrap_or(""),
        pre_ts,
        "timestamp must move forward after a substantive mutation"
    );
}

#[test]
fn modify_graph_does_not_bump_when_closure_skips_save() {
    let file = NamedTempFile::new().unwrap();
    let mut graph = WorkGraph::default();
    graph.add_node(Node::Task(make_task("t1", "First")));
    save_graph(&graph, file.path()).unwrap();

    let pre = load_graph(file.path()).unwrap();
    let pre_ts = pre.get_task("t1").unwrap().last_interaction_at.clone();

    std::thread::sleep(std::time::Duration::from_millis(20));

    // Closure returns false — the bump must not run because save is skipped.
    modify_graph(file.path(), |_g| false).unwrap();

    let post = load_graph(file.path()).unwrap();
    let post_ts = post.get_task("t1").unwrap().last_interaction_at.clone();
    assert_eq!(
        post_ts, pre_ts,
        "skipped saves must leave last_interaction_at untouched"
    );
}

#[test]
fn modify_graph_does_not_bump_when_only_interaction_field_changed() {
    // Closure manually pokes last_interaction_at but changes nothing else.
    // Because the field is excluded from substantively_eq, the auto-bump
    // pass treats this as a no-op — preventing infinite re-bumping.
    let file = NamedTempFile::new().unwrap();
    let mut graph = WorkGraph::default();
    graph.add_node(Node::Task(make_task("t1", "First")));
    save_graph(&graph, file.path()).unwrap();

    modify_graph(file.path(), |g| {
        let t = g.get_task_mut("t1").unwrap();
        t.last_interaction_at = Some("2099-01-01T00:00:00+00:00".to_string());
        true
    })
    .unwrap();

    let post = load_graph(file.path()).unwrap();
    let post_ts = post
        .get_task("t1")
        .unwrap()
        .last_interaction_at
        .clone()
        .unwrap();
    assert_eq!(
        post_ts, "2099-01-01T00:00:00+00:00",
        "closure-set timestamps must be preserved when nothing else changed"
    );
}

#[test]
fn modify_graph_bumps_only_changed_tasks() {
    let file = NamedTempFile::new().unwrap();
    let mut graph = workgraph::graph::WorkGraph::default();
    graph.add_node(Node::Task(make_task("a", "Task A")));
    graph.add_node(Node::Task(make_task("b", "Task B")));
    save_graph(&graph, file.path()).unwrap();

    let pre = load_graph(file.path()).unwrap();
    let a_ts_pre = pre.get_task("a").unwrap().last_interaction_at.clone();
    let b_ts_pre = pre.get_task("b").unwrap().last_interaction_at.clone();

    std::thread::sleep(std::time::Duration::from_millis(20));

    modify_graph(file.path(), |g| {
        g.get_task_mut("a").unwrap().status = Status::InProgress;
        true
    })
    .unwrap();

    let post = load_graph(file.path()).unwrap();
    let a_ts_post = post.get_task("a").unwrap().last_interaction_at.clone();
    let b_ts_post = post.get_task("b").unwrap().last_interaction_at.clone();
    assert_ne!(a_ts_pre, a_ts_post, "changed task A must bump");
    assert_eq!(b_ts_pre, b_ts_post, "untouched task B must NOT bump");
}

#[test]
fn modify_graph_bumps_brand_new_task() {
    let file = NamedTempFile::new().unwrap();
    save_graph(&workgraph::graph::WorkGraph::default(), file.path()).unwrap();

    modify_graph(file.path(), |g| {
        g.add_node(Node::Task(Task {
            id: "fresh".to_string(),
            title: "Fresh".to_string(),
            ..Task::default()
        }));
        true
    })
    .unwrap();

    let post = load_graph(file.path()).unwrap();
    assert!(
        post.get_task("fresh")
            .unwrap()
            .last_interaction_at
            .is_some(),
        "freshly-created tasks without an interaction timestamp must be bumped"
    );
}

#[test]
fn interaction_sort_key_falls_back_to_created_at() {
    let mut t = make_task("x", "X");
    t.last_interaction_at = None;
    t.created_at = Some("2026-04-01T00:00:00+00:00".to_string());
    assert_eq!(t.interaction_sort_key(), "2026-04-01T00:00:00+00:00");
}

#[test]
fn touch_sets_last_interaction_at() {
    let mut t = make_task("x", "X");
    t.last_interaction_at = None;
    t.touch();
    assert!(
        t.last_interaction_at.is_some(),
        "touch() must populate last_interaction_at"
    );
}

#[test]
fn substantively_eq_ignores_interaction_field() {
    let mut a = make_task("x", "X");
    let mut b = a.clone();
    a.last_interaction_at = Some("2026-04-01T00:00:00+00:00".to_string());
    b.last_interaction_at = Some("2099-12-31T00:00:00+00:00".to_string());
    assert!(
        a.substantively_eq(&b),
        "tasks differing only in last_interaction_at must compare substantively-equal"
    );
}

#[test]
fn chat_append_bumps_modern_chat_task() {
    let tmp = tempdir().unwrap();
    let graph_path = tmp.path().join("graph.jsonl");
    let mut graph = WorkGraph::default();
    let mut chat_task = make_task(".chat-0", "Chat 0");
    chat_task.status = Status::InProgress;
    chat_task.last_interaction_at = Some("2026-04-30T00:00:00+00:00".to_string());
    graph.add_node(Node::Task(chat_task));
    save_graph(&graph, &graph_path).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));

    chat::append_inbox_for(tmp.path(), 0, "recent chat activity", "req-1").unwrap();

    let post = load_graph(&graph_path).unwrap();
    let task = post.get_task(".chat-0").unwrap();
    assert_ne!(
        task.last_interaction_at.as_deref(),
        Some("2026-04-30T00:00:00+00:00"),
        "chat appends through numeric aliases must touch the canonical .chat-N task"
    );
}

#[test]
fn task_message_send_bumps_last_interaction_at() {
    let tmp = tempdir().unwrap();
    let graph_path = tmp.path().join("graph.jsonl");
    let mut graph = WorkGraph::default();
    let mut task = make_task("task-a", "Task A");
    task.last_interaction_at = Some("2026-04-30T00:00:00+00:00".to_string());
    graph.add_node(Node::Task(task));
    save_graph(&graph, &graph_path).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));

    messages::send_message(tmp.path(), "task-a", "hello", "user", "normal").unwrap();

    let post = load_graph(&graph_path).unwrap();
    let task = post.get_task("task-a").unwrap();
    assert_ne!(
        task.last_interaction_at.as_deref(),
        Some("2026-04-30T00:00:00+00:00"),
        "wg msg send must touch the target task so recent messages affect TUI ordering"
    );
}

#[test]
fn substantively_eq_detects_real_diffs() {
    let mut a = make_task("x", "X");
    let mut b = a.clone();
    a.status = Status::Open;
    b.status = Status::InProgress;
    assert!(
        !a.substantively_eq(&b),
        "differing status must be detected by substantively_eq"
    );
}
