//! Shared read-side projection of task lifecycle clocks for CLI and TUI views.
//!
//! These clocks deliberately have distinct authority:
//! * Created is the typed task `created_at` value.
//! * Started is the current (or explicitly selected) attempt start.
//! * Agent activity is only the normalized provider-event cursor persisted by
//!   the exact attempt's Pi watchdog. Filesystem mtimes, heartbeats, messages,
//!   task logs, and worktree observations are never fallback clocks.

use crate::attempt_runtime::{AttemptRuntimeKey, resolve_component};
use crate::graph::Task;
use crate::lifecycle::AttemptRef;
use crate::pi_watchdog::PiWatchdog;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;
use std::path::Path;

/// A timestamp together with enough failure state to avoid turning bad or
/// future evidence into a plausible-looking `0s` age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "kebab-case")]
pub enum ProjectedTime {
    Known(DateTime<Utc>),
    Unknown,
    Malformed,
    Future(DateTime<Utc>),
}

impl ProjectedTime {
    fn from_rfc3339(value: Option<&str>, now: DateTime<Utc>) -> Self {
        let Some(value) = value else {
            return Self::Unknown;
        };
        match DateTime::parse_from_rfc3339(value) {
            Ok(value) => Self::validate(value.with_timezone(&Utc), now),
            Err(_) => Self::Malformed,
        }
    }

    fn from_unix(value: i64, now: DateTime<Utc>) -> Self {
        match Utc.timestamp_opt(value, 0).single() {
            Some(value) => Self::validate(value, now),
            None => Self::Malformed,
        }
    }

    fn validate(value: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        if value > now {
            Self::Future(value)
        } else {
            Self::Known(value)
        }
    }

    pub fn compact_age(&self, now: DateTime<Utc>) -> String {
        match self {
            Self::Known(value) => crate::format_duration((now - *value).num_seconds(), true),
            Self::Unknown => "—".to_string(),
            Self::Malformed => "!invalid".to_string(),
            Self::Future(_) => "!future".to_string(),
        }
    }

    /// Local absolute time and relative age, suitable for selected-task detail.
    pub fn detail(&self, now: DateTime<Utc>) -> String {
        match self {
            Self::Known(value) => format!(
                "{} ({} ago)",
                value.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S %:z"),
                crate::format_duration((now - *value).num_seconds(), false)
            ),
            Self::Unknown => "— (unknown)".to_string(),
            Self::Malformed => "! invalid timestamp/evidence".to_string(),
            Self::Future(value) => format!(
                "! future timestamp {}",
                value.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S %:z")
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactWidth {
    Full,
    Narrow,
    Tiny,
}

/// The three lifecycle clocks shown for one exact attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskTimeProjection {
    pub attempt_id: Option<String>,
    pub created: ProjectedTime,
    pub started: ProjectedTime,
    pub agent_activity: ProjectedTime,
}

impl TaskTimeProjection {
    pub fn compact(&self, now: DateTime<Utc>, width: CompactWidth) -> String {
        let created = self.created.compact_age(now);
        let started = self.started.compact_age(now);
        let activity = self.agent_activity.compact_age(now);
        match width {
            CompactWidth::Full => format!("C:{created} S:{started} A:{activity}"),
            CompactWidth::Narrow => format!("C{created} S{started} A{activity}"),
            CompactWidth::Tiny => format!("C{created}/S{started}/A{activity}"),
        }
    }

    pub fn detail_lines(&self, now: DateTime<Utc>) -> [String; 3] {
        [
            format!("Created:        {}", self.created.detail(now)),
            format!("Attempt started: {}", self.started.detail(now)),
            format!("Agent activity: {}", self.agent_activity.detail(now)),
        ]
    }
}

/// Project clocks for the task's exact current attempt.
pub fn project_task_times(wg_dir: &Path, task: &Task, now: DateTime<Utc>) -> TaskTimeProjection {
    let attempt = task.lifecycle.current_attempt.as_ref();
    project_attempt_times(wg_dir, task, attempt, true, now)
}

/// Project an explicitly selected attempt. This is also used by archived
/// attempt detail: the start is recovered from that attempt's accepted
/// reservation event, and activity from that attempt's own runtime namespace.
pub fn project_selected_attempt_times(
    wg_dir: &Path,
    task: &Task,
    attempt: &AttemptRef,
    now: DateTime<Utc>,
) -> TaskTimeProjection {
    let is_current = task
        .lifecycle
        .current_attempt
        .as_ref()
        .is_some_and(|current| current.id == attempt.id && current.fence == attempt.fence);
    project_attempt_times(wg_dir, task, Some(attempt), is_current, now)
}

fn project_attempt_times(
    wg_dir: &Path,
    task: &Task,
    attempt: Option<&AttemptRef>,
    is_current: bool,
    now: DateTime<Utc>,
) -> TaskTimeProjection {
    let created = ProjectedTime::from_rfc3339(task.created_at.as_deref(), now);
    let started_value = if is_current {
        task.started_at.as_deref()
    } else {
        attempt.and_then(|attempt| {
            task.lifecycle
                .audit
                .iter()
                .find(|event| {
                    event.event_kind == "attempt-reserved"
                        && event.attempt_id.as_deref() == Some(attempt.id.as_str())
                        && event.generation == attempt.generation
                        && event.fence == attempt.fence
                })
                .map(|event| event.occurred_at.as_str())
        })
    };
    let started = ProjectedTime::from_rfc3339(started_value, now);
    let agent_activity = attempt.map_or(ProjectedTime::Unknown, |attempt| {
        exact_attempt_activity(wg_dir, task, attempt, now)
    });

    TaskTimeProjection {
        attempt_id: attempt.map(|attempt| attempt.id.clone()),
        created,
        started,
        agent_activity,
    }
}

fn exact_attempt_activity(
    wg_dir: &Path,
    task: &Task,
    attempt: &AttemptRef,
    now: DateTime<Utc>,
) -> ProjectedTime {
    let key = AttemptRuntimeKey::for_attempt(task, attempt);
    let path = match resolve_component(wg_dir, &key, "pi/state.json") {
        Ok(Some(path)) => path,
        Ok(None) => return ProjectedTime::Unknown,
        Err(_) => return ProjectedTime::Malformed,
    };
    let watchdog = match PiWatchdog::open(&path) {
        Ok(watchdog) => watchdog,
        Err(_) => return ProjectedTime::Malformed,
    };
    let state = watchdog.state();
    if state.source.task_id != task.id
        || state.source.generation != attempt.generation
        || state.source.attempt_id != attempt.id
        || state.source.attempt_fence != attempt.fence
        || state.source.worktree_lease_epoch != attempt.fence
    {
        return ProjectedTime::Malformed;
    }
    state
        .native_activity
        .last_activity_at
        .map_or(ProjectedTime::Unknown, |value| {
            ProjectedTime::from_unix(value, now)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{LogEntry, Task};
    use crate::lifecycle::AttemptRef;
    use crate::pi_watchdog::{
        PiWatchdog, ProcessIdentity, QosClass, RouteSnapshot, SessionProof, SourceTuple,
        WatchdogPolicy,
    };
    use std::fs;
    use tempfile::{TempDir, tempdir};

    fn task_with_attempt(id: &str, attempt_id: &str, fence: u64, now: DateTime<Utc>) -> Task {
        let mut task = Task {
            id: id.into(),
            created_at: Some((now - chrono::Duration::hours(2)).to_rfc3339()),
            started_at: Some((now - chrono::Duration::minutes(10)).to_rfc3339()),
            ..Task::default()
        };
        task.lifecycle.generation = 0;
        task.lifecycle.fence = fence;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: attempt_id.into(),
            generation: 0,
            fence,
            actor_id: "agent-test".into(),
            disposition: None,
        });
        task
    }

    fn watchdog_for(dir: &TempDir, task: &Task, now: i64) -> PiWatchdog {
        let attempt = task.lifecycle.current_attempt.as_ref().unwrap();
        let key = AttemptRuntimeKey::for_attempt(task, attempt);
        let root = crate::attempt_runtime::ensure_namespace(dir.path(), &key).unwrap();
        PiWatchdog::new_at(
            root.join("pi/state.json"),
            SourceTuple {
                task_id: task.id.clone(),
                generation: attempt.generation,
                attempt_id: attempt.id.clone(),
                attempt_fence: attempt.fence,
                worktree_lease_epoch: attempt.fence,
                worktree_path: dir.path().join("worktree"),
            },
            RouteSnapshot {
                handler: "pi".into(),
                provider: "test".into(),
                model: "test".into(),
                reasoning: None,
                endpoint_redacted: "test".into(),
                endpoint_hmac: "test".into(),
                qos: QosClass::Standard,
                pi_binary_digest: "test".into(),
                plugin_digest: "test".into(),
            },
            SessionProof {
                session_id: "session".into(),
                branch_leaf: "leaf".into(),
                session_dir: dir.path().join("session"),
                session_file: dir.path().join("session.jsonl"),
                header_digest: "header".into(),
                append_prefix_digest: "prefix".into(),
                append_prefix_len: 0,
            },
            ProcessIdentity {
                pid: 1,
                pgid: 1,
                start_ticks: 1,
                boot_id: "boot".into(),
                nonce: "nonce".into(),
            },
            WatchdogPolicy::default(),
            now,
        )
        .unwrap()
    }

    #[test]
    fn compact_labels_remain_unambiguous_at_every_width() {
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
        let projection = TaskTimeProjection {
            attempt_id: Some("attempt-0-2".into()),
            created: ProjectedTime::Known(now - chrono::Duration::hours(16)),
            started: ProjectedTime::Known(now - chrono::Duration::minutes(40)),
            agent_activity: ProjectedTime::Known(now - chrono::Duration::seconds(3)),
        };
        assert_eq!(
            projection.compact(now, CompactWidth::Full),
            "C:16h S:40m A:3s"
        );
        assert_eq!(
            projection.compact(now, CompactWidth::Narrow),
            "C16h S40m A3s"
        );
        assert_eq!(projection.compact(now, CompactWidth::Tiny), "C16h/S40m/A3s");
    }

    #[test]
    fn missing_malformed_and_future_are_visible_not_zero_seconds() {
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
        let mut task = Task::default();
        task.created_at = Some("not-a-time".into());
        task.started_at = Some((now + chrono::Duration::seconds(5)).to_rfc3339());
        let projection = project_task_times(tempdir().unwrap().path(), &task, now);
        assert_eq!(projection.created, ProjectedTime::Malformed);
        assert!(matches!(projection.started, ProjectedTime::Future(_)));
        assert_eq!(projection.agent_activity, ProjectedTime::Unknown);
        assert_eq!(
            projection.compact(now, CompactWidth::Full),
            "C:!invalid S:!future A:—"
        );
        assert!(!projection.compact(now, CompactWidth::Full).contains("A:0s"));
    }

    #[test]
    fn normalized_text_and_tool_events_advance_only_agent_activity() {
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
        let dir = tempdir().unwrap();
        let mut task = task_with_attempt("task", "attempt-0-1", 1, now);
        let mut watchdog = watchdog_for(&dir, &task, now.timestamp() - 30);
        let before = project_task_times(dir.path(), &task, now);
        assert_eq!(before.agent_activity, ProjectedTime::Unknown);

        watchdog
            .ingest_native_line(
                r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta"}}"#,
                "stream",
                1,
                now.timestamp() - 5,
            )
            .unwrap();
        let text = project_task_times(dir.path(), &task, now);
        assert_eq!(text.created, before.created);
        assert_eq!(text.started, before.started);
        assert_eq!(
            text.agent_activity,
            ProjectedTime::Known(now - chrono::Duration::seconds(5))
        );

        watchdog
            .ingest_native_line(
                r#"{"type":"tool_execution_start","toolName":"read"}"#,
                "stream",
                2,
                now.timestamp() - 2,
            )
            .unwrap();
        let tool = project_task_times(dir.path(), &task, now);
        assert_eq!(tool.created, before.created);
        assert_eq!(tool.started, before.started);
        assert_eq!(
            tool.agent_activity,
            ProjectedTime::Known(now - chrono::Duration::seconds(2))
        );

        // These non-authoritative channels must not impersonate activity.
        task.last_interaction_at = Some(now.to_rfc3339());
        task.last_message_at = Some(now.to_rfc3339());
        task.log.push(LogEntry {
            timestamp: now.to_rfc3339(),
            actor: Some("test".into()),
            user: None,
            message: "heartbeat/log prose".into(),
        });
        fs::write(dir.path().join("graph.jsonl"), "mutation").unwrap();
        fs::write(dir.path().join("arbitrary-output.log"), "new bytes").unwrap();
        assert_eq!(
            project_task_times(dir.path(), &task, now).agent_activity,
            tool.agent_activity
        );
    }

    #[test]
    fn retry_selects_exact_current_attempt_activity() {
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
        let dir = tempdir().unwrap();
        let mut task = task_with_attempt("task", "attempt-0-1", 1, now);
        let old_attempt = task.lifecycle.current_attempt.clone().unwrap();
        let mut old_watchdog = watchdog_for(&dir, &task, now.timestamp() - 60);
        old_watchdog
            .ingest_native_line(
                r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta"}}"#,
                "old",
                1,
                now.timestamp() - 50,
            )
            .unwrap();

        let created = task.created_at.clone();
        task.lifecycle.fence = 2;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-0-2".into(),
            generation: 0,
            fence: 2,
            actor_id: "agent-new".into(),
            disposition: None,
        });
        task.started_at = Some((now - chrono::Duration::seconds(20)).to_rfc3339());
        let mut new_watchdog = watchdog_for(&dir, &task, now.timestamp() - 20);
        new_watchdog
            .ingest_native_line(
                r#"{"type":"tool_execution_start","toolName":"bash"}"#,
                "new",
                1,
                now.timestamp() - 3,
            )
            .unwrap();

        let current = project_task_times(dir.path(), &task, now);
        assert_eq!(task.created_at, created);
        assert_eq!(current.attempt_id.as_deref(), Some("attempt-0-2"));
        assert_eq!(
            current.agent_activity,
            ProjectedTime::Known(now - chrono::Duration::seconds(3))
        );
        let archived = project_selected_attempt_times(dir.path(), &task, &old_attempt, now);
        assert_eq!(archived.attempt_id.as_deref(), Some("attempt-0-1"));
        assert_eq!(
            archived.agent_activity,
            ProjectedTime::Known(now - chrono::Duration::seconds(50))
        );
    }
}
