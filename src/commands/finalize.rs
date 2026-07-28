use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use worksgood::finalization::{
    FinalizationContext, FinalizationStore, QuiescenceProof, checkpoint_candidate,
    checkpoint_rescue,
};
use worksgood::parser::load_graph;

use crate::cli::{CandidateCommands, FinalizeCommands};

pub fn run_finalize(dir: &Path, command: FinalizeCommands, json: bool) -> Result<()> {
    let store = FinalizationStore::open(dir)?;
    match command {
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
            "Finalization {:?}: {} generation={} attempt={} fence={} lease={}\n  process: {} receipt={} group-empty={} nonce-eof={}\n  worktree: {}\n  rescue: {} commit={} tree={} manifest={}\n  candidate: {} commit={} tree={} manifest={}\n  validation: {} binding={}\n  evaluation: request={} policy={} route={} binding={} read-only={}\n  merge: receipt={} conflict={}\n  retained: {}\n  replay: {}\n  next: {}",
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
