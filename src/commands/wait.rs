use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use worksgood::graph::{
    LogEntry, MessageWaitSelector, MessageWaitSubscription, Status, Task, WaitCondition, WaitSpec,
    parse_delay,
};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, PiAuthorizationState, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::modify_graph;
use worksgood::pi_watchdog::PiWatchdog;
use worksgood::service::registry::{AgentRegistry, AgentStatus};

/// Parse a condition string into a WaitCondition.
///
/// Supported formats:
/// - `task:<id>=<status>` — wait for a task to reach a status
/// - `timer:<duration>` — wait for a duration (e.g. 5m, 2h, 30s)
/// - `human-input` — wait for a human message
/// - `message` — wait for any message
/// - `file:<path>` — wait for a file to change
fn message_selector(spec: &WaitSpec) -> Option<MessageWaitSelector> {
    let conditions = match spec {
        WaitSpec::All(conditions) | WaitSpec::Any(conditions) => conditions,
    };
    if conditions
        .iter()
        .any(|condition| matches!(condition, WaitCondition::Message))
    {
        Some(MessageWaitSelector::AnyMessage)
    } else if conditions
        .iter()
        .any(|condition| matches!(condition, WaitCondition::HumanInput))
    {
        Some(MessageWaitSelector::HumanInput)
    } else {
        None
    }
}

fn parse_condition(s: &str, graph: &worksgood::graph::WorkGraph) -> Result<WaitCondition> {
    let s = s.trim();

    if s == "human-input" {
        return Ok(WaitCondition::HumanInput);
    }
    if s == "message" {
        return Ok(WaitCondition::Message);
    }

    if let Some(rest) = s.strip_prefix("task:") {
        // Format: task:<id>=<status>
        let parts: Vec<&str> = rest.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "Invalid task condition '{}'. Expected format: task:<id>=<status>",
                s
            );
        }
        let task_id = parts[0];
        let status_str = parts[1];

        // Validate the referenced task exists
        if graph.get_task(task_id).is_none() {
            anyhow::bail!("Task '{}' referenced in condition does not exist", task_id);
        }

        let status = match status_str {
            "open" => Status::Open,
            "in-progress" => Status::InProgress,
            "waiting" => Status::Waiting,
            "done" => Status::Done,
            "blocked" => Status::Blocked,
            "failed" => Status::Failed,
            "abandoned" => Status::Abandoned,
            other => anyhow::bail!("Unknown status '{}' in condition", other),
        };

        return Ok(WaitCondition::TaskStatus {
            task_id: task_id.to_string(),
            status,
        });
    }

    if let Some(rest) = s.strip_prefix("timer:") {
        let secs = parse_delay(rest).ok_or_else(|| {
            anyhow::anyhow!("Invalid timer duration '{}'. Use e.g. 5m, 2h, 30s", rest)
        })?;
        let resume_after = Utc::now() + chrono::Duration::seconds(secs as i64);
        return Ok(WaitCondition::Timer {
            resume_after: resume_after.to_rfc3339(),
        });
    }

    if let Some(rest) = s.strip_prefix("file:") {
        let path = rest.trim();
        if path.is_empty() {
            anyhow::bail!("Empty file path in condition");
        }
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
            .unwrap_or(0);
        return Ok(WaitCondition::FileChanged {
            path: path.to_string(),
            mtime_at_wait: mtime,
        });
    }

    anyhow::bail!(
        "Unknown condition '{}'. Supported: task:<id>=<status>, timer:<dur>, human-input, message, file:<path>",
        s
    );
}

/// Parse a composite condition string into a WaitSpec.
///
/// Comma-separated = AND (All), pipe-separated = OR (Any).
/// Cannot mix AND and OR in one expression.
fn parse_wait_spec(s: &str, graph: &worksgood::graph::WorkGraph) -> Result<WaitSpec> {
    let has_comma = s.contains(',');
    let has_pipe = s.contains('|');

    if has_comma && has_pipe {
        anyhow::bail!(
            "Cannot mix AND (,) and OR (|) in a single --until expression. \
             Use all commas or all pipes."
        );
    }

    if has_pipe {
        let conditions: Vec<WaitCondition> = s
            .split('|')
            .map(|part| parse_condition(part, graph))
            .collect::<Result<Vec<_>>>()?;
        Ok(WaitSpec::Any(conditions))
    } else if has_comma {
        let conditions: Vec<WaitCondition> = s
            .split(',')
            .map(|part| parse_condition(part, graph))
            .collect::<Result<Vec<_>>>()?;
        Ok(WaitSpec::All(conditions))
    } else {
        // Single condition — wrap as All with one element
        let condition = parse_condition(s, graph)?;
        Ok(WaitSpec::All(vec![condition]))
    }
}

/// Resolve the exact Pi session already authorized for this source attempt.
///
/// Parking is the handoff point after which a replacement may be dispatched,
/// so the resume selector must be persisted in the same graph transaction as
/// `AttemptParked`. The watchdog state alone is not a scheduling field, and an
/// ambient `PI_SESSION_ID` alone is not authority. Bind the two only when the
/// durable watchdog source/session/process proofs match the lifecycle kernel's
/// active continuation authorization exactly.
pub(crate) fn attested_pi_session_id(dir: &Path, task: &Task) -> Result<Option<String>> {
    let Some(authorization) = task.lifecycle.pi_continuation.as_ref() else {
        return Ok(None);
    };
    if authorization.state != PiAuthorizationState::Active {
        anyhow::bail!(
            "cannot park Pi attempt '{}': continuation authorization is {:?}, expected active",
            authorization.attempt_id,
            authorization.state
        );
    }
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("Pi-authorized task has no current attempt")?;
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let state_path = worksgood::attempt_runtime::component_for_update(dir, &runtime_key, "pi")?
        .join("state.json");
    let watchdog = PiWatchdog::open(&state_path).map_err(anyhow::Error::new)?;
    let state = watchdog.state();
    let source = &state.source;
    if source.task_id != task.id
        || source.generation != task.lifecycle.generation
        || source.attempt_id != attempt.id
        || source.attempt_fence != task.lifecycle.fence
        || source.worktree_lease_epoch != authorization.worktree_lease_epoch
        || authorization.task_id != task.id
        || authorization.generation != task.lifecycle.generation
        || authorization.attempt_id != attempt.id
        || authorization.attempt_fence != task.lifecycle.fence
    {
        anyhow::bail!(
            "cannot park Pi attempt '{}': watchdog/continuation source tuple is stale",
            attempt.id
        );
    }
    if state.session.digest() != authorization.session_proof_digest
        || state.route.digest() != authorization.route_snapshot_digest
        || state.process_epoch != task.lifecycle.pi_process_epoch
        || state.process.digest() != task.lifecycle.pi_process_identity_digest
        || state.terminal
        || !state.exact_guards.session
        || !state.exact_guards.route
        || !state.exact_guards.worktree
        || !state.exact_guards.pid_identity
        || !state.exact_guards.terminal_clear
    {
        anyhow::bail!(
            "cannot park Pi attempt '{}': exact session/process guards are not attested",
            attempt.id
        );
    }
    if std::env::var("WG_TASK_ID").as_deref() == Ok(task.id.as_str())
        && let Ok(environment_session) = std::env::var("PI_SESSION_ID")
        && environment_session != state.session.session_id
    {
        anyhow::bail!(
            "cannot park Pi attempt '{}': PI_SESSION_ID does not match the authorized session",
            attempt.id
        );
    }
    if let Some(existing) = task.session_id.as_deref()
        && existing != state.session.session_id
    {
        anyhow::bail!(
            "cannot park Pi attempt '{}': task session selector conflicts with the authorized session",
            attempt.id
        );
    }
    Ok(Some(state.session.session_id.clone()))
}

pub fn run(dir: &Path, id: &str, until: &str, checkpoint: Option<&str>) -> Result<()> {
    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    let mut error: Option<anyhow::Error> = None;
    let mut assigned_agent: Option<String> = None;

    modify_graph(&path, |graph| {
        let task = match graph.get_task(id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };

        // Validate task is InProgress
        if task.status != Status::InProgress {
            error = Some(anyhow::anyhow!(
                "Cannot wait on task '{}': status is '{}', expected 'in-progress'",
                id,
                task.status
            ));
            return false;
        }

        // Resolve a Pi continuation's exact session before changing lifecycle
        // revision. This is read-only attestation; persistence happens only
        // after the park transition succeeds below.
        let attested_session_id = match attested_pi_session_id(dir, task) {
            Ok(session_id) => session_id,
            Err(e) => {
                error = Some(e);
                return false;
            }
        };

        // Parse and validate the condition
        let wait_spec = match parse_wait_spec(until, graph) {
            Ok(ws) => ws,
            Err(e) => {
                error = Some(e);
                return false;
            }
        };

        let selector = message_selector(&wait_spec);

        // Now mutate
        let task = graph.get_task_mut(id).expect("task verified above");
        let bound_attempt = if selector.is_some() {
            match task.lifecycle.current_attempt.as_ref() {
                Some(attempt) if attempt.disposition.is_none() => Some(attempt.clone()),
                _ => {
                    error = Some(anyhow::anyhow!(
                        "Cannot arm message wait on '{}': no current live attempt; retry/reclaim first",
                        id
                    ));
                    return false;
                }
            }
        } else {
            None
        };

        let actor_id = if task.lifecycle.current_attempt.is_some() {
            (std::env::var("WG_TASK_ID").as_deref() == Ok(id))
                .then(|| std::env::var("WG_AGENT_ID").ok())
                .flatten()
                .or_else(|| task.assigned.clone())
                .unwrap_or_else(worksgood::current_user)
        } else {
            task.assigned
                .clone()
                .unwrap_or_else(worksgood::current_user)
        };
        let generation = task.lifecycle.generation;
        let request = TransitionRequest::new(
            TransitionKind::AttemptParked,
            if task.assigned.is_some() {
                LifecycleActor::worker(actor_id)
            } else {
                LifecycleActor::operator(actor_id)
            },
            "explicit_wait",
            format!("wait:{id}:{generation}:{until}"),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(rejection) = apply_transition(task, request) {
            error = Some(anyhow::anyhow!(rejection));
            return false;
        }
        if let Some(session_id) = attested_session_id {
            task.session_id = Some(session_id);
        }
        task.wait_condition = Some(wait_spec);
        task.message_wait = bound_attempt.zip(selector).map(|(attempt, selector)| {
            MessageWaitSubscription {
                id: format!("message-wait:{id}:{}:{}", attempt.generation, attempt.id),
                attempt_epoch: attempt.generation,
                attempt_id: attempt.id,
                selector,
                armed: true,
                consumed_by_message_id: None,
                resume_request_id: None,
            }
        });

        if let Some(cp) = checkpoint {
            task.checkpoint = Some(cp.to_string());
        }

        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: task.assigned.clone(),
            user: Some(worksgood::current_user()),
            message: format!("Agent parked. Waiting for: {}", until),
        });

        assigned_agent = task.assigned.clone();

        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }

    // Update agent status to Parked if there's an assigned agent
    if let Some(ref assigned) = assigned_agent
        && let Ok(mut registry) = AgentRegistry::load_locked(dir)
    {
        if let Some(agent) = registry.registry.get_agent_mut(assigned) {
            agent.status = AgentStatus::Parked;
            agent.completed_at = Some(Utc::now().to_rfc3339());
        }
        for agent in registry.registry.agents.values_mut() {
            if agent.task_id == id && agent.is_alive() {
                agent.status = AgentStatus::Parked;
                if agent.completed_at.is_none() {
                    agent.completed_at = Some(Utc::now().to_rfc3339());
                }
            }
        }
        let _ = registry.save();
    }
    let lease_owner = worksgood::disk_sentinel::caller_agent_for_task(id);
    if let Err(error) =
        worksgood::disk_sentinel::release_owned_cache_leases(dir, id, lease_owner.as_deref())
    {
        eprintln!("Warning: failed to release build-cache lease: {error:#}");
    }

    super::notify_graph_changed(dir);

    println!("Parked task '{}'. Condition: {}", id, until);
    println!("Checkpoint saved. You should now exit cleanly.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{Status, WaitCondition, WaitSpec};
    use worksgood::lifecycle::{AttemptRef, PiContinuationAuthorization};
    use worksgood::parser::{load_graph, modify_graph};
    use worksgood::pi_watchdog::{
        ProcessIdentity, QosClass, RouteSnapshot, SessionProof, SourceTuple, WatchdogPolicy,
    };
    use worksgood::test_helpers::{make_task_with_status as make_task, setup_workgraph};

    fn running_task() -> worksgood::graph::Task {
        let mut task = make_task("main", "Main", Status::InProgress);
        task.lifecycle.fence = 1;
        task.lifecycle.attempt_sequence = 1;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-0-1".to_string(),
            generation: 0,
            fence: 1,
            actor_id: "agent-1".to_string(),
            disposition: None,
        });
        task
    }

    fn graph_path(dir: &Path) -> std::path::PathBuf {
        dir.join("graph.jsonl")
    }

    #[test]
    fn test_wg_wait_basic_task_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let dep = make_task("dep-a", "Dependency A", Status::Open);
        let mut main_task = make_task("main", "Main task", Status::InProgress);
        main_task.assigned =
            Some(std::env::var("WG_AGENT_ID").unwrap_or_else(|_| "agent-1".to_string()));

        setup_workgraph(dir_path, vec![dep, main_task]);

        let result = run(
            dir_path,
            "main",
            "task:dep-a=done",
            Some("Phase 1 complete"),
        );
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        assert_eq!(task.status, Status::Waiting);
        assert!(task.wait_condition.is_some());
        assert_eq!(task.checkpoint.as_deref(), Some("Phase 1 complete"));

        // Check wait condition contents
        if let Some(WaitSpec::All(conditions)) = &task.wait_condition {
            assert_eq!(conditions.len(), 1);
            match &conditions[0] {
                WaitCondition::TaskStatus { task_id, status } => {
                    assert_eq!(task_id, "dep-a");
                    assert_eq!(*status, Status::Done);
                }
                _ => panic!("Expected TaskStatus condition"),
            }
        } else {
            panic!("Expected WaitSpec::All");
        }
    }

    #[test]
    fn test_wg_wait_rejects_non_in_progress() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(dir_path, vec![make_task("t1", "Test", Status::Open)]);

        let result = run(dir_path, "t1", "message", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("in-progress"));
    }

    #[test]
    fn test_wg_wait_rejects_nonexistent_task_in_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(
            dir_path,
            vec![make_task("main", "Main", Status::InProgress)],
        );

        let result = run(dir_path, "main", "task:nonexistent=done", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_wg_wait_timer_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(
            dir_path,
            vec![make_task("main", "Main", Status::InProgress)],
        );

        let result = run(dir_path, "main", "timer:5m", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        assert_eq!(task.status, Status::Waiting);
        if let Some(WaitSpec::All(conditions)) = &task.wait_condition {
            match &conditions[0] {
                WaitCondition::Timer { resume_after } => {
                    // Should be parseable as RFC3339
                    assert!(resume_after.parse::<chrono::DateTime<Utc>>().is_ok());
                }
                _ => panic!("Expected Timer condition"),
            }
        } else {
            panic!("Expected WaitSpec::All");
        }
    }

    #[test]
    fn test_wg_wait_message_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(dir_path, vec![running_task()]);

        let result = run(dir_path, "main", "message", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Waiting);
        let subscription = task.message_wait.as_ref().unwrap();
        assert!(subscription.armed);
        assert_eq!(subscription.attempt_id, "attempt-0-1");
        assert_eq!(subscription.selector, MessageWaitSelector::AnyMessage);
    }

    #[test]
    fn test_wg_wait_persists_only_attested_pi_session_for_resume() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![running_task()]);

        let graph = load_graph(graph_path(dir_path)).unwrap();
        let task = graph.get_task("main").unwrap();
        let attempt = task.lifecycle.current_attempt.as_ref().unwrap();
        let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
        let pi_dir =
            worksgood::attempt_runtime::component_for_update(dir_path, &runtime_key, "pi").unwrap();
        let session_dir = pi_dir.join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        let session_file = session_dir.join("attested-session.jsonl");
        std::fs::write(
            &session_file,
            "{\"type\":\"session\",\"version\":3,\"id\":\"attested-session\"}\n",
        )
        .unwrap();
        let source = SourceTuple {
            task_id: "main".into(),
            generation: 0,
            attempt_id: attempt.id.clone(),
            attempt_fence: 1,
            worktree_lease_epoch: 1,
            worktree_path: dir_path.join("worktree"),
        };
        let route = RouteSnapshot {
            handler: "pi".into(),
            provider: "fake".into(),
            model: "fake-model".into(),
            reasoning: Some("high".into()),
            endpoint_redacted: "pi-owned".into(),
            endpoint_hmac: "fixture-endpoint".into(),
            qos: QosClass::Free,
            pi_binary_digest: "fixture-pi".into(),
            plugin_digest: "fixture-plugin".into(),
        };
        let session = SessionProof {
            session_id: "attested-session".into(),
            branch_leaf: "b3:leaf".into(),
            session_dir,
            session_file,
            header_digest: "b3:header".into(),
            append_prefix_digest: "b3:prefix".into(),
            append_prefix_len: 1,
        };
        let process = ProcessIdentity {
            pid: std::process::id(),
            pgid: std::process::id(),
            start_ticks: 1,
            boot_id: "fixture-boot".into(),
            nonce: "fixture-nonce".into(),
        };
        let process_digest = process.digest();
        let state_path = pi_dir.join("state.json");
        PiWatchdog::new_at(
            state_path.clone(),
            source.clone(),
            route.clone(),
            session.clone(),
            process,
            WatchdogPolicy::default(),
            Utc::now().timestamp(),
        )
        .unwrap();
        let authorization = PiContinuationAuthorization {
            authorization_id: "fixture-auth".into(),
            task_id: "main".into(),
            generation: 0,
            attempt_id: attempt.id.clone(),
            attempt_fence: 1,
            worktree_lease_epoch: 1,
            session_proof_digest: session.digest(),
            route_snapshot_digest: route.digest(),
            state: PiAuthorizationState::Active,
            max_replacement_epochs: 3,
            max_reserved_elapsed_secs: 1800,
            epochs_used: 0,
            elapsed_reserved_secs: 0,
            issued_by_policy: "pi-watchdog-static-v1".into(),
        };
        modify_graph(&graph_path(dir_path), |graph| {
            let task = graph.get_task_mut("main").unwrap();
            task.lifecycle.pi_process_epoch = 1;
            task.lifecycle.pi_process_identity_digest = process_digest.clone();
            task.lifecycle.pi_continuation = Some(authorization.clone());
            true
        })
        .unwrap();

        // A present session ID is not enough: corrupting one exact guard must
        // fail closed without parking or persisting a resume selector.
        let mut persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        persisted["state"]["exact_guards"]["session"] = serde_json::json!(false);
        std::fs::write(&state_path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();
        let rejected = run(dir_path, "main", "message", Some("unattested park")).unwrap_err();
        assert!(
            rejected.to_string().contains("not attested"),
            "{rejected:#}"
        );
        let graph = load_graph(graph_path(dir_path)).unwrap();
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task.session_id.is_none());

        persisted["state"]["exact_guards"]["session"] = serde_json::json!(true);
        std::fs::write(&state_path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();
        run(dir_path, "main", "message", Some("attested park")).unwrap();
        let graph = load_graph(graph_path(dir_path)).unwrap();
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.session_id.as_deref(), Some("attested-session"));
        assert_eq!(task.checkpoint.as_deref(), Some("attested park"));
    }

    #[test]
    fn test_wg_wait_human_input_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(dir_path, vec![running_task()]);

        let result = run(dir_path, "main", "human-input", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();
        assert_eq!(task.status, Status::Waiting);
    }

    #[test]
    fn test_wg_wait_and_conditions() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let dep_a = make_task("dep-a", "Dep A", Status::Open);
        let dep_b = make_task("dep-b", "Dep B", Status::Open);
        let main = make_task("main", "Main", Status::InProgress);

        setup_workgraph(dir_path, vec![dep_a, dep_b, main]);

        let result = run(dir_path, "main", "task:dep-a=done,task:dep-b=done", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        if let Some(WaitSpec::All(conditions)) = &task.wait_condition {
            assert_eq!(conditions.len(), 2);
        } else {
            panic!("Expected WaitSpec::All with 2 conditions");
        }
    }

    #[test]
    fn test_wg_wait_or_conditions() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let dep = make_task("dep-a", "Dep A", Status::Open);
        let main = make_task("main", "Main", Status::InProgress);

        setup_workgraph(dir_path, vec![dep, main]);

        let result = run(dir_path, "main", "task:dep-a=done|timer:5m", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        if let Some(WaitSpec::Any(conditions)) = &task.wait_condition {
            assert_eq!(conditions.len(), 2);
        } else {
            panic!("Expected WaitSpec::Any with 2 conditions");
        }
    }

    #[test]
    fn test_wg_wait_mixed_and_or_rejected() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let dep_a = make_task("dep-a", "Dep A", Status::Open);
        let main = make_task("main", "Main", Status::InProgress);

        setup_workgraph(dir_path, vec![dep_a, main]);

        let result = run(dir_path, "main", "task:dep-a=done,timer:5m|message", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot mix"));
    }

    #[test]
    fn test_wg_wait_invalid_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(
            dir_path,
            vec![make_task("main", "Main", Status::InProgress)],
        );

        let result = run(dir_path, "main", "invalid-condition", None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown condition")
        );
    }

    #[test]
    fn test_wg_wait_creates_log_entry() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(dir_path, vec![running_task()]);

        let result = run(dir_path, "main", "message", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        let last_log = task.log.last().unwrap();
        assert!(last_log.message.contains("Agent parked"));
        assert!(last_log.message.contains("message"));
    }

    #[test]
    fn test_wg_wait_file_condition() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // Create a file to watch
        let watch_file = dir.path().join("watched.txt");
        std::fs::write(&watch_file, "initial").unwrap();

        setup_workgraph(
            dir_path,
            vec![make_task("main", "Main", Status::InProgress)],
        );

        let result = run(
            dir_path,
            "main",
            &format!("file:{}", watch_file.display()),
            None,
        );
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        if let Some(WaitSpec::All(conditions)) = &task.wait_condition {
            match &conditions[0] {
                WaitCondition::FileChanged {
                    path,
                    mtime_at_wait,
                } => {
                    assert!(path.contains("watched.txt"));
                    assert!(*mtime_at_wait > 0);
                }
                _ => panic!("Expected FileChanged condition"),
            }
        } else {
            panic!("Expected WaitSpec::All");
        }
    }

    #[test]
    fn test_wg_wait_without_checkpoint() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        setup_workgraph(dir_path, vec![running_task()]);

        let result = run(dir_path, "main", "message", None);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("main").unwrap();

        assert_eq!(task.status, Status::Waiting);
        assert!(task.checkpoint.is_none());
    }
}
