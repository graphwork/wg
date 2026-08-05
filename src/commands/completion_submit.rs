use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs;
use std::path::Path;
use worksgood::completion_manifest::{
    CompletionArtifactStore, CompletionManifest, EvidenceRef, OutputRef, ReviewResolver,
};
use worksgood::completion_review::{
    ManifestReviewer, ReviewValveOutcome, ReviewValveStatus, ReviewerKind, ReviewerUnavailable,
    SemanticReview, run_review_valve,
};
use worksgood::completion_review_model::ExactModelReviewer;
use worksgood::completion_task::{
    CompletionCandidateRefs, completion_contract, requirements_digest, task_requirements_bytes,
};
use worksgood::config::{Config, DispatchRole};
use worksgood::graph::{LogEntry, Status, Task, WorkGraph};
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
    reject_control_plane_source(dir, path, "completion object")?;
    let store = store(dir)?;
    let artifact = store.put_file(path, media_type)?;
    if let Some(kind) = evidence_kind {
        if kind.trim().is_empty() {
            bail!("evidence kind must not be empty");
        }
        let evidence = EvidenceRef {
            content_digest: artifact.content_digest,
            immutable_locator: artifact.immutable_locator,
            evidence_kind: kind.to_string(),
            media_type: artifact.media_type,
            size: artifact.size,
            review_projection: artifact.review_projection,
        };
        println!("{}", serde_json::to_string_pretty(&evidence)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&artifact)?);
    }
    Ok(())
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
    let outcome = run_with_reviewers(
        dir,
        id,
        manifest_path,
        summary_path,
        flip.as_mut(),
        eval.as_mut(),
    )?;
    match outcome.status {
        ReviewValveStatus::Accepted => {
            println!(
                "Completion candidate accepted: manifest={} FLIP=pass eval=pass",
                outcome.flip.receipt.manifest_digest
            );
            Ok(())
        }
        ReviewValveStatus::FlipRejected => bail!(
            "FLIP rejected manifest {}; repair in the same worker context and submit a new manifest",
            outcome.flip.receipt.manifest_digest
        ),
        ReviewValveStatus::EvalRejected => bail!(
            "eval rejected manifest {}; repair in the same worker context and submit a new manifest",
            outcome.flip.receipt.manifest_digest
        ),
        ReviewValveStatus::ReviewUnavailable => bail!(
            "review unavailable for manifest {}; the candidate is preserved and no source replacement was created",
            outcome.flip.receipt.manifest_digest
        ),
        ReviewValveStatus::IncompleteEvidence => bail!(
            "incomplete evidence for manifest {}; repair the immutable evidence and submit a new manifest",
            outcome.flip.receipt.manifest_digest
        ),
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
        flip_receipt: None,
        eval_receipt: None,
    };

    // Select the immutable candidate before review. This is a single compact
    // graph projection; resolver/reviewer failures preserve it without
    // scheduling a transaction, source retry, or finalizer.
    select_candidate(
        &graph_path,
        id,
        task.lifecycle.generation,
        &requirements_digest,
        candidate,
    )?;

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
    };
    let manifest_digest = manifest.digest().map_err(anyhow::Error::msg)?;
    let outcome = run_review_valve(
        &store,
        &manifest_digest,
        &requirements_digest,
        resolved,
        flip,
        eval,
    )?;
    record_review_outcome(
        &graph_path,
        id,
        task.lifecycle.generation,
        &manifest_digest,
        &requirements_digest,
        &outcome,
    )?;
    Ok(outcome)
}

fn require_source_owner(task: &Task, id: &str) -> Result<()> {
    if task.status != Status::InProgress {
        bail!("task '{id}' must be in progress to submit a completion manifest");
    }
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
    id: &str,
    generation: u64,
    expected_requirements: &worksgood::completion_manifest::ContentDigest,
    candidate: CompletionCandidateRefs,
) -> Result<()> {
    let mut refusal = None;
    modify_graph(graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            refusal = Some("task disappeared while selecting candidate".to_string());
            return false;
        };
        if task.status != Status::InProgress || task.lifecycle.generation != generation {
            refusal = Some("task generation or ownership changed while selecting candidate".into());
            return false;
        }
        if requirements_digest(task).ok().as_ref() != Some(expected_requirements) {
            refusal = Some("task requirements changed while selecting candidate".into());
            return false;
        }
        task.completion_candidate = Some(candidate);
        task.completion_disposition = None;
        task.completion_receipt = None;
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
    Ok(())
}

fn record_review_outcome(
    graph_path: &Path,
    id: &str,
    generation: u64,
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
        if task.lifecycle.generation != generation
            || requirements_digest(task).ok().as_ref() != Some(expected_requirements)
        {
            refusal = Some("task requirements changed while review was running".to_string());
            return false;
        }
        let Some(candidate) = task.completion_candidate.as_mut() else {
            refusal = Some("completion candidate disappeared while recording review".to_string());
            return false;
        };
        if candidate.manifest.content_digest != *manifest_digest {
            refusal = Some("completion candidate changed while review was running".to_string());
            return false;
        }
        candidate.flip_receipt = Some(outcome.flip.receipt_object.clone());
        candidate.eval_receipt = outcome
            .eval
            .as_ref()
            .map(|receipt| receipt.receipt_object.clone());
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("completion-review".to_string()),
            user: None,
            message: format!(
                "Manifest {} review outcome: {:?}",
                manifest_digest, outcome.status
            ),
        });
        true
    })?;
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    Ok(())
}

fn collect_dependency_outputs(
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
    }
}
