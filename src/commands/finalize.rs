use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use worksgood::completion_evidence::{
    AcceptanceOutcome, AttemptSaveKey, CandidateDescriptor as AtomicCandidateDescriptor,
    CleanupCommit, CleanupResult, CompletionIntentReceipt, DispositionReceipt, EffectReceipt,
    EvidenceBinding, EvidenceCidSet, EvidenceHeader, FlipReceipt, GraphSaveBundle,
    GraphSaveReceipt, OutputReceipt, PromotionReceipt, ValidationReceipt, WorkSaveReceipt,
    content_cid,
};
use worksgood::finalization::{
    FinalizationContext, FinalizationStore, QuiescenceProof, checkpoint_candidate,
    checkpoint_rescue,
};
use worksgood::graph::{
    CompletionContract, CompletionDisposition, Task, TokenUsage, WorkGraph, parse_token_usage,
    parse_wg_tokens,
};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::save_transaction::{
    SaveFact, SavePhase, SaveTransactionKernel, SaveTransactionState, SaveTransitionRequest,
};
use worksgood::service::registry::AgentRegistry;

use crate::cli::{CandidateCommands, FinalizeCommands};

#[derive(Clone, Debug, Default)]
struct CompletionAccounting {
    token_usage: Option<TokenUsage>,
    actual_executor: Option<String>,
    actual_model: Option<String>,
}

/// Snapshot the exact assigned registry row before terminal projection clears
/// `task.assigned`. Pi parsing follows output.log to its sibling raw stream and
/// counts only authoritative `turn_end` usage once per turn.
fn completion_accounting(dir: &Path, task: &Task) -> CompletionAccounting {
    let Some(agent_id) = task.assigned.as_deref() else {
        return CompletionAccounting::default();
    };
    let Ok(registry) = AgentRegistry::load(dir) else {
        return CompletionAccounting::default();
    };
    let Some(agent) = registry
        .get_agent(agent_id)
        .filter(|agent| agent.task_id == task.id)
    else {
        return CompletionAccounting::default();
    };
    let output = Path::new(&agent.output_file);
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        dir.parent().unwrap_or(dir).join(output)
    };
    CompletionAccounting {
        token_usage: parse_token_usage(&output).or_else(|| parse_wg_tokens(&output)),
        actual_executor: Some(agent.executor.clone()),
        actual_model: agent.model.clone(),
    }
}

fn apply_completion_accounting(task: &mut Task, accounting: &CompletionAccounting) {
    if task.token_usage.is_none() {
        task.token_usage.clone_from(&accounting.token_usage);
    }
    if task.actual_executor.is_none() {
        task.actual_executor.clone_from(&accounting.actual_executor);
    }
    if task.actual_model.is_none() {
        task.actual_model.clone_from(&accounting.actual_model);
    }
}

/// Atomically project a terminal success through the v2 SaveTransaction and
/// GraphSave authority.  Terminal-facing adapters use this instead of writing
/// `Status::Done` or submitting the legacy `AttemptSucceeded` transition.
pub fn commit_terminal_success(
    dir: &Path,
    id: &str,
    actor_id: Option<&str>,
    reason_code: &str,
) -> Result<String> {
    let graph_path = dir.join("graph.jsonl");
    ensure_terminal_attempt_on_disk(dir, id, actor_id)?;
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(id)?.clone();
    let accounting = completion_accounting(dir, &task);
    let (bundle, state) = prepare_graph_save(dir, &task, reason_code)?;
    persist_save_state(dir, &state)?;
    crash_after(SavePhase::GraphSaved)?;

    let graph_save_cid = content_cid(&bundle.receipt).map_err(anyhow::Error::msg)?;
    persist_graph_save(dir, id, task.lifecycle.generation, &bundle)?;
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            rejection = Some(format!("terminal task '{id}' disappeared"));
            return false;
        };
        let actor = actor_id
            .map(LifecycleActor::worker)
            .unwrap_or_else(|| LifecycleActor::operator(worksgood::current_user()));
        let request = TransitionRequest {
            event_id: bundle.receipt.lifecycle_event_id.clone(),
            idempotency_key: format!("graphsave:{}", state.transaction_id),
            actor: LifecycleActor {
                kind: worksgood::lifecycle::ActorKind::Finalizer,
                id: actor.id,
            },
            reason_code: reason_code.to_string(),
            kind: TransitionKind::GraphSaveCommitted {
                bundle: Box::new(bundle.clone()),
            },
            expected: FenceExpectation::current(task),
            evidence_refs: vec![graph_save_cid.clone()],
            occurred_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error.to_string());
            return false;
        }
        apply_completion_accounting(task, &accounting);
        task.completed_at = Some(chrono::Utc::now().to_rfc3339());
        task.assigned = None;
        true
    })?;
    if let Some(error) = rejection {
        bail!("terminal.graph_save_refused: {error}");
    }
    Ok(graph_save_cid)
}

/// Variant for callers already holding the graph mutation lock (for example
/// human-dispatch reply consumption).  It performs the same WAL/evidence work
/// but applies the GraphSave to the supplied in-memory projection.
pub fn commit_terminal_success_in_graph(
    dir: &Path,
    graph: &mut WorkGraph,
    id: &str,
    actor_id: Option<&str>,
    reason_code: &str,
) -> Result<String> {
    ensure_terminal_attempt_in_task(graph.get_task_mut_or_err(id)?, actor_id)?;
    let task_snapshot = graph.get_task_or_err(id)?.clone();
    let accounting = completion_accounting(dir, &task_snapshot);
    let (bundle, state) = prepare_graph_save(dir, &task_snapshot, reason_code)?;
    persist_save_state(dir, &state)?;
    crash_after(SavePhase::GraphSaved)?;
    let graph_save_cid = content_cid(&bundle.receipt).map_err(anyhow::Error::msg)?;
    persist_graph_save(dir, id, task_snapshot.lifecycle.generation, &bundle)?;
    let task = graph.get_task_mut_or_err(id)?;
    let request = TransitionRequest {
        event_id: bundle.receipt.lifecycle_event_id.clone(),
        idempotency_key: format!("graphsave:{}", state.transaction_id),
        actor: LifecycleActor {
            kind: worksgood::lifecycle::ActorKind::Finalizer,
            id: actor_id.unwrap_or("terminal-adapter").to_string(),
        },
        reason_code: reason_code.to_string(),
        kind: TransitionKind::GraphSaveCommitted {
            bundle: Box::new(bundle.clone()),
        },
        expected: FenceExpectation::current(task),
        evidence_refs: vec![graph_save_cid.clone()],
        occurred_at: chrono::Utc::now().to_rfc3339(),
    };
    apply_transition(task, request).map_err(anyhow::Error::msg)?;
    apply_completion_accounting(task, &accounting);
    task.completed_at = Some(chrono::Utc::now().to_rfc3339());
    task.assigned = None;
    Ok(graph_save_cid)
}

/// Write-ahead terminal reservation used before any source checkpoint or
/// effect.  Replaying the exact source tuple and reason is deterministic.
pub fn record_terminal_prepare(dir: &Path, id: &str, reason: &str) -> Result<String> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let source = source_key(dir, task)?;
    let state = SaveTransactionState::new(source.clone()).map_err(anyhow::Error::msg)?;
    let state = advance_save(
        state,
        SavePhase::Prepared,
        format!("terminal-intent:{reason}"),
        None,
        format!("intent:{}:{reason}", source.attempt_id),
    )?;
    persist_save_state(dir, &state)?;
    crash_after(SavePhase::Prepared)?;
    Ok(state.transaction_id)
}

/// Record a non-success terminal intent as an `AbortedPreserved`
/// SaveTransaction before the caller projects Failed/Abandoned/Incomplete.
pub fn record_terminal_abort(dir: &Path, id: &str, reason: &str) -> Result<String> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let source = match source_key(dir, task) {
        Ok(source) => source,
        Err(_) => AttemptSaveKey {
            graph_id: std::env::var("WG_GRAPH_ID")
                .unwrap_or_else(|_| format!("graph:{}", dir.display())),
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            attempt_id: format!("no-attempt-{}", task.lifecycle.generation),
            attempt_fence: task.lifecycle.fence,
            worktree_lease_epoch: task.lifecycle.fence,
            process_epoch: 0,
            wrapper_epoch: 1,
            route_snapshot_cid: "route:non-running-terminal".into(),
            session_proof_digest: "session:not-applicable".into(),
            worktree_identity_digest: "root:not-applicable".into(),
        },
    };
    let mut state = SaveTransactionState::new(source.clone()).map_err(anyhow::Error::msg)?;
    state = advance_save(
        state,
        SavePhase::Prepared,
        format!("terminal-intent:{reason}"),
        None,
        format!("intent:{}:{reason}", source.attempt_id),
    )?;
    state = advance_save(
        state,
        SavePhase::AbortedPreserved,
        format!("preserved:{reason}"),
        None,
        format!("abort:{}:{reason}", source.attempt_id),
    )?;
    persist_save_state(dir, &state)?;
    Ok(state.transaction_id)
}

fn ensure_terminal_attempt_on_disk(dir: &Path, id: &str, actor_id: Option<&str>) -> Result<()> {
    let mut error = None;
    modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            error = Some(format!("task '{id}' not found"));
            return false;
        };
        match ensure_terminal_attempt_in_task(task, actor_id) {
            Ok(changed) => changed,
            Err(value) => {
                error = Some(value.to_string());
                false
            }
        }
    })?;
    if let Some(error) = error {
        bail!("terminal.attempt_reservation_failed: {error}");
    }
    Ok(())
}

fn ensure_terminal_attempt_in_task(task: &mut Task, actor_id: Option<&str>) -> Result<bool> {
    if matches!(
        task.status,
        worksgood::graph::Status::PendingValidation | worksgood::graph::Status::PendingEval
    ) && let Some(attempt) = task.lifecycle.current_attempt.as_mut()
        && attempt.disposition == Some(worksgood::lifecycle::AttemptDisposition::Succeeded)
    {
        // Pre-v2 pending rows terminalized the attempt before acceptance.  The
        // adapter repairs that compatibility projection so GraphSave remains
        // the sole successful terminal edge.
        attempt.disposition = None;
        return Ok(true);
    }
    if let Some(attempt) = task.lifecycle.current_attempt.as_ref()
        && task.status != worksgood::graph::Status::Waiting
        && (attempt.disposition.is_none()
            || (task.status == worksgood::graph::Status::Failed
                && matches!(
                    attempt.disposition,
                    Some(worksgood::lifecycle::AttemptDisposition::Failed)
                        | Some(worksgood::lifecycle::AttemptDisposition::Lost)
                )))
    {
        return Ok(false);
    }
    if task.status == worksgood::graph::Status::Open {
        let request = TransitionRequest::new(
            TransitionKind::AttemptReserved {
                owner_id: actor_id.map(String::from),
            },
            LifecycleActor::operator(worksgood::current_user()),
            "terminal_adapter_reservation",
            format!("terminal-reserve:{}:{}", task.id, task.lifecycle.generation),
        )
        .expecting(FenceExpectation::current(task));
        apply_transition(task, request).map_err(anyhow::Error::msg)?;
        return Ok(true);
    }
    // Historical non-Open rows without an attempt are ambiguous. Never mint
    // lifecycle authority from compatibility status alone.
    bail!(
        "status {} cannot acquire a terminal source attempt",
        task.status
    )
}

fn prepare_graph_save(
    dir: &Path,
    task: &Task,
    reason_code: &str,
) -> Result<(GraphSaveBundle, SaveTransactionState)> {
    let source = source_key(dir, task)?;
    prepare_graph_save_for_source(dir, task, reason_code, source, None, true)
}

fn prepare_graph_save_for_source(
    dir: &Path,
    task: &Task,
    reason_code: &str,
    source: AttemptSaveKey,
    legacy: Option<&worksgood::finalization::FinalizationTransaction>,
    advance_transaction: bool,
) -> Result<(GraphSaveBundle, SaveTransactionState)> {
    if task.status == worksgood::graph::Status::Done && legacy.is_none() {
        bail!("terminal task '{}' is already done", task.id);
    }
    let project = dir.parent().unwrap_or(dir);
    let legacy_candidate = legacy.and_then(|transaction| transaction.candidate.as_ref());
    let head = legacy_candidate
        .map(|candidate| candidate.base_commit_oid.clone())
        .unwrap_or_else(|| {
            git(project, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "no-git-head".into())
        });
    let tree = legacy_candidate
        .map(|candidate| candidate.candidate_tree_oid.clone())
        .unwrap_or_else(|| {
            git(project, &["rev-parse", "HEAD^{tree}"]).unwrap_or_else(|_| head.clone())
        });
    let candidate_id = match legacy_candidate {
        Some(candidate) => candidate.candidate_id.clone(),
        None => content_cid(&serde_json::json!({
            "source": &source,
            "tree": tree,
            "reason": reason_code,
        }))
        .map_err(anyhow::Error::msg)?,
    };
    let binding = EvidenceBinding {
        source: source.clone(),
        candidate_id: candidate_id.clone(),
        base_commit_oid: head.clone(),
    };
    let build = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
    let header = || EvidenceHeader::v2(build);
    let contract = task.completion_contract;
    let disposition = match contract {
        CompletionContract::Land => CompletionDisposition::Landed,
        CompletionContract::Deliver => CompletionDisposition::Delivered,
        CompletionContract::Report => CompletionDisposition::Reported,
        CompletionContract::Explore => CompletionDisposition::Explored,
    };
    let legacy_validation = legacy.and_then(|transaction| transaction.validation.as_ref());
    let intent = CompletionIntentReceipt {
        header: header(),
        source: source.clone(),
        contract,
        terminal_reservation_cid: format!("terminal:{}:{}", task.id, source.attempt_id),
        capture_policy_cid: "policy:terminal-adapter-capture-v2".into(),
        // The bridge may only project the exact validation mechanics that
        // already ran. Bind the v2 intent to that durable legacy policy rather
        // than inventing a second policy identifier after the fact.
        validation_policy_cid: legacy_validation
            .map(|receipt| receipt.policy_cid.clone())
            .unwrap_or_else(|| "policy:terminal-adapter-validation-v2".into()),
        flip_policy_cid: if reason_code.contains("waiver") {
            format!("policy:operator-waiver:{reason_code}")
        } else {
            "policy:flip-not-required-v2".into()
        },
        smoke_policy_cid: "policy:terminal-adapter-smoke-v2".into(),
        deliverable_policy_cid: "policy:terminal-adapter-deliverables-v2".into(),
        expected_target_ref: (contract == CompletionContract::Land).then(|| "HEAD".into()),
        prepared_base_commit_oid: head.clone(),
        client_idempotency_key: format!("intent:{}:{}", task.id, source.attempt_id),
    };
    let intent_cid = content_cid(&intent).map_err(anyhow::Error::msg)?;
    let work_save = WorkSaveReceipt {
        header: header(),
        binding: binding.clone(),
        completion_intent_cid: intent_cid.clone(),
        quiescence_receipt_cid: legacy
            .map(|transaction| transaction.quiescence.receipt_cid.clone())
            .unwrap_or_else(|| format!("quiescent:{}", source.attempt_id)),
        worktree_root_identity: source.worktree_identity_digest.clone(),
        branch: None,
        worker_head_oid: legacy_candidate
            .map(|candidate| candidate.worker_head_oid.clone())
            .unwrap_or_else(|| head.clone()),
        prepared_base_commit_oid: head.clone(),
        clean: true,
        rescue_commit_oid: legacy
            .and_then(|transaction| transaction.rescue.as_ref())
            .map(|rescue| rescue.rescue_commit_oid.clone())
            .unwrap_or_else(|| head.clone()),
        saved_tree_oid: tree.clone(),
        full_manifest_cid: legacy_candidate
            .map(|candidate| candidate.content_manifest_cid.clone())
            .unwrap_or_else(|| format!("manifest:{tree}")),
        delta_manifest_cid: legacy_candidate
            .map(|candidate| candidate.delta_manifest_cid.clone())
            .unwrap_or_else(|| format!("delta:{tree}")),
        immutable_ref: legacy_candidate
            .map(|candidate| candidate.immutable_ref.clone())
            .unwrap_or_else(|| {
                format!(
                    "refs/wg/work-saves/{}/{}/{}",
                    task.id, source.generation, candidate_id
                )
            }),
        excluded_path_policy_cid: "policy:control-plane-excluded-v2".into(),
        observer_manifest_digest: legacy
            .and_then(|transaction| transaction.rescue.as_ref())
            .map(|rescue| rescue.manifest_cid.clone())
            .unwrap_or_else(|| format!("observer:{tree}")),
        observer_sequence: 1,
        late_mutation_quarantine_cid: None,
    };
    let work_save_cid = content_cid(&work_save).map_err(anyhow::Error::msg)?;
    let candidate = AtomicCandidateDescriptor {
        header: header(),
        binding: binding.clone(),
        work_save_cid: work_save_cid.clone(),
        candidate_version: legacy_candidate
            .map(|candidate| candidate.candidate_version)
            .unwrap_or(1),
        candidate_commit_oid: legacy_candidate
            .map(|candidate| candidate.candidate_commit_oid.clone())
            .unwrap_or_else(|| head.clone()),
        candidate_tree_oid: tree.clone(),
        full_manifest_cid: work_save.full_manifest_cid.clone(),
        delta_manifest_cid: work_save.delta_manifest_cid.clone(),
        inclusion_policy_cid: work_save.excluded_path_policy_cid.clone(),
        immutable_ref: work_save.immutable_ref.clone(),
    };
    let candidate_cid = content_cid(&candidate).map_err(anyhow::Error::msg)?;
    if legacy.is_some() && legacy_validation.is_none_or(|receipt| !receipt.passed) {
        bail!("completion.bridge_validation_evidence_missing");
    }
    let validation = ValidationReceipt {
        header: header(),
        binding: binding.clone(),
        candidate_cid: candidate_cid.clone(),
        policy_cid: legacy_validation
            .map(|receipt| receipt.policy_cid.clone())
            .unwrap_or_else(|| intent.validation_policy_cid.clone()),
        outcome: AcceptanceOutcome::Accepted,
        validator_identity: legacy_validation
            .map(|receipt| receipt.validator_identity.clone())
            .unwrap_or_else(|| "terminal-adapter:local-gates".into()),
    };
    let legacy_evaluation = legacy.and_then(|transaction| transaction.evaluation_receipt.as_ref());
    if legacy_evaluation.is_some_and(|receipt| {
        receipt.outcome != worksgood::finalization::EvaluationReceiptOutcome::Accepted
    }) {
        bail!("completion.bridge_acceptance_evidence_rejected");
    }
    let flip = FlipReceipt {
        header: header(),
        binding: binding.clone(),
        candidate_cid: candidate_cid.clone(),
        policy_cid: intent.flip_policy_cid.clone(),
        route_snapshot_cid: source.route_snapshot_cid.clone(),
        outcome: if legacy_evaluation.is_some() || reason_code.contains("waiver") {
            AcceptanceOutcome::Accepted
        } else {
            AcceptanceOutcome::NotRequired
        },
        evaluator_identity: legacy_evaluation
            .map(|receipt| receipt.evaluator_identity.clone())
            .unwrap_or_else(|| {
                if reason_code.contains("waiver") {
                    "operator:audited-waiver".into()
                } else {
                    "policy:not-required".into()
                }
            }),
    };
    let disposition_receipt = DispositionReceipt {
        header: header(),
        binding: binding.clone(),
        completion_intent_cid: intent_cid,
        candidate_cid,
        contract,
        disposition,
    };
    let disposition_cid = content_cid(&disposition_receipt).map_err(anyhow::Error::msg)?;
    let action_key = format!("effect:{}:{}", task.id, candidate_id);
    let effect = match disposition {
        CompletionDisposition::Landed => {
            let legacy_receipt = legacy.and_then(|transaction| transaction.merge_receipt.as_ref());
            if legacy.is_some() && legacy_receipt.is_none_or(|receipt| !receipt.ref_cas) {
                bail!("completion.bridge_promotion_receipt_missing");
            }
            EffectReceipt::Promotion(PromotionReceipt {
                header: header(),
                binding: binding.clone(),
                disposition_cid,
                action_key,
                target_ref: legacy_receipt
                    .map(|receipt| receipt.target_ref.clone())
                    .unwrap_or_else(|| "HEAD".into()),
                expected_old_commit_oid: legacy_receipt
                    .map(|receipt| receipt.expected_target_commit_oid.clone())
                    .unwrap_or_else(|| head.clone()),
                observed_old_commit_oid: legacy_receipt
                    .map(|receipt| receipt.expected_target_commit_oid.clone())
                    .unwrap_or_else(|| head.clone()),
                integration_commit_oid: legacy_receipt
                    .map(|receipt| receipt.integration_commit_oid.clone())
                    .unwrap_or_else(|| head.clone()),
                result_tree_oid: legacy_receipt
                    .map(|receipt| receipt.result_tree_oid.clone())
                    .unwrap_or_else(|| tree.clone()),
                result_manifest_cid: legacy_receipt
                    .map(|receipt| receipt.result_manifest_cid.clone())
                    .unwrap_or_else(|| format!("manifest:{tree}")),
                ref_cas_succeeded: legacy_receipt.is_none_or(|receipt| receipt.ref_cas),
            })
        }
        CompletionDisposition::Delivered
        | CompletionDisposition::Reported
        | CompletionDisposition::Explored => {
            let legacy_receipt = legacy.and_then(|transaction| transaction.output_receipt.as_ref());
            if legacy.is_some() && legacy_receipt.is_none() {
                bail!("completion.bridge_output_receipt_missing");
            }
            EffectReceipt::Output(OutputReceipt {
                header: header(),
                binding: binding.clone(),
                disposition_cid,
                action_key,
                immutable_output_ref: legacy_receipt
                    .map(|receipt| receipt.immutable_ref.clone())
                    .unwrap_or_else(|| format!("refs/wg/outputs/{}/{}", task.id, candidate_id)),
                output_manifest_cid: legacy_candidate
                    .map(|candidate| candidate.content_manifest_cid.clone())
                    .unwrap_or_else(|| format!("manifest:{tree}")),
            })
        }
    };
    let effect_cid = content_cid(&effect).map_err(anyhow::Error::msg)?;
    let legacy_cleanup = legacy.and_then(|transaction| transaction.cleanup_receipt.as_ref());
    if legacy.is_some() && legacy_cleanup.is_none_or(|receipt| !receipt.removed) {
        bail!("completion.bridge_cleanup_receipt_missing");
    }
    let cleanup = CleanupCommit {
        header: header(),
        binding: binding.clone(),
        work_save_cid,
        effect_receipt_cid: effect_cid,
        cleanup_plan_cid: legacy_cleanup
            .map(|receipt| receipt.receipt_id.clone())
            .unwrap_or_else(|| format!("cleanup-plan:{candidate_id}")),
        worktree_root_identity: source.worktree_identity_digest.clone(),
        worktree_lease_epoch: source.worktree_lease_epoch,
        result: if legacy_cleanup.is_some() {
            CleanupResult::Removed
        } else {
            CleanupResult::NotApplicable
        },
    };
    let evidence = EvidenceCidSet {
        completion_intent: content_cid(&intent).map_err(anyhow::Error::msg)?,
        work_save: content_cid(&work_save).map_err(anyhow::Error::msg)?,
        candidate: content_cid(&candidate).map_err(anyhow::Error::msg)?,
        validation: content_cid(&validation).map_err(anyhow::Error::msg)?,
        flip: content_cid(&flip).map_err(anyhow::Error::msg)?,
        disposition: content_cid(&disposition_receipt).map_err(anyhow::Error::msg)?,
        effect: content_cid(&effect).map_err(anyhow::Error::msg)?,
        cleanup: content_cid(&cleanup).map_err(anyhow::Error::msg)?,
    };
    let event_id = format!(
        "ev_graphsave_{}",
        &candidate_id[candidate_id.len().saturating_sub(24)..]
    );
    let receipt = GraphSaveReceipt {
        header: header(),
        binding: binding.clone(),
        contract,
        disposition,
        bundle_digest: content_cid(&evidence).map_err(anyhow::Error::msg)?,
        evidence,
        graph_revision_before_commit: task.lifecycle.revision,
        lifecycle_event_id: event_id,
    };
    let bundle = GraphSaveBundle {
        receipt,
        completion_intent: intent,
        work_save,
        candidate,
        validation,
        flip,
        disposition: disposition_receipt,
        effect,
        cleanup,
    };

    let mut state = SaveTransactionState::new(source).map_err(anyhow::Error::msg)?;
    if !advance_transaction {
        // The brokered bridge already owns a Prepared transaction for this
        // exact source. It needs only the evidence bundle; writing another
        // head here would create a second authority slot for the same CID.
        return Ok((bundle, state));
    }
    let phases = [
        (
            SavePhase::Prepared,
            bundle.receipt.evidence.completion_intent.clone(),
        ),
        (
            SavePhase::Quiescing,
            bundle.work_save.quiescence_receipt_cid.clone(),
        ),
        (
            SavePhase::WorkSaved,
            bundle.receipt.evidence.work_save.clone(),
        ),
        (
            SavePhase::CandidateSealed,
            bundle.receipt.evidence.candidate.clone(),
        ),
        (
            SavePhase::Validated,
            bundle.receipt.evidence.validation.clone(),
        ),
        (SavePhase::Accepted, bundle.receipt.evidence.flip.clone()),
        (
            SavePhase::DispositionRecorded,
            bundle.receipt.evidence.disposition.clone(),
        ),
        (
            SavePhase::EffectPrepared,
            format!("effect-plan:{}", binding.candidate_id),
        ),
        (
            SavePhase::EffectCommitted,
            bundle.receipt.evidence.effect.clone(),
        ),
        (
            SavePhase::CleanupPrepared,
            bundle.cleanup.cleanup_plan_cid.clone(),
        ),
        (
            SavePhase::CleanupCommitted,
            bundle.receipt.evidence.cleanup.clone(),
        ),
    ];
    for (phase, cid) in phases {
        let post_work = phase >= SavePhase::WorkSaved;
        state = advance_save(
            state,
            phase,
            cid,
            post_work.then(|| binding.clone()),
            format!("{}:{:?}", task.id, phase),
        )?;
        persist_save_state(dir, &state)?;
        crash_after(phase)?;
    }
    let request = SaveTransitionRequest {
        source: state.source.clone(),
        expected_revision: state.revision,
        expected_phase: state.phase,
        next_phase: SavePhase::GraphSaved,
        idempotency_key: format!("graphsave:{}", state.transaction_id),
        action_key: format!("graphsave-action:{}", state.transaction_id),
        fact: SaveFact::GraphSave {
            bundle: Box::new(bundle.clone()),
        },
    };
    state = SaveTransactionKernel::transition(&state, request)
        .map_err(anyhow::Error::msg)?
        .state;
    Ok((bundle, state))
}

fn source_key(dir: &Path, task: &Task) -> Result<AttemptSaveKey> {
    let attempt = task.lifecycle.current_attempt.as_ref().context(
        "terminal.save_source_missing: task has no lifecycle attempt; retry/claim it before terminalizing",
    )?;
    if attempt.generation != task.lifecycle.generation || attempt.fence != task.lifecycle.fence {
        bail!("terminal.save_source_stale: current attempt does not match generation/fence");
    }
    let root = std::env::var("WG_WORKTREE_PATH").unwrap_or_else(|_| "no-worktree".into());
    Ok(AttemptSaveKey {
        graph_id: std::env::var("WG_GRAPH_ID")
            .unwrap_or_else(|_| format!("graph:{}", dir.display())),
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        worktree_lease_epoch: task.lifecycle.fence,
        process_epoch: task.lifecycle.pi_process_epoch.max(1),
        wrapper_epoch: 1,
        route_snapshot_cid: task
            .lifecycle
            .pi_continuation
            .as_ref()
            .map(|v| v.route_snapshot_digest.clone())
            .unwrap_or_else(|| "route:terminal-adapter".into()),
        session_proof_digest: task
            .lifecycle
            .pi_continuation
            .as_ref()
            .map(|v| v.session_proof_digest.clone())
            .unwrap_or_else(|| format!("session:{}", attempt.id)),
        worktree_identity_digest: format!("root:{}", blake3::hash(root.as_bytes()).to_hex()),
    })
}

fn advance_save(
    mut state: SaveTransactionState,
    phase: SavePhase,
    cid: String,
    binding: Option<EvidenceBinding>,
    key: String,
) -> Result<SaveTransactionState> {
    let request = SaveTransitionRequest {
        source: state.source.clone(),
        expected_revision: state.revision,
        expected_phase: state.phase,
        next_phase: phase,
        idempotency_key: key.clone(),
        action_key: format!("action:{key}"),
        fact: SaveFact::Evidence { cid, binding },
    };
    state = SaveTransactionKernel::transition(&state, request)
        .map_err(anyhow::Error::msg)?
        .state;
    Ok(state)
}

fn completion_root(dir: &Path) -> PathBuf {
    dir.join("completion/v2")
}
fn persist_save_state(dir: &Path, state: &SaveTransactionState) -> Result<()> {
    let root = completion_root(dir);
    let tx_dir = root
        .join("transactions")
        .join(state.transaction_id.replace(':', "_"));
    std::fs::create_dir_all(&tx_dir)?;
    worksgood::atomic_file::write_atomic(
        &tx_dir.join("head.json"),
        &serde_json::to_vec_pretty(state)?,
    )?;
    Ok(())
}
fn persist_graph_save(
    dir: &Path,
    id: &str,
    generation: u64,
    bundle: &GraphSaveBundle,
) -> Result<()> {
    let root = completion_root(dir);
    let object_dir = root.join("objects");
    let save_dir = root.join("graph-saves").join(id);
    std::fs::create_dir_all(&object_dir)?;
    std::fs::create_dir_all(&save_dir)?;
    let bytes = serde_json::to_vec_pretty(bundle)?;
    let cid = content_cid(bundle).map_err(anyhow::Error::msg)?;
    worksgood::atomic_file::write_atomic(&object_dir.join(cid.replace(':', "_")), &bytes)?;
    worksgood::atomic_file::write_atomic(&save_dir.join(format!("{generation}.json")), &bytes)?;
    Ok(())
}
fn crash_after(phase: SavePhase) -> Result<()> {
    let requested = std::env::var("WG_TEST_SAVE_CRASH_AFTER").ok();
    let wire = serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(String::from));
    if requested.as_deref() == Some(format!("{:?}", phase).as_str())
        || requested.as_deref() == wire.as_deref()
    {
        bail!("injected terminal SaveTransaction crash after {:?}", phase);
    }
    Ok(())
}

pub fn run_finalize(dir: &Path, command: FinalizeCommands, json: bool) -> Result<()> {
    let store = FinalizationStore::open(dir)?;
    match command {
        FinalizeCommands::Begin { id, ttl_seconds } => {
            let lease = begin_finish(dir, &store, &id, ttl_seconds, None)?;
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
                None,
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
                    worksgood::lifecycle::TransitionKind::EvaluationEvidence {
                        evidence_ref: waiver_id.clone(),
                    },
                    worksgood::lifecycle::LifecycleActor {
                        kind: worksgood::lifecycle::ActorKind::AcceptanceController,
                        id: actor.clone(),
                    },
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
            commit_terminal_success(
                dir,
                &candidate.task_id,
                None,
                "required_flip_operator_waiver_graphsave",
            )?;
            println!(
                "Waived required FLIP: waiver={} candidate={} report={} operator={} (audited; exact candidate merged)",
                waiver_id, candidate.candidate_id, report, actor
            );
            Ok(())
        }
        CandidateCommands::RecoverControlPlane { yes } => {
            if std::env::var_os("WG_AGENT_ID").is_some() {
                bail!(
                    "control-plane.recovery_operator_only: workers cannot rewrite repository history"
                );
            }
            let project = dir.parent().unwrap_or(dir);
            match worksgood::control_plane::recover_tracked_control_plane(project, yes)? {
                Some(receipt) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&receipt)?);
                    } else {
                        println!(
                            "Recovered tracked control plane without touching live bytes: {} -> {} ref={} removed=[{}] snapshot={}",
                            receipt.old_commit,
                            receipt.new_commit,
                            receipt.target_ref,
                            receipt.removed_paths.join(", "),
                            receipt.snapshot_receipt
                        );
                    }
                }
                None => println!("Control plane is clean: no protected Git entries"),
            }
            Ok(())
        }
    }
}

fn finish_context(
    dir: &Path,
    id: &str,
    worktree_override: Option<&Path>,
) -> Result<FinalizationContext> {
    // A brokered caller has already authenticated the exact task/agent/root
    // tuple and passes that retained worktree explicitly. Only direct worker
    // calls use process environment as their ownership proof.
    if worktree_override.is_none()
        && std::env::var_os("WG_AGENT_ID").is_some()
        && std::env::var("WG_TASK_ID").as_deref() != Ok(id)
    {
        bail!("finish.source_owner_mismatch: a worker may finish only its own task");
    }
    context_from_current(
        dir,
        id,
        worktree_override.map(Path::to_path_buf),
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
    worktree_override: Option<&Path>,
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
    let ctx = finish_context(dir, id, worktree_override)?;
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
    worktree_override: Option<&Path>,
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
    let mut ctx = finish_context(dir, id, worktree_override)?;
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
                worksgood::graph::CompletionContract::Explore => {
                    bail!(
                        "legacy finalization does not support Explore; use the immutable manifest review/publication protocol"
                    )
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

/// Converge durable finish handoffs after the exact attempt-owning wrapper has
/// exited.  This is deliberately bounded and semantic-neutral:
///
/// * a sealed/validated transaction may advance only by its exact receipts;
/// * a promoted/output transaction may perform cleanup only;
/// * an exited Pi owner with no transaction is fenced and reopened on the same
///   session/worktree so the agent, not process silence, supplies semantics.
///
/// Replaying this function after a crash is idempotent at every boundary.
pub(crate) fn converge_exited_worker_finishes(dir: &Path) -> Result<Vec<String>> {
    let store = FinalizationStore::open(dir)?;
    let registry = worksgood::service::AgentRegistry::load(dir)
        .unwrap_or_else(|_| worksgood::service::AgentRegistry::new());
    let graph_path = dir.join("graph.jsonl");
    let mut converged = Vec::new();

    for original in store.list()? {
        let graph = load_graph(&graph_path)?;
        let Some(task) = graph.get_task(&original.task_id) else {
            continue;
        };
        if task.lifecycle.audit.iter().any(|event| {
            event.generation == task.lifecycle.generation
                && event.reason_code == "failed_prerequisite_needs_reconciliation"
        }) {
            // The typed failed-prerequisite planner exhausted its finite
            // automatic finish budget. Retain the exact transaction and wait
            // for the actionable operator reconciliation path; polling must
            // not bypass the planner and loop the finish adapter forever.
            continue;
        }
        let owner_id = original
            .candidate
            .as_ref()
            .map(|candidate| candidate.worktree_id.as_str())
            .or_else(|| {
                original
                    .rescue
                    .as_ref()
                    .map(|rescue| rescue.worktree_id.as_str())
            })
            .or(task.assigned.as_deref())
            .unwrap_or("retained");
        if owner_is_live(&registry, &original.task_id, owner_id) {
            continue;
        }
        let source_is_current = original.generation == task.lifecycle.generation
            && original.attempt_id
                == task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str())
                    .unwrap_or_default()
            && original.attempt_fence == task.lifecycle.fence;
        if !source_is_current && task.status != worksgood::graph::Status::Done {
            continue;
        }

        let _ = worksgood::finalization::reconcile(&store, &original.task_id)?;
        let mut tx = store
            .load_task(&original.task_id)?
            .context("finish transaction disappeared during convergence")?;
        if matches!(
            tx.phase,
            worksgood::finalization::FinalizationPhase::Promoted
                | worksgood::finalization::FinalizationPhase::Delivered
                | worksgood::finalization::FinalizationPhase::Reported
                | worksgood::finalization::FinalizationPhase::Cleaning
                | worksgood::finalization::FinalizationPhase::Cleaned
        ) {
            cleanup_finish(dir, &store, &tx.task_id, false)?;
            converged.push(format!("{}:cleanup", tx.task_id));
            continue;
        }
        if !source_is_current {
            continue;
        }
        let Some(candidate) = tx.candidate.clone() else {
            continue;
        };

        if tx.evaluation_receipt.is_none()
            && candidate.evaluation_policy == "none"
            && matches!(
                tx.phase,
                worksgood::finalization::FinalizationPhase::CandidateCheckpointed
                    | worksgood::finalization::FinalizationPhase::MergePending
            )
        {
            worksgood::finalization::record_evaluation_receipt(
                &store,
                &candidate.candidate_id,
                worksgood::finalization::EvaluationReceiptOutcome::Accepted,
                "evaluation-not-required",
                "exited-owner-convergence",
            )?;
            tx = store
                .load_task(&tx.task_id)?
                .context("finish transaction disappeared after evaluation receipt")?;
        }

        match task.completion_contract {
            worksgood::graph::CompletionContract::Land
                if tx.phase == worksgood::finalization::FinalizationPhase::MergePending
                    && tx.evaluation_receipt.as_ref().is_some_and(|receipt| {
                        receipt.outcome
                            == worksgood::finalization::EvaluationReceiptOutcome::Accepted
                    }) =>
            {
                let ctx = context_from_transaction(&tx, &candidate);
                let lease = worksgood::finalization::acquire_finish_lease(&store, &ctx, 1800)?;
                worksgood::finalization::promote_task_owned_candidate(
                    &store,
                    &ctx,
                    &lease.lease_id,
                    &candidate.candidate_id,
                )?;
                cleanup_finish(dir, &store, &tx.task_id, false)?;
                converged.push(format!("{}:promote+cleanup", tx.task_id));
            }
            worksgood::graph::CompletionContract::Deliver
                if tx.validation.as_ref().is_some_and(|receipt| receipt.passed) =>
            {
                worksgood::finalization::publish_output(
                    &store,
                    &tx.task_id,
                    worksgood::finalization::OutputDisposition::Delivered,
                )?;
                cleanup_finish(dir, &store, &tx.task_id, false)?;
                converged.push(format!("{}:deliver+cleanup", tx.task_id));
            }
            worksgood::graph::CompletionContract::Report
                if tx.validation.as_ref().is_some_and(|receipt| receipt.passed) =>
            {
                worksgood::finalization::publish_output(
                    &store,
                    &tx.task_id,
                    worksgood::finalization::OutputDisposition::Reported,
                )?;
                cleanup_finish(dir, &store, &tx.task_id, false)?;
                converged.push(format!("{}:report+cleanup", tx.task_id));
            }
            _ => {}
        }
    }

    // No transaction means there is no durable semantic completion to replay.
    // Fence the dead tuple once and continue the exact session/worktree instead.
    let graph = load_graph(&graph_path)?;
    let candidates = graph
        .tasks()
        .filter(|task| task.status == worksgood::graph::Status::InProgress)
        .filter(|task| task.lifecycle.pi_continuation.is_some())
        .filter(|task| store.load_task(&task.id).ok().flatten().is_none())
        // A brokered completion/v2 terminal intent is already durable
        // authority. Even before its legacy mechanics produce a candidate,
        // process exit must not be reinterpreted as permission to respawn the
        // source and race the exact SaveTransaction.
        .filter(|task| {
            worksgood::worker_control::save_transaction_for_task(dir, task)
                .ok()
                .flatten()
                .is_none()
        })
        .filter_map(|task| {
            let attempt = task.lifecycle.current_attempt.as_ref()?;
            let key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
            let state_path =
                worksgood::attempt_runtime::resolve_component(dir, &key, "pi/state.json")
                    .ok()??;
            let watchdog = worksgood::pi_watchdog::PiWatchdog::open(&state_path).ok()?;
            let state = watchdog.state();
            let wrapper = state.terminal_wrapper.as_ref()?;
            let handoff = state.completion_handoff.as_ref();
            let presented = worksgood::service::WrapperChildCapability {
                task_id: handoff
                    .map(|value| value.source.task_id.clone())
                    .unwrap_or_else(|| state.source.task_id.clone()),
                generation: handoff
                    .map(|value| value.source.generation)
                    .unwrap_or(state.source.generation),
                attempt_id: handoff
                    .map(|value| value.source.attempt_id.clone())
                    .unwrap_or_else(|| state.source.attempt_id.clone()),
                fence: handoff
                    .map(|value| value.source.attempt_fence)
                    .unwrap_or(state.source.attempt_fence),
                wrapper_epoch: handoff
                    .map(|value| value.process_epoch)
                    .unwrap_or(state.process_epoch),
                child_epoch: handoff
                    .map(|value| value.process_epoch)
                    .unwrap_or(state.process_epoch),
                wrapper_identity_digest: handoff
                    .and_then(|value| value.terminal_wrapper_identity_digest.clone())
                    .unwrap_or_else(|| wrapper.digest()),
                child_identity_digest: handoff
                    .map(|value| value.process_identity_digest.clone())
                    .unwrap_or_else(|| state.process.digest()),
                // `terminal_wrapper` is persisted only after bootstrap proved
                // the native process was this exact wrapper's direct child.
                owned_child: true,
            };
            let authoritative = worksgood::service::WrapperChildCapability {
                task_id: task.id.clone(),
                generation: task.lifecycle.generation,
                attempt_id: attempt.id.clone(),
                fence: task.lifecycle.fence,
                wrapper_epoch: task.lifecycle.pi_process_epoch,
                child_epoch: task.lifecycle.pi_process_epoch,
                wrapper_identity_digest: wrapper.digest(),
                child_identity_digest: task.lifecycle.pi_process_identity_digest.clone(),
                owned_child: true,
            };
            let owner_dead = task
                .assigned
                .as_deref()
                .is_none_or(|owner| !owner_is_live(&registry, &task.id, owner));
            let decision = worksgood::service::reduce_exited_worker_finish(
                &worksgood::service::FinishConvergenceSnapshot {
                    presented_capability: presented,
                    authoritative_capability: authoritative,
                    owner_proven_dead: owner_dead && !exact_process_is_live(&state.process),
                    completion_receipted: handoff.is_some(),
                    transaction_phase: None,
                    now_unix: chrono::Utc::now().timestamp(),
                },
            );
            (decision.pending_action
                == worksgood::service::FinishConvergenceAction::ResumeSameSession
                && decision.deadline_unix.is_some()
                && state.source.worktree_path.exists()
                && state.session.session_file.exists())
            .then(|| {
                (
                    task.id.clone(),
                    state.session.session_id.clone(),
                    state.source.attempt_id.clone(),
                    state.source.attempt_fence,
                )
            })
        })
        .collect::<Vec<_>>();
    drop(graph);

    for (task_id, session_id, attempt_id, source_fence) in candidates {
        let mut requested = false;
        let session = session_id.clone();
        modify_graph(&graph_path, |graph| {
            let Some(task) = graph.get_task_mut(&task_id) else {
                return false;
            };
            if task.status != worksgood::graph::Status::InProgress
                || task.lifecycle.fence != source_fence
                || task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_none_or(|attempt| attempt.id != attempt_id)
            {
                return false;
            }
            task.session_id = Some(session.clone());
            match super::reopen::request(
                task,
                "exited-worker-convergence",
                false,
                true,
                "resume exact exited-worker session/worktree",
                worksgood::lifecycle::LifecycleActor {
                    kind: worksgood::lifecycle::ActorKind::Reconciler,
                    id: "exited-worker-finish-convergence".into(),
                },
                "proven_dead_owner_resume_same_session",
            ) {
                Ok((_, created)) => {
                    requested = created;
                    if created {
                        task.log.push(worksgood::graph::LogEntry {
                            timestamp: chrono::Utc::now().to_rfc3339(),
                            actor: Some("exited-worker-finish-convergence".into()),
                            user: Some(worksgood::current_user()),
                            message: format!(
                                "Exact owner exited without a finish transaction; fenced once and scheduled same-session/worktree continuation (session={session_id})"
                            ),
                        });
                    }
                    created
                }
                Err(_) => false,
            }
        })?;
        if requested {
            converged.push(format!("{task_id}:resume-same-session"));
        }
    }
    Ok(converged)
}

fn context_from_transaction(
    tx: &worksgood::finalization::FinalizationTransaction,
    candidate: &worksgood::finalization::CandidateDescriptor,
) -> FinalizationContext {
    FinalizationContext {
        task_id: tx.task_id.clone(),
        generation: tx.generation,
        attempt_id: tx.attempt_id.clone(),
        attempt_fence: tx.attempt_fence,
        process_epoch: candidate.process_epoch,
        worktree_id: candidate.worktree_id.clone(),
        worktree_lease_epoch: tx.worktree_lease_epoch,
        worktree_path: tx.worktree_path.clone(),
        project_root: tx.project_root.clone(),
        terminal_reservation_id: tx.terminal_reservation_id.clone(),
        evaluation_policy: candidate.evaluation_policy.clone(),
        route_snapshot_cid: candidate.route_snapshot_cid.clone(),
        quiescence: tx.quiescence.clone(),
    }
}

fn owner_is_live(
    registry: &worksgood::service::AgentRegistry,
    task_id: &str,
    owner_id: &str,
) -> bool {
    let Some(owner) = registry.get_agent(owner_id) else {
        return false;
    };
    if owner.task_id != task_id || !worksgood::service::is_process_alive(owner.pid) {
        return false;
    }
    owner
        .started_at
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map(|started| worksgood::service::verify_process_identity(owner.pid, started.timestamp()))
        .unwrap_or(true)
}

fn exact_process_is_live(process: &worksgood::pi_watchdog::ProcessIdentity) -> bool {
    if !worksgood::service::is_process_alive(process.pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let boot_matches = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .is_some_and(|value| value.trim() == process.boot_id);
        boot_matches
            && worksgood::service::read_proc_start_ticks(process.pid) == Some(process.start_ticks)
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

pub(crate) fn cleanup_finish(
    dir: &Path,
    store: &FinalizationStore,
    id: &str,
    json: bool,
) -> Result<()> {
    let tx = store.load_task(id)?.context("finish transaction missing")?;
    if let Some(receipt) = tx.cleanup_receipt.as_ref() {
        // The cleanup receipt may have won its durable-file race immediately
        // before a late wrapper/provider failure won graph.jsonl.  Replay must
        // therefore converge the exact transaction even when cleanup itself is
        // already an idempotent no-op.
        project_cleaned_success(dir, &tx)?;
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
    let cleaned = store
        .load_task(id)?
        .context("finish transaction disappeared after cleanup receipt")?;
    project_cleaned_success(dir, &cleaned)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!("Completed({}) cleanup={}", disposition, receipt.receipt_id);
    }
    Ok(())
}

fn load_persisted_graph_save(dir: &Path, state: &SaveTransactionState) -> Result<GraphSaveBundle> {
    let graph_save_cid = state
        .graph_save_cid
        .as_deref()
        .context("completion.bridge_graph_save_cid_missing")?;
    let projected = completion_root(dir)
        .join("graph-saves")
        .join(&state.source.task_id)
        .join(format!("{}.json", state.source.generation));
    let mut candidates = vec![projected];
    let objects = completion_root(dir).join("objects");
    if objects.exists() {
        for entry in std::fs::read_dir(&objects)? {
            candidates.push(entry?.path());
        }
    }
    for path in candidates {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(bundle) = serde_json::from_slice::<GraphSaveBundle>(&bytes) else {
            continue;
        };
        if content_cid(&bundle.receipt).map_err(anyhow::Error::msg)? != graph_save_cid {
            continue;
        }
        let verified = worksgood::completion_evidence::verify_graph_save_bundle(&bundle)
            .map_err(anyhow::Error::msg)?;
        if verified.binding.source != state.source {
            bail!("completion.bridge_graph_save_source_mismatch");
        }
        return Ok(bundle);
    }
    bail!(
        "completion.bridge_graph_save_bundle_missing: no immutable bundle for receipt {graph_save_cid}"
    )
}

fn commit_brokered_cleaned_success(
    dir: &Path,
    task: &Task,
    legacy: &worksgood::finalization::FinalizationTransaction,
    initial: SaveTransactionState,
) -> Result<String> {
    let accounting = completion_accounting(dir, task);
    let mut state = worksgood::worker_control::load_save_transaction(dir, &initial.transaction_id)?
        .context("completion.bridge_transaction_missing")?;
    let bundle = if state.phase == SavePhase::GraphSaved {
        load_persisted_graph_save(dir, &state)?
    } else {
        prepare_graph_save_for_source(
            dir,
            task,
            "brokered_done_exact_receipts",
            initial.source.clone(),
            Some(legacy),
            false,
        )?
        .0
    };
    for cid in [
        worksgood::worker_control::store_completion_object(dir, &bundle.work_save)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.candidate)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.validation)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.flip)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.disposition)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.effect)?,
        worksgood::worker_control::store_completion_object(dir, &bundle.cleanup)?,
    ] {
        if cid.trim().is_empty() {
            bail!("completion.bridge_object_cid_missing");
        }
    }
    let binding = bundle.receipt.binding.clone();
    let phases = [
        (
            SavePhase::Quiescing,
            bundle.work_save.quiescence_receipt_cid.clone(),
            false,
        ),
        (
            SavePhase::WorkSaved,
            bundle.receipt.evidence.work_save.clone(),
            true,
        ),
        (
            SavePhase::CandidateSealed,
            bundle.receipt.evidence.candidate.clone(),
            true,
        ),
        (
            SavePhase::Validated,
            bundle.receipt.evidence.validation.clone(),
            true,
        ),
        (
            SavePhase::Accepted,
            bundle.receipt.evidence.flip.clone(),
            true,
        ),
        (
            SavePhase::DispositionRecorded,
            bundle.receipt.evidence.disposition.clone(),
            true,
        ),
        (
            SavePhase::EffectPrepared,
            format!("effect-plan:{}", binding.candidate_id),
            true,
        ),
        (
            SavePhase::EffectCommitted,
            bundle.receipt.evidence.effect.clone(),
            true,
        ),
        (
            SavePhase::CleanupPrepared,
            bundle.cleanup.cleanup_plan_cid.clone(),
            true,
        ),
        (
            SavePhase::CleanupCommitted,
            bundle.receipt.evidence.cleanup.clone(),
            true,
        ),
    ];
    for (phase, cid, bound) in phases {
        if state.phase >= phase {
            continue;
        }
        state = worksgood::worker_control::commit_save_transition(
            dir,
            SaveTransitionRequest {
                source: state.source.clone(),
                expected_revision: state.revision,
                expected_phase: state.phase,
                next_phase: phase,
                idempotency_key: format!("bridge:{}:{phase:?}", state.transaction_id),
                action_key: format!("bridge-action:{}:{phase:?}", state.transaction_id),
                fact: SaveFact::Evidence {
                    cid,
                    binding: bound.then(|| binding.clone()),
                },
            },
        )?;
    }
    if state.phase != SavePhase::GraphSaved {
        // Persist the immutable bundle before the GraphSaved journal edge. A
        // crash after the kernel commit must be able to replay projection from
        // the exact receipt without rebuilding ambient Git/evaluation state.
        worksgood::worker_control::store_completion_object(dir, &bundle)?;
        persist_graph_save(dir, &task.id, task.lifecycle.generation, &bundle)?;
        let expected_graph_save_cid = content_cid(&bundle.receipt).map_err(anyhow::Error::msg)?;
        state = worksgood::worker_control::commit_save_transition(
            dir,
            SaveTransitionRequest {
                source: state.source.clone(),
                expected_revision: state.revision,
                expected_phase: state.phase,
                next_phase: SavePhase::GraphSaved,
                idempotency_key: format!("bridge:{}:graph-save", state.transaction_id),
                action_key: format!("bridge-action:{}:graph-save", state.transaction_id),
                fact: SaveFact::GraphSave {
                    bundle: Box::new(bundle.clone()),
                },
            },
        )?;
        if state.graph_save_cid.as_deref() != Some(expected_graph_save_cid.as_str()) {
            bail!("completion.bridge_graph_save_receipt_mismatch");
        }
    }
    let graph_save_cid = state
        .graph_save_cid
        .clone()
        .context("completion.bridge_graph_save_cid_missing")?;
    // Idempotently repair the rebuildable graph-save projection for older
    // crash cuts that journaled GraphSaved before this ordering was installed.
    persist_graph_save(dir, &task.id, task.lifecycle.generation, &bundle)?;
    let mut rejection = None;
    modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&legacy.task_id) else {
            rejection = Some("terminal task disappeared".to_string());
            return false;
        };
        if task.status == worksgood::graph::Status::Done {
            return false;
        }
        let request = TransitionRequest {
            event_id: bundle.receipt.lifecycle_event_id.clone(),
            idempotency_key: format!("graphsave:{}", state.transaction_id),
            actor: LifecycleActor {
                kind: worksgood::lifecycle::ActorKind::Finalizer,
                id: "brokered-completion-bridge".to_string(),
            },
            reason_code: "brokered_done_exact_receipts".to_string(),
            kind: TransitionKind::GraphSaveCommitted {
                bundle: Box::new(bundle.clone()),
            },
            expected: FenceExpectation::current(task),
            evidence_refs: vec![graph_save_cid.clone()],
            occurred_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error.to_string());
            return false;
        }
        apply_completion_accounting(task, &accounting);
        task.completed_at = Some(chrono::Utc::now().to_rfc3339());
        task.assigned = None;
        true
    })?;
    if let Some(error) = rejection {
        bail!("completion.bridge_graph_save_refused: {error}");
    }
    Ok(graph_save_cid)
}

fn project_cleaned_success(
    dir: &Path,
    tx: &worksgood::finalization::FinalizationTransaction,
) -> Result<()> {
    let current = load_graph(dir.join("graph.jsonl"))?;
    if let Some(task) = current.get_task(&tx.task_id)
        && let Some(brokered) = worksgood::worker_control::save_transaction_for_task(dir, task)?
        && brokered.phase != SavePhase::GraphSaved
    {
        tx.exact_durable_success(
            &task.id,
            task.lifecycle.generation,
            task.lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str()),
            task.lifecycle.fence,
        )
        .filter(|evidence| evidence.cleanup_receipt_id.is_some())
        .context("brokered completion lacks exact cleaned legacy receipts")?;
        commit_brokered_cleaned_success(dir, task, tx, brokered)?;
        return Ok(());
    }
    if let Some(task) = current
        .get_task(&tx.task_id)
        .filter(|task| task.status != worksgood::graph::Status::Done)
    {
        let evidence = tx
            .exact_durable_success(
                &task.id,
                task.lifecycle.generation,
                task.lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|value| value.id.as_str()),
                task.lifecycle.fence,
            )
            .filter(|value| value.cleanup_receipt_id.is_some())
            .context("cleaned transaction is not bound to the exact current source tuple")?;
        let contract = match evidence.disposition {
            "landed" => CompletionContract::Land,
            "delivered" => CompletionContract::Deliver,
            _ => CompletionContract::Report,
        };
        if task.completion_contract != contract {
            modify_graph(dir.join("graph.jsonl"), |graph| {
                let Some(task) = graph.get_task_mut(&tx.task_id) else {
                    return false;
                };
                task.completion_contract = contract;
                true
            })?;
        }
        let late_failure = (task.status == worksgood::graph::Status::Failed).then(|| {
            task.failure_reason
                .clone()
                .unwrap_or_else(|| "late worker/process exit".into())
        });
        if let Some(brokered) = worksgood::worker_control::save_transaction_for_task(dir, task)? {
            commit_brokered_cleaned_success(dir, task, tx, brokered)?;
        } else {
            commit_terminal_success(
                dir,
                &tx.task_id,
                None,
                "completion_cleanup_graphsave_committed",
            )?;
        }
        if let Some(diagnostic) = late_failure {
            modify_graph(dir.join("graph.jsonl"), |graph| {
                let Some(task) = graph.get_task_mut(&tx.task_id) else {
                    return false;
                };
                task.retry_count = task.retry_count.saturating_sub(1);
                task.failure_class = None;
                task.failure_reason = None;
                task.failure_signal = None;
                task.log.push(worksgood::graph::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    actor: Some("durable-success-convergence".into()),
                    user: Some(worksgood::current_user()),
                    message: format!(
                        "Durable task-owned finish took terminal precedence; retained late process diagnostic without lifecycle authority: {diagnostic}"
                    ),
                });
                true
            })?;
        }
        return Ok(());
    }
    let cleanup_receipt = tx
        .cleanup_receipt
        .as_ref()
        .context("cleanup receipt missing from cleaned transaction")?;
    let mut transition_error = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&tx.task_id) else {
            transition_error = Some("task disappeared after cleanup".into());
            return false;
        };
        let evidence = tx.exact_durable_success(
            &task.id,
            task.lifecycle.generation,
            task.lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str()),
            task.lifecycle.fence,
        );
        let Some(evidence) = evidence.filter(|value| value.cleanup_receipt_id.is_some()) else {
            if task.status != worksgood::graph::Status::Done {
                transition_error = Some(
                    "cleaned transaction is not bound to the exact current task/generation/attempt/fence"
                        .into(),
                );
            }
            // Historical Done rows without an attempt tuple remain untouched:
            // their old cleanup command stays idempotent, but the receipt is
            // not silently upgraded into new success authority.
            return false;
        };

        if task.status != worksgood::graph::Status::Done {
            let failure_won_graph_race = task.status == worksgood::graph::Status::Failed
                || task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.disposition.is_some());
            let kind = worksgood::lifecycle::TransitionKind::DurableSuccessProjected {
                acceptance_ref: cleanup_receipt.receipt_id.clone(),
            };
            let actor_kind = if failure_won_graph_race {
                worksgood::lifecycle::ActorKind::Reconciler
            } else {
                worksgood::lifecycle::ActorKind::ProcessObserver
            };
            let mut request = worksgood::lifecycle::TransitionRequest::new(
                kind,
                worksgood::lifecycle::LifecycleActor {
                    kind: actor_kind,
                    id: "task-wrapper-cleanup".into(),
                },
                if failure_won_graph_race {
                    "durable_success_precedes_late_process_failure"
                } else {
                    "completion_cleanup_committed"
                },
                format!(
                    "finish-cleanup:{}:{}",
                    tx.task_id, cleanup_receipt.receipt_id
                ),
            )
            .with_evidence(evidence.durable_receipt_id.clone())
            .with_evidence(cleanup_receipt.receipt_id.clone());
            request.expected = worksgood::lifecycle::FenceExpectation::current(task);
            if let Err(value) = worksgood::lifecycle::apply_transition(task, request) {
                transition_error = Some(value.to_string());
                return false;
            }
            if failure_won_graph_race {
                let diagnostic = task
                    .failure_reason
                    .as_deref()
                    .unwrap_or("late worker/process exit");
                task.log.push(worksgood::graph::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    actor: Some("durable-success-convergence".into()),
                    user: Some(worksgood::current_user()),
                    message: format!(
                        "Durable task-owned finish took terminal precedence; retained late process diagnostic without lifecycle authority: {diagnostic}"
                    ),
                });
                task.retry_count = task.retry_count.saturating_sub(1);
            }
        }
        let disposition = match evidence.disposition {
            "landed" => worksgood::graph::CompletionDisposition::Landed,
            "delivered" => worksgood::graph::CompletionDisposition::Delivered,
            _ => worksgood::graph::CompletionDisposition::Reported,
        };
        let already_projected = task.status == worksgood::graph::Status::Done
            && task.completion_disposition.as_ref() == Some(&disposition)
            && task.completion_receipt.as_deref() == Some(cleanup_receipt.receipt_id.as_str())
            && task.completed_at.is_some()
            && task.failure_class.is_none()
            && task.failure_reason.is_none()
            && task.failure_signal.is_none()
            && task.assigned.is_none();
        if already_projected {
            // Cleanup convergence is polled repeatedly by the daemon. Once the
            // exact durable receipt is projected, the poll must be a true
            // no-op: rewriting `completed_at`/`last_interaction_at` on every
            // tick makes terminal tasks appear to run again and continuously
            // reorders the TUI.
            return false;
        }
        task.completion_disposition = Some(disposition);
        task.completion_receipt = Some(cleanup_receipt.receipt_id.clone());
        if task.completed_at.is_none() {
            task.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        task.failure_class = None;
        task.failure_reason = None;
        task.failure_signal = None;
        task.assigned = None;
        true
    })?;
    if let Some(error) = transition_error {
        bail!("cleanup.status_receipt_failed: {error}");
    }
    Ok(())
}

pub fn set_contract(dir: &Path, id: &str, value: &str) -> Result<()> {
    let contract = match value {
        "land" => worksgood::graph::CompletionContract::Land,
        "report" => worksgood::graph::CompletionContract::Report,
        "explore" => worksgood::graph::CompletionContract::Explore,
        "deliver" => bail!(
            "the deliver contract is historical-only; new work must use land, report, or explore"
        ),
        _ => bail!("completion contract must be land, report, or explore"),
    };
    let mut refusal = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some(format!("task '{id}' not found"));
            return false;
        };
        if task.status != worksgood::graph::Status::Open || task.assigned.is_some() {
            refusal = Some(format!(
                "task '{id}' must be open and unassigned before changing its completion contract"
            ));
            return false;
        }
        task.completion_contract = contract;
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
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

fn checkpoint_uncommitted_source_work(
    dir: &Path,
    id: &str,
    worktree_override: Option<&Path>,
) -> Result<()> {
    let worktree = worktree_override
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("WG_WORKTREE_PATH").map(PathBuf::from))
        .context("worktree path unavailable")?;
    let project = dir.parent().unwrap_or(dir);
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    let base = git(&project, &["merge-base", &head, "HEAD"])?;
    worksgood::control_plane::assert_worker_boundary(project, &worktree, &base, &head)?;
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
    // Inspect the resulting index as a second, unskippable boundary. This is
    // deliberately after `git add`: ignore/pathspec configuration is not
    // trusted to keep compatibility/case variants out of a commit.
    worksgood::control_plane::assert_index_has_no_control_plane(&worktree)?;
    let _ = Command::new("git")
        .args(["reset", "-q", "HEAD", "--", ".wg-cleanup-pending"])
        .current_dir(&worktree)
        .status();
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
pub fn task_owned_done(dir: &Path, id: &str, worktree_override: Option<&Path>) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    if task.status != worksgood::graph::Status::InProgress {
        return Ok(false);
    }
    let contract = task.completion_contract;
    let brokered_prepare = crate::commands::service::in_worker_control_operation()
        && worksgood::worker_control::save_transaction_for_task(dir, task)?.is_some();
    drop(graph);
    if !brokered_prepare {
        record_terminal_prepare(dir, id, "task-owned-done")?;
    }
    checkpoint_uncommitted_source_work(dir, id, worktree_override)?;
    let store = FinalizationStore::open(dir)?;
    let lease = if contract == worksgood::graph::CompletionContract::Land {
        Some(begin_finish(dir, &store, id, 1800, worktree_override)?.lease_id)
    } else {
        None
    };
    let tx = submit_finish(
        dir,
        &store,
        id,
        lease.as_deref(),
        Some("HEAD"),
        1800,
        worktree_override,
    )?;
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

/// Consume the exact brokered DoneHandoff after the wrapper has become
/// quiescent. The original operation bytes select the same validation/smoke
/// strength the worker requested; no ambient fallback or synthetic waiver is
/// admitted. Durable candidate mechanics remain in the existing task-owned
/// finalization adapter, whose receipts are bridged into completion/v2 after
/// cleanup.
pub(crate) fn settle_prepared_worker_done(
    dir: &Path,
    binding: &worksgood::worker_control::AttemptCapabilityBinding,
) -> Result<bool> {
    let id = &binding.task_id;
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(id)?;
    let Some(state) = worksgood::worker_control::save_transaction_for_task(dir, task)? else {
        return Ok(false);
    };
    if state.phase == SavePhase::GraphSaved {
        return Ok(true);
    }
    let prepared_cid = state
        .evidence_cids
        .get(&SavePhase::Prepared)
        .context("worker_control.prepared_done_evidence_missing")?;
    let operation: worksgood::worker_control::WorkerOperation =
        worksgood::worker_control::load_completion_object(dir, prepared_cid)?;
    let worksgood::worker_control::WorkerOperation::DoneHandoff {
        converged,
        full_smoke,
    } = operation
    else {
        bail!("worker_control.prepared_done_operation_mismatch");
    };
    drop(graph);
    super::done::run_from_worker_control(
        dir,
        id,
        converged,
        full_smoke,
        Path::new(&binding.worktree_path),
        &binding.agent_id,
    )?;
    Ok(true)
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

#[cfg(test)]
mod atomic_terminal_tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Status};
    use worksgood::lifecycle::AttemptRef;
    use worksgood::parser::save_graph;

    fn setup(status: Status) -> (tempfile::TempDir, PathBuf) {
        let root = tempdir().unwrap();
        let dir = root.path().join(".wg");
        std::fs::create_dir_all(&dir).unwrap();
        let mut graph = WorkGraph::new();
        let mut task = Task {
            id: "terminal".into(),
            title: "terminal adapter".into(),
            status,
            ..Task::default()
        };
        if status == Status::InProgress {
            task.lifecycle.fence = 1;
            task.lifecycle.attempt_sequence = 1;
            task.lifecycle.current_attempt = Some(AttemptRef {
                id: "test-attempt:terminal:0:1".into(),
                generation: 0,
                fence: 1,
                actor_id: "test".into(),
                disposition: None,
            });
        }
        graph.add_node(Node::Task(task));
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();
        (root, dir)
    }

    #[test]
    fn explicit_contracts_are_land_report_or_explore() {
        let (_root, dir) = setup(Status::Open);
        set_contract(&dir, "terminal", "explore").unwrap();
        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task("terminal").unwrap().completion_contract,
            CompletionContract::Explore
        );
    }

    #[test]
    fn new_legacy_deliver_contract_is_refused() {
        let (_root, dir) = setup(Status::Open);
        let error = set_contract(&dir, "terminal", "deliver").unwrap_err();
        assert!(error.to_string().contains("historical-only"));
        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task("terminal").unwrap().completion_contract,
            CompletionContract::Land
        );
    }

    #[test]
    fn terminal_adapter_commits_graphsave_and_transaction_head() {
        let (_root, dir) = setup(Status::InProgress);
        let cid = commit_terminal_success(&dir, "terminal", None, "test-terminal").unwrap();
        assert!(cid.starts_with("wgcid:v2:blake3:"));
        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("terminal").unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.completion_receipt.as_deref(), Some(cid.as_str()));
        let transactions = std::fs::read_dir(dir.join("completion/v2/transactions"))
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(transactions.len(), 1);
        let head: SaveTransactionState = serde_json::from_slice(
            &std::fs::read(transactions[0].path().join("head.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(head.phase, SavePhase::GraphSaved);
    }

    #[test]
    fn terminal_graphsave_persists_pi_usage_and_runtime_after_registry_cleanup() {
        let (_root, dir) = setup(Status::InProgress);
        let agent_dir = dir.join("agents/agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("output.log"), "").unwrap();
        std::fs::write(
            agent_dir.join("raw_stream.jsonl"),
            concat!(
                "{\"type\":\"turn_end\",\"message\":{\"usage\":{\"input\":200,\"output\":10,\"cacheRead\":50,\"cacheWrite\":3,\"cost\":{\"total\":0.02}}}}\n",
                "{\"type\":\"message_end\",\"message\":{\"usage\":{\"input\":200,\"output\":10,\"cacheRead\":50,\"cacheWrite\":3,\"cost\":{\"total\":0.02}}}}\n",
                "{\"type\":\"turn_end\",\"message\":{\"usage\":{\"input\":5,\"output\":7,\"cacheRead\":260,\"cacheWrite\":4,\"cost\":{\"total\":0.03}}}}\n"
            ),
        )
        .unwrap();
        let mut registry = AgentRegistry::new();
        let agent = registry.register_agent_with_model(
            std::process::id(),
            "terminal",
            "pi",
            agent_dir.join("output.log").to_str().unwrap(),
            Some("openrouter:test/pi-model"),
        );
        assert_eq!(agent, "agent-1");
        registry.save(&dir).unwrap();
        modify_graph(dir.join("graph.jsonl"), |graph| {
            graph.get_task_mut("terminal").unwrap().assigned = Some(agent.clone());
            true
        })
        .unwrap();

        commit_terminal_success(&dir, "terminal", Some(&agent), "pi-fixture-done").unwrap();
        std::fs::remove_file(AgentRegistry::registry_path(&dir)).unwrap();

        let graph = load_graph(dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("terminal").unwrap();
        let usage = task.token_usage.as_ref().expect("Pi usage persisted");
        assert_eq!(usage.input_tokens, 205);
        assert_eq!(usage.output_tokens, 17);
        assert_eq!(usage.cache_read_input_tokens, 310);
        assert_eq!(usage.cache_creation_input_tokens, 7);
        assert!((usage.cost_usd - 0.05).abs() < 0.000001);
        assert_eq!(task.actual_executor.as_deref(), Some("pi"));
        assert_eq!(
            task.actual_model.as_deref(),
            Some("openrouter:test/pi-model")
        );
    }

    #[test]
    fn terminal_adapter_records_abort_before_failure_projection() {
        let (_root, dir) = setup(Status::InProgress);
        record_terminal_abort(&dir, "terminal", "test failure").unwrap();
        let transaction = std::fs::read_dir(dir.join("completion/v2/transactions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let head: SaveTransactionState =
            serde_json::from_slice(&std::fs::read(transaction.path().join("head.json")).unwrap())
                .unwrap();
        assert_eq!(head.phase, SavePhase::AbortedPreserved);
        assert_eq!(
            load_graph(dir.join("graph.jsonl"))
                .unwrap()
                .get_task("terminal")
                .unwrap()
                .status,
            Status::InProgress
        );
    }
}
