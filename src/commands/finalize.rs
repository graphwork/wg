use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use worksgood::finalization::{
    FinalizationContext, FinalizationStore, QuiescenceProof, checkpoint_candidate,
    checkpoint_rescue,
};
use worksgood::parser::load_graph;

use crate::cli::{CandidateCommands, FinalizeCommands};

pub fn run_finalize(dir: &Path, command: FinalizeCommands, json: bool) -> Result<()> {
    let store = FinalizationStore::open(dir)?;
    match command {
        FinalizeCommands::Begin { id, ttl_seconds } => {
            let lease = begin_finish(dir, &store, &id, ttl_seconds)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&lease)?);
            } else {
                println!(
                    "Finish lease {} task={} base={} tree={} expires={}",
                    lease.lease_id,
                    lease.task_id,
                    lease.base_commit_oid,
                    lease.base_tree_oid,
                    lease.expires_at
                );
            }
            Ok(())
        }
        FinalizeCommands::Submit {
            id,
            lease,
            commit,
            wait_seconds,
        } => {
            let tx = submit_finish(
                dir,
                &store,
                &id,
                lease.as_deref(),
                commit.as_deref(),
                wait_seconds,
            )?;
            print_tx(&tx, json)
        }
        FinalizeCommands::Cleanup { id } => cleanup_finish(dir, &store, &id, json),
        FinalizeCommands::Contract { id, contract } => set_contract(dir, &id, &contract),
        FinalizeCommands::Input { id, from_task } => add_input_dependency(dir, &id, &from_task),
        FinalizeCommands::Status { id } => show_status(&store, &id, json),
        FinalizeCommands::Checkpoint {
            id,
            worktree,
            quiescence_receipt,
            failure,
        } => {
            let ctx = context_from_current(dir, &id, worktree, quiescence_receipt, true)?;
            let tx = if failure {
                checkpoint_rescue(&store, &ctx, false)?
            } else {
                checkpoint_candidate(&store, &ctx)?
            };
            print_tx(&tx, json)
        }
        FinalizeCommands::Reconcile { id, dry_run } => {
            let Some(tx) = store.load_task(&id)? else {
                bail!("no finalization transaction for '{id}'")
            };
            if dry_run {
                println!(
                    "replay={} next={}",
                    tx.replay_action.as_deref().unwrap_or("none"),
                    tx.safe_next_command
                );
                return Ok(());
            }
            let tx = worksgood::finalization::reconcile(&store, &id)?.unwrap_or(tx);
            print_tx(&tx, json)
        }
        FinalizeCommands::Settle { id } => settle(dir, &id),
        FinalizeCommands::Preserve { id, reason } => {
            if reason.trim().is_empty() {
                bail!("preserve reason must not be empty");
            }
            let tx = store
                .load_task(&id)?
                .context("finalization transaction missing")?;
            let path = store.root().join("preserved");
            std::fs::create_dir_all(&path)?;
            worksgood::atomic_file::write_atomic(
                &path.join(format!("{}.txt", safe(&id))),
                reason.as_bytes(),
            )?;
            println!(
                "Preserved {} candidate={} rescue={} reason={}",
                id,
                tx.candidate
                    .as_ref()
                    .map(|c| c.candidate_id.as_str())
                    .unwrap_or("none"),
                tx.rescue
                    .as_ref()
                    .map(|r| r.rescue_id.as_str())
                    .unwrap_or("none"),
                reason
            );
            Ok(())
        }
        FinalizeCommands::Gc { dry_run } => {
            println!(
                "Candidate GC {}: 0 eligible; source-bearing, failed, rejected, conflicted, unmerged and unknown objects are retained",
                if dry_run { "dry-run" } else { "refused" }
            );
            Ok(())
        }
    }
}

pub fn run_candidate(dir: &Path, command: CandidateCommands, json: bool) -> Result<()> {
    let store = FinalizationStore::open(dir)?;
    match command {
        CandidateCommands::Show { id } => {
            let c = resolve_candidate(&store, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&c)?);
            } else {
                println!(
                    "Candidate {} v{}\n  source: {} generation={} attempt={} fence={} lease={}\n  commit: {}\n  tree: {}\n  manifest: {}\n  delta: {}\n  evaluation: {} route={}\n  ref: {}\n  verified: tree+manifest binding (path/branch equality is not proof)",
                    c.candidate_id,
                    c.candidate_version,
                    c.task_id,
                    c.generation,
                    c.attempt_id,
                    c.attempt_fence,
                    c.worktree_lease_epoch,
                    c.candidate_commit_oid,
                    c.candidate_tree_oid,
                    c.content_manifest_cid,
                    c.delta_manifest_cid,
                    c.evaluation_policy,
                    c.route_snapshot_cid,
                    c.immutable_ref
                );
            }
            Ok(())
        }
        CandidateCommands::Verify { id } => {
            let c = resolve_candidate(&store, &id)?;
            let root = dir.parent().unwrap_or(dir);
            let tree = git(
                root,
                &["rev-parse", &format!("{}^{{tree}}", c.candidate_commit_oid)],
            )?;
            if tree != c.candidate_tree_oid {
                bail!("candidate.binding_mismatch");
            }
            println!(
                "Verified candidate {} commit={} tree={} manifest={}",
                c.candidate_id,
                c.candidate_commit_oid,
                c.candidate_tree_oid,
                c.content_manifest_cid
            );
            Ok(())
        }
        CandidateCommands::Materialize { id, to } => {
            let c = resolve_candidate(&store, &id)?;
            store.materialize_commit(dir.parent().unwrap_or(dir), &c.candidate_commit_oid, &to)?;
            println!(
                "Materialized candidate {} commit={} tree={} manifest={} to {}",
                c.candidate_id,
                c.candidate_commit_oid,
                c.candidate_tree_oid,
                c.content_manifest_cid,
                to.display()
            );
            Ok(())
        }
        CandidateCommands::Repair { id, reuse_worktree } => {
            let c = resolve_candidate(&store, &id)?;
            println!(
                "Candidate {} is immutable. Start a lifecycle-authorized repair generation:\n  wg retry {} --reason 'repair candidate {}'{}\nNew bytes must produce candidate v{} and fresh validation/evaluation evidence.",
                c.candidate_id,
                c.task_id,
                c.candidate_id,
                if reuse_worktree {
                    " (retained worktree reuse requested; fence proof required)"
                } else {
                    ""
                },
                c.candidate_version + 1
            );
            Ok(())
        }
        CandidateCommands::Waive { id, report, reason } => {
            if std::env::var_os("WG_AGENT_ID").is_some() {
                bail!("candidate waiver is operator-only; workers cannot waive required FLIP");
            }
            if reason.trim().is_empty() {
                bail!("candidate waiver requires a non-empty operator reason");
            }
            let candidate = resolve_candidate(&store, &id)?;
            let actor = worksgood::current_user();
            let waiver_value = serde_json::json!({
                "schema": 1,
                "candidate": candidate.candidate_id,
                "report": report,
                "operator": actor,
                "reason": reason.trim(),
            });
            let waiver_id = worksgood::identity::content_cid(&waiver_value);
            let waiver_dir = store.root().join("waivers");
            std::fs::create_dir_all(&waiver_dir)?;
            worksgood::atomic_file::write_atomic(
                &waiver_dir.join(format!("{}.json", waiver_id.replace(':', "_"))),
                &serde_json::to_vec_pretty(&waiver_value)?,
            )?;
            let mut failure: Option<String> = None;
            worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
                let Some(task) = graph.get_task_mut(&candidate.task_id) else {
                    failure = Some("candidate source task missing".into());
                    return false;
                };
                let valid_rejection = task.evaluation_records.iter().any(|record| {
                    record.product == worksgood::evaluation::EvaluationProduct::DeepReadonlyFlip
                        && record.policy.applicability
                            == worksgood::eval_lifecycle::EvaluationGateApplicability::Required
                        && record.source.candidate_digest == candidate.candidate_id
                        && worksgood::evaluation::source_candidate_is_current(task, &record.source)
                        && record
                            .deep_report
                            .as_ref()
                            .is_some_and(|value| value.report_id == report)
                        && record.consumed_verdict_id.as_deref() == Some(report.as_str())
                        && record.deep_report.as_ref().is_some_and(|value| {
                            value.outcome == worksgood::evaluation::BoundedVerdictOutcome::Fail
                                || value.score < record.policy.threshold.unwrap_or(1.0)
                        })
                });
                if !valid_rejection || task.status != worksgood::graph::Status::PendingEval {
                    failure = Some(
                        "waiver requires the exact retained candidate and rejected consumed FLIP report in AwaitingAcceptance"
                            .into(),
                    );
                    return false;
                }
                let merged = match worksgood::finalization::merge_candidate(&store, &candidate) {
                    Ok(value) => value,
                    Err(error) => {
                        failure = Some(format!("waiver merge failed: {error:#}"));
                        return false;
                    }
                };
                let Some(receipt) = merged.merge_receipt.as_ref() else {
                    failure = Some(format!(
                        "waiver merge needs repair: {}",
                        merged.safe_next_command
                    ));
                    return false;
                };
                let request = worksgood::lifecycle::TransitionRequest::new(
                    worksgood::lifecycle::TransitionKind::AcceptanceSatisfied {
                        acceptance_ref: waiver_id.clone(),
                    },
                    worksgood::lifecycle::LifecycleActor::operator(actor.clone()),
                    "required_flip_operator_waiver",
                    format!("flip-waiver:{}:{}", candidate.task_id, waiver_id),
                )
                .with_evidence(candidate.candidate_id.clone())
                .with_evidence(report.clone())
                .with_evidence(waiver_id.clone())
                .with_evidence(receipt.receipt_id.clone());
                if let Err(error) = worksgood::lifecycle::apply_transition(task, request) {
                    failure = Some(format!("waiver acceptance CAS refused: {error}"));
                    return false;
                }
                task.log.push(worksgood::graph::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    actor: None,
                    user: Some(actor.clone()),
                    message: format!(
                        "AUDITED FLIP WAIVER {} candidate={} report={} reason_code=operator-supplied merge={}",
                        waiver_id, candidate.candidate_id, report, receipt.receipt_id
                    ),
                });
                true
            })?;
            if let Some(error) = failure {
                bail!("{error}");
            }
            println!(
                "Waived required FLIP: waiver={} candidate={} report={} operator={} (audited; exact candidate merged)",
                waiver_id, candidate.candidate_id, report, actor
            );
            Ok(())
        }
    }
}

fn finish_context(dir: &Path, id: &str) -> Result<FinalizationContext> {
    if std::env::var_os("WG_AGENT_ID").is_some() && std::env::var("WG_TASK_ID").as_deref() != Ok(id)
    {
        bail!("finish.source_owner_mismatch: a worker may finish only its own task");
    }
    context_from_current(
        dir,
        id,
        None,
        Some(format!(
            "finish-live:{}:{}",
            id,
            std::env::var("WG_SPAWN_RUN_ID").unwrap_or_else(|_| "operator".into())
        )),
        true,
    )
}

fn begin_finish(
    dir: &Path,
    store: &FinalizationStore,
    id: &str,
    ttl_seconds: i64,
) -> Result<worksgood::finalization::FinishLease> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    if task.completion_contract != worksgood::graph::CompletionContract::Land {
        bail!(
            "finish.contract_mismatch: {} is {}; only land tasks acquire the repository lease",
            id,
            task.completion_contract
        );
    }
    if task.status != worksgood::graph::Status::InProgress {
        bail!("finish.source_not_working: task status is {}", task.status);
    }
    let ctx = finish_context(dir, id)?;
    let lease = worksgood::finalization::acquire_finish_lease(store, &ctx, ttl_seconds)?;
    integrate_leased_base(&ctx.worktree_path, &lease.base_commit_oid)?;
    Ok(lease)
}

fn integrate_leased_base(worktree: &Path, base: &str) -> Result<()> {
    let status = git(worktree, &["status", "--porcelain"])?;
    let dirty: Vec<_> = status
        .lines()
        .filter(|line| {
            let path = line.get(3..).unwrap_or(line).trim();
            path != ".wg" && path != ".wg-cleanup-pending"
        })
        .collect();
    if !dirty.is_empty() {
        bail!(
            "finish.worktree_dirty: commit/stage your task work before begin; the same source agent retains the worktree"
        );
    }
    let contains = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", base, "HEAD"])
        .current_dir(worktree)
        .status()?;
    if contains.success() {
        return Ok(());
    }
    let output = std::process::Command::new("git")
        .args(["merge", "--no-edit", base])
        .current_dir(worktree)
        .output()?;
    if !output.status.success() {
        bail!(
            "finish.integration_conflict: resolve in this same task worktree and retry submit: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn prepare_candidate_evaluation(
    dir: &Path,
    store: &FinalizationStore,
    id: &str,
    ctx: &mut FinalizationContext,
) -> Result<(worksgood::finalization::FinalizationTransaction, bool)> {
    let config = worksgood::config::Config::load_or_default(dir);
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let selection = worksgood::evaluation::LazyEvaluationSelection::resolve(task, &config)?;
    let required = selection.gate_policy().is_some_and(|policy| {
        policy.applicability == worksgood::eval_lifecycle::EvaluationGateApplicability::Required
    });
    ctx.evaluation_policy = if required {
        "required-task-owned-readonly-before-promotion".into()
    } else {
        "none".into()
    };
    let tx = worksgood::finalization::checkpoint_candidate(store, ctx)?;
    let candidate = tx.candidate.as_ref().context("candidate missing")?;
    let validation = tx.validation.as_ref().context("validation missing")?;
    let dependency_digest = worksgood::evaluation::dependency_revision_digest(&graph, task)?;
    let source = worksgood::evaluation::SourceCandidateRef {
        task_id: id.into(),
        generation: candidate.generation,
        source_attempt_id: candidate.attempt_id.clone(),
        source_fence: candidate.attempt_fence,
        finalization_round: candidate.candidate_version,
        candidate_digest: candidate.candidate_id.clone(),
        candidate_manifest_digest: candidate.content_manifest_cid.clone(),
        dependency_revision_digest: dependency_digest,
        validation_result_id: validation.result_id.clone(),
    };
    let mut error = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |fresh| {
        let Some(task) = fresh.get_task_mut(id) else {
            error = Some("source task disappeared".into());
            return false;
        };
        let mut request = worksgood::lifecycle::TransitionRequest::new(
            worksgood::lifecycle::TransitionKind::CandidateCheckpointed {
                candidate_id: source.candidate_digest.clone(),
                manifest_cid: source.candidate_manifest_digest.clone(),
                validation_result_id: source.validation_result_id.clone(),
                finalization_round: source.finalization_round,
            },
            worksgood::lifecycle::LifecycleActor {
                kind: worksgood::lifecycle::ActorKind::Finalizer,
                id: "task-owned-finish".into(),
            },
            "task_owned_candidate_sealed",
            format!("finish-seal:{}:{}", id, source.candidate_digest),
        )
        .expecting(worksgood::lifecycle::FenceExpectation::current(task));
        request.evidence_refs.extend([
            source.candidate_digest.clone(),
            source.candidate_manifest_digest.clone(),
            source.validation_result_id.clone(),
        ]);
        if let Err(value) = worksgood::lifecycle::apply_transition(task, request) {
            error = Some(value.to_string());
            return false;
        }
        if !selection.is_empty()
            && let Err(value) =
                worksgood::evaluation::mint_for_candidate(task, &source, &selection, &config)
        {
            error = Some(format!("{value:#}"));
            return false;
        }
        true
    })?;
    if let Some(error) = error {
        bail!("finish.evaluation_prepare_failed: {error}");
    }
    Ok((
        store.load_task(id)?.context("transaction disappeared")?,
        required,
    ))
}

fn submit_finish(
    dir: &Path,
    store: &FinalizationStore,
    id: &str,
    lease_arg: Option<&str>,
    commit: Option<&str>,
    wait_seconds: u64,
) -> Result<worksgood::finalization::FinalizationTransaction> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let completion_contract = graph.get_task_or_err(id)?.completion_contract;
    drop(graph);
    if let Some(existing) = store.load_task(id)?
        && matches!(
            existing.phase,
            worksgood::finalization::FinalizationPhase::Promoted
                | worksgood::finalization::FinalizationPhase::Delivered
                | worksgood::finalization::FinalizationPhase::Reported
                | worksgood::finalization::FinalizationPhase::Cleaned
        )
    {
        return Ok(existing);
    }
    let mut ctx = finish_context(dir, id)?;
    if let Some(expected) = commit {
        let head = git(&ctx.worktree_path, &["rev-parse", "HEAD"])?;
        let resolved = git(&ctx.worktree_path, &["rev-parse", expected])?;
        if head != resolved {
            bail!("finish.commit_mismatch: submit commit is not current worktree HEAD");
        }
    }
    if completion_contract != worksgood::graph::CompletionContract::Land {
        ctx.evaluation_policy = "none".into();
        let tx = worksgood::finalization::checkpoint_candidate(store, &ctx)?;
        let _ = tx;
        return worksgood::finalization::publish_output(
            store,
            id,
            match completion_contract {
                worksgood::graph::CompletionContract::Deliver => {
                    worksgood::finalization::OutputDisposition::Delivered
                }
                worksgood::graph::CompletionContract::Report => {
                    worksgood::finalization::OutputDisposition::Reported
                }
                worksgood::graph::CompletionContract::Land => unreachable!(),
            },
        );
    }
    let lease = match lease_arg {
        Some(value) => value.to_string(),
        None => store
            .load_finish_lease()?
            .filter(|value| value.task_id == id)
            .map(|value| value.lease_id)
            .context("finish.lease_missing: run `wg finish begin` first")?,
    };
    // Reassert integration before sealing. This is idempotent and returns a
    // real conflict to the same live source owner.
    let lease_value = store.load_finish_lease()?.context("finish.lease_missing")?;
    integrate_leased_base(&ctx.worktree_path, &lease_value.base_commit_oid)?;
    let (tx, required) = prepare_candidate_evaluation(dir, store, id, &mut ctx)?;
    let candidate_id = tx
        .candidate
        .as_ref()
        .context("candidate missing")?
        .candidate_id
        .clone();
    if !required {
        worksgood::finalization::record_evaluation_receipt(
            store,
            &candidate_id,
            worksgood::finalization::EvaluationReceiptOutcome::Accepted,
            "evaluation-not-required",
            "task-owned-finish-policy",
        )?;
    } else {
        let started = Instant::now();
        loop {
            if let Some(receipt) = store
                .load_task(id)?
                .and_then(|value| value.evaluation_receipt)
            {
                match receipt.outcome {
                    worksgood::finalization::EvaluationReceiptOutcome::Accepted => break,
                    worksgood::finalization::EvaluationReceiptOutcome::Rejected => {
                        let _ = worksgood::finalization::release_finish_lease(store, &lease);
                        bail!(
                            "finish.evaluation_rejected: evidence={}; repair in the same session/worktree and begin again",
                            receipt.evidence_id
                        );
                    }
                    worksgood::finalization::EvaluationReceiptOutcome::InsufficientEvidence
                    | worksgood::finalization::EvaluationReceiptOutcome::Unavailable => {
                        let _ = worksgood::finalization::release_finish_lease(store, &lease);
                        bail!(
                            "finish.evaluation_infrastructure: outcome={:?} evidence={}; source attempt/session unchanged",
                            receipt.outcome,
                            receipt.evidence_id
                        );
                    }
                }
            }
            if let Some((outcome, evidence)) =
                terminal_evaluation_infrastructure(dir, id, &candidate_id)?
            {
                worksgood::finalization::record_evaluation_receipt(
                    store,
                    &candidate_id,
                    outcome,
                    &evidence,
                    "evaluation-service",
                )?;
                continue;
            }
            if started.elapsed() >= Duration::from_secs(wait_seconds) {
                let _ = worksgood::finalization::release_finish_lease(store, &lease);
                bail!(
                    "finish.evaluation_unavailable: timed out waiting for exact candidate {}; source attempt/session unchanged",
                    candidate_id
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    match worksgood::finalization::promote_task_owned_candidate(store, &ctx, &lease, &candidate_id)
    {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = worksgood::finalization::release_finish_lease(store, &lease);
            Err(error)
        }
    }
}

fn terminal_evaluation_infrastructure(
    dir: &Path,
    id: &str,
    candidate_id: &str,
) -> Result<Option<(worksgood::finalization::EvaluationReceiptOutcome, String)>> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    for record in task.evaluation_records.iter().filter(|record| {
        record.source.candidate_digest == candidate_id
            && record.policy.applicability
                == worksgood::eval_lifecycle::EvaluationGateApplicability::Required
    }) {
        let outcome = match record.state {
            worksgood::evaluation::EvaluationState::InsufficientEvidence => {
                Some(worksgood::finalization::EvaluationReceiptOutcome::InsufficientEvidence)
            }
            worksgood::evaluation::EvaluationState::TimedOut
            | worksgood::evaluation::EvaluationState::Malformed
            | worksgood::evaluation::EvaluationState::RouteDrift
            | worksgood::evaluation::EvaluationState::ProcessFailed
            | worksgood::evaluation::EvaluationState::Unavailable => {
                Some(worksgood::finalization::EvaluationReceiptOutcome::Unavailable)
            }
            _ => None,
        };
        if let Some(outcome) = outcome {
            return Ok(Some((outcome, record.evaluation_id.clone())));
        }
    }
    Ok(None)
}

pub(crate) fn cleanup_finish(
    dir: &Path,
    store: &FinalizationStore,
    id: &str,
    json: bool,
) -> Result<()> {
    let tx = store.load_task(id)?.context("finish transaction missing")?;
    if let Some(receipt) = tx.cleanup_receipt.as_ref() {
        if json {
            println!("{}", serde_json::to_string_pretty(receipt)?);
        } else {
            println!("Cleanup already complete: {}", receipt.receipt_id);
        }
        return Ok(());
    }
    let (disposition, durable_receipt) = if let Some(receipt) = tx.merge_receipt.as_ref() {
        let canonical = std::process::Command::new("git")
            .args([
                "merge-base",
                "--is-ancestor",
                &receipt.integration_commit_oid,
                "refs/heads/main",
            ])
            .current_dir(&tx.project_root)
            .status()?;
        if !canonical.success() {
            bail!("cleanup.merge_receipt_not_canonical: promoted commit is not on main");
        }
        ("landed", receipt.receipt_id.clone())
    } else if let Some(receipt) = tx.output_receipt.as_ref() {
        (
            match receipt.disposition {
                worksgood::finalization::OutputDisposition::Delivered => "delivered",
                worksgood::finalization::OutputDisposition::Reported => "reported",
            },
            receipt.receipt_id.clone(),
        )
    } else {
        bail!("cleanup.durable_receipt_missing: promotion/delivery/report not complete");
    };
    if std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        == tx.worktree_path.canonicalize().ok()
    {
        bail!("cleanup.cwd_owned: change out of the task worktree before cleanup");
    }
    let branch = if tx.worktree_path.exists() {
        git(&tx.worktree_path, &["branch", "--show-current"])?
    } else {
        String::new()
    };
    if tx.worktree_path.exists() {
        crate::commands::service::worktree::remove_worktree(
            &tx.project_root,
            &tx.worktree_path,
            &branch,
        )?;
    }
    let receipt = worksgood::finalization::record_cleanup(
        store,
        id,
        disposition,
        &durable_receipt,
        tx.candidate
            .as_ref()
            .map(|value| value.worktree_id.as_str())
            .unwrap_or("unknown"),
        &tx.worktree_path,
        &branch,
    )?;
    if let Some(lease) = tx.finish_lease_id.as_deref() {
        let _ = worksgood::finalization::release_finish_lease(store, lease);
    }
    let mut transition_error = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            transition_error = Some("task disappeared after cleanup".into());
            return false;
        };
        if task.status != worksgood::graph::Status::Done {
            let mut request = worksgood::lifecycle::TransitionRequest::new(
                worksgood::lifecycle::TransitionKind::AttemptSucceeded {
                    acceptance_ref: Some(receipt.receipt_id.clone()),
                    manual_review: false,
                },
                worksgood::lifecycle::LifecycleActor {
                    kind: worksgood::lifecycle::ActorKind::ProcessObserver,
                    id: "task-wrapper-cleanup".into(),
                },
                "completion_cleanup_committed",
                format!("finish-cleanup:{}:{}", id, receipt.receipt_id),
            )
            .with_evidence(durable_receipt.clone())
            .with_evidence(receipt.receipt_id.clone());
            if task.lifecycle.current_attempt.is_some() {
                request.expected = worksgood::lifecycle::FenceExpectation::current(task);
            }
            if let Err(value) = worksgood::lifecycle::apply_transition(task, request) {
                transition_error = Some(value.to_string());
                return false;
            }
        }
        task.completion_disposition = Some(match disposition {
            "landed" => worksgood::graph::CompletionDisposition::Landed,
            "delivered" => worksgood::graph::CompletionDisposition::Delivered,
            _ => worksgood::graph::CompletionDisposition::Reported,
        });
        task.completion_receipt = Some(receipt.receipt_id.clone());
        task.completed_at = Some(chrono::Utc::now().to_rfc3339());
        task.assigned = None;
        true
    })?;
    if let Some(error) = transition_error {
        bail!("cleanup.status_receipt_failed: {error}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!("Completed({}) cleanup={}", disposition, receipt.receipt_id);
    }
    Ok(())
}

fn set_contract(dir: &Path, id: &str, value: &str) -> Result<()> {
    let contract = match value {
        "land" => worksgood::graph::CompletionContract::Land,
        "deliver" => worksgood::graph::CompletionContract::Deliver,
        "report" => worksgood::graph::CompletionContract::Report,
        _ => bail!("completion contract must be land, deliver, or report"),
    };
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        if task.status != worksgood::graph::Status::Open || task.assigned.is_some() {
            return false;
        }
        task.completion_contract = contract;
        true
    })?;
    println!("Completion contract for {id}: {contract}");
    Ok(())
}

fn add_input_dependency(dir: &Path, id: &str, from: &str) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let source = graph.get_task_or_err(from)?;
    if source.completion_contract != worksgood::graph::CompletionContract::Deliver {
        bail!("typed input source {from} must have completion contract deliver");
    }
    drop(graph);
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        if !task.after.iter().any(|value| value == from) {
            task.after.push(from.into());
        }
        if !task.input_dependency_from(from) {
            task.input_dependencies
                .push(worksgood::graph::TaskInputDependency {
                    task_id: from.into(),
                    kind: "contribution".into(),
                });
        }
        true
    })?;
    println!("Added immutable contribution input: {id} <- {from}");
    Ok(())
}

fn checkpoint_uncommitted_source_work(id: &str) -> Result<()> {
    let worktree = std::env::var_os("WG_WORKTREE_PATH")
        .map(PathBuf::from)
        .context("worktree path unavailable")?;
    let add = Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worktree)
        .output()
        .context("stage source work before finish")?;
    if !add.status.success() {
        bail!(
            "finish.checkpoint_stage_failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }
    // A managed worktree contains a `.wg` symlink to the live graph. It is
    // transaction control state, never task output; committing it would make
    // target checkout attempt to replace/delete its own graph. Preserve a
    // repository-owned path only when it was already tracked at source HEAD.
    for control in [".wg", ".wg-cleanup-pending"] {
        let tracked = Command::new("git")
            .args(["ls-tree", "--name-only", "HEAD", "--", control])
            .current_dir(&worktree)
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
        if !tracked {
            let _ = Command::new("git")
                .args(["reset", "-q", "HEAD", "--", control])
                .current_dir(&worktree)
                .status();
        }
    }
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .current_dir(&worktree)
        .status()
        .context("inspect staged source work before finish")?;
    if staged.success() {
        return Ok(());
    }
    let commit = Command::new("git")
        .args(["commit", "-m", &format!("wg task checkpoint {id}")])
        .current_dir(&worktree)
        .output()
        .context("commit source work before finish")?;
    if !commit.status.success() {
        bail!(
            "finish.checkpoint_commit_failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        );
    }
    Ok(())
}

/// Entry used by `wg done` after all ordinary deliverable/verify/smoke gates.
pub fn task_owned_done(dir: &Path, id: &str) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    if task.status != worksgood::graph::Status::InProgress {
        return Ok(false);
    }
    let contract = task.completion_contract;
    drop(graph);
    checkpoint_uncommitted_source_work(id)?;
    let store = FinalizationStore::open(dir)?;
    let lease = if contract == worksgood::graph::CompletionContract::Land {
        Some(begin_finish(dir, &store, id, 1800)?.lease_id)
    } else {
        None
    };
    let tx = submit_finish(dir, &store, id, lease.as_deref(), Some("HEAD"), 1800)?;
    eprintln!(
        "[finish] task-owned {:?}: candidate={} durable={} cleanup will run from wrapper after cwd exit",
        tx.phase,
        tx.candidate
            .as_ref()
            .map(|value| value.candidate_id.as_str())
            .unwrap_or("none"),
        tx.merge_receipt
            .as_ref()
            .map(|value| value.receipt_id.as_str())
            .or_else(|| tx
                .output_receipt
                .as_ref()
                .map(|value| value.receipt_id.as_str()))
            .unwrap_or("none")
    );
    Ok(true)
}

pub fn context_from_current(
    dir: &Path,
    id: &str,
    worktree: Option<PathBuf>,
    receipt: Option<String>,
    operator_override: bool,
) -> Result<FinalizationContext> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("task has no current attempt")?;
    let agent = task
        .assigned
        .clone()
        .or_else(|| std::env::var("WG_AGENT_ID").ok())
        .unwrap_or_else(|| "retained".into());
    let worktree = worktree
        .or_else(|| std::env::var_os("WG_WORKTREE_PATH").map(PathBuf::from))
        .or_else(|| {
            worksgood::service::AgentRegistry::load(dir)
                .ok()?
                .get_agent_by_task(id)?
                .worktree_path
                .as_ref()
                .map(PathBuf::from)
        })
        .context("worktree path unavailable")?;
    let wrapper_quiescent = std::env::var("WG_HANDLER_QUIESCENT").as_deref() == Ok("1");
    let pi_exit = task.lifecycle.audit.iter().any(|e| {
        e.event_kind == "pi-process-epoch-exited"
            && e.generation == task.lifecycle.generation
            && e.attempt_id.as_deref() == Some(&attempt.id)
    });
    let explicit = receipt.is_some() && operator_override;
    if !wrapper_quiescent && !pi_exit && !explicit {
        bail!(
            "finalize.writer_still_current: exact current process can still write; wait for watchdog/supervisor quiescence"
        );
    }
    let terminal = task.lifecycle.pi_terminal_reservation.as_ref();
    let terminal_id = terminal
        .map(|r| r.idempotency_key.clone())
        .unwrap_or_else(|| format!("terminal:{}:{}:{}", id, attempt.id, task.lifecycle.fence));
    let process_identity = pi_identity(dir, task, attempt)
        .or_else(|| generic_process_identity(dir, id))
        .unwrap_or_else(|| {
            format!(
                "wrapper-reap:{}:{}:{}",
                id, attempt.id, task.lifecycle.pi_process_epoch
            )
        });
    let receipt_cid = receipt.unwrap_or_else(|| {
        format!(
            "wgcid:v1:blake3:{}",
            blake3::hash(
                format!(
                    "{}:{}:{}",
                    terminal_id, process_identity, task.lifecycle.fence
                )
                .as_bytes()
            )
            .to_hex()
        )
    });
    Ok(FinalizationContext {
        task_id: id.into(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        process_epoch: task.lifecycle.pi_process_epoch.max(1),
        worktree_id: agent,
        worktree_lease_epoch: task.lifecycle.fence,
        worktree_path: worktree,
        project_root: dir.parent().unwrap_or(dir).to_path_buf(),
        terminal_reservation_id: terminal_id,
        evaluation_policy: if worksgood::config::Config::load_or_default(dir)
            .agency
            .auto_evaluate
        {
            "required".into()
        } else {
            "none".into()
        },
        route_snapshot_cid: task
            .lifecycle
            .pi_continuation
            .as_ref()
            .map(|a| a.route_snapshot_digest.clone())
            .unwrap_or_else(|| "route:non-pi".into()),
        quiescence: QuiescenceProof {
            receipt_cid,
            process_identity_digest: process_identity,
            process_group_empty: true,
            nonce_pipe_eof: true,
            observed_manifest_digest: None,
        },
    })
}

fn settle(dir: &Path, id: &str) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let disposition = task
        .lifecycle
        .pi_terminal_reservation
        .as_ref()
        .map(|r| r.disposition);
    drop(graph);
    unsafe {
        std::env::set_var("WG_HANDLER_QUIESCENT", "1");
    }
    match disposition {
        Some(worksgood::pi_watchdog::TerminalDisposition::SuccessIntent) => {
            super::done::run(dir, id, false, false, false, false, false)
        }
        Some(worksgood::pi_watchdog::TerminalDisposition::Failure) => super::fail::run(
            dir,
            id,
            Some("Pi worker explicitly failed; rescue retained"),
            None,
        ),
        Some(other) => {
            println!(
                "Terminal intent {:?} retained for lifecycle adapter; no candidate promoted",
                other
            );
            Ok(())
        }
        None => Ok(()),
    }
}

fn resolve_candidate(
    store: &FinalizationStore,
    id: &str,
) -> Result<worksgood::finalization::CandidateDescriptor> {
    if id.starts_with("wgcid:") {
        store.read_candidate(id)
    } else {
        store
            .load_task(id)?
            .and_then(|t| t.candidate)
            .context("candidate not found")
    }
}
fn show_status(store: &FinalizationStore, id: &str, json: bool) -> Result<()> {
    let Some(tx) = store.load_task(id)? else {
        println!(
            "No finalization transaction for '{id}'. Safe next command: wait for exact quiescence, then `wg finalize checkpoint {id}`"
        );
        return Ok(());
    };
    print_tx(&tx, json)
}
fn print_tx(tx: &worksgood::finalization::FinalizationTransaction, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(tx)?)
    } else {
        println!(
            "Finalization {:?}: {} generation={} attempt={} fence={} lease={}\n  process: {} receipt={} group-empty={} nonce-eof={}\n  worktree: {}\n  rescue: {} commit={} tree={} manifest={}\n  candidate: {} commit={} tree={} manifest={}\n  validation: {} binding={}\n  evaluation: request={} policy={} route={} binding={} read-only={}\n  finish: lease={} evaluation-receipt={} outcome={} output-receipt={} cleanup-receipt={}\n  merge: receipt={} conflict={}\n  retained: {}\n  replay: {}\n  next: {}",
            tx.phase,
            tx.task_id,
            tx.generation,
            tx.attempt_id,
            tx.attempt_fence,
            tx.worktree_lease_epoch,
            tx.quiescence.process_identity_digest,
            tx.quiescence.receipt_cid,
            tx.quiescence.process_group_empty,
            tx.quiescence.nonce_pipe_eof,
            tx.worktree_path.display(),
            tx.rescue
                .as_ref()
                .map(|r| r.rescue_id.as_str())
                .unwrap_or("none"),
            tx.rescue
                .as_ref()
                .map(|r| r.rescue_commit_oid.as_str())
                .unwrap_or("none"),
            tx.rescue
                .as_ref()
                .map(|r| r.rescue_tree_oid.as_str())
                .unwrap_or("none"),
            tx.rescue
                .as_ref()
                .map(|r| r.manifest_cid.as_str())
                .unwrap_or("none"),
            tx.candidate
                .as_ref()
                .map(|c| c.candidate_id.as_str())
                .unwrap_or("none"),
            tx.candidate
                .as_ref()
                .map(|c| c.candidate_commit_oid.as_str())
                .unwrap_or("none"),
            tx.candidate
                .as_ref()
                .map(|c| c.candidate_tree_oid.as_str())
                .unwrap_or("none"),
            tx.candidate
                .as_ref()
                .map(|c| c.content_manifest_cid.as_str())
                .unwrap_or("none"),
            tx.validation
                .as_ref()
                .map(|v| v.result_id.as_str())
                .unwrap_or("none"),
            tx.validation
                .as_ref()
                .map(|v| v.binding.candidate_id.as_str())
                .unwrap_or("none"),
            tx.evaluation_request
                .as_ref()
                .map(|e| e.request_id.as_str())
                .unwrap_or("none"),
            tx.evaluation_request
                .as_ref()
                .map(|e| e.policy_identity.as_str())
                .unwrap_or("none"),
            tx.evaluation_request
                .as_ref()
                .map(|e| e.route_snapshot_cid.as_str())
                .unwrap_or("none"),
            tx.evaluation_request
                .as_ref()
                .map(|e| e.binding.candidate_id.as_str())
                .unwrap_or("none"),
            tx.evaluation_request
                .as_ref()
                .is_some_and(|e| e.read_only_materialization),
            tx.finish_lease_id.as_deref().unwrap_or("none"),
            tx.evaluation_receipt
                .as_ref()
                .map(|value| value.receipt_id.as_str())
                .unwrap_or("none"),
            tx.evaluation_receipt
                .as_ref()
                .map(|value| format!("{:?}", value.outcome))
                .unwrap_or_else(|| "none".into()),
            tx.output_receipt
                .as_ref()
                .map(|value| value.receipt_id.as_str())
                .unwrap_or("none"),
            tx.cleanup_receipt
                .as_ref()
                .map(|value| value.receipt_id.as_str())
                .unwrap_or("none"),
            tx.merge_receipt
                .as_ref()
                .map(|r| r.receipt_id.as_str())
                .unwrap_or("none"),
            tx.merge_conflict
                .as_ref()
                .map(|c| c.reason_code.as_str())
                .unwrap_or("none"),
            tx.retained_reason.as_deref().unwrap_or("none"),
            tx.replay_action.as_deref().unwrap_or("none"),
            tx.safe_next_command
        )
    }
    Ok(())
}
fn pi_identity(
    dir: &Path,
    task: &worksgood::graph::Task,
    attempt: &worksgood::lifecycle::AttemptRef,
) -> Option<String> {
    let key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let path = worksgood::attempt_runtime::resolve_component(dir, &key, "pi/state.json").ok()??;
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(v.get("state")?.get("process")?.to_string())
}
fn generic_process_identity(dir: &Path, id: &str) -> Option<String> {
    let agent = worksgood::service::AgentRegistry::load(dir)
        .ok()?
        .get_agent_by_task(id)?
        .clone();
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", agent.pid)).ok()?;
        let close = stat.rfind(')')?;
        let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let pgid = fields.get(2)?;
        let start = fields.get(19)?;
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(format!(
            "pid:{}:pgid:{}:start:{}:boot:{}:waited-handler:true",
            agent.pid,
            pgid,
            start,
            boot.trim()
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(format!("pid:{}:platform:waited-handler:true", agent.pid))
    }
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let o = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()?;
    if !o.status.success() {
        bail!("git {:?}: {}", args, String::from_utf8_lossy(&o.stderr))
    }
    Ok(String::from_utf8(o.stdout)?.trim().into())
}
fn safe(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
