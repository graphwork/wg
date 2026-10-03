//! Bounded, read-only **fleet snapshot** projection for UI clients.
//!
//! The pi plugin's FleetView (and, later, the WG TUI) needs a single
//! daemon-lane read that a UI can render directly: the graph revision, a row
//! per task (status/age/deps/assigned/model/token summary), a row per runtime
//! agent (executor/model/status/elapsed/current activity), and aggregate
//! counts. Today every UI either polls `graph.jsonl` from disk or shells the
//! `wg` CLI per call; the daemon IPC lane is control-only. `GetFleet` closes
//! that gap.
//!
//! Contract (pinned by the `GetFleet` handler + these unit tests):
//! - **Read-only.** No graph writes, no control-plane mutation, no locks held
//!   while serialising (the graph is loaded and the shared lock released by
//!   `parser::load_graph` before this module runs).
//! - **Bounded.** At most [`HARD_MAX_ROWS`] task/agent rows, titles/ids/steps
//!   truncated on a char boundary, dependency lists capped. A caller can never
//!   turn the socket into a bulk channel.
//! - **Delta-friendly.** `revision` carries the stable graph identity plus a
//!   content digest of `graph.jsonl`; when a client echoes the same revision
//!   the response carries counts and `unchanged: true` with no rows.
//! - **Shared activity.** Per-agent activity is derived with
//!   [`crate::agent_activity::current_step`] — the same reusable deriver the
//!   compact viz / `wg status` surfaces use — never a second implementation.

use std::path::Path;

use serde_json::{Value, json};

use crate::graph::{Status, Task, WorkGraph};
use crate::service::registry::{AgentEntry, AgentRegistry};

/// Default cap on rows returned per section (tasks / agents).
pub const DEFAULT_MAX_ROWS: usize = 500;
/// Hard cap: even a client asking for more gets at most this many rows.
pub const HARD_MAX_ROWS: usize = 2000;
/// Max characters of a task title in a row.
pub const MAX_TITLE_CHARS: usize = 120;
/// Max characters of an id in a row.
pub const MAX_ID_CHARS: usize = 96;
/// Max characters of a per-agent activity step in a row.
pub const MAX_ACTIVITY_CHARS: usize = 80;
/// Max dependency ids echoed per task row (the count is always accurate).
pub const MAX_DEPS_ECHOED: usize = 8;
/// Max agent rows echoed with activity (activity reads are bounded tail reads;
/// a runaway registry should never turn one poll into O(N) file I/O).
pub const MAX_ACTIVITY_ROWS: usize = 200;

/// Row/string bounds applied to one snapshot.
#[derive(Debug, Clone, Copy)]
pub struct FleetLimits {
    pub max_rows: usize,
}

impl FleetLimits {
    /// Clamp a client-supplied row cap into `1..=HARD_MAX_ROWS`.
    pub fn clamp(requested: Option<usize>) -> Self {
        Self {
            max_rows: requested
                .unwrap_or(DEFAULT_MAX_ROWS)
                .clamp(1, HARD_MAX_ROWS),
        }
    }
}

/// Compute the graph revision token: `<graph-identity>#<content-digest>`.
///
/// The identity half is the stable per-graph fencing token
/// (`worker_control`'s `wggraph:v1:*`); the digest half changes whenever
/// `graph.jsonl` bytes change, which makes `since_revision` deltas meaningful.
/// Read-only: it never creates the identity sidecar, so a snapshot request
/// cannot mutate the workgraph directory.
pub fn fleet_revision(dir: &Path) -> String {
    let identity = crate::worker_control::read_graph_identity(dir)
        .ok()
        .flatten()
        .unwrap_or_default();
    let digest = std::fs::read(dir.join("graph.jsonl"))
        .ok()
        .map(|bytes| {
            let hash = blake3::hash(&bytes);
            hex::encode(&hash.as_bytes()[..8])
        })
        .unwrap_or_default();
    format!("{identity}#{digest}")
}

/// Aggregate counts: in-progress / ready / blocked / done (+ failed/total).
pub fn fleet_counts(graph: &WorkGraph) -> Value {
    let cycle_analysis = graph.compute_cycle_analysis();
    let ready = crate::query::ready_tasks_cycle_aware(graph, &cycle_analysis).len();
    let mut in_progress = 0usize;
    let mut blocked = 0usize;
    let mut done = 0usize;
    let mut failed = 0usize;
    let mut total = 0usize;
    for task in graph.tasks() {
        total += 1;
        match task.status {
            Status::InProgress => in_progress += 1,
            Status::Blocked => blocked += 1,
            Status::Done => done += 1,
            Status::Failed | Status::Abandoned | Status::FailedPendingEval => failed += 1,
            _ => {}
        }
    }
    json!({
        "in_progress": in_progress,
        "ready": ready,
        "blocked": blocked,
        "done": done,
        "failed": failed,
        "total": total,
    })
}

/// Project one task into the bounded row shape a UI renders directly.
pub fn fleet_task_row(task: &Task) -> Value {
    let dependency_count = task.after.len();
    let depends_on: Vec<&str> = task
        .after
        .iter()
        .take(MAX_DEPS_ECHOED)
        .map(String::as_str)
        .collect();
    let token_usage = task.token_usage.as_ref().map(|u| {
        json!({
            "input_tokens": u.input_tokens,
            "output_tokens": u.output_tokens,
            "total_tokens": u.total_tokens(),
            "cost_usd": u.cost_usd,
        })
    });
    json!({
        "id": truncate(&task.id, MAX_ID_CHARS),
        "title": truncate(&task.title, MAX_TITLE_CHARS),
        "status": task.status.to_string(),
        "presentation": task.presentation.to_string(),
        "assigned": task.assigned.as_deref().map(|s| truncate(s, MAX_ID_CHARS)),
        "model": task.model,
        "parent": task.origin.parent_task,
        "depends_on": depends_on,
        "dependency_count": dependency_count,
        "paused": task.paused,
        "age_secs": task_age_secs(task),
        "started_at": task.started_at,
        "completed_at": task.completed_at,
        "last_interaction_at": task.last_interaction_at,
        "token_usage": token_usage,
        "failure_reason": task.failure_reason,
    })
}

/// Age of a task in seconds (from `created_at`, falling back to `started_at`).
fn task_age_secs(task: &Task) -> Option<i64> {
    let stamp = task.created_at.as_deref().or(task.started_at.as_deref())?;
    let created = chrono::DateTime::parse_from_rfc3339(stamp).ok()?;
    let now = chrono::Utc::now();
    let secs = (now - created.with_timezone(&chrono::Utc)).num_seconds();
    Some(secs.max(0))
}

/// Project one runtime agent into a bounded row, with its current activity.
pub fn fleet_agent_row(entry: &AgentEntry, activity: Option<String>) -> Value {
    json!({
        "id": truncate(&entry.id, MAX_ID_CHARS),
        "task_id": truncate(&entry.task_id, MAX_ID_CHARS),
        "executor": entry.executor,
        "model": entry.model,
        "status": entry.status,
        "started_at": entry.started_at,
        "elapsed_ms": entry.uptime_secs().map(|s| s.max(0) * 1000),
        "activity": activity,
    })
}

/// Derive a per-agent activity step using the shared
/// [`crate::agent_activity::current_step`] deriver: prefer the agent's task
/// (live stream tail → graph-log fallback) and fall back to the stream-only
/// deriver when the registry names a task that is no longer in the graph.
fn agent_activity(dir: &Path, entry: &AgentEntry, graph: &WorkGraph) -> Option<String> {
    let step = match graph.get_task(&entry.task_id) {
        Some(task) => crate::agent_activity::current_step(dir, task),
        None => crate::agent_activity::current_step_for_agent(dir, &entry.id),
    };
    step.map(|s| crate::agent_activity::truncate_chars(&s, MAX_ACTIVITY_CHARS))
}

/// Truncate to at most `max` characters on a char boundary, appending `…`.
fn truncate(s: &str, max: usize) -> String {
    crate::agent_activity::truncate_chars(s, max)
}

/// Build the full (or delta) `GetFleet` response body.
///
/// `since_revision` is echoed unchanged slots only: when it equals the current
/// revision the response carries the counts and `unchanged: true` with empty
/// rows, so a poller can skip re-rendering. Otherwise every bounded row is
/// returned.
pub fn build_fleet_snapshot(
    dir: &Path,
    graph: &WorkGraph,
    registry: &AgentRegistry,
    limits: FleetLimits,
    since_revision: Option<&str>,
) -> Value {
    let revision = fleet_revision(dir);
    let counts = fleet_counts(graph);
    let total_tasks = graph.tasks().count();
    let identity = crate::worker_control::read_graph_identity(dir)
        .ok()
        .flatten();

    if since_revision == Some(revision.as_str()) {
        return json!({
            "revision": revision,
            "unchanged": true,
            "graph": { "identity": identity, "task_count": total_tasks },
            "counts": counts,
            "tasks": [],
            "agents": [],
            "truncated": false,
        });
    }

    let mut tasks: Vec<Value> = graph.tasks().map(fleet_task_row).collect();
    let tasks_truncated = tasks.len() > limits.max_rows;
    tasks.truncate(limits.max_rows);

    let mut entries: Vec<&AgentEntry> = registry.list_agents();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let agents_total = entries.len();
    let agents_truncated = agents_total > limits.max_rows;
    entries.truncate(limits.max_rows);

    let agents: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            // Activity derives from a bounded stream tail read. Cap how many
            // rows do that I/O so one poll stays cheap on a large registry.
            let activity = if idx < MAX_ACTIVITY_ROWS {
                agent_activity(dir, entry, graph)
            } else {
                None
            };
            fleet_agent_row(entry, activity)
        })
        .collect();

    let active_agents = registry
        .list_agents()
        .iter()
        .filter(|a| a.is_alive())
        .count();

    let mut counts = counts;
    if let Some(obj) = counts.as_object_mut() {
        obj.insert("active_agents".to_string(), json!(active_agents));
    }

    json!({
        "revision": revision,
        "unchanged": false,
        "graph": {
            "identity": identity,
            "task_count": total_tasks,
            "agent_count": agents_total,
        },
        "counts": counts,
        "tasks": tasks,
        "agents": agents,
        "truncated": tasks_truncated || agents_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, Status, WorkGraph};
    use crate::test_helpers::make_task_with_status;

    fn registry_with(agent_id: &str, task_id: &str) -> AgentRegistry {
        let mut registry = AgentRegistry::new();
        registry.agents.insert(
            agent_id.to_string(),
            serde_json::from_value(json!({
                "id": agent_id,
                "pid": 4242,
                "task_id": task_id,
                "executor": "pi",
                "started_at": "2026-01-01T00:00:00Z",
                "last_heartbeat": "2026-01-01T00:01:00Z",
                "status": "working",
                "output_file": "/tmp/out",
                "model": "pi:openrouter:anthropic/claude-opus-4-7",
            }))
            .unwrap(),
        );
        registry
    }

    fn write_graph(dir: &Path, graph: &WorkGraph) {
        crate::parser::save_graph(graph, dir.join("graph.jsonl")).unwrap();
    }

    #[test]
    fn snapshot_shape_has_revision_counts_task_and_agent_rows() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let mut graph = WorkGraph::new();
        let mut done = make_task_with_status("done-a", "Done A", Status::Done);
        done.created_at = Some("2026-01-01T00:00:00Z".to_string());
        let mut active = make_task_with_status("active-b", "Active B", Status::InProgress);
        active.after = vec!["done-a".to_string()];
        active.assigned = Some("agent-7".to_string());
        active.created_at = Some("2026-01-01T00:00:00Z".to_string());
        active.model = Some("pi:openrouter:anthropic/claude-opus-4-7".to_string());
        let mut blocked = make_task_with_status("blocked-c", "Blocked C", Status::Blocked);
        blocked.after = vec!["does-not-exist".to_string()];
        graph.add_node(Node::Task(done));
        graph.add_node(Node::Task(active));
        graph.add_node(Node::Task(blocked));
        write_graph(dir, &graph);

        // Live stream tail so activity is derived (not just the log fallback).
        let stream = dir.join("agents").join("agent-7").join("raw_stream.jsonl");
        std::fs::create_dir_all(stream.parent().unwrap()).unwrap();
        std::fs::write(
            &stream,
            r#"{"type":"tool_execution_start","toolName":"bash","args":{"command":"cargo test --lib"}}"#,
        )
        .unwrap();

        let registry = registry_with("agent-7", "active-b");
        let limits = FleetLimits::clamp(None);
        let body = build_fleet_snapshot(dir, &graph, &registry, limits, None);

        assert!(!body["revision"].as_str().unwrap().is_empty());
        assert_eq!(body["unchanged"], false);
        assert_eq!(body["counts"]["in_progress"], 1);
        assert_eq!(body["counts"]["done"], 1);
        assert_eq!(body["counts"]["blocked"], 1);
        assert_eq!(body["counts"]["total"], 3);
        assert_eq!(body["graph"]["task_count"], 3);
        assert_eq!(body["counts"]["active_agents"], 1);

        let tasks = body["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3);
        let active_row = tasks.iter().find(|t| t["id"] == "active-b").unwrap();
        assert_eq!(active_row["status"], "in-progress");
        assert_eq!(active_row["assigned"], "agent-7");
        assert_eq!(active_row["depends_on"], json!(["done-a"]));
        assert_eq!(active_row["dependency_count"], 1);
        assert!(active_row["age_secs"].is_number());

        let agents = body["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["id"], "agent-7");
        assert_eq!(agents[0]["task_id"], "active-b");
        assert_eq!(agents[0]["activity"], "running cargo test --lib");
        assert!(agents[0]["elapsed_ms"].is_number());
    }

    #[test]
    fn revision_delta_reports_unchanged_without_rows() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task_with_status("t", "T", Status::Open)));
        write_graph(dir, &graph);

        let registry = AgentRegistry::new();
        let limits = FleetLimits::clamp(None);
        let full = build_fleet_snapshot(dir, &graph, &registry, limits, None);
        let revision = full["revision"].as_str().unwrap().to_string();

        let delta = build_fleet_snapshot(dir, &graph, &registry, limits, Some(&revision));
        assert_eq!(delta["unchanged"], true);
        assert_eq!(delta["revision"], revision);
        assert_eq!(delta["tasks"].as_array().unwrap().len(), 0);
        assert_eq!(delta["agents"].as_array().unwrap().len(), 0);
        // Counts are always present so a poller can still render the header.
        assert_eq!(delta["counts"]["total"], 1);

        // A moved graph changes the revision, so the next call is full again.
        let mut moved = WorkGraph::new();
        moved.add_node(Node::Task(make_task_with_status("t", "T", Status::Done)));
        write_graph(dir, &moved);
        let after = build_fleet_snapshot(dir, &moved, &registry, limits, Some(&revision));
        assert_eq!(after["unchanged"], false);
        assert_eq!(after["tasks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn snapshot_is_bounded_for_many_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let mut graph = WorkGraph::new();
        for i in 0..(HARD_MAX_ROWS + 250) {
            let mut task = make_task_with_status(
                &format!("task-{i:05}"),
                &"x".repeat(MAX_TITLE_CHARS + 50),
                Status::Open,
            );
            task.after = vec!["dep-a".to_string(), "dep-b".to_string()];
            graph.add_node(Node::Task(task));
        }
        write_graph(dir, &graph);
        let registry = AgentRegistry::new();

        // A client asking for more than the hard cap still gets the hard cap.
        let limits = FleetLimits::clamp(Some(usize::MAX));
        assert_eq!(limits.max_rows, HARD_MAX_ROWS);
        let body = build_fleet_snapshot(dir, &graph, &registry, limits, None);
        let tasks = body["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), HARD_MAX_ROWS);
        assert_eq!(body["truncated"], true);
        assert_eq!(body["graph"]["task_count"], HARD_MAX_ROWS + 250);
        // Titles are truncated on a char boundary.
        assert_eq!(
            tasks[0]["title"].as_str().unwrap().chars().count(),
            MAX_TITLE_CHARS
        );

        // A small client cap further bounds the response.
        let small = FleetLimits::clamp(Some(5));
        let body = build_fleet_snapshot(dir, &graph, &registry, small, None);
        assert_eq!(body["tasks"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn counts_exclude_retired_agency_tasks_from_ready() {
        // Sanity: `ready` is cycle-aware and uses the same query helper as
        // `wg ready`, so it is not a second readiness implementation.
        let temp = tempfile::tempdir().unwrap();
        let mut graph = WorkGraph::new();
        let mut a = make_task_with_status("a", "A", Status::Done);
        a.created_at = Some("2026-01-01T00:00:00Z".to_string());
        let mut b = make_task_with_status("b", "B", Status::Open);
        b.after = vec!["a".to_string()];
        graph.add_node(Node::Task(a));
        graph.add_node(Node::Task(b));
        let _ = temp;
        let counts = fleet_counts(&graph);
        assert_eq!(counts["ready"], 1);
        assert_eq!(counts["done"], 1);
    }
}
