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
    // A task-owned `wg done` returns only after an accepted promotion/output
    // receipt is durable. From that boundary onward wrapper/provider exit is
    // diagnostic evidence, not a competing terminal writer. This check is
    // exact-source fenced and intentionally does not cover legacy status-only
    // Done rows or semantic evaluation rejection.
    if !eval_reject && contain_late_failure_after_durable_success(dir, id, reason, class)? {
        println!(
            "Task '{}' already has exact durable successful finalization; retained late process failure as diagnostic only",
            id
        );
        return Ok(());
    }

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

    // Pi terminal tools reserve intent while the source handler can still
    // write. The post-wait wrapper calls `wg finalize settle`, which re-enters
    // here with WG_HANDLER_QUIESCENT=1 and rescue-checkpoints before failure.
    let in_isolated_worktree = std::env::var_os("WG_WORKTREE_PATH").is_some()
        && std::env::var("WG_TASK_ID").as_deref() == Ok(id);
    let handler_quiescent = std::env::var("WG_HANDLER_QUIESCENT").as_deref() == Ok("1");
    if !eval_reject
        && in_isolated_worktree
        && !handler_quiescent
        && std::env::var("WG_EXECUTOR_TYPE").as_deref() == Ok("pi")
    {
        let tool_call = format!(
            "wg-fail:{}",
            std::env::var("WG_SPAWN_RUN_ID").unwrap_or_else(|_| id.to_string())
        );
        super::pi_watchdog::reserve_worker_terminal(
            dir,
            id,
            worksgood::pi_watchdog::TerminalDisposition::Failure,
            &tool_call,
        )?;
        println!(
            "Failure intent reserved for '{}'; exact writer will be fenced and WIP rescued after exit",
            id
        );
        return Ok(());
    }

    let mut finalization_rescue_id: Option<String> = None;
    if !eval_reject && in_isolated_worktree && handler_quiescent {
        let context = super::finalize::context_from_current(dir, id, None, None, false)?;
        let store = worksgood::finalization::FinalizationStore::open(dir)?;
        let retained = worksgood::finalization::checkpoint_rescue(&store, &context, false)?;
        finalization_rescue_id = retained.rescue.as_ref().map(|r| r.rescue_id.clone());
        eprintln!(
            "[finalize] failure rescue={} commit={} tree={} manifest={} retained (no candidate correctness claim)",
            retained
                .rescue
                .as_ref()
                .map(|r| r.rescue_id.as_str())
                .unwrap_or("none"),
            retained
                .rescue
                .as_ref()
                .map(|r| r.rescue_commit_oid.as_str())
                .unwrap_or("none"),
            retained
                .rescue
                .as_ref()
                .map(|r| r.rescue_tree_oid.as_str())
                .unwrap_or("none"),
            retained
                .rescue
                .as_ref()
                .map(|r| r.manifest_cid.as_str())
                .unwrap_or("none"),
        );
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
        if let Some(ref rescue_id) = finalization_rescue_id {
            request.evidence_refs.push(rescue_id.clone());
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

fn contain_late_failure_after_durable_success(
    dir: &Path,
    id: &str,
    reason: Option<&str>,
    class: Option<FailureClass>,
) -> Result<bool> {
    let store = worksgood::finalization::FinalizationStore::open(dir)?;
    let Some(tx) = store.load_task(id)? else {
        return Ok(false);
    };
    let presented_agent = (std::env::var("WG_TASK_ID").as_deref() == Ok(id))
        .then(|| std::env::var("WG_AGENT_ID").ok())
        .flatten();
    if presented_agent.as_deref().is_some_and(|agent| {
        tx.candidate
            .as_ref()
            .is_none_or(|candidate| candidate.worktree_id != agent)
    }) {
        // Do not let an old wrapper borrow the current attempt's transaction.
        // The ordinary lifecycle path below will reject its stale actor/fence.
        return Ok(false);
    }

    let mut contained = false;
    let class_text = class
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unclassified-process-exit".into());
    let reason_text = reason.unwrap_or("late worker/process exit").to_string();
    let diagnostic_key = format!(
        "late-process-diagnostic:{}:{}:{}:{}",
        tx.generation, tx.attempt_id, tx.attempt_fence, class_text
    );
    modify_graph(super::graph_path(dir), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let Some(evidence) = tx.exact_durable_success(
            &task.id,
            task.lifecycle.generation,
            task.lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str()),
            task.lifecycle.fence,
        ) else {
            return false;
        };
        contained = true;
        if task.log.iter().any(|entry| {
            entry.actor.as_deref() == Some("late-process-diagnostic")
                && entry.message.contains(&diagnostic_key)
        }) {
            return false;
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("late-process-diagnostic".into()),
            user: Some(worksgood::current_user()),
            message: format!(
                "{diagnostic_key} observed after durable {} receipt {}; lifecycle authority suppressed: {}",
                evidence.disposition, evidence.durable_receipt_id, reason_text
            ),
        });
        true
    })?;
    Ok(contained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::finalization::{
        CandidateBinding, CandidateDescriptor, CleanupReceipt, EvaluationReceipt,
        EvaluationReceiptOutcome, FinalizationPhase, FinalizationTransaction, MergeReceipt,
        OutputDisposition, OutputReceipt, QuiescenceProof, ValidationResult,
    };
    use worksgood::test_helpers::{make_task_with_status as make_task, setup_workgraph};

    fn running_task(id: &str, agent: &str) -> worksgood::graph::Task {
        let mut task = make_task(id, id, Status::Open);
        let request = TransitionRequest::new(
            TransitionKind::AttemptReserved {
                owner_id: Some(agent.into()),
            },
            LifecycleActor {
                kind: ActorKind::Dispatcher,
                id: "fixture-dispatcher".into(),
            },
            "fixture_reserve",
            format!("fixture-reserve:{id}"),
        );
        apply_transition(&mut task, request).unwrap();
        task.assigned = Some(agent.into());
        task
    }

    fn incident_transaction(
        task: &worksgood::graph::Task,
        land: bool,
        cleaned: bool,
    ) -> FinalizationTransaction {
        let attempt = task.lifecycle.current_attempt.as_ref().unwrap();
        let commit = if land { "347a1696" } else { "c433cb68" };
        let binding = CandidateBinding {
            candidate_id: format!("candidate:{commit}"),
            commit_oid: commit.into(),
            tree_oid: format!("tree:{commit}"),
            manifest_cid: format!("manifest:{commit}"),
            delta_manifest_cid: "delta".into(),
        };
        let candidate = CandidateDescriptor {
            schema_version: 1,
            candidate_id: binding.candidate_id.clone(),
            candidate_version: 1,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: attempt.id.clone(),
            attempt_fence: task.lifecycle.fence,
            process_epoch: 1,
            terminal_reservation_id: format!("terminal:{}", task.id),
            quiescence_receipt_cid: format!("quiescence:{}", task.id),
            rescue_id: format!("rescue:{}", task.id),
            worktree_id: attempt.actor_id.clone(),
            worktree_lease_epoch: task.lifecycle.fence,
            base_commit_oid: "base".into(),
            base_tree_oid: "base-tree".into(),
            // The broker incident retained later smoke commits through
            // ccf51d90 after its first durable c433cb68 output.
            worker_head_oid: if land {
                commit.into()
            } else {
                "ccf51d90".into()
            },
            candidate_commit_oid: commit.into(),
            candidate_tree_oid: binding.tree_oid.clone(),
            content_manifest_cid: binding.manifest_cid.clone(),
            delta_manifest_cid: "delta".into(),
            validation_policy_cid: "validation-policy".into(),
            evaluation_policy: "none".into(),
            merge_policy_cid: "merge-policy".into(),
            route_snapshot_cid: "route".into(),
            immutable_ref: format!("refs/wg/fixtures/{}", task.id),
            created_at: "2026-08-03T00:00:00Z".into(),
            binding: binding.clone(),
        };
        let validation = ValidationResult {
            result_id: "validation".into(),
            request_id: "validation-request".into(),
            binding: binding.clone(),
            policy_cid: "validation-policy".into(),
            materialized_tree_oid: binding.tree_oid.clone(),
            materialized_manifest_cid: binding.manifest_cid.clone(),
            passed: true,
            validator_identity: "fixture".into(),
            created_at: "2026-08-03T00:00:01Z".into(),
        };
        let merge_receipt = land.then(|| MergeReceipt {
            receipt_id: "merge:347a1696".into(),
            action_id: "merge-action".into(),
            binding: binding.clone(),
            base_commit_oid: "base".into(),
            expected_target_commit_oid: "base".into(),
            expected_target_tree_oid: "base-tree".into(),
            integration_commit_oid: "347a1696".into(),
            result_tree_oid: binding.tree_oid.clone(),
            result_manifest_cid: binding.manifest_cid.clone(),
            candidate_projection_digest: "delta".into(),
            target_ref: "refs/heads/main".into(),
            ref_cas: true,
            created_at: "2026-08-03T00:00:02Z".into(),
        });
        let output_receipt = (!land).then(|| OutputReceipt {
            receipt_id: "output:c433cb68".into(),
            task_id: task.id.clone(),
            disposition: OutputDisposition::Reported,
            binding: binding.clone(),
            immutable_ref: format!("refs/wg/reports/{}/v1", task.id),
            created_at: "2026-08-03T00:00:02Z".into(),
        });
        let durable_receipt_id = if land {
            "merge:347a1696"
        } else {
            "output:c433cb68"
        };
        let cleanup_receipt = cleaned.then(|| CleanupReceipt {
            receipt_id: format!("cleanup:{commit}"),
            task_id: task.id.clone(),
            disposition: if land { "landed" } else { "reported" }.into(),
            durable_receipt_id: durable_receipt_id.into(),
            worktree_id: attempt.actor_id.clone(),
            worktree_path: std::path::PathBuf::from(format!("/retained/{}", task.id)),
            branch: format!("wg/source/{}", task.id),
            removed: true,
            created_at: "2026-08-03T00:00:03Z".into(),
        });
        FinalizationTransaction {
            schema_version: 1,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: attempt.id.clone(),
            attempt_fence: task.lifecycle.fence,
            worktree_lease_epoch: task.lifecycle.fence,
            worktree_path: std::path::PathBuf::from(format!("/retained/{}", task.id)),
            project_root: std::path::PathBuf::from("/project"),
            phase: if cleaned {
                FinalizationPhase::Cleaned
            } else if land {
                FinalizationPhase::Promoted
            } else {
                FinalizationPhase::Reported
            },
            terminal_reservation_id: format!("terminal:{}", task.id),
            quiescence: QuiescenceProof {
                receipt_cid: format!("quiescence:{}", task.id),
                process_identity_digest: "process".into(),
                process_group_empty: true,
                nonce_pipe_eof: true,
                observed_manifest_digest: None,
            },
            rescue: None,
            candidate: Some(candidate),
            validation: Some(validation),
            evaluation_request: None,
            finish_lease_id: land.then(|| "lease".into()),
            evaluation_receipt: land.then(|| EvaluationReceipt {
                receipt_id: "evaluation:accepted".into(),
                binding,
                outcome: EvaluationReceiptOutcome::Accepted,
                evidence_id: "evaluation-not-required".into(),
                evaluator_identity: "fixture".into(),
                created_at: "2026-08-03T00:00:02Z".into(),
            }),
            output_receipt,
            cleanup_receipt,
            merge_receipt,
            merge_conflict: None,
            retained_reason: None,
            replay_action: None,
            safe_next_command: format!("wg show {}", task.id),
            updated_at: "2026-08-03T00:00:03Z".into(),
        }
    }

    fn write_transaction(dir: &Path, tx: &FinalizationTransaction) {
        let store = worksgood::finalization::FinalizationStore::open(dir).unwrap();
        let path = store
            .root()
            .join("transactions")
            .join(format!("{}.json", tx.task_id));
        std::fs::write(path, serde_json::to_vec_pretty(tx).unwrap()).unwrap();
    }

    #[test]
    fn formal_incident_durable_land_then_provider_timeout_stays_successful() {
        let dir = tempdir().unwrap();
        let task = running_task("formalize-lifecycle-finish-lean4", "agent-formal");
        let tx = incident_transaction(&task, true, false);
        setup_workgraph(dir.path(), vec![task]);
        write_transaction(dir.path(), &tx);

        run(
            dir.path(),
            "formalize-lifecycle-finish-lean4",
            Some("provider timeout after main 347a1696"),
            Some(FailureClass::AgentExitNonzero),
        )
        .unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("formalize-lifecycle-finish-lean4").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.retry_count, 0);
        assert!(
            !task
                .lifecycle
                .audit
                .iter()
                .any(|event| event.event_kind == "attempt-failed")
        );
        assert!(task.log.iter().any(|entry| {
            entry.actor.as_deref() == Some("late-process-diagnostic")
                && entry.message.contains("provider timeout")
                && entry.message.contains("347a1696")
        }));
    }

    #[test]
    fn broker_incident_cleaned_then_provider_unavailable_converges_success() {
        let dir = tempdir().unwrap();
        let task = running_task(
            "fix-brokered-deliverable-preflight-worktree",
            "agent-broker",
        );
        setup_workgraph(dir.path(), vec![task.clone()]);
        // Model the graph side of the incident winning first, without
        // inheriting this test runner's real Pi wrapper environment.
        modify_graph(graph_path(dir.path()), |graph| {
            let task = graph.get_task_mut(&task.id).unwrap();
            let mut request = TransitionRequest::new(
                TransitionKind::AttemptFailed {
                    class: Some(FailureClass::AgentExitNonzero),
                },
                LifecycleActor::worker("agent-broker"),
                "source_execution_failed",
                "incident-provider-unavailable",
            );
            request.expected = FenceExpectation::current(task);
            apply_transition(task, request).unwrap();
            task.retry_count = 1;
            task.failure_class = Some(FailureClass::AgentExitNonzero);
            task.failure_reason = Some("provider-unavailable after wg done succeeded".into());
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("agent-broker".into()),
                user: Some("fixture".into()),
                message: "Task marked as failed: provider-unavailable after wg done succeeded"
                    .into(),
            });
            true
        })
        .unwrap();
        let cleaned = incident_transaction(&task, false, true);
        write_transaction(dir.path(), &cleaned);

        let store = worksgood::finalization::FinalizationStore::open(dir.path()).unwrap();
        super::super::finalize::cleanup_finish(dir.path(), &store, &task.id, false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task(&task.id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(
            task.completion_disposition,
            Some(worksgood::graph::CompletionDisposition::Reported)
        );
        assert_eq!(task.completion_receipt.as_deref(), Some("cleanup:c433cb68"));
        assert_eq!(task.retry_count, 0);
        assert!(task.lifecycle.audit.iter().any(|event| {
            event.event_kind == "durable-success-projected"
                && event
                    .evidence_refs
                    .iter()
                    .any(|value| value == "output:c433cb68")
        }));
        assert!(task.log.iter().any(|entry| {
            entry.message.contains("provider-unavailable")
                && entry.message.contains("without lifecycle authority")
        }));
    }

    #[test]
    fn durable_success_evidence_rejects_each_stale_source_coordinate() {
        let task = running_task("tuple-fence", "agent-tuple");
        let tx = incident_transaction(&task, false, true);
        let attempt = task.lifecycle.current_attempt.as_ref().unwrap();
        assert!(
            tx.exact_durable_success(
                &task.id,
                task.lifecycle.generation,
                Some(&attempt.id),
                task.lifecycle.fence,
            )
            .is_some()
        );
        assert!(
            tx.exact_durable_success(
                &task.id,
                task.lifecycle.generation + 1,
                Some(&attempt.id),
                task.lifecycle.fence,
            )
            .is_none()
        );
        assert!(
            tx.exact_durable_success(
                &task.id,
                task.lifecycle.generation,
                Some("attempt-newer"),
                task.lifecycle.fence,
            )
            .is_none()
        );
        assert!(
            tx.exact_durable_success(
                &task.id,
                task.lifecycle.generation,
                Some(&attempt.id),
                task.lifecycle.fence + 1,
            )
            .is_none()
        );
    }

    #[test]
    fn cleaned_transaction_does_not_upgrade_unrelated_legacy_done_row() {
        let dir = tempdir().unwrap();
        let running = running_task("legacy-done", "agent-old");
        let cleaned = incident_transaction(&running, false, true);
        let legacy = make_task("legacy-done", "legacy", Status::Done);
        setup_workgraph(dir.path(), vec![legacy]);
        write_transaction(dir.path(), &cleaned);
        let store = worksgood::finalization::FinalizationStore::open(dir.path()).unwrap();

        super::super::finalize::cleanup_finish(dir.path(), &store, "legacy-done", false).unwrap();

        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("legacy-done").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.completion_disposition, None);
        assert_eq!(task.completion_receipt, None);
        assert!(
            !task
                .lifecycle
                .audit
                .iter()
                .any(|event| event.event_kind == "durable-success-projected")
        );
    }

    #[test]
    fn stale_durable_transaction_cannot_bless_newer_attempt() {
        let dir = tempdir().unwrap();
        let mut task = running_task("stale-finalization", "agent-old");
        let stale = incident_transaction(&task, false, true);
        apply_transition(
            &mut task,
            TransitionRequest::new(
                TransitionKind::GenerationCreated,
                LifecycleActor::operator("fixture"),
                "fixture_retry",
                "fixture-retry:stale-finalization",
            ),
        )
        .unwrap();
        apply_transition(
            &mut task,
            TransitionRequest::new(
                TransitionKind::AttemptReserved {
                    owner_id: Some("agent-new".into()),
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "fixture-dispatcher".into(),
                },
                "fixture_reserve_new",
                "fixture-reserve-new:stale-finalization",
            ),
        )
        .unwrap();
        task.assigned = Some("agent-new".into());
        setup_workgraph(dir.path(), vec![task]);
        write_transaction(dir.path(), &stale);

        assert!(
            !contain_late_failure_after_durable_success(
                dir.path(),
                "stale-finalization",
                Some("new attempt genuine process failure"),
                Some(FailureClass::AgentExitNonzero),
            )
            .unwrap()
        );
        modify_graph(graph_path(dir.path()), |graph| {
            let task = graph.get_task_mut("stale-finalization").unwrap();
            let mut request = TransitionRequest::new(
                TransitionKind::AttemptFailed {
                    class: Some(FailureClass::AgentExitNonzero),
                },
                LifecycleActor::worker("agent-new"),
                "source_execution_failed",
                "new-attempt-genuine-failure",
            );
            request.expected = FenceExpectation::current(task);
            apply_transition(task, request).unwrap();
            true
        })
        .unwrap();
        let graph = load_graph(graph_path(dir.path())).unwrap();
        let task = graph.get_task("stale-finalization").unwrap();
        assert_eq!(task.status, Status::Failed);
        assert!(
            task.lifecycle
                .audit
                .iter()
                .any(|event| event.event_kind == "attempt-failed")
        );
    }

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
