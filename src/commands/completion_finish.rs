//! One-operation completion for ordinary trusted local workers.
//!
//! The public workflow is `wg done <task>`. Candidate/object/review/landing
//! mechanics remain available for diagnostics, but are not required knowledge
//! for a worker completing normal work.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use worksgood::completion_manifest::{EvidenceRef, OutputRef};
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
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?
        .clone();
    let cwd = std::env::current_dir().context("determine worker worktree")?;

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
            true
        };
        let current_activity = task
            .completion_review_activity
            .iter()
            .filter(|activity| {
                activity.manifest_digest == candidate.manifest.content_digest
                    && match (&candidate.review_binding, &activity.binding) {
                        (Some(candidate), Some(activity)) => candidate == activity,
                        (None, None) => true,
                        _ => false,
                    }
            })
            .collect::<Vec<_>>();
        let strict_passed = current_activity.iter().any(|activity| {
            activity.reviewer_kind == worksgood::completion_review::ReviewerKind::Flip
                && activity.verdict == worksgood::simple_land::ReviewVerdict::Pass
        }) && current_activity.iter().any(|activity| {
            activity.reviewer_kind == worksgood::completion_review::ReviewerKind::Eval
                && activity.verdict == worksgood::simple_land::ReviewVerdict::Pass
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
        if candidate_matches_head
            && candidate_matches_source_tuple
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
        if config.agency.completion_review_strict
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
            print_validation_failure(&captured);
            bail!(
                "configured deterministic validation rejected completion (exit={:?}, signal={:?}, timeout={}): {} [evidence={}]",
                captured.exit.code,
                captured.exit.signal,
                captured.exit.timed_out,
                command,
                reference.content_digest
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
            print_validation_failure(&captured);
            bail!(
                "baseline deterministic validation rejected completion (exit={:?}, signal={:?}, timeout={}) [evidence={}]",
                captured.exit.code,
                captured.exit.signal,
                captured.exit.timed_out,
                reference.content_digest
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
    let temp = std::env::temp_dir();
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

fn store_validation_evidence(
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

fn record_validation_result(
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

fn print_validation_failure(captured: &DeterministicValidationEvidence) {
    if !captured.stdout.content.is_empty() {
        eprintln!(
            "deterministic validation stdout ({}{}):\n{}",
            captured.stdout.encoding,
            if captured.stdout.truncated {
                ", truncated"
            } else {
                ""
            },
            captured.stdout.content
        );
    }
    if !captured.stderr.content.is_empty() {
        eprintln!(
            "deterministic validation stderr ({}{}):\n{}",
            captured.stderr.encoding,
            if captured.stderr.truncated {
                ", truncated"
            } else {
                ""
            },
            captured.stderr.content
        );
    }
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
