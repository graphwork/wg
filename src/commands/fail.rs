use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::agency::capture_task_output;
use worksgood::dispatch::plan::ExecutorKind;
use worksgood::graph::{
    FailureClass, FailureReason, FailureSignal, LogEntry, Status, evaluate_cycle_on_failure,
    parse_token_usage, parse_wg_tokens,
};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::modify_graph;
use worksgood::service::registry::AgentRegistry;

#[cfg(test)]
use super::graph_path;
#[cfg(test)]
use worksgood::parser::load_graph;

fn failure_signal_for_class(
    class: Option<FailureClass>,
    message: Option<&str>,
    executor: ExecutorKind,
    route: Option<String>,
) -> FailureSignal {
    let reason = match class {
        Some(FailureClass::ApiError429RateLimit) => FailureReason::RateLimit,
        Some(FailureClass::ApiError5xxTransient) => FailureReason::Transient5xx,
        Some(FailureClass::ApiError400Document | FailureClass::ExecutorConfig) => {
            FailureReason::Hard
        }
        Some(FailureClass::AgentHardTimeout) => FailureReason::HardTimeout,
        Some(FailureClass::ResourceExhaustedDisk) => FailureReason::Disk,
        _ => FailureReason::Unknown,
    };
    let mut signal = worksgood::telemetry::failure_signal_from_evidence(
        None,
        None,
        None,
        None,
        message.unwrap_or_default(),
        executor,
        route,
    );
    if reason != FailureReason::Unknown {
        signal.reason = reason;
        signal.confidence = 0.2;
    }
    signal
}

pub fn run(dir: &Path, id: &str, reason: Option<&str>, class: Option<FailureClass>) -> Result<()> {
    run_inner(dir, id, reason, class, false)
}

/// Reject a done task via evaluation gate. This allows failing a task that is
/// already Done — the evaluator determined the work is unacceptable.
pub fn run_eval_reject(dir: &Path, id: &str, reason: Option<&str>) -> Result<()> {
    run_inner(dir, id, reason, None, true)
}

fn run_inner(
    dir: &Path,
    id: &str,
    reason: Option<&str>,
    class: Option<FailureClass>,
    eval_reject: bool,
) -> Result<()> {
    // Pre-check with a non-atomic read (gate only — not used for mutation).
    {
        let (graph, _path) = super::load_workgraph_mut(dir)?;
        let task = graph.get_task_or_err(id)?;

        if task.status == Status::Done {
            anyhow::bail!(
                "Task '{}' is already done and its terminal generation cannot be rewritten; use `wg retry` for a new generation",
                id
            );
        }

        if task.status == Status::Abandoned {
            anyhow::bail!("Task '{}' is already abandoned", id);
        }

        if task.status == Status::Failed {
            println!(
                "Task '{}' is already failed (retry_count: {})",
                id, task.retry_count
            );
            return Ok(());
        }

        // PendingEval is the new soft-done state: eval-gated rejection from
        // this state is the primary path. External `wg fail` is also allowed
        // (no special-case needed — the generic "anything non-terminal can be
        // failed" branch below covers it).
    }

    let path = super::graph_path(dir);

    // Resolve usage and provider evidence outside the graph lock (registry +
    // stream file I/O). The wrapper may record the same attempt afterward;
    // telemetry append deduplicates by task/attempt/executor/bucket.
    let registry = AgentRegistry::load(dir).ok();
    let agent = registry
        .as_ref()
        .and_then(|registry| registry.get_agent_by_task(id));
    let output_path = agent.map(|agent| {
        let path = std::path::Path::new(&agent.output_file);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            dir.parent().unwrap_or(dir).join(path)
        }
    });
    let token_usage = output_path
        .as_deref()
        .and_then(|path| parse_token_usage(path).or_else(|| parse_wg_tokens(path)));
    let executor = agent
        .and_then(|agent| ExecutorKind::from_str(&agent.executor))
        .unwrap_or_default();
    let route = agent.and_then(|agent| agent.model.clone());
    let failure_signal = if eval_reject {
        None
    } else if let Some(output_path) = output_path.as_deref() {
        let raw_stream = output_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("raw_stream.jsonl");
        let detected = super::spawn::raw_stream_classifier::classify_signal_from_raw_stream(
            &raw_stream,
            Some(output_path),
            1,
            executor,
            route.clone(),
        );
        Some(if detected.reason == FailureReason::Unknown {
            failure_signal_for_class(class, reason, executor, route.clone())
        } else {
            detected
        })
    } else {
        Some(failure_signal_for_class(
            class,
            reason,
            executor,
            route.clone(),
        ))
    };

    // Atomically load the freshest graph, apply the mutation, and save.
    // Using modify_graph prevents lost updates from concurrent graph writers.
    let mut retry_count = 0u32;
    let mut max_retries = None;
    let mut agent_id_for_archive = None;
    let mut cycle_reactivated = Vec::new();
    let mut already_failed = false;
    let mut transition_rejection: Option<String> = None;
    let id_owned = id.to_string();
    let reason_owned = reason.map(String::from);
    let graph = modify_graph(&path, |graph| {
        let task = match graph.get_task_mut(&id_owned) {
            Some(t) => t,
            None => return false,
        };

        // Re-check status under lock
        if task.status == Status::Failed {
            already_failed = true;
            retry_count = task.retry_count;
            return false;
        }
        if task.status == Status::Abandoned {
            return false;
        }
        if task.status == Status::Done {
            return false;
        }
        // PendingEval → Failed is allowed from both `wg fail` and the
        // eval-reject path. Falls through to the generic mutation below.
        //
        // FailedPendingEval → Failed is the terminal path after eval rejection
        // (or operator-forced fail). Does NOT trigger auto-rescue spawn.

        // Resource admission deferrals happen before reservation and are
        // recorded by the dispatcher. Once a worker attempt is running,
        // ENOSPC is a real typed attempt failure; it is never rewritten to
        // Open here.

        // Evaluator infrastructure and source execution are independent.
        // A source process failure terminalizes the source attempt; no
        // `FailedPendingEval` rescue status is produced by the authoritative
        // path. Evaluation may append advisory evidence only.

        let actor = if eval_reject {
            LifecycleActor {
                kind: ActorKind::AcceptanceController,
                id: "evaluation-gate".to_string(),
            }
        } else if task.lifecycle.current_attempt.is_some() {
            (std::env::var("WG_TASK_ID").as_deref() == Ok(id_owned.as_str()))
                .then(|| std::env::var("WG_AGENT_ID").ok())
                .flatten()
                .or_else(|| task.assigned.clone())
                .map(LifecycleActor::worker)
                .unwrap_or_else(|| LifecycleActor::operator(worksgood::current_user()))
        } else if let Some(assigned) = task.assigned.clone() {
            // One-release compatibility for pre-ledger rows.
            LifecycleActor::worker(assigned)
        } else {
            LifecycleActor::operator(worksgood::current_user())
        };
        let kind = if eval_reject {
            TransitionKind::AcceptanceRejected {
                evidence_ref: reason_owned
                    .clone()
                    .unwrap_or_else(|| "evaluation-rejected".to_string()),
            }
        } else {
            TransitionKind::AttemptFailed { class }
        };
        let generation = task.lifecycle.generation;
        let mut request = TransitionRequest::new(
            kind,
            actor,
            if eval_reject {
                "acceptance_rejected"
            } else {
                "source_execution_failed"
            },
            format!("fail:{id_owned}:{generation}:{}", task.retry_count),
        );
        if task.lifecycle.current_attempt.is_some() {
            request.expected = FenceExpectation::current(task);
        }
        if let Err(rejection) = apply_transition(task, request) {
            transition_rejection = Some(rejection.to_string());
            return false;
        }
        task.retry_count += 1;
        task.failure_reason = reason_owned.clone();
        task.failure_class = class;
        task.failure_signal = failure_signal.clone();

        let log_message = if eval_reject {
            match reason_owned.as_deref() {
                Some(r) => format!("Evaluation rejected task: {}", r),
                None => "Evaluation rejected task".to_string(),
            }
        } else {
            match reason_owned.as_deref() {
                Some(r) => format!("Task marked as failed: {}", r),
                None => "Task marked as failed".to_string(),
            }
        };
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: task.assigned.clone(),
            user: Some(worksgood::current_user()),
            message: log_message,
        });

        // Apply pre-resolved token usage
        if task.token_usage.is_none()
            && let Some(ref usage) = token_usage
        {
            task.token_usage = Some(usage.clone());
        }

        // Extract values we need before cycle restart may modify the task
        retry_count = task.retry_count;
        max_retries = task.max_retries;
        agent_id_for_archive = task.assigned.clone();

        // Evaluate cycle failure restart — if this task is part of a cycle with
        // restart_on_failure (default true), reset all cycle members to Open.
        let cycle_analysis = graph.compute_cycle_analysis();
        cycle_reactivated = evaluate_cycle_on_failure(graph, &id_owned, &cycle_analysis);

        true
    })
    .context("Failed to save graph")?;

    if already_failed {
        println!(
            "Task '{}' is already failed (retry_count: {})",
            id, retry_count
        );
        return Ok(());
    }
    if let Some(rejection) = transition_rejection {
        anyhow::bail!("Lifecycle transition rejected for '{}': {}", id, rejection);
    }

    if !eval_reject
        && let Some(signal) = failure_signal.clone()
        && let Err(error) = worksgood::telemetry::append_record(
            dir,
            worksgood::telemetry::TelemetryRecord::new(id, retry_count.max(1), signal),
        )
    {
        eprintln!("Warning: failed to append provider telemetry for '{id}': {error:#}");
    }

    super::notify_graph_changed(dir);

    // Update agent registry to reflect task failure.
    // Without this, the registry entry stays at Working until the daemon's
    // periodic triage detects the dead process.
    if let Ok(mut locked_registry) = AgentRegistry::load_locked(dir) {
        if let Some(agent) = locked_registry.get_agent_by_task_mut(id) {
            use worksgood::service::registry::AgentStatus;
            agent.status = AgentStatus::Failed;
            if agent.completed_at.is_none() {
                agent.completed_at = Some(Utc::now().to_rfc3339());
            }
        }
        let _ = locked_registry.save_ref();
    }
    if let Err(error) = worksgood::disk_sentinel::release_owned_cache_leases(dir, id, None) {
        eprintln!("Warning: failed to release build-cache lease: {error:#}");
    }

    if !cycle_reactivated.is_empty() {
        println!(
            "  Cycle failure restart: re-activated {} task(s): {:?}",
            cycle_reactivated.len(),
            cycle_reactivated
        );
    }

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let detail = match reason {
        Some(r) => serde_json::json!({ "reason": r }),
        None => serde_json::Value::Null,
    };
    let _ = worksgood::provenance::record(
        dir,
        "fail",
        Some(id),
        None,
        detail,
        config.log.rotation_threshold,
    );

    let reason_msg = reason.map(|r| format!(" ({})", r)).unwrap_or_default();
    println!(
        "Marked '{}' as failed{} (retry #{})",
        id, reason_msg, retry_count
    );

    // Show retry info if max_retries is set
    if let Some(max) = max_retries {
        if retry_count >= max {
            println!(
                "  Warning: Max retries ({}) reached. Consider abandoning or increasing limit.",
                max
            );
        } else {
            println!("  Retries remaining: {}", max - retry_count);
        }
    }

    // Archive agent conversation (prompt + output) for provenance
    // Use agent_id captured before cycle restart (which clears assigned)
    if let Some(ref agent_id) = agent_id_for_archive {
        match super::log::archive_agent(dir, id, agent_id) {
            Ok(archive_dir) => {
                eprintln!("Agent archived to {}", archive_dir.display());
            }
            Err(e) => {
                eprintln!("Warning: agent archive failed: {}", e);
            }
        }
    }

    // Capture task output (git diff, artifacts, log) for evaluation.
    // Failed tasks are also evaluated when auto_evaluate is enabled — there is
    // useful signal in what kinds of tasks cause which agents to fail.
    if let Some(task) = graph.get_task(id) {
        match capture_task_output(dir, task) {
            Ok(output_dir) => {
                eprintln!("Output captured to {}", output_dir.display());
            }
            Err(e) => {
                eprintln!("Warning: output capture failed: {}", e);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::test_helpers::{make_task_with_status as make_task, setup_workgraph};

    #[test]
    fn test_fail_in_progress_task() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", Some("compilation error"), None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Failed);
    }

    #[test]
    fn disk_resource_failure_terminalizes_running_attempt_without_evaluation_state() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("disk-task", "cargo test full suite", Status::InProgress);
        task.assigned = Some("agent-disk".into());
        setup_workgraph(dir_path, vec![task]);
        let target = dir_path.join("owned-target");
        std::fs::create_dir_all(&target).unwrap();
        worksgood::disk_sentinel::register_owned_cache(
            dir_path,
            worksgood::disk_sentinel::make_owned_cache(
                &target,
                worksgood::disk_sentinel::CacheKind::CargoTarget,
                "disk-task",
                "agent-disk",
                999_999_999,
                None,
                3600,
            ),
        )
        .unwrap();

        run(
            dir_path,
            "disk-task",
            Some("No space left on device"),
            Some(FailureClass::ResourceExhaustedDisk),
        )
        .unwrap();

        let graph = load_graph(graph_path(dir_path)).unwrap();
        let task = graph.get_task("disk-task").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert_eq!(task.assigned.as_deref(), Some("agent-disk"));
        assert_eq!(task.retry_count, 1);
        assert_eq!(
            task.failure_class,
            Some(FailureClass::ResourceExhaustedDisk)
        );
        assert!(
            task.log
                .last()
                .unwrap()
                .message
                .contains("marked as failed")
        );
        let ownership = worksgood::disk_sentinel::load_ownership(dir_path).unwrap();
        let expiry = chrono::DateTime::parse_from_rfc3339(
            &ownership.caches.first().unwrap().lease_expires_at,
        )
        .unwrap()
        .with_timezone(&Utc);
        assert!(
            expiry <= Utc::now(),
            "terminal resource path releases its lease"
        );
        assert!(
            target.exists(),
            "failure bookkeeping never deletes source/cache inline"
        );
    }

    #[test]
    fn test_fail_open_task() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "t1", None, None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Failed);
    }

    #[test]
    fn test_fail_already_done_task_errors() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Done)]);

        let result = run(dir_path, "t1", Some("reason"), None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("already done"),
            "Expected 'already done' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_fail_already_abandoned_task_errors() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::Abandoned)],
        );

        let result = run(dir_path, "t1", Some("reason"), None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("already abandoned"),
            "Expected 'already abandoned' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_fail_increments_retry_count() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        run(dir_path, "t1", None, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.retry_count, 1);
    }

    #[test]
    fn test_fail_stores_failure_reason() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Test task", Status::InProgress)],
        );

        run(dir_path, "t1", Some("timeout exceeded"), None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.failure_reason.as_deref(), Some("timeout exceeded"));
    }

    #[test]
    fn test_fail_no_reason_clears_failure_reason() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.failure_reason = Some("old reason".to_string());
        setup_workgraph(dir_path, vec![task]);

        run(dir_path, "t1", None, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.failure_reason, None);
    }

    #[test]
    fn test_fail_log_entry_includes_reason() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        run(dir_path, "t1", Some("network failure"), None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(!task.log.is_empty());
        let last_log = task.log.last().unwrap();
        assert!(
            last_log.message.contains("network failure"),
            "Log message should contain reason, got: {}",
            last_log.message
        );
    }

    #[test]
    fn test_fail_log_entry_without_reason() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        run(dir_path, "t1", None, None).unwrap();

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        let last_log = task.log.last().unwrap();
        assert_eq!(last_log.message, "Task marked as failed");
    }

    #[test]
    fn test_fail_already_failed_is_noop() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 2;
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", Some("new reason"), None);
        assert!(result.is_ok());

        // Verify nothing changed
        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.retry_count, 2); // Unchanged
        assert_eq!(task.status, Status::Failed);
    }

    #[test]
    fn test_fail_task_not_found() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        let result = run(dir_path, "nonexistent", None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_fail_captures_task_output() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Open)]);

        // Run fail - capture_task_output will be called but may fail in test env
        // (no git repo). The important thing is that run() itself still succeeds.
        let result = run(dir_path, "t1", None, None);
        assert!(result.is_ok());

        // Verify the task was still properly marked as failed despite capture outcome
        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Failed);
    }

    #[test]
    fn test_eval_reject_done_task() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task", Status::Done)]);

        // Normal fail should error on done tasks
        let result = run(dir_path, "t1", Some("reason"), None);
        assert!(result.is_err());

        // Evaluation evidence cannot rewrite an already terminal source.
        let result = run_eval_reject(
            dir_path,
            "t1",
            Some("evaluation score 0.3 below threshold 0.5"),
        );
        assert!(result.is_err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.retry_count, 0);
        assert!(task.failure_reason.is_none());
    }

    #[test]
    fn test_eval_reject_already_failed_is_noop() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task", Status::Failed);
        task.retry_count = 1;
        setup_workgraph(dir_path, vec![task]);

        let result = run_eval_reject(dir_path, "t1", Some("reason"));
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.retry_count, 1); // Unchanged
    }

    #[test]
    fn test_fail_updates_agent_registry() {
        // When a task is marked failed, the agent registry entry should also
        // transition to Failed so the agent slot is freed immediately.
        use worksgood::service::registry::{AgentRegistry, AgentStatus};

        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let mut task = make_task("t1", "Test task", Status::InProgress);
        task.assigned = Some("agent-1".to_string());
        setup_workgraph(dir_path, vec![task]);

        // Set up a registry with an agent working on this task
        let mut registry = AgentRegistry::new();
        registry.register_agent(99999, "t1", "claude", "/tmp/output.log");
        registry.save(dir_path).unwrap();

        let result = run(dir_path, "t1", Some("test failure"), None);
        assert!(result.is_ok());

        // Verify registry was updated
        let registry = AgentRegistry::load(dir_path).unwrap();
        let agent = registry.get_agent("agent-1").unwrap();
        assert_eq!(
            agent.status,
            AgentStatus::Failed,
            "Agent registry should be updated to Failed when task fails"
        );
        assert!(
            agent.completed_at.is_some(),
            "Agent should have a completed_at timestamp"
        );
    }
}
