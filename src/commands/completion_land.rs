use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{
    ArtifactOutput, ContentDigest, EvidenceRef, GitOutput, OutputRef, ReviewResolver,
};
use worksgood::completion_task::{
    load_exact_review_pair, load_review_evidence, load_submission_bytes,
};
use worksgood::completion_validation::{
    BASELINE_VALIDATION_EVIDENCE_KIND, CONFIGURED_VALIDATION_EVIDENCE_KIND, ValidationPurpose,
    capture_validation, configured_validation_commands, land_baseline_command,
};
use worksgood::config::Config;
use worksgood::graph::{
    CompletionBlocker, CompletionBlockerKind, CompletionContract, CompletionDisposition,
    LandingReconciliationState, LogEntry, Status,
};
use worksgood::identity::canonical_json;
use worksgood::parser::{load_graph, modify_graph};

use super::completion_submit::{collect_dependency_outputs, require_source_owner, store};

#[derive(Clone, Debug, Serialize)]
struct LandingReceipt {
    receipt_version: u32,
    task_id: String,
    generation: u64,
    manifest_digest: String,
    integration_ref: String,
    integrated_main_oid: String,
    accepted_commit_oid: String,
    observed_main_before: String,
    observed_main_after: String,
    already_published: bool,
    root_checkout_synchronized: bool,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LandingReconciliationReceipt {
    receipt_version: u32,
    task_id: String,
    generation: u64,
    attempt_id: Option<String>,
    fence: u64,
    manifest_digest: String,
    source_candidate_commit_oid: String,
    source_candidate_tree_oid: String,
    expected_target_oid: String,
    refreshed_target_oid: String,
    refreshed_target_tree_oid: String,
    integration_commit_oid: String,
    integration_tree_oid: String,
    validation_inputs_digest: String,
    validation_evidence: Vec<EvidenceRef>,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LandingReconciliationRecord {
    schema_version: u32,
    task_id: String,
    generation: u64,
    fence: u64,
    manifest_digest: String,
    source_candidate_commit_oid: String,
    expected_target_oid: String,
    observed_target_oid: String,
    state: LandingReconciliationState,
    integration_commit_oid: Option<String>,
    validation_inputs_digest: Option<String>,
    validation_evidence: Vec<EvidenceRef>,
    receipt_ref: Option<ArtifactOutput>,
    reason: String,
    safe_next: String,
    updated_at: String,
}

pub fn run(dir: &Path, id: &str, integration_ref: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("determine worker working directory")?;
    run_at(dir, id, integration_ref, Some(&cwd))
}

pub fn run_at(
    dir: &Path,
    id: &str,
    integration_ref: &str,
    worker_worktree: Option<&Path>,
) -> Result<()> {
    run_at_inner(dir, id, integration_ref, worker_worktree, None).map(|_| ())
}

/// Resume only the landing phase from an exact typed completion wait. No
/// source execution or model review is repeated.
pub(crate) fn pending_checkout_is_clean(dir: &Path, id: &str) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let blocker = task
        .completion_blocker
        .as_ref()
        .context("task has no pending completion finalization")?;
    if blocker.kind != CompletionBlockerKind::LandingPending || task.status != Status::Waiting {
        return Ok(false);
    }
    super::completion_wait::validate_current(task, blocker)?;
    if blocker.reconciliation_state == LandingReconciliationState::Blocked {
        return Ok(false);
    }
    let integration_ref = blocker
        .integration_ref
        .as_deref()
        .context("LandingPending has no integration ref")?;
    root_checkout_dirty_if_attached(
        dir.parent()
            .context("workgraph directory has no project root")?,
        integration_ref,
    )
    .map(|dirty| !dirty)
}

pub(crate) fn resume_pending(dir: &Path, id: &str) -> Result<bool> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    let blocker = task
        .completion_blocker
        .clone()
        .context("task has no pending completion finalization")?;
    if blocker.kind != CompletionBlockerKind::LandingPending || task.status != Status::Waiting {
        bail!("task '{id}' is not Waiting/LandingPending");
    }
    super::completion_wait::validate_current(task, &blocker)?;
    let integration_ref = blocker
        .integration_ref
        .as_deref()
        .context("LandingPending has no integration ref")?;
    let worker = blocker
        .worker_worktree
        .as_deref()
        .map(Path::new)
        .context("LandingPending has no retained worker worktree")?;
    run_at_inner(dir, id, integration_ref, Some(worker), Some(&blocker))
}

fn run_at_inner(
    dir: &Path,
    id: &str,
    integration_ref: &str,
    worker_worktree: Option<&Path>,
    pending: Option<&CompletionBlocker>,
) -> Result<bool> {
    validate_integration_ref(integration_ref)?;
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    if let Some(blocker) = pending {
        super::completion_wait::validate_current(task, blocker)?;
    } else {
        require_source_owner(task, id)?;
    }
    if task.completion_contract != CompletionContract::Land {
        bail!(
            "wg land applies only to Land tasks; '{}' is {}",
            id,
            task.completion_contract
        );
    }
    let completion_store = store(dir)?;
    let (submission, manifest, requirements, summary) =
        load_submission_bytes(&completion_store, task)?;
    let current_dependencies = collect_dependency_outputs(&completion_store, &graph, task)?;
    let selected_dependencies = task
        .completion_candidate
        .as_ref()
        .context("missing completion candidate")?
        .dependency_outputs
        .clone();
    if current_dependencies != selected_dependencies {
        bail!("dependency outputs changed after review; submit a new manifest");
    }
    let project_root = dir
        .parent()
        .context("workgraph directory has no project root")?;
    let resolved = ReviewResolver::new(&completion_store)
        .repository(project_root)
        .resolve_submission(
            &submission.manifest_ref,
            &requirements,
            &summary,
            &current_dependencies,
        )
        .map_err(|error| anyhow::anyhow!("completion evidence no longer resolves: {error}"))?;
    let config = Config::load_merged(dir)?;
    if config.agency.completion_review_strict {
        load_exact_review_pair(&completion_store, &submission, &manifest, &resolved)?;
    } else {
        let evidence = load_review_evidence(&completion_store, &submission, &manifest, &resolved)?;
        if evidence.flip.verdict != worksgood::simple_land::ReviewVerdict::Pass
            || evidence.eval.as_ref().is_some_and(|receipt| {
                receipt.verdict != worksgood::simple_land::ReviewVerdict::Pass
            })
        {
            eprintln!(
                "Advisory model review did not pass; deterministic publication continues. Inspect `wg show {id}` for findings."
            );
        }
    }
    let git_output = exact_git_output(&manifest.outputs)?;
    worksgood::control_plane::assert_tree_has_no_control_plane(
        project_root,
        &git_output.commit_oid,
    )?;

    // A source worker is no longer needed once the immutable candidate and
    // review receipts exist. User dirtiness is nevertheless authoritative:
    // park/retain before doing target reconciliation, and never hide it behind
    // WG's own registered runtime worktrees.
    let dirty = root_checkout_dirty_if_attached(project_root, integration_ref)?;
    if dirty {
        if pending.is_none() {
            let worker =
                worker_worktree.context("landing wait requires the retained worker worktree")?;
            super::completion_wait::park_landing_pending(
                dir,
                id,
                "attached integration checkout has tracked, index, or user-owned untracked changes; publication deferred without modifying user bytes",
                super::completion_wait::LandingWait {
                    integration_ref,
                    // Preserve the reviewed target base. `observed` may have
                    // advanced before user dirtiness was noticed; binding the
                    // blocker to it would make that new target look validated.
                    target_ref_oid: &git_output.integrated_main_oid,
                    worker_worktree: worker,
                },
            )?;
        }
        eprintln!(
            "Landing pending: attached integration checkout has user changes; preserve them, clean the checkout, then run `wg resume {id} --only`"
        );
        return Ok(false);
    }

    let _lock = LandingLock::acquire(project_root)?;
    if root_checkout_dirty_if_attached(project_root, integration_ref)? {
        eprintln!(
            "Landing pending: checkout changed while finalization was locking; preserve user bytes and run `wg resume {id} --only` after cleaning"
        );
        return Ok(false);
    }
    let observed_before = git(project_root, &["rev-parse", integration_ref])?;
    let candidate_reachable = is_ancestor(project_root, &git_output.commit_oid, &observed_before)?;
    // Ancestry alone is not refreshed validation authority. Exact candidate
    // publication is valid crash replay; a strict descendant adds target bytes
    // that the original baseline never saw and must enter reconciliation.
    if pending.is_none() && candidate_reachable && observed_before != git_output.commit_oid {
        if is_ancestor(
            project_root,
            &git_output.integrated_main_oid,
            &observed_before,
        )? {
            let worker = worker_worktree
                .context("strict-descendant target wait requires the retained worker worktree")?;
            super::completion_wait::park_landing_pending(
                dir,
                id,
                "integration ref contains the candidate plus unvalidated descendant bytes; target-dependent validation must be renewed",
                super::completion_wait::LandingWait {
                    integration_ref,
                    target_ref_oid: &git_output.integrated_main_oid,
                    worker_worktree: worker,
                },
            )?;
            eprintln!(
                "Landing pending: candidate-containing target advanced; run `wg resume {id} --only` to renew validation before completion"
            );
            return Ok(false);
        }
        bail!(
            "candidate is reachable only through a target that diverged from reviewed base {}; candidate retained",
            git_output.integrated_main_oid
        );
    }
    let mut already_published = candidate_reachable;
    if !already_published && pending.is_none() {
        let worker = worker_worktree.context(
            "initial landing requires the retained worker worktree; crash recovery may omit it after publication",
        )?;
        verify_worker_worktree(worker, &git_output.commit_oid)?;
    }

    let mut publication_commit = git_output.commit_oid.clone();
    if let Some(blocker) = pending {
        let expected = blocker
            .target_ref_oid
            .as_deref()
            .context("LandingPending has no target-ref CAS binding")?;
        let ready_commit = blocker.reconciled_commit_oid.as_deref().filter(|commit| {
            blocker.reconciliation_state == LandingReconciliationState::ReadyToLand
                && (*commit == observed_before || observed_before == expected)
        });
        if let Some(commit) = ready_commit {
            verify_ready_reconciliation(
                dir,
                task,
                blocker,
                &manifest,
                git_output,
                &observed_before,
                commit,
            )?;
            publication_commit = commit.to_string();
            already_published = is_ancestor(project_root, commit, &observed_before)?;
        } else if observed_before != expected
            || (candidate_reachable && observed_before != git_output.commit_oid)
        {
            if observed_before == expected || is_ancestor(project_root, expected, &observed_before)?
            {
                // Even when an operator has already made the source candidate
                // reachable, the changed target invalidates the old baseline.
                // Reconcile/validate it and mint renewed evidence rather than
                // treating ancestry alone as a waiver.
                publication_commit = reconcile_descendant_target(
                    dir,
                    task,
                    blocker,
                    &manifest,
                    git_output,
                    &current_dependencies,
                    integration_ref,
                    &observed_before,
                )?;
                already_published =
                    is_ancestor(project_root, &publication_commit, &observed_before)?;
            } else {
                record_reconciliation_blocked(
                    dir,
                    task,
                    blocker,
                    &manifest,
                    git_output,
                    &observed_before,
                    "target diverged from the exact pending expectation",
                )?;
                bail!(
                    "landing reconciliation refused: target {} is not a descendant of expected {}; candidate bytes remain immutable. No automated mutation is authorized for this divergence; inspect the auditable blocker with `wg merge-resolution status {id}`",
                    observed_before,
                    expected
                );
            }
        }
    } else if !already_published && observed_before != git_output.integrated_main_oid {
        if is_ancestor(
            project_root,
            &git_output.integrated_main_oid,
            &observed_before,
        )? {
            let worker = worker_worktree
                .context("stale target wait requires the retained worker worktree")?;
            super::completion_wait::park_landing_pending(
                dir,
                id,
                "finalizer target expectation advanced after review; immutable candidate retained for target reconciliation",
                super::completion_wait::LandingWait {
                    integration_ref,
                    target_ref_oid: &git_output.integrated_main_oid,
                    worker_worktree: worker,
                },
            )?;
            eprintln!(
                "Landing pending: target advanced after review; run `wg resume {id} --only` to reconcile, refresh validation, and land without the source worker"
            );
            return Ok(false);
        }
        bail!(
            "landing target diverged from reviewed base {}; candidate retained. Supported recovery: inspect `wg show {id}`; do not reset history",
            git_output.integrated_main_oid
        );
    }

    let root_checkout_synchronized = if already_published {
        // If the integration ref already contains the candidate, a clean
        // attached checkout is already synchronized. A stale index/worktree
        // appears dirty above and is deferred rather than overwritten.
        symbolic_head(project_root).as_deref() == Some(integration_ref)
    } else {
        let publication_base = if publication_commit == git_output.commit_oid {
            &git_output.integrated_main_oid
        } else {
            &observed_before
        };
        if observed_before != *publication_base {
            bail!(
                "landing target fence changed before publication; candidate retained. Run `wg resume {id} --only` to reconcile the new descendant target"
            );
        }
        if !is_ancestor(project_root, publication_base, &publication_commit)? {
            bail!("refreshed publication commit is not a fast-forward of its target snapshot");
        }
        if symbolic_head(project_root).as_deref() == Some(integration_ref) {
            // `merge --ff-only` is Git's checked worktree/index update: it
            // protects local tracked, staged, and obstructing untracked bytes.
            // Unlike reset --hard it refuses rather than overwriting a user
            // race. Its ref transaction is locked against the observed HEAD.
            if let Err(error) = git(
                project_root,
                &["merge", "--ff-only", "--no-edit", &publication_commit],
            ) {
                if root_checkout_dirty_if_attached(project_root, integration_ref)? {
                    if pending.is_none() {
                        let worker = worker_worktree
                            .context("landing wait requires the retained worker worktree")?;
                        super::completion_wait::park_landing_pending(
                            dir,
                            id,
                            "attached integration checkout changed during publication; Git refused the update and preserved user bytes",
                            super::completion_wait::LandingWait {
                                integration_ref,
                                target_ref_oid: &observed_before,
                                worker_worktree: worker,
                            },
                        )?;
                    }
                    eprintln!(
                        "Landing pending: attached integration checkout changed; user bytes were not modified"
                    );
                    return Ok(false);
                }
                return Err(error).context(
                    "atomic checked fast-forward failed; no destructive fallback was attempted",
                );
            }
            true
        } else {
            git(
                project_root,
                &[
                    "update-ref",
                    integration_ref,
                    &publication_commit,
                    &observed_before,
                ],
            )
            .context("atomic compare-and-fast-forward failed; no fallback was attempted")?;
            false
        }
    };

    let observed_after = git(project_root, &["rev-parse", integration_ref])?;
    if let Err(fence_error) = require_exact_publication(&publication_commit, &observed_after) {
        if let Some(blocker) = pending {
            record_reconciliation_blocked(
                dir,
                task,
                blocker,
                &manifest,
                git_output,
                &observed_after,
                "integration ref advanced after publication CAS; refreshed evidence is no longer target-exact",
            )?;
        } else if let Some(worker) = worker_worktree {
            super::completion_wait::park_landing_pending(
                dir,
                id,
                "integration ref advanced after publication; candidate retained for target-exact renewed validation",
                super::completion_wait::LandingWait {
                    integration_ref,
                    target_ref_oid: &observed_before,
                    worker_worktree: worker,
                },
            )?;
        }
        return Err(fence_error).with_context(|| {
            format!(
                "candidate retained and completion refused; run `wg resume {id} --only` to renew target-bound validation"
            )
        });
    }
    if !is_ancestor(project_root, &publication_commit, &observed_after)?
        || !is_ancestor(project_root, &git_output.commit_oid, &observed_after)?
    {
        bail!(
            "landing postcondition failed: refreshed integration and immutable candidate are not both reachable from integration ref"
        );
    }
    let manifest_digest = manifest.digest().map_err(anyhow::Error::msg)?;
    let receipt = LandingReceipt {
        receipt_version: 1,
        task_id: id.to_string(),
        generation: task.lifecycle.generation,
        manifest_digest: manifest_digest.to_string(),
        integration_ref: integration_ref.to_string(),
        integrated_main_oid: git_output.integrated_main_oid.clone(),
        accepted_commit_oid: publication_commit.clone(),
        observed_main_before: observed_before,
        observed_main_after: observed_after.clone(),
        already_published,
        root_checkout_synchronized,
        created_at: Utc::now().to_rfc3339(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(&receipt)?);
    let receipt_ref = completion_store.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.landing-receipt+json",
    )?;
    record_landing(
        &graph_path,
        id,
        task.lifecycle.generation,
        task.lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.as_str()),
        task.lifecycle.fence,
        &manifest_digest,
        &receipt_ref.content_digest.to_string(),
    )?;
    if publication_commit != git_output.commit_oid {
        mark_reconciliation_landed(dir, id, &publication_commit, &observed_after)?;
    }

    if !root_checkout_synchronized {
        eprintln!(
            "WARNING: integration ref contains the reviewed commit, but this invocation did not synchronize a root checkout"
        );
    }
    println!(
        "Landed '{}' at {}{}",
        id,
        publication_commit,
        if already_published {
            " (already published)"
        } else {
            ""
        }
    );
    Ok(true)
}

fn reconcile_descendant_target(
    dir: &Path,
    task: &worksgood::graph::Task,
    blocker: &CompletionBlocker,
    manifest: &worksgood::completion_manifest::CompletionManifest,
    git_output: &GitOutput,
    dependencies: &[EvidenceRef],
    integration_ref: &str,
    observed_target: &str,
) -> Result<String> {
    let project = dir
        .parent()
        .context("workgraph directory has no project root")?;
    let expected_target = blocker
        .target_ref_oid
        .as_deref()
        .context("LandingPending has no target-ref CAS binding")?;
    if !is_ancestor(project, expected_target, observed_target)? {
        bail!("landing reconciliation requires a descendant-only target advance");
    }
    if git_output.integrated_main_oid != expected_target && blocker.reconciliation_receipt.is_none()
    {
        bail!("pending target expectation no longer binds the reviewed Git output");
    }

    let active = update_reconciliation_projection(
        dir,
        blocker,
        LandingReconciliationState::Reconciling,
        "descendant target advance is being integrated and revalidated",
        None,
        None,
        None,
        Some(
            format!(
                "reconciliation:{}:{}:{}",
                blocker.task_id, expected_target, observed_target
            )
            .as_str(),
        ),
    )?;
    save_reconciliation_record(
        dir,
        &LandingReconciliationRecord {
            schema_version: 1,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            fence: task.lifecycle.fence,
            manifest_digest: manifest.digest().map_err(anyhow::Error::msg)?.to_string(),
            source_candidate_commit_oid: git_output.commit_oid.clone(),
            expected_target_oid: expected_target.to_string(),
            observed_target_oid: observed_target.to_string(),
            state: LandingReconciliationState::Reconciling,
            integration_commit_oid: None,
            validation_inputs_digest: None,
            validation_evidence: Vec::new(),
            receipt_ref: None,
            reason: "descendant target advance accepted for deterministic reconciliation".into(),
            safe_next: format!("wg resume {} --only", task.id),
            updated_at: Utc::now().to_rfc3339(),
        },
    )?;

    let merge = Command::new("git")
        .args([
            "merge-tree",
            "--write-tree",
            observed_target,
            &git_output.commit_oid,
        ])
        .current_dir(project)
        .output()?;
    let integration_tree = String::from_utf8_lossy(&merge.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if !merge.status.success()
        || integration_tree.is_empty()
        || git(project, &["cat-file", "-t", &integration_tree])
            .ok()
            .as_deref()
            != Some("tree")
    {
        let detail = format!(
            "deterministic merge conflicted: {}{}",
            String::from_utf8_lossy(&merge.stderr).trim(),
            if merge.stdout.is_empty() {
                String::new()
            } else {
                format!(" {}", String::from_utf8_lossy(&merge.stdout).trim())
            }
        );
        record_reconciliation_blocked(
            dir,
            task,
            &active,
            manifest,
            git_output,
            observed_target,
            &detail,
        )?;
        bail!(
            "landing reconciliation conflicted; immutable candidate {} is retained. No automated mutation is authorized for this conflict; inspect the auditable blocker with `wg merge-resolution status {}`",
            git_output.commit_oid,
            task.id
        );
    }
    let integration_commit = commit_integration_tree(
        project,
        &integration_tree,
        observed_target,
        &git_output.commit_oid,
        &task.id,
    )?;
    worksgood::control_plane::assert_tree_has_no_control_plane(project, &integration_tree)?;

    let inputs_digest = reconciliation_inputs_digest(task, dependencies)?;
    let validation_worktree = ValidationWorktree::materialize(project, &integration_commit)?;
    let mut validation_evidence = Vec::new();
    let commands = configured_validation_commands(task);
    for (index, command) in commands.iter().enumerate() {
        let captured = capture_validation(
            task,
            command,
            u32::try_from(index).unwrap_or(u32::MAX),
            ValidationPurpose::Configured,
            &validation_worktree.path,
        )
        .with_context(|| format!("capture refreshed validation command: {command}"))?;
        let reference = super::completion_finish::store_validation_evidence(
            dir,
            &captured,
            CONFIGURED_VALIDATION_EVIDENCE_KIND,
        )?;
        super::completion_finish::record_validation_result(dir, task, &captured, &reference)?;
        validation_evidence.push(reference);
        if !captured.authoritative_pass(worksgood::simple_land::CompletionContract::Land) {
            let detail = format!(
                "refreshed configured validation failed: command={} exit={:?} timeout={}",
                command, captured.exit.code, captured.exit.timed_out
            );
            drop(validation_worktree);
            record_reconciliation_blocked(
                dir,
                task,
                &active,
                manifest,
                git_output,
                observed_target,
                &detail,
            )?;
            attach_blocked_validation_evidence(
                dir,
                &task.id,
                &integration_commit,
                &inputs_digest,
                &validation_evidence,
            )?;
            bail!(
                "landing reconciliation validation failed; candidate bytes remain intact. Fix the validation condition without changing required inputs, inspect `wg merge-resolution status {}`, then run `wg resume {} --only`",
                task.id,
                task.id
            );
        }
    }
    let baseline = capture_validation(
        task,
        land_baseline_command(),
        u32::try_from(commands.len()).unwrap_or(u32::MAX),
        ValidationPurpose::Baseline,
        &validation_worktree.path,
    )
    .context("capture refreshed target-dependent baseline validation")?;
    let baseline_ref = super::completion_finish::store_validation_evidence(
        dir,
        &baseline,
        BASELINE_VALIDATION_EVIDENCE_KIND,
    )?;
    super::completion_finish::record_validation_result(dir, task, &baseline, &baseline_ref)?;
    validation_evidence.push(baseline_ref);
    if !baseline.authoritative_pass(worksgood::simple_land::CompletionContract::Land) {
        drop(validation_worktree);
        record_reconciliation_blocked(
            dir,
            task,
            &active,
            manifest,
            git_output,
            observed_target,
            "refreshed target-dependent baseline validation failed",
        )?;
        attach_blocked_validation_evidence(
            dir,
            &task.id,
            &integration_commit,
            &inputs_digest,
            &validation_evidence,
        )?;
        bail!(
            "landing reconciliation baseline validation failed; candidate retained. Inspect `wg merge-resolution status {}` and retry with `wg resume {} --only` after correcting the condition",
            task.id,
            task.id
        );
    }
    drop(validation_worktree);

    // Re-read every mutable authority input after executing validators. A
    // passing command cannot authorize a candidate if requirements, dependency
    // outputs, command policy, generation, attempt, fence, or target changed
    // while it ran.
    let fresh_graph = load_graph(dir.join("graph.jsonl"))?;
    let fresh_task = fresh_graph
        .get_task(&task.id)
        .context("task disappeared during landing reconciliation")?;
    let fresh_blocker = fresh_task
        .completion_blocker
        .as_ref()
        .context("landing blocker disappeared during reconciliation")?;
    super::completion_wait::validate_current(fresh_task, fresh_blocker)?;
    if fresh_blocker != &active {
        bail!("landing reconciliation state changed while validation ran");
    }
    let fresh_dependencies = collect_dependency_outputs(&store(dir)?, &fresh_graph, fresh_task)?;
    if reconciliation_inputs_digest(fresh_task, &fresh_dependencies)? != inputs_digest {
        record_reconciliation_blocked(
            dir,
            fresh_task,
            fresh_blocker,
            manifest,
            git_output,
            observed_target,
            "required validation inputs changed while refreshed validation ran",
        )?;
        attach_blocked_validation_evidence(
            dir,
            &task.id,
            &integration_commit,
            &inputs_digest,
            &validation_evidence,
        )?;
        bail!(
            "landing reconciliation refused changed required inputs; immutable candidate retained. Inspect `wg show {}`; submit a new candidate only from an authorized source generation",
            task.id
        );
    }
    if git(project, &["rev-parse", integration_ref])? != observed_target {
        record_reconciliation_blocked(
            dir,
            fresh_task,
            fresh_blocker,
            manifest,
            git_output,
            observed_target,
            "target fence changed while refreshed validation ran",
        )?;
        attach_blocked_validation_evidence(
            dir,
            &task.id,
            &integration_commit,
            &inputs_digest,
            &validation_evidence,
        )?;
        bail!(
            "landing reconciliation target fence changed during validation; candidate retained. Run `wg resume {} --only` to reconcile the newer descendant target",
            task.id
        );
    }

    let receipt = LandingReconciliationReceipt {
        receipt_version: 1,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        attempt_id: task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone()),
        fence: task.lifecycle.fence,
        manifest_digest: manifest.digest().map_err(anyhow::Error::msg)?.to_string(),
        source_candidate_commit_oid: git_output.commit_oid.clone(),
        source_candidate_tree_oid: git_output.tree_oid.clone(),
        expected_target_oid: expected_target.to_string(),
        refreshed_target_oid: observed_target.to_string(),
        refreshed_target_tree_oid: git(
            project,
            &["rev-parse", &format!("{observed_target}^{{tree}}")],
        )?,
        integration_commit_oid: integration_commit.clone(),
        integration_tree_oid: integration_tree,
        validation_inputs_digest: inputs_digest.clone(),
        validation_evidence: validation_evidence.clone(),
        created_at: Utc::now().to_rfc3339(),
    };
    let receipt_bytes = canonical_json(&serde_json::to_value(&receipt)?);
    let receipt_ref = store(dir)?.put_bytes(
        &receipt_bytes,
        "application/vnd.worksgood.landing-reconciliation+json",
    )?;
    let ready_reason = "descendant target integrated; target-dependent validation renewed";
    let ready_next = format!("land:{}:{}", task.id, receipt_ref.content_digest);
    // The durable authority record is written before its graph projection. A
    // crash may leave an orphan record (safe to recompute), but can never leave
    // ReadyToLand pointing at authority bytes that were not made durable.
    save_reconciliation_record(
        dir,
        &LandingReconciliationRecord {
            schema_version: 1,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            fence: task.lifecycle.fence,
            manifest_digest: receipt.manifest_digest,
            source_candidate_commit_oid: git_output.commit_oid.clone(),
            expected_target_oid: expected_target.to_string(),
            observed_target_oid: observed_target.to_string(),
            state: LandingReconciliationState::ReadyToLand,
            integration_commit_oid: Some(integration_commit.clone()),
            validation_inputs_digest: Some(inputs_digest),
            validation_evidence,
            receipt_ref: Some(receipt_ref.clone()),
            reason: ready_reason.to_string(),
            safe_next: ready_next.clone(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )?;
    update_reconciliation_projection(
        dir,
        fresh_blocker,
        LandingReconciliationState::ReadyToLand,
        ready_reason,
        Some(receipt_ref.content_digest.to_string()),
        Some(integration_commit.clone()),
        Some(observed_target.to_string()),
        Some(&ready_next),
    )?;
    Ok(integration_commit)
}

fn reconciliation_inputs_digest(
    task: &worksgood::graph::Task,
    dependencies: &[EvidenceRef],
) -> Result<String> {
    let bytes = canonical_json(&serde_json::json!({
        "requirements": worksgood::completion_task::requirements_digest(task)?,
        "commands": configured_validation_commands(task),
        "baseline": land_baseline_command(),
        "dependencies": dependencies,
        "generation": task.lifecycle.generation,
        "attempt": task.lifecycle.current_attempt.as_ref().map(|attempt| &attempt.id),
        "fence": task.lifecycle.fence,
    }));
    Ok(ContentDigest::of_bytes(&bytes).to_string())
}

fn commit_integration_tree(
    project: &Path,
    tree: &str,
    target: &str,
    candidate: &str,
    task: &str,
) -> Result<String> {
    // Derive both dates from the immutable target so identical candidate,
    // target, tree, identities, and message yield identical commit bytes.
    let stable_date = git(project, &["show", "-s", "--format=%aI", target])?;
    let output = Command::new("git")
        .args([
            "commit-tree",
            tree,
            "-p",
            target,
            "-p",
            candidate,
            "-m",
            &format!("wg reconcile landing target for {task}"),
        ])
        .current_dir(project)
        .env("GIT_AUTHOR_NAME", "WG Completion Finalizer")
        .env("GIT_AUTHOR_EMAIL", "finalizer@worksgood.local")
        .env("GIT_COMMITTER_NAME", "WG Completion Finalizer")
        .env("GIT_COMMITTER_EMAIL", "finalizer@worksgood.local")
        .env("GIT_AUTHOR_DATE", &stable_date)
        .env("GIT_COMMITTER_DATE", &stable_date)
        .output()?;
    if !output.status.success() {
        bail!(
            "create reconciled integration commit: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

struct ValidationWorktree {
    project: PathBuf,
    path: PathBuf,
}

impl ValidationWorktree {
    fn materialize(project: &Path, commit: &str) -> Result<Self> {
        let common = PathBuf::from(git(project, &["rev-parse", "--git-common-dir"])?);
        let common = if common.is_absolute() {
            common
        } else {
            project.join(common)
        };
        let path = common
            .join("wg-landing-validation")
            .join(uuid::Uuid::now_v7().to_string());
        fs::create_dir_all(path.parent().expect("validation path has parent"))?;
        let output = Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg(commit)
            .current_dir(project)
            .output()?;
        if !output.status.success() {
            bail!(
                "materialize refreshed validation tree: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(Self {
            project: project.to_path_buf(),
            path,
        })
    }
}

impl Drop for ValidationWorktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.project)
            .output();
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_reconciliation_projection(
    dir: &Path,
    expected: &CompletionBlocker,
    state: LandingReconciliationState,
    reason: &str,
    receipt: Option<String>,
    commit: Option<String>,
    target: Option<String>,
    idempotency: Option<&str>,
) -> Result<CompletionBlocker> {
    let mut updated = None;
    let mut refusal = None;
    modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&expected.task_id) else {
            refusal = Some("task disappeared during landing reconciliation".to_string());
            return false;
        };
        if task.status != Status::Waiting
            || task.completion_blocker.as_ref() != Some(expected)
            || task.completion_candidate.as_ref() != Some(&expected.candidate)
            || task.lifecycle.generation != expected.generation
            || task.lifecycle.fence != expected.fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != expected.attempt_id.as_deref()
        {
            refusal = Some("landing reconciliation binding is stale".to_string());
            return false;
        }
        let blocker = task.completion_blocker.as_mut().expect("checked blocker");
        blocker.reconciliation_state = state;
        blocker.reason = reason.to_string();
        blocker.safe_next = match state {
            LandingReconciliationState::Waiting | LandingReconciliationState::Reconciling => {
                format!("wg resume {} --only", task.id)
            }
            LandingReconciliationState::ReadyToLand => format!("wg resume {} --only", task.id),
            LandingReconciliationState::Landed => format!("wg show {}", task.id),
            LandingReconciliationState::Blocked => {
                format!(
                    "wg merge-resolution status {}; then wg resume {} --only after resolving the named condition",
                    task.id, task.id
                )
            }
        };
        if let Some(value) = receipt {
            blocker.reconciliation_receipt = Some(value);
        }
        if let Some(value) = commit {
            blocker.reconciled_commit_oid = Some(value);
        }
        if let Some(value) = target {
            blocker.target_ref_oid = Some(value);
        }
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("completion-finalizer-reconciler".into()),
            user: None,
            message: format!(
                "Landing reconciliation {:?}: {} action={}",
                state,
                reason,
                idempotency.unwrap_or("none")
            ),
        });
        updated = Some(blocker.clone());
        true
    })?;
    if let Some(error) = refusal {
        bail!(error);
    }
    updated.context("landing reconciliation projection was not updated")
}

fn record_reconciliation_blocked(
    dir: &Path,
    task: &worksgood::graph::Task,
    blocker: &CompletionBlocker,
    manifest: &worksgood::completion_manifest::CompletionManifest,
    git_output: &GitOutput,
    observed_target: &str,
    reason: &str,
) -> Result<()> {
    let blocked = update_reconciliation_projection(
        dir,
        blocker,
        LandingReconciliationState::Blocked,
        reason,
        None,
        None,
        None,
        Some(format!("blocked:{}:{}", task.id, observed_target).as_str()),
    )?;
    save_reconciliation_record(
        dir,
        &LandingReconciliationRecord {
            schema_version: 1,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            fence: task.lifecycle.fence,
            manifest_digest: manifest.digest().map_err(anyhow::Error::msg)?.to_string(),
            source_candidate_commit_oid: git_output.commit_oid.clone(),
            expected_target_oid: blocker.target_ref_oid.clone().unwrap_or_default(),
            observed_target_oid: observed_target.to_string(),
            state: LandingReconciliationState::Blocked,
            integration_commit_oid: blocker.reconciled_commit_oid.clone(),
            validation_inputs_digest: None,
            validation_evidence: Vec::new(),
            receipt_ref: None,
            reason: reason.to_string(),
            safe_next: blocked.safe_next,
            updated_at: Utc::now().to_rfc3339(),
        },
    )
}

fn attach_blocked_validation_evidence(
    dir: &Path,
    id: &str,
    integration_commit: &str,
    inputs_digest: &str,
    evidence: &[EvidenceRef],
) -> Result<()> {
    let mut record = load_reconciliation_record(dir, id)?
        .context("blocked landing reconciliation record is missing")?;
    if record.state != LandingReconciliationState::Blocked {
        bail!("refused to attach failed validation evidence to a non-blocked reconciliation");
    }
    record.integration_commit_oid = Some(integration_commit.to_string());
    record.validation_inputs_digest = Some(inputs_digest.to_string());
    record.validation_evidence = evidence.to_vec();
    record.updated_at = Utc::now().to_rfc3339();
    save_reconciliation_record(dir, &record)
}

fn reconciliation_record_path(dir: &Path, id: &str) -> PathBuf {
    let safe = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    dir.join("completion/v3/landing-reconciliation")
        .join(format!("{safe}.json"))
}

fn save_reconciliation_record(dir: &Path, record: &LandingReconciliationRecord) -> Result<()> {
    let path = reconciliation_record_path(dir, &record.task_id);
    fs::create_dir_all(path.parent().expect("record path has parent"))?;
    worksgood::atomic_file::write_atomic(&path, &serde_json::to_vec_pretty(record)?)?;
    let journal = path.with_extension("jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(journal)?;
    writeln!(file, "{}", serde_json::to_string(record)?)?;
    file.sync_all()?;
    Ok(())
}

fn load_reconciliation_record(dir: &Path, id: &str) -> Result<Option<LandingReconciliationRecord>> {
    let path = reconciliation_record_path(dir, id);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn verify_ready_reconciliation(
    dir: &Path,
    task: &worksgood::graph::Task,
    blocker: &CompletionBlocker,
    manifest: &worksgood::completion_manifest::CompletionManifest,
    git_output: &GitOutput,
    observed_target: &str,
    commit: &str,
) -> Result<()> {
    if blocker.reconciliation_state != LandingReconciliationState::ReadyToLand {
        bail!("reconciled commit exists without ReadyToLand authority");
    }
    let record = load_reconciliation_record(dir, &task.id)?
        .context("ready landing reconciliation record is missing")?;
    let receipt_ref = record
        .receipt_ref
        .as_ref()
        .context("ready landing reconciliation receipt is missing")?;
    if blocker.reconciliation_receipt.as_deref() != Some(receipt_ref.content_digest.as_str()) {
        bail!("landing reconciliation receipt projection differs from immutable record");
    }
    let bytes = store(dir)?.read_artifact(
        receipt_ref,
        worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
    )?;
    let receipt: LandingReconciliationReceipt = serde_json::from_slice(&bytes)?;
    if canonical_json(&serde_json::to_value(&receipt)?) != bytes
        || receipt.task_id != task.id
        || receipt.generation != task.lifecycle.generation
        || receipt.fence != task.lifecycle.fence
        || receipt.attempt_id.as_deref()
            != task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
        || receipt.manifest_digest != manifest.digest().map_err(anyhow::Error::msg)?.to_string()
        || receipt.source_candidate_commit_oid != git_output.commit_oid
        || receipt.source_candidate_tree_oid != git_output.tree_oid
        || (receipt.refreshed_target_oid != observed_target && commit != observed_target)
        || receipt.integration_commit_oid != commit
        || record.state != LandingReconciliationState::ReadyToLand
    {
        bail!("landing reconciliation receipt binding is stale or invalid");
    }
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let current = graph.get_task(&task.id).context("task disappeared")?;
    let dependencies = collect_dependency_outputs(&store(dir)?, &graph, current)?;
    if reconciliation_inputs_digest(current, &dependencies)? != receipt.validation_inputs_digest {
        bail!("landing reconciliation required inputs changed after renewed validation");
    }
    let project = dir
        .parent()
        .context("workgraph directory has no project root")?;
    if git(project, &["rev-parse", &format!("{commit}^{{tree}}")])? != receipt.integration_tree_oid
        || !is_ancestor(project, &receipt.refreshed_target_oid, commit)?
        || !is_ancestor(project, &git_output.commit_oid, commit)?
        || (observed_target != receipt.refreshed_target_oid
            && !is_ancestor(project, commit, observed_target)?)
    {
        bail!("landing reconciliation commit no longer matches its target/candidate receipt");
    }
    verify_refreshed_validation_evidence(dir, current, &receipt)
}

fn verify_refreshed_validation_evidence(
    dir: &Path,
    task: &worksgood::graph::Task,
    receipt: &LandingReconciliationReceipt,
) -> Result<()> {
    let commands = configured_validation_commands(task);
    let mut seen = BTreeSet::new();
    let mut baseline = 0_usize;
    for reference in &receipt.validation_evidence {
        let artifact = ArtifactOutput {
            content_digest: reference.content_digest.clone(),
            immutable_locator: reference.immutable_locator.clone(),
            media_type: reference.media_type.clone(),
            size: reference.size,
            review_projection: reference.review_projection.clone(),
        };
        let bytes = store(dir)?.read_artifact(
            &artifact,
            worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
        )?;
        let evidence: worksgood::completion_validation::DeterministicValidationEvidence =
            serde_json::from_slice(&bytes)?;
        worksgood::completion_validation::verify_capture_authority(
            dir,
            &reference.content_digest,
            &evidence,
        )?;
        if !evidence.authoritative_pass(worksgood::simple_land::CompletionContract::Land)
            || evidence.lifecycle.task_id != task.id
            || evidence.lifecycle.generation != task.lifecycle.generation
            || evidence.lifecycle.attempt_fence != task.lifecycle.fence
            || evidence.repository.before_head_oid != receipt.integration_commit_oid
            || evidence.repository.before_tree_oid != receipt.integration_tree_oid
            || evidence.repository.integrated_main_oid != receipt.refreshed_target_oid
        {
            bail!("refreshed validation evidence is not authoritative for the reconciled tree");
        }
        match evidence.purpose {
            ValidationPurpose::Configured => {
                let index =
                    usize::try_from(evidence.command.configured_index).unwrap_or(usize::MAX);
                if commands.get(index).map(String::as_str)
                    != evidence.command.argv.get(1).map(String::as_str)
                    || !seen.insert(index)
                {
                    bail!("refreshed configured validation command identity changed");
                }
            }
            ValidationPurpose::Baseline => {
                baseline += 1;
                if evidence.command.argv.get(1).map(String::as_str) != Some(land_baseline_command())
                {
                    bail!("refreshed baseline validation command identity changed");
                }
            }
        }
    }
    if seen.len() != commands.len() || baseline != 1 {
        bail!("refreshed validation evidence set is incomplete");
    }
    Ok(())
}

pub(crate) fn mark_reconciliation_landed(
    dir: &Path,
    id: &str,
    commit: &str,
    observed_after: &str,
) -> Result<()> {
    if let Some(mut record) = load_reconciliation_record(dir, id)? {
        if record.integration_commit_oid.as_deref() != Some(commit) {
            bail!("landed reconciliation commit differs from recorded ready commit");
        }
        record.state = LandingReconciliationState::Landed;
        record.reason = format!("reconciled candidate landed at {observed_after}");
        record.safe_next = format!("wg show {id}");
        record.updated_at = Utc::now().to_rfc3339();
        save_reconciliation_record(dir, &record)?;
    }
    Ok(())
}

pub(crate) fn print_reconciliation_status(dir: &Path, id: &str, json: bool) -> Result<bool> {
    let Some(record) = load_reconciliation_record(dir, id)? else {
        return Ok(false);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!(
            "Landing reconciliation {:?}: {}\n  candidate: {}\n  target: {} -> {}\n  integration: {}\n  renewed validation: {} receipt={}\n  reason: {}\n  next: {}",
            record.state,
            record.task_id,
            record.source_candidate_commit_oid,
            record.expected_target_oid,
            record.observed_target_oid,
            record.integration_commit_oid.as_deref().unwrap_or("none"),
            record.validation_evidence.len(),
            record
                .receipt_ref
                .as_ref()
                .map(|value| value.content_digest.as_str())
                .unwrap_or("none"),
            record.reason,
            record.safe_next
        );
    }
    Ok(true)
}

fn exact_git_output(outputs: &[OutputRef]) -> Result<&GitOutput> {
    let mut git_outputs = outputs.iter().filter_map(|output| match output {
        OutputRef::Git(git) => Some(git),
        _ => None,
    });
    let output = git_outputs
        .next()
        .context("Land manifest has no Git output")?;
    if git_outputs.next().is_some() {
        bail!("Land manifest has more than one Git output");
    }
    Ok(output)
}

fn validate_integration_ref(reference: &str) -> Result<()> {
    if !reference.starts_with("refs/heads/") || reference.contains("..") {
        bail!("integration ref must be an explicit refs/heads/* reference");
    }
    Ok(())
}

fn verify_worker_worktree(worktree: &Path, accepted_commit: &str) -> Result<()> {
    let head = git(worktree, &["rev-parse", "HEAD"])?;
    if head != accepted_commit {
        bail!(
            "worker worktree HEAD {} is not the exact reviewed commit {}",
            head,
            accepted_commit
        );
    }
    let status = git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        bail!("worker worktree is not clean at the reviewed commit");
    }
    Ok(())
}

fn root_checkout_dirty_if_attached(project: &Path, integration_ref: &str) -> Result<bool> {
    if symbolic_head(project).as_deref() != Some(integration_ref) {
        return Ok(false);
    }
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(project)
        .output()?;
    if !output.status.success() {
        bail!(
            "git status failed while fencing the landing checkout: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if entry.len() < 4 {
            return Ok(true);
        }
        let status = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).replace('\\', "/");
        // Defense in depth for repositories created before WG installed its
        // repository-local info/exclude stanza. Only an untracked path proven
        // to consist exclusively of registered WG worktrees is runtime. A
        // similarly named user directory, or any tracked/index change, stays
        // dirty and therefore blocks landing.
        if status == b"??" && managed_wg_runtime_status_path(project, &path)? {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn managed_wg_runtime_status_path(project: &Path, relative: &str) -> Result<bool> {
    let normalized = relative.trim_end_matches('/');
    if normalized != ".wg-worktrees" && !normalized.starts_with(".wg-worktrees/") {
        return Ok(false);
    }
    let runtime_root = project.join(".wg-worktrees");
    if !runtime_root.is_dir() {
        return Ok(false);
    }
    let runtime_root = runtime_root.canonicalize()?;
    let listed = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project)
        .output()?;
    if !listed.status.success() {
        return Ok(false);
    }
    let registered = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| Path::new(path).canonicalize().ok())
        .filter(|path| path.starts_with(&runtime_root))
        .collect::<BTreeSet<_>>();
    if registered.is_empty() {
        return Ok(false);
    }
    if normalized == ".wg-worktrees" {
        for entry in fs::read_dir(&runtime_root)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_symlink() || !registered.contains(&path) {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let suffix = normalized
        .strip_prefix(".wg-worktrees/")
        .context("checked runtime prefix disappeared")?;
    let candidate = runtime_root.join(suffix);
    if fs::symlink_metadata(&candidate).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(false);
    }
    // Lexical membership is mandatory before canonicalization. Otherwise an
    // untracked user symlink sibling resolving into a registered worktree
    // would be mistaken for WG-owned runtime state.
    let Some(registered_root) = registered.iter().find(|root| candidate.starts_with(root)) else {
        return Ok(false);
    };
    Ok(candidate
        .canonicalize()
        .ok()
        .is_some_and(|path| path.starts_with(registered_root)))
}

fn symbolic_head(project: &Path) -> Option<String> {
    git(project, &["symbolic-ref", "-q", "HEAD"]).ok()
}

fn require_exact_publication(expected: &str, observed: &str) -> Result<()> {
    if observed != expected {
        bail!(
            "landing target fence changed after publication: expected exact {expected} but observed {observed}"
        );
    }
    Ok(())
}

fn is_ancestor(project: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(project)
        .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base --is-ancestor failed"),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn record_landing(
    graph_path: &Path,
    id: &str,
    generation: u64,
    attempt_id: Option<&str>,
    fence: u64,
    manifest_digest: &worksgood::completion_manifest::ContentDigest,
    receipt_digest: &str,
) -> Result<()> {
    let mut refusal = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared after Git publication".to_string());
            return false;
        };
        if task.lifecycle.generation != generation
            || task.lifecycle.fence != fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != attempt_id
            || task
                .completion_candidate
                .as_ref()
                .map(|candidate| &candidate.manifest.content_digest)
                != Some(manifest_digest)
        {
            refusal = Some(
                "task generation, attempt, fence, or candidate changed after Git publication; accepted commit remains recoverable by ancestry"
                    .to_string(),
            );
            return false;
        }
        task.completion_disposition = Some(CompletionDisposition::Landed);
        task.completion_receipt = Some(receipt_digest.to_string());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("land".to_string()),
            user: None,
            message: format!("Reviewed manifest {manifest_digest} published to integration ref"),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}

struct LandingLock {
    file: File,
}

impl LandingLock {
    fn acquire(project: &Path) -> Result<Self> {
        let common = git(project, &["rev-parse", "--git-common-dir"])?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            project.join(common)
        };
        fs::create_dir_all(&common)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(common.join("wg-land.lock"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = file.as_raw_fd();
            worksgood::lock::retry_acquire(
                &worksgood::lock::RetryPolicy::default(),
                worksgood::lock::is_transient_blocking,
                || {
                    let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
                    if result == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                },
            )?;
        }
        Ok(Self { file })
    }
}

impl Drop for LandingLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use worksgood::completion_manifest::{
        COMPLETION_MANIFEST_VERSION, CompletionManifest, ContentDigest, GitOutput, OutputRef,
    };
    use worksgood::completion_review::{
        ManifestReviewer, ReviewFinding, ReviewerKind, ReviewerUnavailable, SemanticReview,
        SemanticVerdict,
    };
    use worksgood::graph::{Node, Status, Task, WorkGraph};
    use worksgood::parser::save_graph;

    struct PassReviewer {
        route: &'static str,
        calls: Arc<Mutex<Vec<ReviewerKind>>>,
    }

    struct RejectReviewer {
        route: &'static str,
        calls: Arc<Mutex<Vec<ReviewerKind>>>,
    }

    impl ManifestReviewer for PassReviewer {
        fn route(&self) -> &str {
            self.route
        }
        fn review(
            &mut self,
            kind: ReviewerKind,
            _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
        ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
            self.calls.lock().unwrap().push(kind);
            Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::<ReviewFinding>::new(),
            })
        }
    }

    impl ManifestReviewer for RejectReviewer {
        fn route(&self) -> &str {
            self.route
        }
        fn review(
            &mut self,
            kind: ReviewerKind,
            _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
        ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
            self.calls.lock().unwrap().push(kind);
            Ok(SemanticReview {
                verdict: SemanticVerdict::Reject,
                findings: vec![ReviewFinding::new(
                    "advisory.fixture",
                    "bounded actionable finding",
                )],
            })
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        dir: PathBuf,
        worker: PathBuf,
        candidate: String,
        integrated: String,
        manifest_path: PathBuf,
        summary_path: PathBuf,
        review_calls: Arc<Mutex<Vec<ReviewerKind>>>,
        task_id: String,
    }

    fn command(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn fixture() -> Fixture {
        fixture_with_validation(None)
    }

    fn fixture_with_validation(validation: Option<&str>) -> Fixture {
        let temp = tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        command(&root, &["init", "-b", "main"]);
        command(&root, &["config", "user.email", "test@example.com"]);
        command(&root, &["config", "user.name", "Test"]);
        fs::write(root.join(".gitignore"), ".wg\n").unwrap();
        fs::write(root.join("base.txt"), "base\n").unwrap();
        command(&root, &["add", ".gitignore", "base.txt"]);
        command(&root, &["commit", "-m", "base"]);
        let integrated = command(&root, &["rev-parse", "HEAD"]);
        let worker = temp.path().join("worker");
        command(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "wg/test-land",
                worker.to_str().unwrap(),
                "main",
            ],
        );
        fs::write(worker.join("result.txt"), "accepted\n").unwrap();
        command(&worker, &["add", "result.txt"]);
        command(&worker, &["commit", "-m", "candidate"]);
        let candidate = command(&worker, &["rev-parse", "HEAD"]);
        let tree = command(&worker, &["rev-parse", "HEAD^{tree}"]);
        let diff = Command::new("git")
            .args([
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &integrated,
                &candidate,
                "--",
            ])
            .current_dir(&root)
            .output()
            .unwrap()
            .stdout;

        let dir = root.join(".wg");
        fs::create_dir_all(&dir).unwrap();
        let task_id = std::env::var("WG_TASK_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "land-task".to_string());
        let mut task = Task {
            id: task_id.clone(),
            title: "Land exact candidate".to_string(),
            description: Some("Land result.\n\n## Validation\nInspect diff.".to_string()),
            status: Status::InProgress,
            completion_contract: CompletionContract::Land,
            validation_commands: validation.into_iter().map(str::to_string).collect(),
            ..Task::default()
        };
        let source_agent = std::env::var("WG_AGENT_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "agent-land".to_string());
        task.assigned = Some(source_agent.clone());
        task.lifecycle.generation = 2;
        task.lifecycle.fence = 9;
        task.lifecycle.current_attempt = Some(worksgood::lifecycle::AttemptRef {
            id: "attempt-2-1".to_string(),
            generation: 2,
            fence: 9,
            actor_id: source_agent,
            disposition: None,
        });
        let requirements = worksgood::completion_task::requirements_digest(&task).unwrap();
        let summary = b"candidate complete\n";
        let mut evidence = Vec::new();
        if let Some(command_text) = validation {
            let captured = capture_validation(
                &task,
                command_text,
                0,
                ValidationPurpose::Configured,
                &worker,
            )
            .unwrap();
            assert!(captured.authoritative_pass(worksgood::simple_land::CompletionContract::Land));
            evidence.push(
                super::super::completion_finish::store_validation_evidence(
                    &dir,
                    &captured,
                    CONFIGURED_VALIDATION_EVIDENCE_KIND,
                )
                .unwrap(),
            );
            let baseline = capture_validation(
                &task,
                land_baseline_command(),
                1,
                ValidationPurpose::Baseline,
                &worker,
            )
            .unwrap();
            evidence.push(
                super::super::completion_finish::store_validation_evidence(
                    &dir,
                    &baseline,
                    BASELINE_VALIDATION_EVIDENCE_KIND,
                )
                .unwrap(),
            );
        } else {
            evidence.push(
                store(&dir)
                    .unwrap()
                    .evidence_from_bytes(b"tests pass\n", "validation", "text/plain")
                    .unwrap(),
            );
        }
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Land,
            requirements_digest: requirements,
            source_revision: "worker:test".to_string(),
            outputs: vec![OutputRef::Git(GitOutput {
                commit_oid: candidate.clone(),
                integrated_main_oid: integrated.clone(),
                tree_oid: tree,
                diff_bundle_digest: ContentDigest::of_bytes(&diff),
            })],
            validation_evidence: evidence,
            worker_summary_digest: ContentDigest::of_bytes(summary),
        };
        let manifest_path = temp.path().join("manifest.json");
        fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
        let summary_path = temp.path().join("summary.txt");
        fs::write(&summary_path, summary).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = PassReviewer {
            route: "pi:test-flip",
            calls: calls.clone(),
        };
        let mut eval = PassReviewer {
            route: "codex:test-eval",
            calls: calls.clone(),
        };
        super::super::completion_submit::run_with_reviewers(
            &dir,
            &task_id,
            &manifest_path,
            &summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();

        Fixture {
            _temp: temp,
            root,
            dir,
            worker,
            candidate,
            integrated,
            manifest_path,
            summary_path,
            review_calls: calls,
            task_id,
        }
    }

    #[test]
    fn finalizer_distinguishes_registered_runtime_from_similarly_named_user_bytes() {
        let fixture = fixture();
        let runtime = fixture.root.join(".wg-worktrees/agent-runtime");
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        command(
            &fixture.root,
            &[
                "worktree",
                "add",
                "-b",
                "wg/agent-runtime/runtime-test",
                runtime.to_str().unwrap(),
                "main",
            ],
        );
        fs::write(
            fixture.root.join(".git/info/exclude"),
            "# BEGIN worksgood managed runtime\n/.wg-worktrees/agent-runtime/\n# END worksgood managed runtime\n",
        )
        .unwrap();
        assert!(
            !root_checkout_dirty_if_attached(&fixture.root, "refs/heads/main").unwrap(),
            "a registered WG runtime path is not user dirtiness"
        );
        fs::write(
            fixture.root.join(".wg-worktrees/user-owned.txt"),
            "must block landing\n",
        )
        .unwrap();
        assert!(
            root_checkout_dirty_if_attached(&fixture.root, "refs/heads/main").unwrap(),
            "similarly named user bytes must remain authoritative dirtiness"
        );
        fs::remove_file(fixture.root.join(".wg-worktrees/user-owned.txt")).unwrap();
        #[cfg(unix)]
        {
            let user_link = fixture.root.join(".wg-worktrees/user-link");
            std::os::unix::fs::symlink(&runtime, &user_link).unwrap();
            assert!(
                root_checkout_dirty_if_attached(&fixture.root, "refs/heads/main").unwrap(),
                "a user symlink resolving into registered runtime must remain dirtiness"
            );
            fs::remove_file(user_link).unwrap();
        }
        command(
            &fixture.root,
            &["worktree", "remove", "--force", runtime.to_str().unwrap()],
        );
    }

    #[test]
    fn land_compare_and_fast_forwards_reviewed_commit() {
        let fixture = fixture();
        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "HEAD"]),
            fixture.candidate
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(
            task.completion_disposition,
            Some(CompletionDisposition::Landed)
        );
        assert!(task.completion_receipt.is_some());
        assert_eq!(task.status, Status::InProgress);

        // Simulate a crash after the Git CAS but before a durable landing
        // projection. Done is recovered from ancestry plus exact reviews.
        modify_graph(fixture.dir.join("graph.jsonl"), |graph| {
            let task = graph.get_task_mut(&fixture.task_id).unwrap();
            task.completion_disposition = None;
            task.completion_receipt = None;
            true
        })
        .unwrap();
        super::super::completion_done::run(&fixture.dir, &fixture.task_id, "refs/heads/main")
            .unwrap();
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph.get_task(&fixture.task_id).unwrap().status,
            Status::Done
        );
    }

    #[test]
    fn advisory_flip_rejection_survives_landing_and_done() {
        let fixture = fixture();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = RejectReviewer {
            route: "pi:test-advisory-flip",
            calls: calls.clone(),
        };
        let mut eval = PassReviewer {
            route: "pi:test-eval",
            calls: calls.clone(),
        };
        let outcome = super::super::completion_submit::run_with_reviewers(
            &fixture.dir,
            &fixture.task_id,
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();
        assert_eq!(
            outcome.status,
            worksgood::completion_review::ReviewValveStatus::FlipRejected
        );
        assert_eq!(*calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        super::super::completion_done::run(&fixture.dir, &fixture.task_id, "refs/heads/main")
            .unwrap();

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.completion_review_activity.len(), 3);
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(
            verified.activities.last().unwrap().candidate_state,
            worksgood::completion_review::ReviewCandidateState::Current
        );
        assert_eq!(
            verified.activities.last().unwrap().failure_class,
            Some(worksgood::completion_review::ReviewFailureClass::SemanticRejection)
        );
        assert_eq!(
            verified.activities.last().unwrap().findings[0].code,
            "advisory.fixture"
        );
    }

    #[test]
    fn completion_blockers_dirty_attached_main_preserves_bytes_and_resumes_once() {
        let fixture = fixture();
        fs::write(fixture.root.join("base.txt"), "user staged bytes\n").unwrap();
        command(&fixture.root, &["add", "base.txt"]);
        fs::write(fixture.root.join("base.txt"), "user worktree bytes\n").unwrap();
        fs::write(
            fixture.root.join("user-untracked.txt"),
            "user untracked bytes\n",
        )
        .unwrap();
        let index_before = command(&fixture.root, &["show", ":base.txt"]);
        let worktree_before = fs::read(fixture.root.join("base.txt")).unwrap();
        let untracked_before = fs::read(fixture.root.join("user-untracked.txt")).unwrap();
        let status_before = command(
            &fixture.root,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        let main_before = command(&fixture.root, &["rev-parse", "main"]);
        let candidate_before = load_graph(fixture.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&fixture.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();

        assert_eq!(command(&fixture.root, &["rev-parse", "main"]), main_before);
        assert_eq!(command(&fixture.root, &["show", ":base.txt"]), index_before);
        assert_eq!(
            fs::read(fixture.root.join("base.txt")).unwrap(),
            worktree_before
        );
        assert_eq!(
            fs::read(fixture.root.join("user-untracked.txt")).unwrap(),
            untracked_before
        );
        assert_eq!(
            command(
                &fixture.root,
                &["status", "--porcelain", "--untracked-files=all"],
            ),
            status_before
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.assigned.is_none());
        assert!(task.failure_reason.is_none());
        assert_eq!(task.completion_candidate.as_ref(), Some(&candidate_before));
        assert_eq!(
            task.lifecycle.current_attempt.as_ref().unwrap().disposition,
            Some(worksgood::lifecycle::AttemptDisposition::Parked)
        );
        let blocker = task.completion_blocker.clone().unwrap();
        assert_eq!(blocker.kind, CompletionBlockerKind::LandingPending);
        assert_eq!(blocker.generation, 2);
        assert_eq!(blocker.attempt_id.as_deref(), Some("attempt-2-1"));
        assert_eq!(blocker.fence, 9);
        assert_eq!(
            blocker.target_ref_oid.as_deref(),
            Some(main_before.as_str())
        );
        assert_eq!(blocker.candidate, candidate_before);
        let blocker_bytes = serde_json::to_vec(&blocker).unwrap();
        drop(graph);

        // A restart is a pure reload: exact candidate/fence/review receipts are
        // unchanged and no live source/session process is needed.
        let reopened = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            serde_json::to_vec(
                reopened
                    .get_task(&fixture.task_id)
                    .unwrap()
                    .completion_blocker
                    .as_ref()
                    .unwrap()
            )
            .unwrap(),
            blocker_bytes
        );
        drop(reopened);

        command(&fixture.root, &["restore", "--staged", "base.txt"]);
        command(&fixture.root, &["restore", "base.txt"]);
        fs::remove_file(fixture.root.join("user-untracked.txt")).unwrap();
        assert!(pending_checkout_is_clean(&fixture.dir, &fixture.task_id).unwrap());
        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.completion_blocker.is_none());
        assert_eq!(
            task.completion_disposition,
            Some(CompletionDisposition::Landed)
        );
        assert_eq!(
            *fixture.review_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
        drop(graph);

        // Explicit replay is idempotent: target/ref, candidate, and review
        // call count stay fixed and source work is never dispatched.
        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        assert_eq!(
            command(&fixture.root, &["rev-parse", "main"]),
            fixture.candidate
        );
        assert_eq!(
            *fixture.review_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
    }

    #[test]
    fn completion_blockers_stale_candidate_fence_and_moved_target_fail_closed() {
        let park = |fixture: &Fixture| {
            fs::write(fixture.root.join("base.txt"), "dirty\n").unwrap();
            run_at(
                &fixture.dir,
                &fixture.task_id,
                "refs/heads/main",
                Some(&fixture.worker),
            )
            .unwrap();
            command(&fixture.root, &["restore", "base.txt"]);
        };

        let stale_fence = fixture();
        park(&stale_fence);
        modify_graph(stale_fence.dir.join("graph.jsonl"), |graph| {
            graph
                .get_task_mut(&stale_fence.task_id)
                .unwrap()
                .lifecycle
                .fence += 1;
            true
        })
        .unwrap();
        let before = command(&stale_fence.root, &["rev-parse", "main"]);
        let error = resume_pending(&stale_fence.dir, &stale_fence.task_id).unwrap_err();
        assert!(error.to_string().contains("binding is stale"));
        assert_eq!(command(&stale_fence.root, &["rev-parse", "main"]), before);

        let stale_candidate = fixture();
        park(&stale_candidate);
        modify_graph(stale_candidate.dir.join("graph.jsonl"), |graph| {
            graph
                .get_task_mut(&stale_candidate.task_id)
                .unwrap()
                .completion_candidate
                .as_mut()
                .unwrap()
                .eval_receipt = None;
            true
        })
        .unwrap();
        let before = command(&stale_candidate.root, &["rev-parse", "main"]);
        let error = resume_pending(&stale_candidate.dir, &stale_candidate.task_id).unwrap_err();
        assert!(error.to_string().contains("binding is stale"));
        assert_eq!(
            command(&stale_candidate.root, &["rev-parse", "main"]),
            before
        );
    }

    #[test]
    fn exact_publication_fence_rejects_a_strict_descendant_race() {
        let error =
            require_exact_publication("validated", "validated-plus-racing-commit").unwrap_err();
        assert!(error.to_string().contains("expected exact validated"));
    }

    #[test]
    fn candidate_containing_strict_descendant_requires_renewed_validation() {
        let fixture = fixture();
        command(&fixture.root, &["merge", "--ff-only", &fixture.candidate]);
        fs::write(fixture.root.join("later.txt"), "unvalidated descendant\n").unwrap();
        command(&fixture.root, &["add", "later.txt"]);
        command(&fixture.root, &["commit", "-m", "later target bytes"]);
        let strict_descendant = command(&fixture.root, &["rev-parse", "main"]);

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .reconciliation_state,
            LandingReconciliationState::Waiting
        );
        drop(graph);

        assert!(resume_pending(&fixture.dir, &fixture.task_id).unwrap());
        let landed = command(&fixture.root, &["rev-parse", "main"]);
        assert_ne!(landed, strict_descendant);
        assert!(is_ancestor(&fixture.root, &strict_descendant, &landed).unwrap());
        let record = load_reconciliation_record(&fixture.dir, &fixture.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.state, LandingReconciliationState::Landed);
        assert!(!record.validation_evidence.is_empty());
    }

    #[test]
    fn descendant_target_advance_renews_evidence_and_lands_without_live_worker() {
        let fixture = fixture();
        let selected = load_graph(fixture.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&fixture.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        fs::write(fixture.root.join("base.txt"), "temporary user dirtiness\n").unwrap();
        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        command(&fixture.root, &["restore", "base.txt"]);
        fs::write(fixture.root.join("other.txt"), "target moved\n").unwrap();
        command(&fixture.root, &["add", "other.txt"]);
        command(&fixture.root, &["commit", "-m", "target moved"]);
        let moved = command(&fixture.root, &["rev-parse", "main"]);

        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        let landed = command(&fixture.root, &["rev-parse", "main"]);
        assert_ne!(landed, moved);
        assert!(is_ancestor(&fixture.root, &moved, &landed).unwrap());
        assert!(is_ancestor(&fixture.root, &fixture.candidate, &landed).unwrap());
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.assigned.is_none(), "released worker is not reacquired");
        assert_eq!(task.completion_candidate.as_ref(), Some(&selected));
        drop(graph);
        let record = load_reconciliation_record(&fixture.dir, &fixture.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.state, LandingReconciliationState::Landed);
        assert_eq!(record.observed_target_oid, moved);
        assert!(record.receipt_ref.is_some());
        assert_eq!(record.validation_evidence.len(), 1);
        let deterministic = commit_integration_tree(
            &fixture.root,
            &command(&fixture.root, &["rev-parse", &format!("{landed}^{{tree}}")]),
            &moved,
            &fixture.candidate,
            &fixture.task_id,
        )
        .unwrap();
        assert_eq!(
            deterministic, landed,
            "retries must produce identical commit bytes"
        );
        assert_eq!(
            *fixture.review_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval],
            "target reconciliation renews deterministic evidence, not model review"
        );
    }

    #[test]
    fn divergence_and_conflict_are_audited_and_keep_candidate_bytes() {
        let park = |fixture: &Fixture| {
            fs::write(fixture.root.join("base.txt"), "dirty\n").unwrap();
            run_at(
                &fixture.dir,
                &fixture.task_id,
                "refs/heads/main",
                Some(&fixture.worker),
            )
            .unwrap();
            command(&fixture.root, &["restore", "base.txt"]);
        };

        let divergent = fixture();
        park(&divergent);
        let selected = load_graph(divergent.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&divergent.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        let base_tree = command(&divergent.root, &["rev-parse", "HEAD^{tree}"]);
        let unrelated = command(
            &divergent.root,
            &["commit-tree", &base_tree, "-m", "unrelated target"],
        );
        command(
            &divergent.root,
            &["update-ref", "refs/heads/main", &unrelated],
        );
        command(&divergent.root, &["reset", "--hard", &unrelated]);
        let error = resume_pending(&divergent.dir, &divergent.task_id).unwrap_err();
        assert!(error.to_string().contains("not a descendant"));
        let graph = load_graph(divergent.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&divergent.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.completion_candidate.as_ref(), Some(&selected));
        assert_eq!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .reconciliation_state,
            LandingReconciliationState::Blocked
        );
        assert!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .safe_next
                .contains("merge-resolution status")
        );
        assert_eq!(command(&divergent.root, &["rev-parse", "main"]), unrelated);

        let conflicting = fixture();
        park(&conflicting);
        let selected = load_graph(conflicting.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&conflicting.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        fs::write(conflicting.root.join("result.txt"), "target version\n").unwrap();
        command(&conflicting.root, &["add", "result.txt"]);
        command(&conflicting.root, &["commit", "-m", "conflicting target"]);
        let target = command(&conflicting.root, &["rev-parse", "main"]);
        let error = resume_pending(&conflicting.dir, &conflicting.task_id).unwrap_err();
        assert!(error.to_string().contains("reconciliation conflicted"));
        let graph = load_graph(conflicting.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&conflicting.task_id).unwrap();
        assert_eq!(task.completion_candidate.as_ref(), Some(&selected));
        assert_eq!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .reconciliation_state,
            LandingReconciliationState::Blocked
        );
        assert_eq!(command(&conflicting.root, &["rev-parse", "main"]), target);
        assert_eq!(
            fs::read(conflicting.worker.join("result.txt")).unwrap(),
            b"accepted\n"
        );
    }

    #[test]
    fn changed_inputs_and_failed_refreshed_validation_fail_closed() {
        let changed = fixture();
        fs::write(changed.root.join("base.txt"), "dirty\n").unwrap();
        run_at(
            &changed.dir,
            &changed.task_id,
            "refs/heads/main",
            Some(&changed.worker),
        )
        .unwrap();
        command(&changed.root, &["restore", "base.txt"]);
        fs::write(changed.root.join("advance.txt"), "advance\n").unwrap();
        command(&changed.root, &["add", "advance.txt"]);
        command(&changed.root, &["commit", "-m", "advance"]);
        let target = command(&changed.root, &["rev-parse", "main"]);
        let selected = load_graph(changed.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&changed.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        modify_graph(changed.dir.join("graph.jsonl"), |graph| {
            graph.get_task_mut(&changed.task_id).unwrap().description =
                Some("changed required input\n\n## Validation\nDifferent".into());
            true
        })
        .unwrap();
        let error = resume_pending(&changed.dir, &changed.task_id).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("validation evidence no longer resolves")
                || error.to_string().contains("requirements"),
            "unexpected changed-input refusal: {error:#}"
        );
        let graph = load_graph(changed.dir.join("graph.jsonl")).unwrap();
        assert_eq!(
            graph
                .get_task(&changed.task_id)
                .unwrap()
                .completion_candidate
                .as_ref(),
            Some(&selected)
        );
        assert_eq!(command(&changed.root, &["rev-parse", "main"]), target);

        let failed = fixture_with_validation(Some("test ! -f target-blocks-validation"));
        fs::write(failed.root.join("base.txt"), "dirty\n").unwrap();
        run_at(
            &failed.dir,
            &failed.task_id,
            "refs/heads/main",
            Some(&failed.worker),
        )
        .unwrap();
        command(&failed.root, &["restore", "base.txt"]);
        fs::write(
            failed.root.join("target-blocks-validation"),
            "target input\n",
        )
        .unwrap();
        command(&failed.root, &["add", "target-blocks-validation"]);
        command(
            &failed.root,
            &["commit", "-m", "advance with validation failure"],
        );
        let target = command(&failed.root, &["rev-parse", "main"]);
        let selected = load_graph(failed.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&failed.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        let error = resume_pending(&failed.dir, &failed.task_id).unwrap_err();
        assert!(error.to_string().contains("validation failed"));
        let graph = load_graph(failed.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&failed.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.completion_candidate.as_ref(), Some(&selected));
        assert_eq!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .reconciliation_state,
            LandingReconciliationState::Blocked
        );
        assert_eq!(command(&failed.root, &["rev-parse", "main"]), target);
        let record = load_reconciliation_record(&failed.dir, &failed.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.state, LandingReconciliationState::Blocked);
        assert_eq!(record.validation_evidence.len(), 1);
        assert_eq!(
            fs::read(failed.worker.join("result.txt")).unwrap(),
            b"accepted\n"
        );
    }

    #[test]
    fn target_fence_change_during_refresh_never_consumes_candidate() {
        let fixture = fixture_with_validation(Some(
            "if test -f move-target; then git update-ref refs/heads/main $(git rev-parse refs/heads/main^); fi",
        ));
        fs::write(fixture.root.join("base.txt"), "dirty\n").unwrap();
        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        command(&fixture.root, &["restore", "base.txt"]);
        fs::write(fixture.root.join("move-target"), "advance\n").unwrap();
        command(&fixture.root, &["add", "move-target"]);
        command(&fixture.root, &["commit", "-m", "advance"]);
        let selected = load_graph(fixture.dir.join("graph.jsonl"))
            .unwrap()
            .get_task(&fixture.task_id)
            .unwrap()
            .completion_candidate
            .clone()
            .unwrap();
        let error = resume_pending(&fixture.dir, &fixture.task_id).unwrap_err();
        assert!(
            error.to_string().contains("target fence changed"),
            "{error:#}"
        );
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.completion_candidate.as_ref(), Some(&selected));
        assert_eq!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .reconciliation_state,
            LandingReconciliationState::Blocked
        );
        assert!(
            !is_ancestor(
                &fixture.root,
                &fixture.candidate,
                &command(&fixture.root, &["rev-parse", "main"]),
            )
            .unwrap()
        );
    }

    #[test]
    fn dirty_checkout_preserves_reviewed_base_when_target_already_advanced() {
        for candidate_already_reachable in [false, true] {
            let fixture = fixture();
            if candidate_already_reachable {
                command(&fixture.root, &["merge", "--ff-only", &fixture.candidate]);
            }
            fs::write(fixture.root.join("advanced.txt"), "advanced target\n").unwrap();
            command(&fixture.root, &["add", "advanced.txt"]);
            command(
                &fixture.root,
                &["commit", "-m", "target advanced before dirt"],
            );
            let advanced = command(&fixture.root, &["rev-parse", "main"]);
            fs::write(fixture.root.join("base.txt"), "operator dirt\n").unwrap();

            run_at(
                &fixture.dir,
                &fixture.task_id,
                "refs/heads/main",
                Some(&fixture.worker),
            )
            .unwrap();
            let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
            let blocker = graph
                .get_task(&fixture.task_id)
                .unwrap()
                .completion_blocker
                .as_ref()
                .unwrap();
            assert_eq!(
                blocker.target_ref_oid.as_deref(),
                Some(fixture.integrated.as_str()),
                "dirtiness must not bless the already-advanced target"
            );
            drop(graph);
            command(&fixture.root, &["restore", "base.txt"]);
            assert!(resume_pending(&fixture.dir, &fixture.task_id).unwrap());
            let landed = command(&fixture.root, &["rev-parse", "main"]);
            assert!(is_ancestor(&fixture.root, &advanced, &landed).unwrap());
            assert!(is_ancestor(&fixture.root, &fixture.candidate, &landed).unwrap());
            let record = load_reconciliation_record(&fixture.dir, &fixture.task_id)
                .unwrap()
                .unwrap();
            assert_eq!(record.state, LandingReconciliationState::Landed);
            assert!(!record.validation_evidence.is_empty());
        }
    }

    #[test]
    fn initially_advanced_descendant_parks_then_uses_supported_resume() {
        let fixture = fixture();
        fs::write(fixture.root.join("other.txt"), "other\n").unwrap();
        command(&fixture.root, &["add", "other.txt"]);
        command(&fixture.root, &["commit", "-m", "main moved"]);
        let moved = command(&fixture.root, &["rev-parse", "main"]);
        assert_ne!(moved, fixture.integrated);

        run_at(
            &fixture.dir,
            &fixture.task_id,
            "refs/heads/main",
            Some(&fixture.worker),
        )
        .unwrap();
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task(&fixture.task_id).unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.assigned.is_none());
        assert!(
            task.completion_blocker
                .as_ref()
                .unwrap()
                .safe_next
                .contains("wg resume")
        );
        drop(graph);

        assert!(
            super::super::resume::resume_landing_finalization(&fixture.dir, &fixture.task_id)
                .unwrap()
        );
        let landed = command(&fixture.root, &["rev-parse", "main"]);
        assert!(is_ancestor(&fixture.root, &moved, &landed).unwrap());
        assert!(is_ancestor(&fixture.root, &fixture.candidate, &landed).unwrap());
    }
}
