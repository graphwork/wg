use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::graph::{LogEntry, Status, parse_token_usage, parse_wg_tokens};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::modify_graph;
use worksgood::service::registry::AgentRegistry;

/// Fail the exact running attempt while preserving its work for an explicit
/// `wg retry`. Automatic incomplete retries, cooldowns, and tier escalation are
/// retired: they were a second attempt authority outside the lifecycle kernel.
pub fn run(dir: &Path, id: &str, reason: Option<&str>) -> Result<()> {
    {
        let (graph, _) = super::load_workgraph_mut(dir)?;
        let task = graph.get_task_or_err(id)?;
        if task.status != Status::InProgress
            || !task
                .lifecycle
                .current_attempt
                .as_ref()
                .is_some_and(|attempt| {
                    attempt.generation == task.lifecycle.generation && attempt.disposition.is_none()
                })
        {
            anyhow::bail!(
                "Task '{}' has no exact running attempt to mark incomplete (status: {}). Use 'wg retry {}' only after a terminal failure.",
                id,
                task.status,
                id
            );
        }
    }

    super::finalize::record_terminal_abort(
        dir,
        id,
        reason.unwrap_or("task incomplete; work preserved for explicit retry"),
    )?;

    let path = super::graph_path(dir);
    let token_usage = AgentRegistry::load(dir).ok().and_then(|registry| {
        let agent = registry.get_agent_by_task(id)?;
        let output_path = std::path::Path::new(&agent.output_file);
        let absolute = if output_path.is_absolute() {
            output_path.to_path_buf()
        } else {
            dir.parent().unwrap_or(dir).join(output_path)
        };
        parse_token_usage(&absolute).or_else(|| parse_wg_tokens(&absolute))
    });

    let mut agent_id_for_archive = None;
    let mut transition_error = None;
    let reason_owned = reason.map(String::from);
    modify_graph(&path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            transition_error = Some(anyhow::anyhow!("Task '{}' disappeared", id));
            return false;
        };
        if task.status != Status::InProgress {
            transition_error = Some(anyhow::anyhow!(
                "Task '{}' changed before incomplete commit",
                id
            ));
            return false;
        }

        agent_id_for_archive = task.assigned.clone();
        let actor = task.assigned.clone().map_or_else(
            || LifecycleActor::operator(worksgood::current_user()),
            |agent_id| LifecycleActor {
                kind: ActorKind::Worker,
                id: agent_id,
            },
        );
        let attempt_id = task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone())
            .unwrap_or_default();
        let request = TransitionRequest::new(
            TransitionKind::AttemptFailed { class: None },
            actor,
            "worker_reported_incomplete",
            format!("incomplete:{id}:{attempt_id}"),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(rejection) = apply_transition(task, request) {
            transition_error = Some(anyhow::anyhow!(rejection));
            return false;
        }

        task.retry_count = task.retry_count.saturating_add(1);
        task.failure_reason = Some(
            reason_owned
                .clone()
                .unwrap_or_else(|| "Worker reported incomplete work".to_string()),
        );
        task.assigned = None;
        task.completed_at = Some(Utc::now().to_rfc3339());
        task.session_id = None;
        task.checkpoint = None;
        if task.token_usage.is_none()
            && let Some(usage) = token_usage.clone()
        {
            task.token_usage = Some(usage);
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: agent_id_for_archive.clone(),
            user: Some(worksgood::current_user()),
            message: format!(
                "Running attempt failed as incomplete; work preserved for explicit retry: {}",
                reason_owned.as_deref().unwrap_or("unspecified")
            ),
        });
        true
    })
    .context("Failed to save graph")?;
    if let Some(error) = transition_error {
        return Err(error);
    }

    super::notify_graph_changed(dir);
    if let Ok(mut registry) = AgentRegistry::load_locked(dir) {
        if let Some(agent) = registry.get_agent_by_task_mut(id) {
            use worksgood::service::registry::AgentStatus;
            agent.status = AgentStatus::Done;
            agent
                .completed_at
                .get_or_insert_with(|| Utc::now().to_rfc3339());
        }
        let _ = registry.save_ref();
    }
    if let Err(error) = worksgood::disk_sentinel::release_owned_cache_leases(dir, id, None) {
        eprintln!("Warning: failed to release build-cache lease: {error:#}");
    }

    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "incomplete",
        Some(id),
        agent_id_for_archive.as_deref(),
        serde_json::json!({
            "reason": reason,
            "final_status": "failed",
            "retry_policy": "explicit-only",
        }),
        config.log.rotation_threshold,
    );

    let suffix = reason
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    println!(
        "Task '{}' failed as incomplete{}; run 'wg retry {}' to create a new attempt",
        id, suffix, id
    );

    if let Some(agent_id) = agent_id_for_archive {
        match super::log::archive_agent(dir, id, &agent_id) {
            Ok(archive_dir) => eprintln!("Agent archived to {}", archive_dir.display()),
            Err(error) => eprintln!("Warning: failed to archive agent: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::lifecycle::{
        AttemptDisposition, LifecycleActor, TransitionKind, TransitionRequest,
    };
    use worksgood::parser::{load_graph, save_graph};

    fn running_task(id: &str) -> Task {
        let mut task = Task {
            id: id.into(),
            title: id.into(),
            status: Status::Open,
            ..Task::default()
        };
        let request = TransitionRequest::new(
            TransitionKind::AttemptReserved {
                owner_id: Some("agent-1".into()),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "test".into(),
            },
            "test_reservation",
            format!("test-reserve:{id}"),
        );
        apply_transition(&mut task, request).unwrap();
        task.assigned = Some("agent-1".into());
        task.session_id = Some("session-1".into());
        task
    }

    #[test]
    fn incomplete_fails_exact_attempt_and_requires_explicit_retry() {
        let dir = tempdir().unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(running_task("work")));
        save_graph(&graph, &dir.path().join("graph.jsonl")).unwrap();

        run(dir.path(), "work", Some("tests remain red")).unwrap();
        let graph = load_graph(dir.path().join("graph.jsonl")).unwrap();
        let task = graph.get_task("work").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert_eq!(task.retry_count, 1);
        assert!(task.assigned.is_none());
        assert!(task.session_id.is_none());
        assert_eq!(
            task.lifecycle
                .current_attempt
                .as_ref()
                .and_then(|attempt| attempt.disposition),
            Some(AttemptDisposition::Failed)
        );
    }

    #[test]
    fn incomplete_refuses_non_running_task() {
        let dir = tempdir().unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "work".into(),
            title: "work".into(),
            status: Status::Open,
            ..Task::default()
        }));
        save_graph(&graph, &dir.path().join("graph.jsonl")).unwrap();
        assert!(run(dir.path(), "work", None).is_err());
    }
}
