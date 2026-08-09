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

    // Reuse an already-selected immutable candidate. This makes `wg done`
    // idempotent after a lost submit/land response and prevents another model
    // call merely because the worker does not know which internal phase won.
    if task.completion_candidate.is_some() {
        if task.completion_contract == CompletionContract::Land
            && task.completion_disposition != Some(worksgood::graph::CompletionDisposition::Landed)
        {
            super::completion_land::run_at(dir, id, integration_ref, Some(&cwd))?;
        }
        return super::completion_done::run(dir, id, integration_ref);
    }

    let mut evidence = Vec::new();
    if let Some(verify) = task
        .verify
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        eprintln!("Running configured deterministic validation: {verify}");
        let output = Command::new("bash")
            .args(["-lc", verify])
            .current_dir(&cwd)
            .output()
            .with_context(|| format!("run configured verify command: {verify}"))?;
        let mut transcript = Vec::new();
        transcript.extend_from_slice(format!("$ {verify}\n").as_bytes());
        transcript.extend_from_slice(&output.stdout);
        transcript.extend_from_slice(&output.stderr);
        if !output.status.success() {
            eprint!("{}", String::from_utf8_lossy(&transcript));
            bail!(
                "configured deterministic validation failed with {}",
                output.status
            );
        }
        let artifact =
            super::completion_submit::store(dir)?.put_bytes(&transcript, "text/plain")?;
        evidence.push(evidence_ref(artifact, "configured-verify"));
    }
    if evidence.is_empty() {
        let transcript = if task.completion_contract == CompletionContract::Land {
            let output = Command::new("git")
                .args(["diff", "--check", "refs/heads/main..HEAD"])
                .current_dir(&cwd)
                .output()
                .context("run baseline git diff validation")?;
            if !output.status.success() {
                bail!(
                    "baseline git diff validation failed:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            b"$ git diff --check refs/heads/main..HEAD\nclean\n".to_vec()
        } else {
            b"WG verified that every declared completion artifact is a regular file before snapshotting it.\n".to_vec()
        };
        let artifact =
            super::completion_submit::store(dir)?.put_bytes(&transcript, "text/plain")?;
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
    if task.completion_contract == CompletionContract::Land {
        super::completion_land::run_at(dir, id, integration_ref, Some(&cwd))?;
    }
    super::completion_done::run(dir, id, integration_ref)
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
