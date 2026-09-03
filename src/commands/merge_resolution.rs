use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use worksgood::merge_resolution::{ResolutionStore, RunOptions, run_task};

use crate::cli::MergeResolutionCommands;

pub fn run(dir: &Path, command: MergeResolutionCommands, json: bool) -> Result<()> {
    match command {
        MergeResolutionCommands::Run {
            id,
            adapter,
            integration_check,
            generated,
            generated_owned,
            ambiguous_intent,
        } => {
            let adapter = adapter
                .or_else(|| std::env::var_os("WG_STRONG_MERGER_ADAPTER").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/nonexistent/wg-strong-merger"));
            let record = run_task(
                dir,
                &id,
                RunOptions {
                    adapter: &adapter,
                    integration_check: integration_check.as_deref(),
                    generated,
                    generated_owned,
                    ambiguous_intent,
                },
            )?;
            print_record(&record, json)
        }
        MergeResolutionCommands::Status { id } => {
            // A task may have an older source merge-resolution record and a
            // newer finalizer reconciliation. Prefer the active landing record
            // so the operator sees the recovery action named by the blocker,
            // not stale guidance from the source-resolution phase.
            if crate::commands::completion_land::print_reconciliation_status(dir, &id, json)? {
                Ok(())
            } else if let Some(record) = ResolutionStore::open(dir)?.load_task(&id)? {
                print_record(&record, json)
            } else {
                bail!("no merge resolution or landing reconciliation record for '{id}'")
            }
        }
        MergeResolutionCommands::Inspect { id, materialize } => {
            let record = ResolutionStore::open(dir)?
                .load_task(&id)?
                .with_context(|| format!("no merge resolution record for '{id}'"))?;
            if let Some(to) = materialize {
                let descriptor = record
                    .descriptor
                    .as_ref()
                    .context("resolution descriptor not sealed")?;
                if to.exists() && std::fs::read_dir(&to)?.next().is_some() {
                    bail!("materialize target is not empty");
                }
                std::fs::create_dir_all(&to)?;
                let workspace = record
                    .workspace
                    .as_ref()
                    .context("integration workspace not retained")?;
                let archive = std::process::Command::new("git")
                    .args(["-C"])
                    .arg(&workspace.path)
                    .args(["archive", &descriptor.resolution_commit_oid])
                    .stdout(std::process::Stdio::piped())
                    .spawn()?;
                let status = std::process::Command::new("tar")
                    .args(["-x", "-C"])
                    .arg(&to)
                    .stdin(archive.stdout.context("archive pipe")?)
                    .status()?;
                if !status.success() {
                    bail!("materialization failed");
                }
                println!(
                    "Materialized immutable resolution {} to {}",
                    descriptor.resolution_candidate_id,
                    to.display()
                );
                Ok(())
            } else {
                print_record(&record, json)
            }
        }
        MergeResolutionCommands::Retry { id } => action_hold(
            dir,
            &id,
            "retry-same-route",
            "Retry requires a new audited run generation; the exact snapshotted route is retained and no fallback is permitted.",
        ),
        MergeResolutionCommands::Resume { id } => action_hold(
            dir,
            &id,
            "resume",
            "Resume creates a new content-bound generation and still traverses route, safety, validation, evaluation and CAS gates.",
        ),
        MergeResolutionCommands::ChangeRoute {
            id,
            route,
            reasoning,
        } => {
            if !matches!(reasoning.as_str(), "high" | "xhigh") {
                bail!("MR_ROUTE_REASONING_UNSUPPORTED: use high or xhigh");
            }
            if !route.contains(':') {
                bail!("MR_ROUTE_INVALID: exact handler-first route required");
            }
            action_hold(
                dir,
                &id,
                "change-route",
                &format!(
                    "Configure models.merger={route} reasoning={reasoning}; resumption creates a new audited route/run generation, never mutates the old run."
                ),
            )
        }
        MergeResolutionCommands::Decide {
            id,
            rationale,
            constraints,
        } => {
            let author = worksgood::current_user();
            let decision = ResolutionStore::open(dir)?.record_human_decision(
                &id,
                &author,
                &rationale,
                constraints.as_deref().unwrap_or(""),
            )?;
            println!(
                "Human decision {} recorded for classification={} candidate={} target={}/{} evidence={} author={} rationale-cid={} constraints-cid={}. Resume must create a new generation and run safety, validation, evaluation and CAS gates; approval cannot write main.",
                decision.decision_id,
                decision.classification_id,
                decision.candidate_id,
                decision.target_commit_oid,
                decision.target_tree_oid,
                decision.evidence_digest,
                decision.author,
                decision.rationale_cid,
                decision.constraints_cid
            );
            Ok(())
        }
        MergeResolutionCommands::Reject { id, reason } => action_hold(
            dir,
            &id,
            "reject",
            &format!(
                "Reject retains source and resolution descriptors. Reason cid={}",
                content_cid(reason.as_deref().unwrap_or("operator-reject"))
            ),
        ),
        MergeResolutionCommands::RefreshTarget { id } => {
            // Completion/v3 owns a real finalizer reconciliation path: it keeps
            // the selected candidate, snapshots the descendant target, reruns
            // target-dependent validation, emits renewed evidence, then CASes.
            // Historical resolution records retain their generation action.
            if crate::commands::resume::resume_landing_finalization(dir, &id)? {
                Ok(())
            } else {
                action_hold(
                    dir,
                    &id,
                    "refresh-target",
                    "A fresh immutable target snapshot must create a new classification; stale bytes and verdicts are never rebased or reused.",
                )
            }
        }
        MergeResolutionCommands::RepairSource { id } => action_hold(
            dir,
            &id,
            "repair-source",
            "Source repair creates a linked new immutable candidate version through lifecycle authority; the original candidate is retained.",
        ),
        MergeResolutionCommands::EscalateHuman { id } => action_hold(
            dir,
            &id,
            "escalate-human",
            "A human decision must bind candidate/target/evidence digests, named author, rationale and constraints; it cannot write main.",
        ),
        MergeResolutionCommands::Abort { id } => action_hold(
            dir,
            &id,
            "abort",
            "Abort retains all source-bearing evidence and leaves canonical main/source untouched; cleanup is ancillary.",
        ),
        MergeResolutionCommands::Rollback { receipt } => {
            println!(
                "Rollback {} requires a compensating immutable candidate through normal finalization, review, validation, evaluation, classification and central CAS. Hard reset and receipt deletion are forbidden.",
                receipt
            );
            Ok(())
        }
    }
}

fn action_hold(dir: &Path, id: &str, action: &str, detail: &str) -> Result<()> {
    let record = ResolutionStore::open(dir)?
        .load_task(id)?
        .context("merge resolution record missing")?;
    println!(
        "{} requested for {} classification={} target={} route={} (no fallback). {}",
        action,
        id,
        record.classification.classification_id,
        record.target.commit_oid,
        record
            .route
            .as_ref()
            .map(|r| r.exact_handler_first_spec.as_str())
            .unwrap_or("none"),
        detail
    );
    Ok(())
}

fn print_record(
    record: &worksgood::merge_resolution::MergeResolutionRecord,
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(record)?);
        return Ok(());
    }
    println!(
        "Merge resolution {:?}: {}\n  classifier: {:?} / {} evidence={}\n  candidate: {} base/target: {} tree={}\n  route: {} strength={} reasoning={} provenance={} (pinned, no fallback)\n  run: generation={} calls={} workspace={} isolated={}\n  resolution: {} tree={}\n  gates: safety={} validation={} evaluation={}\n  target drift: {}\n  merge receipt: {} result-tree={}\n  retained: {}\n  next: {}",
        record.state,
        record.task_id,
        record.classification.classification,
        record.classification.reason_code,
        record.classification.evidence_digest,
        record.source_candidate_id,
        record.target.commit_oid,
        record.target.tree_oid,
        record
            .route
            .as_ref()
            .map(|r| r.exact_handler_first_spec.as_str())
            .unwrap_or("none"),
        record
            .route
            .as_ref()
            .map(|r| format!("{:?}", r.declared_class))
            .unwrap_or_else(|| "none".into()),
        record
            .route
            .as_ref()
            .map(|r| r.reasoning.as_str())
            .unwrap_or("none"),
        record
            .route
            .as_ref()
            .map(|r| r.provenance.as_str())
            .unwrap_or("none"),
        record.run_generation,
        record.runner_invocations,
        record
            .workspace
            .as_ref()
            .map(|w| w.path.display().to_string())
            .unwrap_or_else(|| "none".into()),
        record.workspace.as_ref().is_some_and(|w| w.no_remote
            && w.shared_ref_probe_denied
            && w.push_probe_denied
            && w.graph_absent),
        record
            .descriptor
            .as_ref()
            .map(|d| d.resolution_candidate_id.as_str())
            .unwrap_or("none"),
        record
            .descriptor
            .as_ref()
            .map(|d| d.resolution_tree_oid.as_str())
            .unwrap_or("none"),
        record
            .gates
            .as_ref()
            .map(|g| g.safety_verdict.as_str())
            .unwrap_or("none"),
        record.gates.as_ref().is_some_and(|g| g.validation_passed),
        record.gates.as_ref().is_some_and(|g| g.evaluation_accepted),
        matches!(
            record.state,
            worksgood::merge_resolution::ResolutionState::Stale
        ),
        record
            .merge_receipt
            .as_ref()
            .map(|r| r.receipt_id.as_str())
            .unwrap_or("none"),
        record
            .merge_receipt
            .as_ref()
            .map(|r| r.result_tree_oid.as_str())
            .unwrap_or("none"),
        record.retained,
        record.safe_next_action
    );
    Ok(())
}

fn content_cid(s: &str) -> String {
    format!("wgcid:v1:blake3:{}", blake3::hash(s.as_bytes()).to_hex())
}
