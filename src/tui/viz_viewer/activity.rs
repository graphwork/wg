//! Bounded, safe graph-wide activity projection for the TUI.
//!
//! All storage access happens on the auxiliary lane.  This module deliberately
//! extracts only typed labels and counters; message bodies, prompts, reasoning
//! text and tool arguments are never copied into the projection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::Value;

pub const MAX_EVENTS: usize = 500;
const MAX_SOURCE_BYTES: u64 = 512 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const COALESCE_MILLIS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    Observed,
    Proven,
    Control,
}

impl Authority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed/unproven",
            Self::Proven => "proven",
            Self::Control => "control",
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ActivityEventKind {
    Task,
    Lifecycle,
    Agent,
    Native,
    Worktree,
    Validation,
    Finalization,
    Service,
    Message,
}

#[derive(Debug, Clone)]
pub struct ActivityEvent {
    pub id: String,
    /// Full source timestamp retained for detail/selection identity.
    pub timestamp: String,
    pub timestamp_millis: i64,
    pub kind: ActivityEventKind,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub summary: String,
    pub authority: Authority,
    pub count: u32,
    coalescible: bool,
}

impl ActivityEvent {
    pub fn icon(&self) -> &'static str {
        match self.kind {
            ActivityEventKind::Task => "+",
            ActivityEventKind::Lifecycle => "→",
            ActivityEventKind::Agent => "▶",
            ActivityEventKind::Native => "◌",
            ActivityEventKind::Worktree => "Δ",
            ActivityEventKind::Validation => "◆",
            ActivityEventKind::Finalization => "✓",
            ActivityEventKind::Service => "⚙",
            ActivityEventKind::Message => "✉",
        }
    }

    pub fn local_clock(&self) -> String {
        Utc.timestamp_millis_opt(self.timestamp_millis)
            .single()
            .map(|at| at.with_timezone(&Local).format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "??:??:??".into())
    }

    pub fn relative_age_at(&self, now_millis: i64) -> String {
        let seconds = now_millis.saturating_sub(self.timestamp_millis).max(0) / 1_000;
        if seconds < 60 {
            format!("{seconds}s")
        } else if seconds < 3_600 {
            format!("{}m", seconds / 60)
        } else if seconds < 86_400 {
            format!("{}h", seconds / 3_600)
        } else {
            format!("{}d", seconds / 86_400)
        }
    }

    pub fn identity(&self) -> &str {
        self.task_id
            .as_deref()
            .or(self.agent_id.as_deref())
            .unwrap_or("graph")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Projection {
    pub events: VecDeque<ActivityEvent>,
    pub malformed: usize,
    pub unavailable: Vec<&'static str>,
}

pub fn load(workgraph_dir: &Path) -> Projection {
    let mut projection = Projection::default();
    let mut events = Vec::new();

    let operations = workgraph_dir.join("log/operations.jsonl");
    if operations.exists() {
        parse_file(&operations, &mut projection.malformed, |line| {
            parse_operation_into(line, &mut events)
        });
    } else {
        projection.unavailable.push("operations");
    }

    let lifecycle = workgraph_dir.join("lifecycle/events.jsonl");
    if lifecycle.exists() {
        parse_file(&lifecycle, &mut projection.malformed, |line| {
            events.push(lifecycle_event(line)?);
            Some(())
        });
    } else {
        projection.unavailable.push("lifecycle");
    }

    load_required_flip_events(workgraph_dir, &mut events, &mut projection.malformed);
    load_observer_events(workgraph_dir, &mut events, &mut projection.malformed);
    load_finalization_events(workgraph_dir, &mut events, &mut projection.malformed);
    load_native_events(workgraph_dir, &mut events, &mut projection.malformed);

    // Stable de-duplication is independent of source traversal or restart.
    let mut seen = HashSet::new();
    events.retain(|event| seen.insert(event.id.clone()));
    events.sort_by(|a, b| {
        a.timestamp_millis
            .cmp(&b.timestamp_millis)
            .then_with(|| a.id.cmp(&b.id))
    });
    let events = coalesce(events);
    projection.events = events
        .into_iter()
        .rev()
        .take(MAX_EVENTS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    projection
}

fn parse_file(path: &Path, malformed: &mut usize, mut parse: impl FnMut(&str) -> Option<()>) {
    let Ok(lines) = tail_lines(path) else {
        *malformed = malformed.saturating_add(1);
        return;
    };
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_LINE_BYTES || parse(&line).is_none() {
            *malformed = malformed.saturating_add(1);
        }
    }
}

fn tail_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(MAX_SOURCE_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0
        && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
    {
        bytes.drain(..=newline);
    }
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::to_owned)
        .collect())
}

fn timestamp(value: &Value, keys: &[&str]) -> Option<(String, i64)> {
    for key in keys {
        if let Some(raw) = value.get(*key) {
            if let Some(text) = raw.as_str()
                && let Ok(parsed) = DateTime::parse_from_rfc3339(text)
            {
                return Some((text.to_owned(), parsed.timestamp_millis()));
            }
            if let Some(ms) = raw.as_i64() {
                let ms = if ms < 10_000_000_000 { ms * 1_000 } else { ms };
                let text = Utc.timestamp_millis_opt(ms).single()?.to_rfc3339();
                return Some((text, ms));
            }
        }
    }
    None
}

fn stable_id(source: &str, value: &Value) -> String {
    format!(
        "{source}:{}",
        blake3::hash(value.to_string().as_bytes()).to_hex()
    )
}

fn bounded(value: &str, max: usize) -> String {
    let clean: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max)
        .collect();
    if value.chars().count() > max {
        format!("{clean}…")
    } else {
        clean
    }
}

fn operation_event(line: &str) -> Option<ActivityEvent> {
    let value: Value = serde_json::from_str(line).ok()?;
    let (timestamp, timestamp_millis) = timestamp(&value, &["timestamp"])?;
    let op = value.get("op")?.as_str()?;
    let task_id = value.get("task_id").and_then(Value::as_str).map(bounded_id);
    let actor = value.get("actor").and_then(Value::as_str).map(bounded_id);
    let detail = value.get("detail").unwrap_or(&Value::Null);
    let (kind, summary) = match op {
        "add_task" => (ActivityEventKind::Task, "task created".to_string()),
        "edit" => (ActivityEventKind::Task, "task edited".to_string()),
        "publish" => (ActivityEventKind::Task, "task published".to_string()),
        "link" | "unlink" => (ActivityEventKind::Task, format!("dependency {op}")),
        "abandon" => {
            let affected = detail
                .get("affected_ordinary_dependents")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            (
                ActivityEventKind::Lifecycle,
                if affected == 0 {
                    "abandoned (required-success edges fail closed)".to_string()
                } else {
                    format!("abandoned; {affected} ordinary dependent(s) remain blocked")
                },
            )
        }
        "pause" | "resume" | "retry" | "requeue" => (ActivityEventKind::Lifecycle, op.to_string()),
        "claim" | "spawn" => (
            ActivityEventKind::Agent,
            format!("{} admitted", actor.as_deref().unwrap_or("agent")),
        ),
        "done" => (
            ActivityEventKind::Finalization,
            "task completed".to_string(),
        ),
        "fail" => (ActivityEventKind::Lifecycle, "task failed".to_string()),
        "approve" | "evaluate" | "flip" | "validation" => {
            (ActivityEventKind::Validation, bounded(op, 32))
        }
        "message" | "msg" | "msg_send" | "log" => (
            ActivityEventKind::Message,
            "message metadata updated".to_string(),
        ),
        "artifact_add" => (
            ActivityEventKind::Validation,
            "artifact recorded".to_string(),
        ),
        _ if op.contains("merge") || op.contains("candidate") || op.contains("checkpoint") => {
            (ActivityEventKind::Finalization, bounded(op, 48))
        }
        _ if op.contains("watchdog")
            || op.contains("resource")
            || op.contains("service")
            || op.contains("dispatch") =>
        {
            (ActivityEventKind::Service, bounded(op, 48))
        }
        _ => (ActivityEventKind::Task, bounded(op, 48)),
    };
    // Only bounded counters and labels are admitted from detail. Never reasons,
    // bodies, prompts, output, or arbitrary strings.
    let count = detail
        .get("count")
        .or_else(|| detail.get("reset_count"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .min(u32::MAX as u64) as u32;
    Some(ActivityEvent {
        id: value
            .get("operation_id")
            .and_then(Value::as_str)
            .map(|id| format!("op:{}", bounded(id, 96)))
            .unwrap_or_else(|| stable_id("op", &value)),
        timestamp,
        timestamp_millis,
        kind,
        task_id,
        agent_id: actor,
        summary,
        authority: Authority::Control,
        count,
        coalescible: false,
    })
}

fn lifecycle_event(line: &str) -> Option<ActivityEvent> {
    let frame: Value = serde_json::from_str(line).ok()?;
    let value = frame.get("event").unwrap_or(&frame);
    let (timestamp, timestamp_millis) = timestamp(value, &["occurred_at", "committed_at"])?;
    let event_kind = value.get("event_kind")?.as_str()?;
    let task_id = value.get("task_id")?.as_str().map(bounded_id);
    let actor_id = value
        .get("actor_id")
        .and_then(Value::as_str)
        .map(bounded_id);
    let new_state = value.get("new_state").and_then(Value::as_str).unwrap_or("");
    let reason_code = value
        .get("reason_code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let terminal = matches!(new_state, "done" | "failed" | "abandoned")
        || event_kind.contains("succeeded")
        || event_kind.contains("failed")
        || event_kind == "abandoned";
    let kind = if event_kind.contains("attempt-running") || event_kind.contains("reserved") {
        ActivityEventKind::Agent
    } else if event_kind.contains("evaluation") || event_kind.contains("acceptance") {
        ActivityEventKind::Validation
    } else if terminal {
        ActivityEventKind::Finalization
    } else if event_kind.contains("admission") || event_kind.contains("pi-") {
        ActivityEventKind::Service
    } else {
        ActivityEventKind::Lifecycle
    };
    let summary = match reason_code {
        "waiting_on_required_flip" => "waiting on required FLIP".to_string(),
        "deep_flip_rejected_repair_needed" => "FLIP rejected—repair needed".to_string(),
        "deep_flip_accepted_candidate_merged" => "FLIP passed—candidate merged".to_string(),
        "required_flip_operator_waiver" => "required FLIP explicitly waived (audited)".to_string(),
        _ if new_state.is_empty() => bounded(event_kind, 64),
        _ => format!("{} → {}", bounded(event_kind, 48), bounded(new_state, 24)),
    };
    Some(ActivityEvent {
        id: value
            .get("event_id")
            .and_then(Value::as_str)
            .map(|id| format!("life:{}", bounded(id, 96)))
            .unwrap_or_else(|| stable_id("life", value)),
        timestamp,
        timestamp_millis,
        kind,
        task_id,
        agent_id: actor_id,
        summary,
        authority: if terminal {
            Authority::Proven
        } else {
            Authority::Control
        },
        count: 1,
        coalescible: false,
    })
}

fn load_required_flip_events(root: &Path, events: &mut Vec<ActivityEvent>, malformed: &mut usize) {
    let graph_path = root.join("graph.jsonl");
    if !graph_path.exists() {
        return;
    }
    let Ok(graph) = worksgood::parser::load_graph(&graph_path) else {
        *malformed = malformed.saturating_add(1);
        return;
    };
    for task in graph.tasks() {
        let Some(flip) = worksgood::evaluation::flip_gate_projection(task) else {
            continue;
        };
        let Some((timestamp, timestamp_millis)) = timestamp(
            &serde_json::json!({"updated_at": flip.updated_at}),
            &["updated_at"],
        ) else {
            *malformed = malformed.saturating_add(1);
            continue;
        };
        let label = match flip.state.as_str() {
            "flip-queued" => "FLIP queued",
            "flip-running" => "FLIP running",
            "waiting-on-required-flip" => "waiting on required FLIP",
            "flip-rejected-repair-needed" => "FLIP rejected—repair needed",
            "flip-infrastructure-unavailable" => "FLIP infrastructure unavailable",
            "flip-passed-merging" => "FLIP passed—merging",
            "flip-passed-merged" => "FLIP passed—merged",
            _ => "required FLIP",
        };
        let report = flip.report_id.as_deref().unwrap_or("pending");
        let summary = format!(
            "{label} · c={} · r={}",
            bounded(&flip.candidate_id, 8),
            bounded(report, 8)
        );
        events.push(ActivityEvent {
            id: stable_id(
                "required-flip",
                &serde_json::json!({
                    "task": task.id,
                    "state": flip.state,
                    "candidate": flip.candidate_id,
                    "report": flip.report_id,
                    "at": timestamp,
                }),
            ),
            timestamp,
            timestamp_millis,
            kind: ActivityEventKind::Validation,
            task_id: Some(bounded_id(&task.id)),
            agent_id: None,
            summary,
            authority: if matches!(
                flip.state.as_str(),
                "flip-queued" | "flip-running" | "waiting-on-required-flip"
            ) {
                Authority::Control
            } else {
                Authority::Proven
            },
            count: 1,
            coalescible: false,
        });
    }
}

fn load_observer_events(root: &Path, events: &mut Vec<ActivityEvent>, malformed: &mut usize) {
    let attempts = root.join("attempts");
    let mut paths = child_files(&attempts, "worktree-observer/activity.jsonl", 48);
    paths.sort();
    for path in paths {
        parse_file(&path, malformed, |line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let (timestamp, timestamp_millis) = timestamp(&value, &["wall_timestamp"])?;
            let source = value.get("source_tuple")?;
            let task_id = source.get("task_id")?.as_str().map(bounded_id);
            let count = value
                .get("changed_paths")
                .and_then(Value::as_array)
                .map(|paths| paths.len().min(u32::MAX as usize) as u32)
                .unwrap_or(0);
            if count == 0 {
                return Some(());
            }
            let operation = value
                .get("operation_kind")
                .and_then(Value::as_str)
                .unwrap_or("worktree change");
            events.push(ActivityEvent {
                id: value
                    .get("record_hash")
                    .and_then(Value::as_str)
                    .map(|id| format!("observer:{id}"))
                    .unwrap_or_else(|| stable_id("observer", &value)),
                timestamp,
                timestamp_millis,
                kind: ActivityEventKind::Worktree,
                task_id,
                agent_id: source
                    .get("worktree_id")
                    .and_then(Value::as_str)
                    .map(bounded_id),
                summary: format!("{} changed path(s) · {}", count, bounded(operation, 40)),
                authority: Authority::Observed,
                count,
                coalescible: true,
            });
            Some(())
        });
    }
}

fn load_finalization_events(root: &Path, events: &mut Vec<ActivityEvent>, malformed: &mut usize) {
    let dir = root.join("finalization/journal");
    for path in newest_files(&dir, 48) {
        let task_id = path.file_stem().and_then(|v| v.to_str()).map(bounded_id);
        parse_file(&path, malformed, |line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let (timestamp, timestamp_millis) = timestamp(&value, &["at", "timestamp"])?;
            let phase = value.get("phase")?.as_str()?;
            events.push(ActivityEvent {
                id: value
                    .get("digest")
                    .and_then(Value::as_str)
                    .map(|id| format!("final:{id}"))
                    .unwrap_or_else(|| stable_id("final", &value)),
                timestamp,
                timestamp_millis,
                kind: if phase.contains("validat") {
                    ActivityEventKind::Validation
                } else {
                    ActivityEventKind::Finalization
                },
                task_id: task_id.clone(),
                agent_id: None,
                summary: bounded(phase, 64),
                authority: Authority::Proven,
                count: 1,
                coalescible: false,
            });
            Some(())
        });
    }
}

fn load_native_events(root: &Path, events: &mut Vec<ActivityEvent>, malformed: &mut usize) {
    let agents = root.join("agents");
    for metadata_path in newest_named_files(&agents, "metadata.json", 32) {
        let Ok(metadata_text) = fs::read_to_string(&metadata_path) else {
            continue;
        };
        let Ok(metadata): Result<Value, _> = serde_json::from_str(&metadata_text) else {
            *malformed = malformed.saturating_add(1);
            continue;
        };
        let task_id = metadata
            .get("task_id")
            .and_then(Value::as_str)
            .map(bounded_id);
        let agent_id = metadata
            .get("agent_id")
            .and_then(Value::as_str)
            .map(bounded_id);
        let Some(parent) = metadata_path.parent() else {
            continue;
        };
        let stream = parent.join("stream.jsonl");
        if !stream.exists() {
            continue;
        }
        parse_file(&stream, malformed, |line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let event_type = value.get("type")?.as_str()?;
            let label = match event_type {
                "thinking_chunk" => "thinking",
                "text_chunk" => "output",
                "tool_start" | "tool_end" => "tool activity",
                "turn" => "usage receipt",
                "result" => "agent exited",
                _ => return Some(()),
            };
            let (timestamp, timestamp_millis) = timestamp(&value, &["timestamp_ms", "timestamp"])?;
            events.push(ActivityEvent {
                id: stable_id(
                    "native",
                    &serde_json::json!({
                        "agent": agent_id,
                        "task": task_id,
                        "type": event_type,
                        "at": timestamp_millis,
                        "turn": value.get("turn_number"),
                        "name": value.get("name"),
                    }),
                ),
                timestamp,
                timestamp_millis,
                kind: ActivityEventKind::Native,
                task_id: task_id.clone(),
                agent_id: agent_id.clone(),
                summary: label.into(),
                authority: if event_type == "result" {
                    Authority::Proven
                } else {
                    Authority::Observed
                },
                count: 1,
                coalescible: event_type != "result",
            });
            Some(())
        });
    }
}

fn bounded_id(value: &str) -> String {
    bounded(value, 72)
}

fn child_files(root: &Path, suffix: &str, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut children: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path().join(suffix))
        .filter(|path| path.is_file())
        .collect();
    children.sort_by_key(|path| fs::metadata(path).and_then(|m| m.modified()).ok());
    children.into_iter().rev().take(limit).collect()
}

fn newest_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    paths.sort_by_key(|path| fs::metadata(path).and_then(|m| m.modified()).ok());
    paths.into_iter().rev().take(limit).collect()
}

fn newest_named_files(root: &Path, name: &str, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_file())
        .collect();
    paths.sort_by_key(|path| fs::metadata(path).and_then(|m| m.modified()).ok());
    paths.into_iter().rev().take(limit).collect()
}

fn coalesce(events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
    let mut result: Vec<ActivityEvent> = Vec::with_capacity(events.len());
    let mut last_by_key: HashMap<(Option<String>, ActivityEventKind, String), usize> =
        HashMap::new();
    for event in events {
        let key = (
            event.task_id.clone(),
            event.kind.clone(),
            event.summary.clone(),
        );
        if event.coalescible
            && let Some(index) = last_by_key.get(&key).copied()
            && event.timestamp_millis - result[index].timestamp_millis <= COALESCE_MILLIS
        {
            let prior = &mut result[index];
            prior.timestamp = event.timestamp;
            prior.timestamp_millis = event.timestamp_millis;
            prior.count = prior.count.saturating_add(event.count.max(1));
            prior.id = format!("coalesced:{}:{}", prior.id, event.id);
            continue;
        }
        let index = result.len();
        if event.coalescible {
            last_by_key.insert(key, index);
        }
        result.push(event);
    }
    result.sort_by(|a, b| {
        a.timestamp_millis
            .cmp(&b.timestamp_millis)
            .then_with(|| a.id.cmp(&b.id))
    });
    result
}

// parse_file needs an operation parser that also installs the event.
fn parse_operation_into(line: &str, events: &mut Vec<ActivityEvent>) -> Option<()> {
    events.push(operation_event(line)?);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_dedupe_coalescing_authority_and_safe_native_labels() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("log")).unwrap();
        fs::create_dir_all(tmp.path().join("lifecycle")).unwrap();
        fs::create_dir_all(tmp.path().join("attempts/a/worktree-observer")).unwrap();
        fs::create_dir_all(tmp.path().join("finalization/journal")).unwrap();
        fs::create_dir_all(tmp.path().join("agents/agent-1")).unwrap();
        let op = r#"{"timestamp":"2026-01-01T09:00:00Z","op":"add_task","task_id":"t","detail":{"title":"SECRET PROMPT"}}"#;
        fs::write(
            tmp.path().join("log/operations.jsonl"),
            format!("{op}\n{op}\n{{bad\n"),
        )
        .unwrap();
        fs::write(tmp.path().join("lifecycle/events.jsonl"), r#"{"event":{"event_id":"e1","task_id":"t","event_kind":"attempt-running","new_state":"in-progress","actor_id":"agent-1","occurred_at":"2026-01-01T09:00:01Z"}}
"#).unwrap();
        fs::write(tmp.path().join("attempts/a/worktree-observer/activity.jsonl"), r#"{"source_tuple":{"task_id":"t","worktree_id":"agent-1"},"changed_paths":[{"path":"src/a"}],"operation_kind":"candidate-manifest-advance","wall_timestamp":1767258002,"record_hash":"r1"}
{"source_tuple":{"task_id":"t","worktree_id":"agent-1"},"changed_paths":[{"path":"src/b"}],"operation_kind":"candidate-manifest-advance","wall_timestamp":1767258003,"record_hash":"r2"}
"#).unwrap();
        fs::write(
            tmp.path().join("finalization/journal/t.jsonl"),
            r#"{"at":"2026-01-01T09:00:04Z","digest":"d1","phase":"validating"}
{"at":"2026-01-01T09:00:05Z","digest":"d2","phase":"merged"}
"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("agents/agent-1/metadata.json"),
            r#"{"agent_id":"agent-1","task_id":"t"}"#,
        )
        .unwrap();
        fs::write(tmp.path().join("agents/agent-1/stream.jsonl"), r#"{"type":"thinking_chunk","text":"CHAIN OF THOUGHT SECRET","timestamp_ms":1767258006000}
{"type":"thinking_chunk","text":"MORE SECRET","timestamp_ms":1767258007000}
{"type":"result","success":true,"timestamp_ms":1767258008000}
"#).unwrap();

        let projection = load(tmp.path());
        let ordered: Vec<_> = projection.events.iter().collect();
        assert!(
            ordered
                .windows(2)
                .all(|w| w[0].timestamp_millis <= w[1].timestamp_millis)
        );
        assert_eq!(
            projection
                .events
                .iter()
                .filter(|e| e.summary == "task created")
                .count(),
            1
        );
        assert!(
            projection
                .events
                .iter()
                .any(|e| e.authority == Authority::Observed)
        );
        assert!(
            projection
                .events
                .iter()
                .any(|e| e.authority == Authority::Proven)
        );
        assert!(
            projection
                .events
                .iter()
                .any(|e| e.authority == Authority::Control)
        );
        assert!(
            projection
                .events
                .iter()
                .any(|e| e.summary == "thinking" && e.count == 2)
        );
        let rendered = projection
            .events
            .iter()
            .map(|e| &e.summary)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!rendered.contains("SECRET"));
        assert!(projection.malformed > 0);
    }

    #[test]
    fn relative_age_advances_while_clock_stays_stable() {
        let event = ActivityEvent {
            id: "x".into(),
            timestamp: "2026-01-01T09:00:00Z".into(),
            timestamp_millis: 1_000,
            kind: ActivityEventKind::Task,
            task_id: None,
            agent_id: None,
            summary: "x".into(),
            authority: Authority::Control,
            count: 1,
            coalescible: false,
        };
        assert_eq!(event.relative_age_at(13_000), "12s");
        assert_eq!(event.relative_age_at(73_000), "1m");
        assert_eq!(event.local_clock(), event.local_clock());
    }
}
