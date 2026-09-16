//! Read-only graph snapshot projection for embedded graph viewers.
//!
//! The pi plugin (`worksgood-pi`) renders a live work-graph panel inside a Pi
//! session. Rather than reading `graph.jsonl` from disk (a second, drifting
//! reader) it polls the daemon's IPC lane — the same protocol the TUI family
//! uses — with a `VizSnapshot` request served by
//! `crate::commands::service::ipc::handle_viz_snapshot`, which delegates the
//! per-task projection to [`viz_snapshot_task`].
//!
//! The projection is deliberately **bounded**: identity, status, dependency
//! edges, timing, aggregate token usage, and a small log tail. No
//! descriptions, transcripts, receipts, or completion evidence — a viewer
//! surface never becomes a bulk channel over the socket, and the request never
//! mutates graph or control-plane state (lifecycle authority stays with
//! workers and the coordinator).

use crate::graph::Task;
use serde_json::Value;

/// Default per-task log entries included in a snapshot.
pub const DEFAULT_LOG_TAIL: usize = 20;

/// Clamp a client-supplied log-tail request into the bounded range.
pub fn clamp_log_tail(requested: Option<usize>) -> usize {
    requested.unwrap_or(DEFAULT_LOG_TAIL).clamp(1, 100)
}

/// Project one task into the bounded JSON shape the viz panel consumes.
///
/// Field names are kebab-case and mirror the graph's own serde encoding so a
/// viewer reads one vocabulary (`in-progress`, `done`, …) instead of a
/// parallel debug-format spelling.
pub fn viz_snapshot_task(task: &Task, log_tail: usize) -> Value {
    let log_tail_entries: Vec<Value> = task
        .log
        .iter()
        .rev()
        .take(log_tail)
        .map(|entry| {
            serde_json::json!({
                "timestamp": entry.timestamp,
                "actor": entry.actor,
                "message": entry.message,
            })
        })
        .collect();
    serde_json::json!({
        "id": task.id,
        "title": task.title,
        "presentation": task.presentation,
        "status": task.status,
        "assigned": task.assigned,
        "after": task.after,
        "before": task.before,
        // Bounded head of the description (the TUI HUD detail shows the full
        // text; the panel gets a bounded preview). Truncated on a char
        // boundary at 512 bytes with an ellipsis marker.
        "description_head": task.description.as_deref().map(|d| {
            if d.len() <= 512 {
                d.to_string()
            } else {
                let mut cut = 512;
                while cut > 0 && !d.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}…", &d[..cut])
            }
        }),
        "created_at": task.created_at,
        "started_at": task.started_at,
        "completed_at": task.completed_at,
        "last_interaction_at": task.last_interaction_at,
        "token_usage": task.token_usage,
        "retry_count": task.retry_count,
        "failure_reason": task.failure_reason,
        "log_count": task.log.len(),
        "log_tail": log_tail_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{LogEntry, Node, Status, WorkGraph};
    use crate::test_helpers::make_task_with_status;

    #[test]
    fn viz_snapshot_projection_is_bounded_and_read_only() {
        let mut graph = WorkGraph::new();
        let mut parent = make_task_with_status("parent-a", "Parent A", Status::Done);
        for i in 0..30 {
            parent.log.push(LogEntry {
                timestamp: format!("2026-01-01T00:00:{:02}Z", i),
                actor: Some("tester".to_string()),
                user: None,
                message: format!("entry {i}"),
            });
        }
        let mut child = make_task_with_status("child-b", "Child B", Status::InProgress);
        child.after = vec!["parent-a".to_string()];
        child.assigned = Some("agent-1".to_string());
        for i in 0..30 {
            child.log.push(LogEntry {
                timestamp: format!("2026-01-01T00:00:{:02}Z", i),
                actor: Some("tester".to_string()),
                user: None,
                message: format!("entry {i}"),
            });
        }
        graph.add_node(Node::Task(parent));
        graph.add_node(Node::Task(child));

        let tail = clamp_log_tail(None);
        assert_eq!(tail, 20);
        assert_eq!(clamp_log_tail(Some(0)), 1, "clamped to the 1..=100 range");
        assert_eq!(clamp_log_tail(Some(500)), 100);

        let projected = graph
            .tasks()
            .map(|t| viz_snapshot_task(t, tail))
            .collect::<Vec<_>>();
        assert_eq!(projected.len(), 2);

        let child_json = projected.iter().find(|t| t["id"] == "child-b").unwrap();
        assert_eq!(child_json["status"], "in-progress");
        assert_eq!(child_json["presentation"], "primary");
        assert_eq!(child_json["after"], serde_json::json!(["parent-a"]));
        assert_eq!(child_json["assigned"], "agent-1");
        // The log tail is bounded even when the task carries 30 entries, and
        // the tail is newest-first.
        assert_eq!(child_json["log_count"], 30);
        let entries = child_json["log_tail"].as_array().unwrap();
        assert_eq!(entries.len(), 20);
        assert_eq!(entries[0]["message"], "entry 29");
        assert_eq!(entries[19]["message"], "entry 10");
        // No description / transcript fields leak into the bounded projection.
        assert!(child_json.get("description").is_none());
    }
}
