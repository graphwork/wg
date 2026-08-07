use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::graph::{LogEntry, Status};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use worksgood::parser::modify_graph;

/// Reject the current acceptance candidate without inferring a worker retry.
///
/// The source attempt and its immutable evidence stay in `PendingEval`.
/// Repair, waiver, or an explicit operator retry is a separate request.
pub fn run(dir: &Path, id: &str, reason: &str) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }
    if reason.trim().is_empty() {
        anyhow::bail!("Rejection requires a non-empty evidence reason");
    }

    let reason_owned = reason.to_string();
    let evidence_ref = format!(
        "rejection:{}",
        blake3::hash(reason_owned.as_bytes()).to_hex()
    );
    let mut error = None;
    let mut rejection_count = 0;
    modify_graph(&path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            error = Some(anyhow::anyhow!("Task '{}' not found", id));
            return false;
        };
        if !matches!(task.status, Status::PendingValidation | Status::PendingEval) {
            error = Some(anyhow::anyhow!(
                "Task '{}' is not awaiting acceptance (status: {})",
                id,
                task.status
            ));
            return false;
        }

        let request = TransitionRequest::new(
            TransitionKind::AcceptanceRejected {
                evidence_ref: evidence_ref.clone(),
            },
            LifecycleActor::operator(worksgood::current_user()),
            "acceptance_candidate_rejected",
            format!(
                "reject-candidate:{id}:{}:{}",
                task.lifecycle.generation,
                task.rejection_count.saturating_add(1)
            ),
        )
        .with_evidence(evidence_ref.clone())
        .expecting(FenceExpectation::current(task));
        if let Err(rejection) = apply_transition(task, request) {
            error = Some(anyhow::anyhow!(rejection));
            return false;
        }
        task.rejection_count = task.rejection_count.saturating_add(1);
        rejection_count = task.rejection_count;
        task.failure_reason = Some(format!("Acceptance candidate rejected: {reason_owned}"));
        task.assigned = None;
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: std::env::var("WG_AGENT_ID").ok(),
            user: Some(worksgood::current_user()),
            message: format!(
                "Acceptance candidate rejected (observation {}): {}. Source attempt retained; no retry inferred.",
                rejection_count, reason_owned
            ),
        });
        true
    })
    .context("Failed to save graph")?;
    if let Some(error) = error {
        return Err(error);
    }

    super::notify_graph_changed(dir);
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "reject",
        Some(id),
        None,
        serde_json::json!({
            "reason": reason,
            "rejection_count": rejection_count,
            "outcome": "awaiting-acceptance",
            "evidence_ref": evidence_ref,
        }),
        config.log.rotation_threshold,
    );
    println!(
        "Rejected candidate for '{}'; source attempt retained awaiting repair, waiver, or explicit retry",
        id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::{load_graph, save_graph};

    fn setup(status: Status) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "work".into(),
            title: "work".into(),
            status,
            assigned: Some("agent-1".into()),
            ..Task::default()
        }));
        save_graph(&graph, &dir.path().join("graph.jsonl")).unwrap();
        dir
    }

    #[test]
    fn rejection_retains_candidate_without_retrying() {
        let dir = setup(Status::PendingEval);
        run(dir.path(), "work", "missing edge case").unwrap();
        let graph = load_graph(dir.path().join("graph.jsonl")).unwrap();
        let task = graph.get_task("work").unwrap();
        assert_eq!(task.status, Status::PendingEval);
        assert_eq!(task.rejection_count, 1);
        assert!(task.assigned.is_none());
        assert_eq!(task.lifecycle.audit.len(), 1);
        assert_eq!(
            task.lifecycle.audit[0].reason_code,
            "acceptance_candidate_rejected"
        );
    }

    #[test]
    fn repeated_rejection_never_becomes_retry_or_terminal_failure() {
        let dir = setup(Status::PendingValidation);
        for reason in ["first", "second", "third", "fourth"] {
            run(dir.path(), "work", reason).unwrap();
        }
        let graph = load_graph(dir.path().join("graph.jsonl")).unwrap();
        let task = graph.get_task("work").unwrap();
        assert_eq!(task.status, Status::PendingEval);
        assert_eq!(task.rejection_count, 4);
        assert!(task.lifecycle.reopen_intent.is_none());
    }

    #[test]
    fn rejection_refuses_non_acceptance_state() {
        let dir = setup(Status::Open);
        assert!(run(dir.path(), "work", "no").is_err());
    }
}
