use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{
    COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest, ContentDigest,
    EvidenceRef, GitOutput, OutputRef, ResolvedReviewBundle, ReviewResolver,
};
use worksgood::completion_review::{
    CompletionReviewBinding, ManifestReviewer, ReviewFailureClass, ReviewValveOutcome,
    ReviewValveStatus, ReviewerKind, ReviewerUnavailable, SemanticReview, StoredReviewReceipt,
    load_stored_review_receipt, load_stored_review_receipt_by_digest,
    run_review_valve_bound_reusing_observed,
};
use worksgood::completion_review_model::ExactModelReviewer;
use worksgood::completion_task::{
    CompletionCandidateRefs, completion_contract, requirements_digest, task_requirements_bytes,
};
use worksgood::config::{Config, DispatchRole};
use worksgood::graph::{CompletionBlockerKind, LogEntry, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, modify_graph};

const COMPLETION_STORE_DIR: &str = "completion/v3";

pub fn store(dir: &Path) -> Result<CompletionArtifactStore> {
    CompletionArtifactStore::open(dir.join(COMPLETION_STORE_DIR)).map_err(Into::into)
}

/// Snapshot one worker output/evidence file and print its immutable JSON
/// reference for insertion into a completion manifest.
pub fn put_object(
    dir: &Path,
    path: &Path,
    media_type: &str,
    evidence_kind: Option<&str>,
) -> Result<()> {
    let value = put_object_value(dir, path, media_type, evidence_kind)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub fn build_manifest_command(
    dir: &Path,
    id: &str,
    summary_path: &Path,
    output_ref_paths: &[PathBuf],
    evidence_ref_paths: &[PathBuf],
    git_output: bool,
    source_revision: Option<&str>,
) -> Result<()> {
    reject_control_plane_source(dir, summary_path, "worker summary")?;
    let summary = read_regular_file(summary_path, "worker summary")?;
    let mut outputs = Vec::with_capacity(output_ref_paths.len());
    for path in output_ref_paths {
        reject_control_plane_source(dir, path, "output reference")?;
        outputs.push(
            serde_json::from_slice::<OutputRef>(&read_regular_file(path, "output reference")?)
                .with_context(|| {
                    format!("invalid immutable output reference {}", path.display())
                })?,
        );
    }
    let mut evidence = Vec::with_capacity(evidence_ref_paths.len());
    for path in evidence_ref_paths {
        reject_control_plane_source(dir, path, "evidence reference")?;
        evidence.push(
            serde_json::from_slice::<EvidenceRef>(&read_regular_file(path, "evidence reference")?)
                .with_context(|| {
                    format!("invalid immutable evidence reference {}", path.display())
                })?,
        );
    }
    let cwd = std::env::current_dir()?;
    let manifest = build_manifest(
        dir,
        id,
        &summary,
        outputs,
        evidence,
        git_output,
        source_revision,
        Some(&cwd),
    )?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

pub(crate) fn build_manifest(
    dir: &Path,
    id: &str,
    summary: &[u8],
    mut outputs: Vec<OutputRef>,
    evidence: Vec<EvidenceRef>,
    git_output: bool,
    source_revision: Option<&str>,
    worker_worktree: Option<&Path>,
) -> Result<CompletionManifest> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    require_source_owner(task, id)?;
    let contract = completion_contract(task)?;
    if git_output {
        if contract != worksgood::simple_land::CompletionContract::Land {
            bail!("--git is valid only for Land tasks");
        }
        if !outputs.is_empty() {
            bail!("Land manifests use one auto-built Git output, not --output-ref");
        }
        let worker = worker_worktree.context("--git requires the retained worker worktree")?;
        let project = dir
            .parent()
            .context("workgraph directory has no project root")?;
        let integrated = git(project, &["rev-parse", "refs/heads/main"])?;
        let commit = git(worker, &["rev-parse", "HEAD"])?;
        let tree = git(worker, &["rev-parse", "HEAD^{tree}"])?;
        let ancestor = Command::new("git")
            .args(["merge-base", "--is-ancestor", &integrated, &commit])
            .current_dir(worker)
            .status()?;
        if !ancestor.success() {
            bail!("worker HEAD does not integrate current main; merge main, revalidate, and retry");
        }
        worksgood::control_plane::assert_tree_has_no_control_plane(project, &commit)?;
        let diff = Command::new("git")
            .args([
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &integrated,
                &commit,
                "--",
            ])
            .current_dir(worker)
            .output()?;
        if !diff.status.success() {
            bail!("failed to construct exact Git diff bundle");
        }
        outputs.push(OutputRef::Git(GitOutput {
            commit_oid: commit.clone(),
            integrated_main_oid: integrated,
            tree_oid: tree,
            diff_bundle_digest: ContentDigest::of_bytes(&diff.stdout),
        }));
    }
    let revision = source_revision
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| worker_worktree.and_then(|worker| git(worker, &["rev-parse", "HEAD"]).ok()))
        .unwrap_or_else(|| format!("worker-session:{}", task.assigned.as_deref().unwrap_or(id)));
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: id.to_string(),
        generation: task.lifecycle.generation,
        completion_contract: contract,
        requirements_digest: requirements_digest(task)?,
        source_revision: revision,
        outputs,
        validation_evidence: evidence,
        worker_summary_digest: ContentDigest::of_bytes(summary),
    };
    manifest.validate().map_err(anyhow::Error::msg)?;
    Ok(manifest)
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub(crate) fn put_object_value(
    dir: &Path,
    path: &Path,
    media_type: &str,
    evidence_kind: Option<&str>,
) -> Result<serde_json::Value> {
    reject_control_plane_source(dir, path, "completion object")?;
    let store = store(dir)?;
    let artifact = store.put_file(path, media_type)?;
    if let Some(kind) = evidence_kind {
        if kind.trim().is_empty() {
            bail!("evidence kind must not be empty");
        }
        Ok(serde_json::to_value(EvidenceRef {
            content_digest: artifact.content_digest,
            immutable_locator: artifact.immutable_locator,
            evidence_kind: kind.to_string(),
            media_type: artifact.media_type,
            size: artifact.size,
            review_projection: artifact.review_projection,
        })?)
    } else {
        Ok(serde_json::to_value(OutputRef::Artifact(artifact))?)
    }
}

pub fn run(dir: &Path, id: &str, manifest_path: &Path, summary_path: &Path) -> Result<()> {
    let config = Config::load_merged(dir)?;
    let mut flip: Box<dyn ManifestReviewer + '_> =
        match ExactModelReviewer::for_role(&config, ReviewerKind::Flip, DispatchRole::Reviewer) {
            Ok(reviewer) => Box::new(reviewer),
            Err(error) => Box::new(SetupUnavailableReviewer::new("reviewer", error)),
        };
    let mut eval: Box<dyn ManifestReviewer + '_> =
        match ExactModelReviewer::for_role(&config, ReviewerKind::Eval, DispatchRole::Evaluator) {
            Ok(reviewer) => Box::new(reviewer),
            Err(error) => Box::new(SetupUnavailableReviewer::new("evaluator", error)),
        };
    let outcome = match run_with_reviewers(
        dir,
        id,
        manifest_path,
        summary_path,
        flip.as_mut(),
        eval.as_mut(),
    ) {
        Ok(outcome) => outcome,
        Err(error) if task_is_waiting_for_review(dir, id) => {
            eprintln!("{error:#}");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    print_review_findings(dir, &outcome);
    match outcome.status {
        ReviewValveStatus::Accepted => {
            println!(
                "Completion candidate accepted: manifest={} FLIP=pass eval=pass",
                outcome.flip.receipt.manifest_digest
            );
            Ok(())
        }
        ReviewValveStatus::IncompleteEvidence => bail!(
            "incomplete deterministic evidence for manifest {}; repair the immutable evidence and submit a new manifest",
            outcome.flip.receipt.manifest_digest
        ),
        status if !config.agency.completion_review_strict => {
            println!(
                "Completion candidate recorded with advisory model review status={status:?}: manifest={}. Deterministic publication may continue; inspect `wg show {id}` for history.",
                outcome.flip.receipt.manifest_digest
            );
            Ok(())
        }
        ReviewValveStatus::FlipRejected => bail!(
            "strict FLIP rejected manifest {}; repair using the findings above (bounded by agency.gate_max_attempts) or request operator review",
            outcome.flip.receipt.manifest_digest
        ),
        ReviewValveStatus::EvalRejected => bail!(
            "strict eval rejected manifest {}; repair using the findings above (bounded by agency.gate_max_attempts) or request operator review",
            outcome.flip.receipt.manifest_digest
        ),
        ReviewValveStatus::ReviewUnavailable => bail!(
            "strict review unavailable for manifest {}; the candidate is preserved and no source quality failure was recorded",
            outcome.flip.receipt.manifest_digest
        ),
    }
}

fn task_is_waiting_for_review(dir: &Path, id: &str) -> bool {
    load_graph(dir.join("graph.jsonl"))
        .ok()
        .and_then(|graph| {
            graph.get_task(id).map(|task| {
                task.status == Status::Waiting
                    && task
                        .completion_blocker
                        .as_ref()
                        .is_some_and(|blocker| blocker.kind == CompletionBlockerKind::NeedsReview)
            })
        })
        .unwrap_or(false)
}

fn print_review_findings(dir: &Path, outcome: &ReviewValveOutcome) {
    let Ok(store) = store(dir) else {
        return;
    };
    for review in std::iter::once(&outcome.flip).chain(outcome.eval.iter()) {
        let Ok(bytes) = store.read_artifact(
            &review.findings_object,
            worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
        ) else {
            continue;
        };
        let Ok(findings) =
            serde_json::from_slice::<Vec<worksgood::completion_review::ReviewFinding>>(&bytes)
        else {
            continue;
        };
        for finding in findings {
            eprintln!(
                "Review {:?} [{}]: {}{}",
                review.receipt.reviewer_kind,
                finding.code,
                finding.message,
                finding
                    .evidence
                    .as_deref()
                    .map(|evidence| format!(" (evidence: {evidence})"))
                    .unwrap_or_default()
            );
        }
    }
}

pub fn run_with_reviewers(
    dir: &Path,
    id: &str,
    manifest_path: &Path,
    summary_path: &Path,
    flip: &mut dyn ManifestReviewer,
    eval: &mut dyn ManifestReviewer,
) -> Result<ReviewValveOutcome> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?;
    require_source_owner(task, id)?;
    let requirements = task_requirements_bytes(task)?;
    let requirements_digest = requirements_digest(task)?;
    let expected_contract = completion_contract(task)?;

    reject_control_plane_source(dir, manifest_path, "manifest")?;
    reject_control_plane_source(dir, summary_path, "worker summary")?;
    let manifest_bytes = read_regular_file(manifest_path, "manifest")?;
    let manifest: CompletionManifest =
        serde_json::from_slice(&manifest_bytes).context("completion manifest is not valid JSON")?;
    manifest.validate().map_err(anyhow::Error::msg)?;
    if manifest.task_id != id
        || manifest.generation != task.lifecycle.generation
        || manifest.completion_contract != expected_contract
        || manifest.requirements_digest != requirements_digest
    {
        bail!("manifest does not bind the current task id, generation, contract, and requirements");
    }
    let config = Config::load_merged(dir)?;
    let summary_bytes = read_regular_file(summary_path, "worker summary")?;
    if worksgood::completion_manifest::ContentDigest::of_bytes(&summary_bytes)
        != manifest.worker_summary_digest
    {
        bail!("worker summary digest does not match manifest");
    }

    let store = store(dir)?;
    let requirements_ref =
        store.put_bytes(&requirements, "application/vnd.worksgood.requirements+json")?;
    let summary_ref = store.put_bytes(&summary_bytes, "text/plain")?;
    let manifest_ref = store.put_manifest(&manifest)?;
    let dependency_outputs = collect_dependency_outputs(&store, &graph, task)?;
    let candidate = CompletionCandidateRefs {
        manifest: manifest_ref.clone(),
        requirements: requirements_ref,
        worker_summary: summary_ref,
        dependency_outputs: dependency_outputs.clone(),
        review_binding: None,
        flip_receipt: None,
        eval_receipt: None,
    };

    // Select the immutable candidate before review. This is a single compact
    // graph projection; resolver/reviewer failures preserve it without
    // scheduling a transaction, source retry, or finalizer.
    let source_accounting = super::completion_done::source_accounting(dir, task);
    let expected_binding = CompletionReviewBinding {
        task_id: id.to_string(),
        generation: task.lifecycle.generation,
        attempt_id: task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.clone()),
        attempt_fence: task.lifecycle.fence,
        candidate_sequence: 0,
    };
    let review_binding = select_candidate(
        &graph_path,
        &expected_binding,
        &requirements_digest,
        candidate,
        &source_accounting,
    )?;
    // Candidate selection is durably appended before any external reviewer
    // call. A crash can therefore recover the exact immutable candidate
    // without fabricating a schedulable `.flip-*`/`.evaluate-*` task.
    super::adaptive_agency::prepare_candidate(dir, id)?;

    let project_root = dir
        .parent()
        .context("workgraph directory has no project root")?;
    let resolver = ReviewResolver::new(&store);
    let resolved = if expected_contract == worksgood::simple_land::CompletionContract::Land {
        resolver.repository(project_root).resolve_submission(
            &manifest_ref,
            &requirements,
            &summary_bytes,
            &dependency_outputs,
        )
    } else {
        resolver.resolve_submission(
            &manifest_ref,
            &requirements,
            &summary_bytes,
            &dependency_outputs,
        )
    }
    .and_then(|bundle| {
        worksgood::completion_validation::verify_validation_evidence(
            task,
            &manifest,
            Some(&review_binding),
            &bundle,
            project_root,
            dir,
        )?;
        Ok(bundle)
    });
    let manifest_digest = manifest.digest().map_err(anyhow::Error::msg)?;
    let selected_graph = load_graph(&graph_path)?;
    let selected_task = selected_graph
        .get_task(id)
        .with_context(|| format!("task '{id}' disappeared after candidate selection"))?;
    let selected_candidate = selected_task
        .completion_candidate
        .as_ref()
        .context("selected completion candidate disappeared before review")?;
    let inspected_output_digests = resolved
        .as_ref()
        .ok()
        .map(|bundle| bundle.inspected_output_digests.as_slice());
    let prior_flip = prior_receipt_for_route(
        &store,
        selected_task,
        selected_candidate.flip_receipt.as_ref(),
        &manifest_digest,
        &requirements_digest,
        &review_binding,
        ReviewerKind::Flip,
        flip.route(),
        inspected_output_digests,
    )?;
    let prior_eval = prior_receipt_for_route(
        &store,
        selected_task,
        selected_candidate.eval_receipt.as_ref(),
        &manifest_digest,
        &requirements_digest,
        &review_binding,
        ReviewerKind::Eval,
        eval.route(),
        inspected_output_digests,
    )?;

    if config.agency.completion_review_strict
        && let Ok(bundle) = resolved.as_ref()
    {
        enforce_strict_review_budget(
            dir,
            selected_task,
            &manifest_digest,
            &requirements_digest,
            &review_binding,
            bundle,
            flip.route(),
            eval.route(),
            prior_flip.as_ref(),
            prior_eval.as_ref(),
            config.agency.gate_max_attempts.max(1),
        )?;
    }

    let mut adaptive_observer = super::adaptive_agency::live_review_observer(dir, id)?;
    let outcome = run_review_valve_bound_reusing_observed(
        &store,
        &manifest_digest,
        &requirements_digest,
        resolved,
        flip,
        eval,
        Some(&review_binding),
        prior_flip,
        prior_eval,
        &mut adaptive_observer,
    )?;
    record_review_outcome(
        &graph_path,
        id,
        &review_binding,
        &manifest_digest,
        &requirements_digest,
        &outcome,
    )?;
    // Dual-write the immutable adaptive candidate/attempt ledger. This is an
    // observation append only: it has no graph, retry, publication, or
    // lifecycle capability, and therefore cannot apply the verdict it records.
    super::adaptive_agency::sync_candidate_and_reviews(dir, id)?;
    Ok(outcome)
}

pub(crate) fn require_source_owner(task: &Task, id: &str) -> Result<()> {
    if task.status != Status::InProgress {
        bail!("task '{id}' must be in progress to submit a completion manifest");
    }
    // Unit tests construct isolated graph fixtures inside the worker process
    // that is running `cargo test`; the outer WG_TASK_ID/WG_AGENT_ID belong to
    // that test runner, not to each fixture. Production binaries and
    // integration tests still enforce the ambient worker binding.
    #[cfg(not(test))]
    {
        if let Ok(bound_task) = std::env::var("WG_TASK_ID")
            && !bound_task.is_empty()
            && bound_task != id
        {
            bail!("worker is bound to task '{bound_task}', not '{id}'");
        }
        if let Ok(agent) = std::env::var("WG_AGENT_ID")
            && !agent.is_empty()
            && task.assigned.as_deref() != Some(agent.as_str())
        {
            bail!("worker '{agent}' does not own task '{id}'");
        }
    }
    Ok(())
}

fn reject_control_plane_source(dir: &Path, path: &Path, label: &str) -> Result<()> {
    let project = dir
        .parent()
        .context("workgraph directory has no project root")?
        .canonicalize()
        .context("canonicalize project root")?;
    let source = path
        .canonicalize()
        .with_context(|| format!("canonicalize {label} path {}", path.display()))?;
    if let Ok(relative) = source.strip_prefix(&project)
        && worksgood::control_plane::is_protected_repo_path(relative.as_os_str().as_encoded_bytes())
    {
        bail!("{label} must not come from the protected .wg control plane");
    }
    Ok(())
}

fn read_regular_file(path: &Path, label: &str) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {label} metadata at {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{label} must be a regular non-symlink file");
    }
    fs::read(path).with_context(|| format!("read {label} at {}", path.display()))
}

fn select_candidate(
    graph_path: &Path,
    expected_binding: &CompletionReviewBinding,
    expected_requirements: &worksgood::completion_manifest::ContentDigest,
    mut candidate: CompletionCandidateRefs,
    source_accounting: &super::completion_done::SourceAccounting,
) -> Result<CompletionReviewBinding> {
    let mut refusal = None;
    let mut selected_binding = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(&expected_binding.task_id) else {
            refusal = Some("task disappeared while selecting candidate".to_string());
            return false;
        };
        if task.status != Status::InProgress
            || task.lifecycle.generation != expected_binding.generation
            || task.lifecycle.fence != expected_binding.attempt_fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != expected_binding.attempt_id.as_deref()
        {
            refusal = Some(
                "task generation, attempt, fence, or ownership changed while selecting candidate"
                    .into(),
            );
            return false;
        }
        if requirements_digest(task).ok().as_ref() != Some(expected_requirements) {
            refusal = Some("task requirements changed while selecting candidate".into());
            return false;
        }
        if let Some(binding) = task
            .completion_candidate
            .as_ref()
            .filter(|current| same_immutable_candidate(current, &candidate))
            .and_then(|current| current.review_binding.as_ref())
            .filter(|binding| same_source_tuple(binding, expected_binding))
            .cloned()
        {
            selected_binding = Some(binding);
            let mut changed = false;
            if source_accounting.usage.is_some() && task.token_usage != source_accounting.usage {
                task.token_usage.clone_from(&source_accounting.usage);
                changed = true;
            }
            if source_accounting.executor.is_some()
                && task.actual_executor != source_accounting.executor
            {
                task.actual_executor.clone_from(&source_accounting.executor);
                changed = true;
            }
            if source_accounting.model.is_some() && task.actual_model != source_accounting.model {
                task.actual_model.clone_from(&source_accounting.model);
                changed = true;
            }
            return changed;
        }
        let candidate_sequence = task
            .completion_review_activity
            .iter()
            .filter_map(|activity| {
                activity
                    .binding
                    .as_ref()
                    .map(|binding| binding.candidate_sequence)
            })
            .chain(
                task.completion_candidate
                    .as_ref()
                    .and_then(|current| current.review_binding.as_ref())
                    .map(|binding| binding.candidate_sequence),
            )
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let binding = CompletionReviewBinding {
            candidate_sequence,
            ..expected_binding.clone()
        };
        candidate.review_binding = Some(binding.clone());
        selected_binding = Some(binding);
        task.completion_candidate = Some(candidate);
        if source_accounting.usage.is_some() {
            task.token_usage.clone_from(&source_accounting.usage);
        }
        if source_accounting.executor.is_some() {
            task.actual_executor.clone_from(&source_accounting.executor);
        }
        if source_accounting.model.is_some() {
            task.actual_model.clone_from(&source_accounting.model);
        }
        task.completion_disposition = None;
        task.completion_receipt = None;
        task.completion_blocker = None;
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("completion-submit".to_string()),
            user: None,
            message: "Selected immutable completion candidate; prior review receipts invalidated"
                .to_string(),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    selected_binding.context("candidate selection produced no binding")
}

#[allow(clippy::too_many_arguments)]
fn prior_receipt_for_route(
    store: &CompletionArtifactStore,
    task: &Task,
    selected_reference: Option<&worksgood::completion_manifest::ArtifactOutput>,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    binding: &CompletionReviewBinding,
    kind: ReviewerKind,
    route: &str,
    inspected_output_digests: Option<&[String]>,
) -> Result<Option<StoredReviewReceipt>> {
    let selected = selected_reference
        .map(|reference| load_stored_review_receipt(store, reference))
        .transpose()?;
    let reusable = |stored: &StoredReviewReceipt| {
        inspected_output_digests.is_some_and(|outputs| {
            stored.receipt.is_reusable_semantic(
                manifest_digest,
                requirements_digest,
                kind,
                route,
                Some(binding),
                outputs,
            )
        })
    };
    if selected.as_ref().is_some_and(reusable) {
        return Ok(selected);
    }

    // The selected projection stores one receipt per reviewer kind. If an
    // operator changes a route and later restores it, the earlier immutable
    // route-specific receipt remains in activity history and is still the
    // semantic decision for this exact candidate. Reuse it instead of paying
    // for the same candidate/reviewer/route twice.
    for activity in task.completion_review_activity.iter().rev() {
        if activity.reviewer_kind != kind
            || &activity.manifest_digest != manifest_digest
            || &activity.requirements_digest != requirements_digest
            || activity.binding.as_ref() != Some(binding)
            || activity.model_route.as_deref() != Some(route)
            || !matches!(
                activity.verdict,
                worksgood::simple_land::ReviewVerdict::Pass
                    | worksgood::simple_land::ReviewVerdict::Reject
            )
        {
            continue;
        }
        let Ok(digest) = ContentDigest::parse(&activity.activity_id) else {
            continue;
        };
        let Ok(stored) = load_stored_review_receipt_by_digest(store, &digest) else {
            continue;
        };
        if reusable(&stored) {
            return Ok(Some(stored));
        }
    }
    Ok(selected)
}

fn same_source_tuple(
    current: &CompletionReviewBinding,
    expected: &CompletionReviewBinding,
) -> bool {
    current.task_id == expected.task_id
        && current.generation == expected.generation
        && current.attempt_id == expected.attempt_id
        && current.attempt_fence == expected.attempt_fence
}

fn same_immutable_candidate(
    current: &CompletionCandidateRefs,
    proposed: &CompletionCandidateRefs,
) -> bool {
    current.manifest == proposed.manifest
        && current.requirements == proposed.requirements
        && current.worker_summary == proposed.worker_summary
        && current.dependency_outputs == proposed.dependency_outputs
}

fn review_belongs_to_current_source_attempt(
    task: &Task,
    activity: &worksgood::completion_review::CompletionReviewActivity,
) -> bool {
    let current_attempt_id = task
        .lifecycle
        .current_attempt
        .as_ref()
        .map(|attempt| attempt.id.as_str());
    activity.binding.as_ref().is_none_or(|binding| {
        binding.generation == task.lifecycle.generation
            && binding.attempt_id.as_deref() == current_attempt_id
    })
}

fn semantic_iterations_for_current_source_attempt(
    task: &Task,
    activities: &[worksgood::completion_review::VerifiedCompletionReviewActivity],
) -> u32 {
    // One immutable candidate consumes one semantic iteration even though its
    // FLIP and Eval decisions are separate receipts. Unbound legacy rows stay
    // counted fail-closed; rows bound to a superseded source attempt do not.
    activities
        .iter()
        .filter(|activity| review_belongs_to_current_source_attempt(task, &activity.activity))
        .filter(|activity| {
            matches!(
                activity.verdict,
                worksgood::simple_land::ReviewVerdict::Pass
                    | worksgood::simple_land::ReviewVerdict::Reject
            )
        })
        .map(|activity| {
            (
                activity.manifest_digest.to_string(),
                activity
                    .binding
                    .as_ref()
                    .map(|binding| binding.generation)
                    .unwrap_or_default(),
                activity
                    .binding
                    .as_ref()
                    .and_then(|binding| binding.attempt_id.clone()),
                activity
                    .binding
                    .as_ref()
                    .map(|binding| binding.candidate_sequence)
                    .unwrap_or_default(),
            )
        })
        .collect::<std::collections::HashSet<_>>()
        .len() as u32
}

pub(crate) fn rejected_current_candidate_at_source_budget(
    dir: &Path,
    task: &Task,
    candidate: &CompletionCandidateRefs,
    max_iterations: u32,
) -> Result<Option<u32>> {
    let verified = worksgood::completion_review::verified_review_activities(dir, task);
    if verified.invalid_count > 0 {
        bail!(
            "strict completion review budget cannot be verified: {} projected receipt(s) are invalid",
            verified.invalid_count
        );
    }
    let semantic_iterations =
        semantic_iterations_for_current_source_attempt(task, &verified.activities);
    let current_rejected = verified.activities.iter().any(|activity| {
        review_belongs_to_current_source_attempt(task, &activity.activity)
            && activity.manifest_digest == candidate.manifest.content_digest
            && activity.binding.as_ref() == candidate.review_binding.as_ref()
            && activity.verdict == worksgood::simple_land::ReviewVerdict::Reject
    });
    Ok((semantic_iterations >= max_iterations && current_rejected).then_some(semantic_iterations))
}

#[allow(clippy::too_many_arguments)]
fn enforce_strict_review_budget(
    dir: &Path,
    task: &Task,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    binding: &CompletionReviewBinding,
    bundle: &ResolvedReviewBundle,
    flip_route: &str,
    eval_route: &str,
    prior_flip: Option<&StoredReviewReceipt>,
    prior_eval: Option<&StoredReviewReceipt>,
    max_iterations: u32,
) -> Result<()> {
    let verified = worksgood::completion_review::verified_review_activities(dir, task);
    if verified.invalid_count > 0 {
        bail!(
            "strict completion review budget cannot be verified: {} projected receipt(s) are invalid",
            verified.invalid_count
        );
    }
    // Candidate revisions are bounded within one source attempt. An operator
    // retry gets a fresh budget, while FLIP+Eval and route changes for one
    // immutable candidate still consume only one semantic iteration.
    // Infrastructure receipts remain visible but do not consume that budget.
    let semantic_iterations =
        semantic_iterations_for_current_source_attempt(task, &verified.activities);
    if semantic_iterations < max_iterations {
        return Ok(());
    }

    let flip_reusable = prior_flip.is_some_and(|stored| {
        stored.receipt.is_reusable_semantic(
            manifest_digest,
            requirements_digest,
            ReviewerKind::Flip,
            flip_route,
            Some(binding),
            &bundle.inspected_output_digests,
        )
    });
    let flip_infrastructure_retry = prior_flip.is_some_and(|stored| {
        receipt_is_exact_infrastructure_retry(
            stored,
            manifest_digest,
            requirements_digest,
            ReviewerKind::Flip,
            flip_route,
            binding,
            &bundle.inspected_output_digests,
        )
    });
    let eval_reusable = prior_eval.is_some_and(|stored| {
        stored.receipt.is_reusable_semantic(
            manifest_digest,
            requirements_digest,
            ReviewerKind::Eval,
            eval_route,
            Some(binding),
            &bundle.inspected_output_digests,
        )
    });
    let current_rejected = prior_flip.is_some_and(|stored| {
        flip_reusable && stored.receipt.verdict == worksgood::simple_land::ReviewVerdict::Reject
    }) || prior_eval.is_some_and(|stored| {
        flip_reusable
            && eval_reusable
            && stored.receipt.verdict == worksgood::simple_land::ReviewVerdict::Reject
    });

    // A same-candidate provider failure may be retried separately. It did not
    // consume a semantic revision. A new candidate/route, or replay of an
    // already-rejected candidate at the task ceiling, parks without a call.
    if current_rejected || (!flip_reusable && !flip_infrastructure_retry) {
        park_for_review_budget(dir, &task.id, semantic_iterations, max_iterations)?;
        bail!(
            "Needs review: strict model-review attempt limit ({max_iterations}) reached; no further model call was made and the worker was released for operator accept/reject"
        );
    }
    Ok(())
}

fn receipt_is_exact_infrastructure_retry(
    stored: &StoredReviewReceipt,
    manifest_digest: &ContentDigest,
    requirements_digest: &ContentDigest,
    kind: ReviewerKind,
    route: &str,
    binding: &CompletionReviewBinding,
    inspected_output_digests: &[String],
) -> bool {
    let receipt = &stored.receipt;
    receipt.receipt_version == worksgood::completion_review::COMPLETION_REVIEW_RECEIPT_VERSION
        && &receipt.manifest_digest == manifest_digest
        && &receipt.requirements_digest == requirements_digest
        && receipt.reviewer_kind == kind
        && receipt.verdict == worksgood::simple_land::ReviewVerdict::Unavailable
        && receipt.failure_class == Some(ReviewFailureClass::ReviewerUnavailable)
        && receipt.model_route.as_deref() == Some(route)
        && receipt.binding.as_ref() == Some(binding)
        && receipt.inspected_output_digests == inspected_output_digests
}

pub(crate) fn park_for_review_budget(
    dir: &Path,
    id: &str,
    semantic_iterations: u32,
    max_iterations: u32,
) -> Result<()> {
    super::completion_wait::park_needs_review(
        dir,
        id,
        &format!(
            "task-level semantic review ceiling {semantic_iterations}/{max_iterations} reached; no further model call was made"
        ),
    )
}

fn record_review_outcome(
    graph_path: &Path,
    id: &str,
    expected_binding: &CompletionReviewBinding,
    manifest_digest: &worksgood::completion_manifest::ContentDigest,
    expected_requirements: &worksgood::completion_manifest::ContentDigest,
    outcome: &ReviewValveOutcome,
) -> Result<()> {
    let mut refusal = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared while recording review".to_string());
            return false;
        };
        if task.lifecycle.generation != expected_binding.generation
            || task.lifecycle.fence != expected_binding.attempt_fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != expected_binding.attempt_id.as_deref()
            || requirements_digest(task).ok().as_ref() != Some(expected_requirements)
        {
            refusal = Some(
                "task requirements, generation, attempt, or fence changed while review was running"
                    .to_string(),
            );
            return false;
        }
        let Some(candidate) = task.completion_candidate.as_mut() else {
            refusal = Some("completion candidate disappeared while recording review".to_string());
            return false;
        };
        if candidate.manifest.content_digest != *manifest_digest
            || candidate.review_binding.as_ref() != Some(expected_binding)
        {
            refusal = Some("completion candidate changed while review was running".to_string());
            return false;
        }
        let selected_flip = Some(outcome.flip.receipt_object.clone());
        let selected_eval = outcome
            .eval
            .as_ref()
            .map(|receipt| receipt.receipt_object.clone());
        let mut changed =
            candidate.flip_receipt != selected_flip || candidate.eval_receipt != selected_eval;
        candidate.flip_receipt = selected_flip;
        candidate.eval_receipt = selected_eval;
        for stored in std::iter::once(&outcome.flip).chain(outcome.eval.iter()) {
            let activity_id = stored.receipt_object.content_digest.to_string();
            if task
                .completion_review_activity
                .iter()
                .any(|activity| activity.activity_id == activity_id)
            {
                continue;
            }
            changed = true;
            task.completion_review_activity.push(
                worksgood::completion_review::CompletionReviewActivity {
                    activity_id,
                    reviewer_kind: stored.receipt.reviewer_kind,
                    verdict: stored.receipt.verdict,
                    manifest_digest: stored.receipt.manifest_digest.clone(),
                    requirements_digest: stored.receipt.requirements_digest.clone(),
                    binding: stored.receipt.binding.clone(),
                    findings_digest: Some(stored.receipt.findings_digest.clone()),
                    failure_class: stored.receipt.failure_class,
                    model_route: stored.receipt.model_route.clone(),
                    executor: stored.receipt.executor.clone(),
                    usage: stored.receipt.usage.clone(),
                    duration_ms: stored.receipt.duration_ms,
                    created_at: stored.receipt.created_at.clone(),
                },
            );
        }
        if changed {
            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: Some("completion-review".to_string()),
                user: None,
                message: format!(
                    "Manifest {} review outcome: {:?}",
                    manifest_digest, outcome.status
                ),
            });
        }
        changed
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}

pub(crate) fn collect_dependency_outputs(
    store: &CompletionArtifactStore,
    graph: &WorkGraph,
    task: &Task,
) -> Result<Vec<EvidenceRef>> {
    let mut outputs = Vec::new();
    for dependency_id in &task.after {
        let dependency = graph
            .get_task(dependency_id)
            .with_context(|| format!("dependency '{dependency_id}' is missing"))?;
        if dependency.status != Status::Done {
            bail!("dependency '{dependency_id}' is not Done");
        }
        let candidate = dependency.completion_candidate.as_ref().with_context(|| {
            format!("dependency '{dependency_id}' has no immutable completion candidate")
        })?;
        let manifest = store.read_manifest(
            &candidate.manifest,
            worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
        )?;
        for output in manifest.outputs {
            match output {
                OutputRef::Artifact(artifact) => outputs.push(EvidenceRef {
                    content_digest: artifact.content_digest,
                    immutable_locator: artifact.immutable_locator,
                    evidence_kind: format!("dependency-output:{dependency_id}"),
                    media_type: artifact.media_type,
                    size: artifact.size,
                    review_projection: artifact.review_projection,
                }),
                OutputRef::Git(_) => outputs.push(EvidenceRef {
                    content_digest: candidate.manifest.content_digest.clone(),
                    immutable_locator: candidate.manifest.immutable_locator.clone(),
                    evidence_kind: format!("dependency-manifest:{dependency_id}"),
                    media_type: "application/vnd.worksgood.completion+json".to_string(),
                    size: candidate.manifest.size,
                    review_projection: None,
                }),
                OutputRef::External(external) => {
                    outputs.push(external.operation_receipt);
                    outputs.push(external.verification_probe);
                }
            }
        }
    }
    Ok(outputs)
}

struct SetupUnavailableReviewer {
    route: String,
    message: String,
}

impl SetupUnavailableReviewer {
    fn new(role: &str, error: anyhow::Error) -> Self {
        Self {
            route: format!("unavailable:{role}"),
            message: format!("{error:#}"),
        }
    }
}

impl ManifestReviewer for SetupUnavailableReviewer {
    fn route(&self) -> &str {
        &self.route
    }

    fn review(
        &mut self,
        _kind: ReviewerKind,
        _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
    ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
        Err(ReviewerUnavailable {
            code: "reviewer.configuration_unavailable".to_string(),
            message: self.message.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use worksgood::completion_manifest::{COMPLETION_MANIFEST_VERSION, ContentDigest, OutputRef};
    use worksgood::completion_review::{ReviewFinding, SemanticReview, SemanticVerdict};
    use worksgood::completion_task::{
        load_exact_review_pair, load_submission_bytes, task_submission,
    };
    use worksgood::graph::{CompletionContract, Node};
    use worksgood::lifecycle::{AttemptDisposition, AttemptRef};
    use worksgood::parser::save_graph;

    struct FakeReviewer {
        route: String,
        result: std::result::Result<SemanticReview, ReviewerUnavailable>,
        calls: Arc<Mutex<Vec<ReviewerKind>>>,
    }

    impl ManifestReviewer for FakeReviewer {
        fn route(&self) -> &str {
            &self.route
        }

        fn review(
            &mut self,
            kind: ReviewerKind,
            _bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
        ) -> std::result::Result<SemanticReview, ReviewerUnavailable> {
            self.calls.lock().unwrap().push(kind);
            self.result.clone()
        }
    }

    fn semantic(verdict: SemanticVerdict) -> SemanticReview {
        SemanticReview {
            verdict,
            findings: if verdict == SemanticVerdict::Reject {
                vec![ReviewFinding::new("test.reject", "repair required")]
            } else {
                Vec::new()
            },
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        dir: std::path::PathBuf,
        manifest_path: std::path::PathBuf,
        summary_path: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempdir().unwrap();
        let dir = root.path().join(".wg");
        std::fs::create_dir_all(&dir).unwrap();
        let mut task = Task {
            id: "report".to_string(),
            title: "Produce report".to_string(),
            description: Some("Exact report.\n\n## Validation\nCheck bytes.".to_string()),
            status: Status::InProgress,
            completion_contract: CompletionContract::Report,
            ..Task::default()
        };
        task.lifecycle.generation = 3;
        let requirements = requirements_digest(&task).unwrap();
        let summary = b"report complete\n";
        let completion_store = store(&dir).unwrap();
        let output = completion_store
            .put_bytes(b"review me\n", "text/plain")
            .unwrap();
        let evidence = completion_store
            .evidence_from_bytes(b"validation ok\n", "validation", "text/plain")
            .unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Report,
            requirements_digest: requirements,
            source_revision: "session:test".to_string(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![evidence],
            worker_summary_digest: ContentDigest::of_bytes(summary),
        };
        let manifest_path = root.path().join("manifest.json");
        std::fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
        let summary_path = root.path().join("summary.txt");
        std::fs::write(&summary_path, summary).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();
        Fixture {
            _root: root,
            dir,
            manifest_path,
            summary_path,
        }
    }

    fn configure_strict_review(fixture: &Fixture, max_attempts: u32) {
        std::fs::write(
            fixture.dir.join("config.toml"),
            format!(
                "[agency]\ncompletion_review_strict = true\ngate_max_attempts = {max_attempts}\n"
            ),
        )
        .unwrap();
    }

    fn bind_running_attempt(fixture: &Fixture) {
        let graph_path = fixture.dir.join("graph.jsonl");
        modify_graph(&graph_path, |graph| {
            let task = graph.get_task_mut("report").unwrap();
            task.assigned = Some("review-worker".to_string());
            task.lifecycle.fence = 1;
            task.lifecycle.attempt_sequence = 1;
            task.lifecycle.current_attempt = Some(AttemptRef {
                id: "attempt-3-1".to_string(),
                generation: 3,
                fence: 1,
                actor_id: "review-worker".to_string(),
                disposition: None,
            });
            true
        })
        .unwrap();
    }

    fn bind_retried_source_attempt(fixture: &Fixture) {
        let graph_path = fixture.dir.join("graph.jsonl");
        modify_graph(&graph_path, |graph| {
            let task = graph.get_task_mut("report").unwrap();
            task.status = Status::InProgress;
            task.assigned = Some("retry-worker".to_string());
            task.lifecycle.generation = 4;
            task.lifecycle.fence = 2;
            task.lifecycle.attempt_sequence = 1;
            task.lifecycle.current_attempt = Some(AttemptRef {
                id: "attempt-4-1".to_string(),
                generation: 4,
                fence: 2,
                actor_id: "retry-worker".to_string(),
                disposition: None,
            });
            task.completion_blocker = None;
            true
        })
        .unwrap();
    }

    fn rewrite_candidate(fixture: &Fixture, revision: &str, bytes: &[u8]) {
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        let summary = std::fs::read(&fixture.summary_path).unwrap();
        let completion_store = store(&fixture.dir).unwrap();
        let output = completion_store.put_bytes(bytes, "text/plain").unwrap();
        let evidence = completion_store
            .evidence_from_bytes(
                format!("validation for {revision}\n").as_bytes(),
                "validation",
                "text/plain",
            )
            .unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Report,
            requirements_digest: requirements_digest(task).unwrap(),
            source_revision: revision.to_string(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![evidence],
            worker_summary_digest: ContentDigest::of_bytes(&summary),
        };
        std::fs::write(&fixture.manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
    }

    #[test]
    fn same_candidate_reuses_each_route_receipt_across_restart() {
        let fixture = fixture();
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut first_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls.clone(),
        };
        let mut first_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls.clone(),
        };
        let first = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut first_flip,
            &mut first_eval,
        )
        .unwrap();
        assert_eq!(first.status, ReviewValveStatus::Accepted);
        assert_eq!(
            *first_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
        let first_flip_id = first.flip.receipt_object.content_digest.clone();
        let first_eval_id = first
            .eval
            .as_ref()
            .unwrap()
            .receipt_object
            .content_digest
            .clone();
        let graph_path = fixture.dir.join("graph.jsonl");
        let first_graph_bytes = std::fs::read(&graph_path).unwrap();

        // `run_with_reviewers` reloads the graph and immutable objects. New
        // reviewer instances model a process restart; neither may be called.
        let replay_calls = Arc::new(Mutex::new(Vec::new()));
        let mut replay_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: replay_calls.clone(),
        };
        let mut replay_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: replay_calls.clone(),
        };
        let replay = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut replay_flip,
            &mut replay_eval,
        )
        .unwrap();
        assert_eq!(replay.status, ReviewValveStatus::Accepted);
        assert!(replay_calls.lock().unwrap().is_empty());
        assert_eq!(replay.flip.receipt_object.content_digest, first_flip_id);
        assert_eq!(
            replay.eval.as_ref().unwrap().receipt_object.content_digest,
            first_eval_id
        );

        assert_eq!(
            std::fs::read(&graph_path).unwrap(),
            first_graph_bytes,
            "receipt replay must not grow mutable counters or logs"
        );
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.completion_review_activity.len(), 2);
        assert_eq!(
            task.completion_candidate
                .as_ref()
                .unwrap()
                .review_binding
                .as_ref()
                .unwrap()
                .candidate_sequence,
            1
        );
    }

    #[test]
    fn restored_route_reuses_historical_receipt_for_same_candidate() {
        let fixture = fixture();
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut first_flip = FakeReviewer {
            route: "pi:test/flip-a".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls.clone(),
        };
        let mut eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls,
        };
        let first = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut first_flip,
            &mut eval,
        )
        .unwrap();
        let route_a_receipt = first.flip.receipt_object.content_digest.clone();

        let changed_calls = Arc::new(Mutex::new(Vec::new()));
        let mut changed_flip = FakeReviewer {
            route: "pi:test/flip-b".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: changed_calls.clone(),
        };
        let mut cached_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: changed_calls.clone(),
        };
        run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut changed_flip,
            &mut cached_eval,
        )
        .unwrap();
        assert_eq!(*changed_calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        let restored_calls = Arc::new(Mutex::new(Vec::new()));
        let mut restored_flip = FakeReviewer {
            route: "pi:test/flip-a".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: restored_calls.clone(),
        };
        let mut still_cached_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: restored_calls.clone(),
        };
        let restored = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut restored_flip,
            &mut still_cached_eval,
        )
        .unwrap();
        assert!(restored_calls.lock().unwrap().is_empty());
        assert_eq!(restored.flip.receipt_object.content_digest, route_a_receipt);

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.completion_review_activity.len(), 3);
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(
            verified
                .activities
                .iter()
                .filter(|activity| activity.candidate_state
                    == worksgood::completion_review::ReviewCandidateState::Current)
                .count(),
            2
        );
    }

    #[test]
    fn strict_revised_candidate_receives_review_after_rejection() {
        let fixture = fixture();
        configure_strict_review(&fixture, 2);
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut rejected_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: first_calls.clone(),
        };
        let mut skipped_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls.clone(),
        };
        let rejected = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut rejected_flip,
            &mut skipped_eval,
        )
        .unwrap();
        assert_eq!(rejected.status, ReviewValveStatus::FlipRejected);
        assert_eq!(*first_calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        rewrite_candidate(
            &fixture,
            "session:repaired",
            b"materially repaired output\n",
        );
        let repaired_calls = Arc::new(Mutex::new(Vec::new()));
        let mut repaired_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: repaired_calls.clone(),
        };
        let mut repaired_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: repaired_calls.clone(),
        };
        let repaired = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut repaired_flip,
            &mut repaired_eval,
        )
        .unwrap();
        assert_eq!(repaired.status, ReviewValveStatus::Accepted);
        assert_eq!(
            *repaired_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(verified.activities.len(), 3);
        assert_eq!(
            verified.activities[0].candidate_state,
            worksgood::completion_review::ReviewCandidateState::Superseded
        );
        assert!(
            verified.activities[1..]
                .iter()
                .all(|activity| activity.candidate_state
                    == worksgood::completion_review::ReviewCandidateState::Current)
        );
    }

    #[test]
    fn strict_task_ceiling_parks_revised_candidate_without_model_call_or_failure() {
        let fixture = fixture();
        configure_strict_review(&fixture, 1);
        bind_running_attempt(&fixture);
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut rejected_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: first_calls.clone(),
        };
        let mut skipped_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls,
        };
        run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut rejected_flip,
            &mut skipped_eval,
        )
        .unwrap();

        rewrite_candidate(
            &fixture,
            "session:over-budget",
            b"another material repair\n",
        );
        let blocked_calls = Arc::new(Mutex::new(Vec::new()));
        let mut blocked_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: blocked_calls.clone(),
        };
        let mut blocked_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: blocked_calls.clone(),
        };
        let error = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut blocked_flip,
            &mut blocked_eval,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Needs review"));
        assert!(blocked_calls.lock().unwrap().is_empty());

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert!(task.assigned.is_none());
        assert!(task.failure_reason.is_none());
        assert_eq!(task.completion_review_activity.len(), 1);
        assert_eq!(
            task.completion_blocker.as_ref().unwrap().kind,
            CompletionBlockerKind::NeedsReview
        );
        assert_eq!(
            task.lifecycle.current_attempt.as_ref().unwrap().disposition,
            Some(AttemptDisposition::Parked)
        );
        assert!(
            task.log
                .iter()
                .any(|entry| entry.message.contains("Completion waiting/NeedsReview"))
        );
    }

    #[test]
    fn source_retry_resets_budget_but_revised_candidates_remain_bounded() {
        let fixture = fixture();
        configure_strict_review(&fixture, 1);
        bind_running_attempt(&fixture);

        let old_calls = Arc::new(Mutex::new(Vec::new()));
        let mut old_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: old_calls.clone(),
        };
        let mut old_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: old_calls.clone(),
        };
        run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut old_flip,
            &mut old_eval,
        )
        .unwrap();
        assert_eq!(*old_calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        // Operator retry changes the source tuple. The prior rejection stays
        // immutable history but cannot consume this attempt's one-candidate
        // semantic budget.
        bind_retried_source_attempt(&fixture);
        rewrite_candidate(
            &fixture,
            "session:retry-first",
            b"first candidate from retried source attempt\n",
        );
        let retry_calls = Arc::new(Mutex::new(Vec::new()));
        let mut retry_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: retry_calls.clone(),
        };
        let mut retry_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: retry_calls.clone(),
        };
        run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut retry_flip,
            &mut retry_eval,
        )
        .unwrap();
        assert_eq!(*retry_calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        let candidate = task.completion_candidate.as_ref().unwrap();
        assert_eq!(
            rejected_current_candidate_at_source_budget(&fixture.dir, task, candidate, 1).unwrap(),
            Some(1),
            "finish-path accounting must see only the current attempt's rejected candidate"
        );

        // Candidate scoping still applies inside the retried source attempt:
        // its second immutable revision parks without a third model call.
        rewrite_candidate(
            &fixture,
            "session:retry-over-budget",
            b"second candidate from retried source attempt\n",
        );
        let blocked_calls = Arc::new(Mutex::new(Vec::new()));
        let mut blocked_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: blocked_calls.clone(),
        };
        let mut blocked_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: blocked_calls.clone(),
        };
        let error = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut blocked_flip,
            &mut blocked_eval,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Needs review"));
        assert!(blocked_calls.lock().unwrap().is_empty());

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.status, Status::Waiting);
        assert_eq!(task.completion_review_activity.len(), 2);
        assert_eq!(
            task.completion_blocker.as_ref().unwrap().kind,
            CompletionBlockerKind::NeedsReview
        );
    }

    #[test]
    fn infrastructure_retry_does_not_consume_semantic_revision_allowance() {
        let fixture = fixture();
        configure_strict_review(&fixture, 1);
        let outage_calls = Arc::new(Mutex::new(Vec::new()));
        let mut unavailable_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Err(ReviewerUnavailable {
                code: "test.provider_down".to_string(),
                message: "provider unavailable".to_string(),
            }),
            calls: outage_calls.clone(),
        };
        let mut skipped_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: outage_calls.clone(),
        };
        let unavailable = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut unavailable_flip,
            &mut skipped_eval,
        )
        .unwrap();
        assert_eq!(unavailable.status, ReviewValveStatus::ReviewUnavailable);
        assert_eq!(*outage_calls.lock().unwrap(), vec![ReviewerKind::Flip]);

        let retry_calls = Arc::new(Mutex::new(Vec::new()));
        let mut retry_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: retry_calls.clone(),
        };
        let mut retry_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: retry_calls.clone(),
        };
        let retried = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut retry_flip,
            &mut retry_eval,
        )
        .unwrap();
        assert_eq!(retried.status, ReviewValveStatus::Accepted);
        assert_eq!(
            *retry_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.completion_review_activity.len(), 3);
        assert_eq!(
            task.completion_review_activity[0].failure_class,
            Some(ReviewFailureClass::ReviewerUnavailable)
        );
    }

    #[test]
    fn evaluator_infrastructure_retry_is_allowed_after_candidate_consumes_ceiling() {
        let fixture = fixture();
        configure_strict_review(&fixture, 1);
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut passing_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls.clone(),
        };
        let mut unavailable_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Err(ReviewerUnavailable {
                code: "test.evaluator_down".to_string(),
                message: "evaluator unavailable".to_string(),
            }),
            calls: first_calls.clone(),
        };
        let unavailable = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut passing_flip,
            &mut unavailable_eval,
        )
        .unwrap();
        assert_eq!(unavailable.status, ReviewValveStatus::ReviewUnavailable);
        assert_eq!(
            *first_calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );

        let retry_calls = Arc::new(Mutex::new(Vec::new()));
        let mut cached_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: retry_calls.clone(),
        };
        let mut recovered_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: retry_calls.clone(),
        };
        let recovered = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut cached_flip,
            &mut recovered_eval,
        )
        .unwrap();
        assert_eq!(recovered.status, ReviewValveStatus::Accepted);
        assert_eq!(
            *retry_calls.lock().unwrap(),
            vec![ReviewerKind::Eval],
            "the semantic FLIP receipt must be reused while only evaluator infrastructure retries"
        );

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.completion_review_activity.len(), 3);
        assert_eq!(
            task.completion_review_activity[1].failure_class,
            Some(ReviewFailureClass::ReviewerUnavailable)
        );
    }

    #[test]
    fn manifest_builder_supplies_task_bound_fields_from_immutable_refs() {
        let root = tempdir().unwrap();
        let dir = root.path().join(".wg");
        std::fs::create_dir_all(&dir).unwrap();
        let task = Task {
            id: "build-report".to_string(),
            title: "Build report manifest".to_string(),
            description: Some("Exact report.\n\n## Validation\nCheck bytes.".to_string()),
            status: Status::InProgress,
            completion_contract: CompletionContract::Report,
            ..Task::default()
        };
        let expected_requirements = requirements_digest(&task).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, dir.join("graph.jsonl")).unwrap();
        let completion_store = store(&dir).unwrap();
        let output = completion_store
            .put_bytes(b"report\n", "text/plain")
            .unwrap();
        let evidence = completion_store
            .evidence_from_bytes(b"ok\n", "validation", "text/plain")
            .unwrap();
        let manifest = build_manifest(
            &dir,
            "build-report",
            b"summary\n",
            vec![OutputRef::Artifact(output)],
            vec![evidence],
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(manifest.task_id, "build-report");
        assert_eq!(manifest.requirements_digest, expected_requirements);
        assert_eq!(
            manifest.worker_summary_digest,
            ContentDigest::of_bytes(b"summary\n")
        );
        assert_eq!(manifest.source_revision, "worker-session:build-report");
        manifest.validate().unwrap();
    }

    #[test]
    fn submit_records_exact_receipts_in_flip_then_eval_order() {
        let fixture = fixture();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: calls.clone(),
        };
        let mut eval = FakeReviewer {
            route: "codex:test-eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: calls.clone(),
        };
        let outcome = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();
        assert_eq!(outcome.status, ReviewValveStatus::Accepted);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        let completion_store = store(&fixture.dir).unwrap();
        let (submission, manifest, requirements, summary) =
            load_submission_bytes(&completion_store, task).unwrap();
        let resolved = ReviewResolver::new(&completion_store)
            .resolve_submission(&submission.manifest_ref, &requirements, &summary, &[])
            .unwrap();
        load_exact_review_pair(&completion_store, &submission, &manifest, &resolved).unwrap();
        assert_eq!(task.completion_review_activity.len(), 2);
        assert_eq!(
            task.completion_review_activity
                .iter()
                .map(|activity| activity.reviewer_kind)
                .collect::<Vec<_>>(),
            vec![ReviewerKind::Flip, ReviewerKind::Eval]
        );
        assert!(
            task.completion_review_activity
                .iter()
                .all(|activity| !activity.activity_id.is_empty())
        );

        super::super::completion_done::run(&fixture.dir, "report", "refs/heads/main").unwrap();
        let graph_path = fixture.dir.join("graph.jsonl");
        let graph = load_graph(&graph_path).unwrap();
        assert_eq!(graph.get_task("report").unwrap().status, Status::Done);
        let first_done_bytes = std::fs::read(&graph_path).unwrap();
        super::super::completion_done::run(&fixture.dir, "report", "refs/heads/main").unwrap();
        assert_eq!(std::fs::read(&graph_path).unwrap(), first_done_bytes);
    }

    #[test]
    fn flip_rejection_preserves_candidate_and_skips_eval() {
        let fixture = fixture();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: calls.clone(),
        };
        let mut eval = FakeReviewer {
            route: "codex:test-eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: calls.clone(),
        };
        let outcome = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();
        assert_eq!(outcome.status, ReviewValveStatus::FlipRejected);
        assert_eq!(*calls.lock().unwrap(), vec![ReviewerKind::Flip]);
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let candidate = graph
            .get_task("report")
            .unwrap()
            .completion_candidate
            .as_ref()
            .unwrap();
        assert!(candidate.flip_receipt.is_some());
        assert!(candidate.eval_receipt.is_none());
        let task = graph.get_task("report").unwrap();
        let activity = &task.completion_review_activity;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].reviewer_kind, ReviewerKind::Flip);
        assert_eq!(
            activity[0].verdict,
            worksgood::simple_land::ReviewVerdict::Reject
        );
        assert_eq!(
            activity[0].failure_class,
            Some(worksgood::completion_review::ReviewFailureClass::SemanticRejection)
        );
        let binding = activity[0].binding.as_ref().unwrap();
        assert_eq!(binding.task_id, "report");
        assert_eq!(binding.generation, 3);
        assert_eq!(binding.candidate_sequence, 1);
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(verified.activities[0].findings[0].code, "test.reject");
        assert_eq!(
            verified.activities[0].candidate_state,
            worksgood::completion_review::ReviewCandidateState::Current
        );
    }

    #[test]
    fn changed_candidate_preserves_superseded_review_chronology() {
        let fixture = fixture();
        let first_calls = Arc::new(Mutex::new(Vec::new()));
        let mut rejected_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Reject)),
            calls: first_calls.clone(),
        };
        let mut skipped_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: first_calls,
        };
        let first = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut rejected_flip,
            &mut skipped_eval,
        )
        .unwrap();
        assert_eq!(first.status, ReviewValveStatus::FlipRejected);

        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        let summary = std::fs::read(&fixture.summary_path).unwrap();
        let completion_store = store(&fixture.dir).unwrap();
        let output = completion_store
            .put_bytes(b"changed reviewed bytes\n", "text/plain")
            .unwrap();
        let evidence = completion_store
            .evidence_from_bytes(b"changed validation ok\n", "validation", "text/plain")
            .unwrap();
        let changed_manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: task.id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: worksgood::simple_land::CompletionContract::Report,
            requirements_digest: requirements_digest(task).unwrap(),
            source_revision: "session:changed".to_string(),
            outputs: vec![OutputRef::Artifact(output)],
            validation_evidence: vec![evidence],
            worker_summary_digest: ContentDigest::of_bytes(&summary),
        };
        std::fs::write(
            &fixture.manifest_path,
            changed_manifest.canonical_bytes().unwrap(),
        )
        .unwrap();

        let accepted_calls = Arc::new(Mutex::new(Vec::new()));
        let mut accepted_flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: accepted_calls.clone(),
        };
        let mut accepted_eval = FakeReviewer {
            route: "pi:test/eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls: accepted_calls,
        };
        let second = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut accepted_flip,
            &mut accepted_eval,
        )
        .unwrap();
        assert_eq!(second.status, ReviewValveStatus::Accepted);

        // Serialization reload retains all immutable rows. Only the exact
        // selected candidate's FLIP+Eval are current acceptance evidence.
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.completion_review_activity.len(), 3);
        assert_eq!(
            task.completion_review_activity
                .iter()
                .map(|activity| { activity.binding.as_ref().unwrap().candidate_sequence })
                .collect::<Vec<_>>(),
            vec![1, 2, 2]
        );
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(
            verified
                .activities
                .iter()
                .map(|activity| activity.candidate_state)
                .collect::<Vec<_>>(),
            vec![
                worksgood::completion_review::ReviewCandidateState::Superseded,
                worksgood::completion_review::ReviewCandidateState::Current,
                worksgood::completion_review::ReviewCandidateState::Current,
            ]
        );
        super::super::completion_done::run(&fixture.dir, "report", "refs/heads/main").unwrap();
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        assert_eq!(graph.get_task("report").unwrap().status, Status::Done);
        assert_eq!(
            graph
                .get_task("report")
                .unwrap()
                .completion_review_activity
                .len(),
            3
        );
    }

    #[test]
    fn completion_object_refuses_control_plane_source() {
        let fixture = fixture();
        let protected = fixture.dir.join("secret.txt");
        std::fs::write(&protected, b"must not escape").unwrap();
        let error = put_object(&fixture.dir, &protected, "text/plain", None).unwrap_err();
        assert!(error.to_string().contains("protected .wg"));
    }

    #[test]
    fn reviewer_unavailability_preserves_submission_without_source_replacement() {
        let fixture = fixture();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut flip = FakeReviewer {
            route: "pi:test/flip".to_string(),
            result: Err(ReviewerUnavailable {
                code: "test.offline".to_string(),
                message: "offline".to_string(),
            }),
            calls: calls.clone(),
        };
        let mut eval = FakeReviewer {
            route: "codex:test-eval".to_string(),
            result: Ok(semantic(SemanticVerdict::Pass)),
            calls,
        };
        let outcome = run_with_reviewers(
            &fixture.dir,
            "report",
            &fixture.manifest_path,
            &fixture.summary_path,
            &mut flip,
            &mut eval,
        )
        .unwrap();
        assert_eq!(outcome.status, ReviewValveStatus::ReviewUnavailable);
        let graph = load_graph(fixture.dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("report").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task_submission(task).is_ok());
        assert!(task.after.is_empty());
        assert_eq!(task.completion_review_activity.len(), 1);
        assert_eq!(
            task.completion_review_activity[0].failure_class,
            Some(worksgood::completion_review::ReviewFailureClass::ReviewerUnavailable)
        );
        let verified = worksgood::completion_review::verified_review_activities(&fixture.dir, task);
        assert_eq!(verified.invalid_count, 0);
        assert_eq!(verified.activities[0].findings[0].code, "test.offline");
    }
}
