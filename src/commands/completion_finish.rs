//! One-operation completion for ordinary trusted local workers.
//!
//! The public workflow is `wg done <task>`. Candidate/object/review/landing
//! mechanics remain available for diagnostics, but are not required knowledge
//! for a worker completing normal work.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{EvidenceRef, OutputRef, ReviewResolver};
use worksgood::completion_task::{load_review_evidence, load_submission_bytes};
use worksgood::completion_validation::{
    BASELINE_VALIDATION_EVIDENCE_KIND, CONFIGURED_VALIDATION_EVIDENCE_KIND,
    DETERMINISTIC_VALIDATION_MEDIA_TYPE, DeterministicValidationEvidence, ValidationPurpose,
    capture_validation, configured_validation_commands, land_baseline_command,
};
use worksgood::graph::CompletionContract;
use worksgood::parser::load_graph;

struct TempFiles {
    paths: Vec<PathBuf>,
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn run(dir: &Path, id: &str, integration_ref: &str) -> Result<()> {
    // The ordinary publication-derived completion path retains the existing
    // owned smoke gate. Run it before deterministic validation/model work so a
    // known regression cannot consume repair or reviewer budget.
    super::done::run_smoke_gate(
        dir,
        id,
        false,
        false,
        std::env::var_os("WG_AGENT_ID").is_some(),
    )?;
    let cwd = std::env::current_dir().context("determine worker worktree")?;
    run_at(dir, id, integration_ref, &cwd)
}

pub(crate) fn run_at(dir: &Path, id: &str, integration_ref: &str, cwd: &Path) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?
        .clone();
    if let Some(repair) = task.completion_repair.as_ref()
        && repair.disposition == worksgood::graph::CompletionRepairDisposition::NeedsAttention
        && repair.requirements_digest == worksgood::completion_task::requirements_digest(&task)?
    {
        bail!(
            "NeedsAttention: deterministic completion repair is stopped ({}, feedback={}). No check or model call was made. Root blocker: {}. Next: {}",
            repair.reason_code,
            repair.feedback_id,
            repair.exit_category,
            repair.safe_next
        );
    }

    // Reuse an already-selected immutable candidate after a lost response.
    // In explicit strict mode, a non-passing candidate means the worker has
    // repaired and is intentionally asking WG to snapshot a new revision.
    let config = worksgood::config::Config::load_or_default(dir);
    if let Some(candidate) = task.completion_candidate.as_ref() {
        let candidate_matches_head = if task.completion_contract == CompletionContract::Land {
            let head = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&cwd)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string());
            let reviewed_commit = super::completion_submit::store(dir)?
                .read_manifest(
                    &candidate.manifest,
                    worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
                )?
                .outputs
                .into_iter()
                .find_map(|output| match output {
                    OutputRef::Git(git) => Some(git.commit_oid),
                    OutputRef::Artifact(_) | OutputRef::External(_) => None,
                });
            head.is_some() && head == reviewed_commit
        } else {
            non_land_candidate_matches(dir, &task, candidate, &cwd).unwrap_or(false)
        };
        // The mutable activity projection is display-only. Lost-response
        // recovery must reload the selected candidate plus its immutable FLIP
        // chain before it can skip directly to landing/Done.
        let verified_review = (|| -> Result<_> {
            let completion_store = super::completion_submit::store(dir)?;
            let (submission, manifest, requirements, summary) =
                load_submission_bytes(&completion_store, &task)?;
            let resolver = ReviewResolver::new(&completion_store);
            let resolved = if task.completion_contract == CompletionContract::Land {
                resolver.repository(&cwd).resolve_submission(
                    &submission.manifest_ref,
                    &requirements,
                    &summary,
                    &candidate.dependency_outputs,
                )?
            } else {
                resolver.resolve_submission(
                    &submission.manifest_ref,
                    &requirements,
                    &summary,
                    &candidate.dependency_outputs,
                )?
            };
            load_review_evidence(&completion_store, &submission, &manifest, &resolved)
                .map_err(Into::into)
        })()
        .ok();
        let strict_passed = verified_review.as_ref().is_some_and(|evidence| {
            evidence.flip.verdict == worksgood::simple_land::ReviewVerdict::Pass
                && evidence
                    .eval
                    .as_ref()
                    .is_some_and(|eval| eval.verdict == worksgood::simple_land::ReviewVerdict::Pass)
        });
        let semantic_rejection = verified_review.as_ref().is_some_and(|evidence| {
            evidence.flip.verdict == worksgood::simple_land::ReviewVerdict::Reject
                || evidence.eval.as_ref().is_some_and(|eval| {
                    eval.verdict == worksgood::simple_land::ReviewVerdict::Reject
                })
        });
        let incomplete_review = verified_review.as_ref().is_none_or(|evidence| {
            evidence.flip.verdict == worksgood::simple_land::ReviewVerdict::IncompleteEvidence
                || evidence.eval.as_ref().is_some_and(|eval| {
                    eval.verdict == worksgood::simple_land::ReviewVerdict::IncompleteEvidence
                })
        });
        let candidate_matches_source_tuple = candidate.requirements.content_digest
            == worksgood::completion_task::requirements_digest(&task)?
            && candidate.review_binding.as_ref().is_some_and(|binding| {
                binding.task_id == task.id
                    && binding.generation == task.lifecycle.generation
                    && binding.attempt_fence == task.lifecycle.fence
                    && binding.attempt_id.as_deref()
                        == task
                            .lifecycle
                            .current_attempt
                            .as_ref()
                            .map(|attempt| attempt.id.as_str())
            });
        if candidate_matches_head && candidate_matches_source_tuple && semantic_rejection {
            bail!(
                "current completion candidate was semantically rejected; publication and Done are refused. The same source attempt/worktree/session is retained: repair the candidate bytes, rerun the declared validation, then run `wg done {id}` again"
            );
        }
        if candidate_matches_head
            && candidate_matches_source_tuple
            && !semantic_rejection
            && !incomplete_review
            && (!config.agency.completion_review_strict || strict_passed)
        {
            if task.completion_contract == CompletionContract::Land
                && task.completion_disposition
                    != Some(worksgood::graph::CompletionDisposition::Landed)
            {
                super::completion_land::run_at(dir, id, integration_ref, Some(&cwd))?;
                if load_graph(dir.join("graph.jsonl"))?
                    .get_task(id)
                    .is_some_and(|task| task.status == worksgood::graph::Status::Waiting)
                {
                    return Ok(());
                }
            }
            return super::completion_done::run(dir, id, integration_ref);
        }

        // A repeated `wg done` for an exact rejected candidate must not rerun
        // deterministic validation or another model call once this source
        // attempt has consumed its semantic-candidate budget. Superseded
        // source attempts do not count, and unavailable FLIP/Eval receipts do
        // not block their candidate-scoped infrastructure retry.
        if candidate_matches_head
            && candidate_matches_source_tuple
            && config.agency.completion_review_strict
            && let Some(iterations) =
                super::completion_submit::rejected_current_candidate_at_source_budget(
                    dir,
                    &task,
                    candidate,
                    config.agency.gate_max_attempts.max(1),
                )?
        {
            super::completion_submit::park_for_review_budget(
                dir,
                id,
                iterations,
                config.agency.gate_max_attempts.max(1),
            )?;
            bail!(
                "Needs review: strict model-review attempt limit ({}) reached; no further validation or model call was made",
                config.agency.gate_max_attempts.max(1)
            );
        }
    }

    let mut evidence = Vec::new();
    let configured_commands = configured_validation_commands(&task);
    for (index, command) in configured_commands.iter().enumerate() {
        eprintln!("Running configured deterministic validation: {command}");
        let captured = capture_validation(
            &task,
            command,
            u32::try_from(index).unwrap_or(u32::MAX),
            ValidationPurpose::Configured,
            &cwd,
        )
        .with_context(|| format!("capture configured validation command: {command}"))?;
        let reference =
            store_validation_evidence(dir, &captured, CONFIGURED_VALIDATION_EVIDENCE_KIND)?;
        record_validation_result(dir, &task, &captured, &reference)?;
        if !captured.authoritative_pass(worksgood::completion_task::completion_contract(&task)?) {
            let repair = record_repair_feedback(dir, &task, &captured, &reference)?;
            print_repair_feedback(&repair);
            bail!(
                "configured deterministic validation rejected completion (exit={:?}, signal={:?}, timeout={}): {} [evidence={}; feedback={}]",
                captured.exit.code,
                captured.exit.signal,
                captured.exit.timed_out,
                command,
                reference.content_digest,
                repair.feedback_id
            );
        }
        evidence.push(reference);
    }

    if task.completion_contract == CompletionContract::Land {
        let index = u32::try_from(configured_commands.len()).unwrap_or(u32::MAX);
        let captured = capture_validation(
            &task,
            land_baseline_command(),
            index,
            ValidationPurpose::Baseline,
            &cwd,
        )
        .context("capture baseline git diff validation")?;
        let reference =
            store_validation_evidence(dir, &captured, BASELINE_VALIDATION_EVIDENCE_KIND)?;
        record_validation_result(dir, &task, &captured, &reference)?;
        if !captured.authoritative_pass(worksgood::completion_task::completion_contract(&task)?) {
            let repair = record_repair_feedback(dir, &task, &captured, &reference)?;
            print_repair_feedback(&repair);
            bail!(
                "baseline deterministic validation rejected completion (exit={:?}, signal={:?}, timeout={}) [evidence={}; feedback={}]",
                captured.exit.code,
                captured.exit.signal,
                captured.exit.timed_out,
                reference.content_digest,
                repair.feedback_id
            );
        }
        evidence.push(reference);
    } else if evidence.is_empty() {
        let transcript = b"WG verified that every declared completion artifact is a regular file before snapshotting it.\n";
        let artifact = super::completion_submit::store(dir)?.put_bytes(transcript, "text/plain")?;
        evidence.push(evidence_ref(artifact, "baseline-integrity-check"));
    }

    let summary = worker_summary(dir, &task);
    let mut outputs = Vec::new();
    if task.completion_contract != CompletionContract::Land {
        for artifact_path in &task.artifacts {
            let path = Path::new(artifact_path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            if path.is_file() {
                outputs.push(OutputRef::Artifact(
                    super::completion_submit::store(dir)?
                        .put_file(&path, "application/octet-stream")?,
                ));
            }
        }
        if outputs.is_empty() {
            bail!(
                "{} completion requires at least one declared artifact; add it with `wg artifact {id} <path>`",
                task.completion_contract
            );
        }
    }

    mark_repair_resolved(dir, &task)?;

    let manifest = super::completion_submit::build_manifest(
        dir,
        id,
        summary.as_bytes(),
        outputs,
        evidence,
        task.completion_contract == CompletionContract::Land,
        None,
        Some(&cwd),
    )?;
    let nonce = uuid::Uuid::now_v7();
    let configured_temp = std::env::temp_dir();
    let project_root = dir
        .parent()
        .context("workgraph directory has no project root")?;
    // Project-local build scratch may intentionally place TMPDIR under .wg.
    // Completion submission correctly refuses control-plane-sourced manifests,
    // so keep these small transient handoff files outside the repository rather
    // than weakening that provenance boundary.
    let temp = if configured_temp.starts_with(project_root) {
        project_root
            .parent()
            .context("project-local TMPDIR has no safe parent")?
            .to_path_buf()
    } else {
        configured_temp
    };
    let summary_path = temp.join(format!("wg-completion-{nonce}.summary.txt"));
    let manifest_path = temp.join(format!("wg-completion-{nonce}.manifest.json"));
    fs::write(&summary_path, summary.as_bytes())?;
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    let _cleanup = TempFiles {
        paths: vec![summary_path.clone(), manifest_path.clone()],
    };

    super::completion_submit::run(dir, id, &manifest_path, &summary_path)?;
    if load_graph(dir.join("graph.jsonl"))?
        .get_task(id)
        .is_some_and(|task| {
            task.status == worksgood::graph::Status::Waiting && task.completion_blocker.is_some()
        })
    {
        return Ok(());
    }
    if task.completion_contract == CompletionContract::Land {
        super::completion_land::run_at(dir, id, integration_ref, Some(&cwd))?;
        if load_graph(dir.join("graph.jsonl"))?
            .get_task(id)
            .is_some_and(|task| task.status == worksgood::graph::Status::Waiting)
        {
            return Ok(());
        }
    }
    super::completion_done::run(dir, id, integration_ref)
}

pub(crate) fn store_validation_evidence(
    dir: &Path,
    captured: &DeterministicValidationEvidence,
    evidence_kind: &str,
) -> Result<EvidenceRef> {
    let bytes = captured
        .canonical_bytes()
        .context("serialize deterministic validation evidence")?;
    let artifact = super::completion_submit::store(dir)?
        .put_bytes(&bytes, DETERMINISTIC_VALIDATION_MEDIA_TYPE)?;
    worksgood::completion_validation::register_capture_authority(
        dir,
        &artifact.content_digest,
        captured,
    )?;
    Ok(evidence_ref(artifact, evidence_kind))
}

pub(crate) fn record_validation_result(
    dir: &Path,
    expected: &worksgood::graph::Task,
    captured: &DeterministicValidationEvidence,
    reference: &EvidenceRef,
) -> Result<()> {
    let mut refusal = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&expected.id) else {
            refusal = Some("task disappeared while recording deterministic validation".to_string());
            return false;
        };
        if task.lifecycle.generation != expected.lifecycle.generation
            || task.lifecycle.fence != expected.lifecycle.fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != expected
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.as_str())
            || worksgood::completion_task::requirements_digest(task).ok()
                != Some(captured.lifecycle.requirements_digest.clone())
        {
            refusal = Some(
                "task requirements, generation, attempt, or fence changed during deterministic validation"
                    .to_string(),
            );
            return false;
        }
        task.log.push(worksgood::graph::LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            actor: Some("deterministic-validation".to_string()),
            user: None,
            message: format!(
                "Captured deterministic validation purpose={:?} command={} exit={:?} timeout={} duration_ms={} evidence={}",
                captured.purpose,
                captured.command.command_digest,
                captured.exit.code,
                captured.exit.timed_out,
                captured.duration_ms,
                reference.content_digest
            ),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}

fn mark_repair_resolved(dir: &Path, expected: &worksgood::graph::Task) -> Result<()> {
    let mut refusal = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&expected.id) else {
            refusal = Some("task disappeared while resolving completion repair".to_string());
            return false;
        };
        if task.lifecycle.generation != expected.lifecycle.generation
            || task.lifecycle.fence != expected.lifecycle.fence
            || task.lifecycle.current_attempt != expected.lifecycle.current_attempt
            || worksgood::completion_task::requirements_digest(task).ok()
                != worksgood::completion_task::requirements_digest(expected).ok()
        {
            refusal = Some(
                "task requirements, generation, attempt, or fence changed while resolving completion repair"
                    .to_string(),
            );
            return false;
        }
        let Some(repair) = task.completion_repair.as_mut() else {
            return false;
        };
        if repair.disposition == worksgood::graph::CompletionRepairDisposition::Resolved {
            return false;
        }
        repair.disposition = worksgood::graph::CompletionRepairDisposition::Resolved;
        repair.reason_code = "deterministic-checks-passed".into();
        repair.safe_next =
            "deterministic repair resolved; continue exact candidate review/publication".into();
        repair.attention_event_id = None;
        repair.updated_at = chrono::Utc::now().to_rfc3339();
        task.log.push(worksgood::graph::LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            actor: Some("completion-repair".into()),
            user: None,
            message: format!(
                "Resolved deterministic completion repair after {}/{} opportunities",
                repair.opportunities_used, repair.opportunity_limit
            ),
        });
        true
    })?;
    if let Some(error) = refusal {
        bail!(error);
    }
    Ok(())
}

fn record_repair_feedback(
    dir: &Path,
    expected: &worksgood::graph::Task,
    captured: &DeterministicValidationEvidence,
    reference: &EvidenceRef,
) -> Result<worksgood::graph::CompletionRepairState> {
    let mut refusal = None;
    let mut recorded = None;
    let mut notify_parent = None;
    let mut notify_event = None;
    worksgood::parser::modify_graph(dir.join("graph.jsonl"), |graph| {
        let Some(task) = graph.get_task_mut(&expected.id) else {
            refusal =
                Some("task disappeared while recording deterministic repair feedback".to_string());
            return false;
        };
        let prior_attention = task
            .completion_repair
            .as_ref()
            .and_then(|prior| prior.attention_event_id.clone());
        match worksgood::completion_validation::record_deterministic_repair_failure(
            task, captured, reference,
        ) {
            Ok(state) => {
                let newly_attention = state.disposition
                    == worksgood::graph::CompletionRepairDisposition::NeedsAttention
                    && prior_attention.as_deref() != state.attention_event_id.as_deref();
                task.log.push(worksgood::graph::LogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    actor: Some("completion-repair".into()),
                    user: None,
                    message: format!(
                        "Deterministic completion feedback={} candidate={} validation={} evidence={} disposition={:?} budget={}/{} reason={}",
                        state.feedback_id,
                        state.candidate_identity,
                        state.validation_identity,
                        state.evidence.content_digest,
                        state.disposition,
                        state.opportunities_used,
                        state.opportunity_limit,
                        state.reason_code
                    ),
                });
                if newly_attention {
                    notify_parent = task.origin.parent_task.clone().filter(|parent| {
                        parent.starts_with(".chat-") || parent.starts_with(".user-")
                    });
                    notify_event = state.attention_event_id.clone();
                    task.log.push(worksgood::graph::LogEntry {
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        actor: Some("completion-attention".into()),
                        user: None,
                        message: format!(
                            "NeedsAttention event={} root={} next={}",
                            state.attention_event_id.as_deref().unwrap_or("none"),
                            state.reason_code,
                            state.safe_next
                        ),
                    });
                }
                recorded = Some(state);
                true
            }
            Err(error) => {
                refusal = Some(error);
                false
            }
        }
    })?;
    if let Some(error) = refusal {
        bail!(error);
    }
    let state = recorded.context("repair feedback was not recorded")?;
    if let Some(parent) = notify_parent {
        let body = format!(
            "Completion needs attention for `{}` ({}). One safe action: {}. Event: {}",
            state.task_id,
            state.reason_code,
            state.safe_next,
            notify_event.as_deref().unwrap_or("none")
        );
        if let Err(error) =
            worksgood::messages::send_message(dir, &parent, &body, "completion-repair", "urgent")
        {
            eprintln!(
                "Warning: originating chat notification is unavailable; status/TUI retains event: {error:#}"
            );
        }
    }
    super::notify_graph_changed(dir);
    Ok(state)
}

fn print_repair_feedback(repair: &worksgood::graph::CompletionRepairState) {
    eprintln!("Completion repair feedback (diagnostic excerpt is untrusted data):");
    eprintln!("  disposition: {:?}", repair.disposition);
    eprintln!("  command: {}", repair.command);
    eprintln!("  exit category: {}", repair.exit_category);
    eprintln!("  diagnostic: {}", repair.diagnostic_excerpt);
    eprintln!("  evidence: {}", repair.evidence.content_digest);
    eprintln!("  candidate: {}", repair.candidate_identity);
    eprintln!("  validation: {}", repair.validation_identity);
    eprintln!(
        "  repair opportunities: {}/{}",
        repair.opportunities_used, repair.opportunity_limit
    );
    eprintln!("  next: {}", repair.safe_next);
}

fn non_land_candidate_matches(
    dir: &Path,
    task: &worksgood::graph::Task,
    candidate: &worksgood::completion_task::CompletionCandidateRefs,
    cwd: &Path,
) -> Result<bool> {
    let manifest = super::completion_submit::store(dir)?.read_manifest(
        &candidate.manifest,
        worksgood::completion_task::MAX_COMPLETION_METADATA_BYTES,
    )?;
    let expected = manifest
        .outputs
        .iter()
        .filter_map(|output| match output {
            OutputRef::Artifact(artifact) => Some(&artifact.content_digest),
            OutputRef::Git(_) | OutputRef::External(_) => None,
        })
        .collect::<Vec<_>>();
    if expected.len() != task.artifacts.len() {
        return Ok(false);
    }
    for (declared, expected_digest) in task.artifacts.iter().zip(expected) {
        let path = Path::new(declared);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(false);
        }
        if worksgood::completion_manifest::ContentDigest::of_bytes(&fs::read(path)?)
            != *expected_digest
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn evidence_ref(
    artifact: worksgood::completion_manifest::ArtifactOutput,
    evidence_kind: &str,
) -> EvidenceRef {
    EvidenceRef {
        content_digest: artifact.content_digest,
        immutable_locator: artifact.immutable_locator,
        evidence_kind: evidence_kind.to_string(),
        media_type: artifact.media_type,
        size: artifact.size,
        review_projection: artifact.review_projection,
    }
}

fn worker_summary(dir: &Path, task: &worksgood::graph::Task) -> String {
    if let Some(agent) = task.assigned.as_deref() {
        let path = dir.join("agents").join(agent).join("session-summary.md");
        if let Ok(summary) = fs::read_to_string(path)
            && !summary.trim().is_empty()
        {
            return summary;
        }
    }
    let recent = task
        .log
        .iter()
        .rev()
        .take(8)
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Completed task {}: {}\n\n{}",
        task.id,
        task.title,
        if recent.is_empty() {
            "Worker completed the declared validation contract."
        } else {
            recent.as_str()
        }
    )
}
