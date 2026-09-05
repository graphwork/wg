use super::{completion_done, completion_submit};
use std::sync::{Arc, Barrier};
use tempfile::tempdir;
use worksgood::completion_manifest::{
    ArtifactOutput, COMPLETION_MANIFEST_VERSION, CompletionManifest, ContentDigest,
    ImmutableLocator, OutputRef,
};
use worksgood::completion_review::{
    ManifestReviewer, ReviewerKind, ReviewerUnavailable, SemanticReview, SemanticVerdict,
};
use worksgood::completion_task::requirements_digest;
use worksgood::graph::{CompletionContract, Node, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, save_graph};
use worksgood::simple_land::CompletionContract as ManifestContract;

#[derive(Clone, Copy)]
enum Script {
    Pass,
    Reject,
    Unavailable,
}

struct ScriptedReviewer {
    route: String,
    script: Script,
}

impl ScriptedReviewer {
    fn new(route: impl Into<String>, script: Script) -> Self {
        Self {
            route: route.into(),
            script,
        }
    }
}

impl ManifestReviewer for ScriptedReviewer {
    fn route(&self) -> &str {
        &self.route
    }

    fn review(
        &mut self,
        kind: ReviewerKind,
        bundle: &worksgood::completion_manifest::ResolvedReviewBundle,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        let proof = || worksgood::completion_review::FlipProof {
            protocol: "prompt-reconstruction-two-phase-v1".into(),
            latent_hypothesis: ArtifactOutput {
                content_digest: bundle.manifest_digest.clone(),
                immutable_locator: ImmutableLocator::CompletionObject {
                    digest: bundle.manifest_digest.clone(),
                },
                media_type: "application/vnd.worksgood.flip-latent-hypothesis+json".into(),
                size: bundle.manifest_bytes.len() as u64,
                review_projection: None,
            },
            inference_route: self.route.clone(),
            comparison_route: self.route.clone(),
        };
        match self.script {
            Script::Pass => Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::new(),
                flip_proof: (kind == ReviewerKind::Flip).then(proof),
            }),
            Script::Reject => Ok(SemanticReview {
                verdict: SemanticVerdict::Reject,
                findings: vec![worksgood::completion_review::ReviewFinding::new(
                    "canary.rejected",
                    "scripted semantic rejection",
                )],
                flip_proof: (kind == ReviewerKind::Flip).then(proof),
            }),
            Script::Unavailable => Err(ReviewerUnavailable {
                code: "canary.unavailable".to_string(),
                message: "scripted reviewer outage".to_string(),
            }),
        }
    }
}

struct Candidate {
    id: String,
    manifest: std::path::PathBuf,
    summary: std::path::PathBuf,
    flip: Script,
    eval: Script,
    expected: worksgood::completion_review::ReviewValveStatus,
}

fn contract_for(index: usize) -> CompletionContract {
    if index == 4 || index == 5 {
        CompletionContract::Explore
    } else {
        CompletionContract::Report
    }
}

fn manifest_contract(contract: CompletionContract) -> ManifestContract {
    match contract {
        CompletionContract::Report => ManifestContract::Report,
        CompletionContract::Explore => ManifestContract::Explore,
        other => panic!("unsupported canary contract: {other}"),
    }
}

#[test]
#[ignore = "opt-in real Pi adapter smoke; run explicitly with PI_PROVIDER and PI_MODEL"]
fn real_pi_reviews_one_isolated_report_without_fallback() {
    let provider = std::env::var("PI_PROVIDER").expect("PI_PROVIDER is required");
    let model = std::env::var("PI_MODEL").expect("PI_MODEL is required");
    let route = format!("pi:{provider}:{model}");
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let candidate_dir = project.join("candidate");
    std::fs::create_dir_all(&wg_dir).unwrap();
    std::fs::create_dir_all(&candidate_dir).unwrap();
    std::fs::write(
        wg_dir.join("config.toml"),
        format!(
            "[models.reviewer]\nmodel = {route:?}\nreasoning = \"low\"\n\n[models.evaluator]\nmodel = {route:?}\nreasoning = \"low\"\n"
        ),
    )
    .unwrap();

    let task = Task {
        id: "real-pi-report".to_string(),
        title: "Review one exact report through Pi".to_string(),
        description: Some(
            "Publish the exact report bytes.\n\n## Validation\nVerify the report says adapter smoke passed."
                .to_string(),
        ),
        status: Status::InProgress,
        assigned: Some("real-pi-agent".to_string()),
        completion_contract: CompletionContract::Report,
        ..Task::default()
    };
    let store = completion_submit::store(&wg_dir).unwrap();
    let output = store
        .put_bytes(b"adapter smoke passed\n", "text/plain")
        .unwrap();
    let evidence = store
        .evidence_from_bytes(b"exact bytes checked\n", "adapter-smoke", "text/plain")
        .unwrap();
    let summary = b"real Pi adapter smoke completed\n";
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        completion_contract: ManifestContract::Report,
        requirements_digest: requirements_digest(&task).unwrap(),
        source_revision: "real-pi-smoke".to_string(),
        outputs: vec![OutputRef::Artifact(output)],
        validation_evidence: vec![evidence],
        worker_summary_digest: ContentDigest::of_bytes(summary),
    };
    let manifest_path = candidate_dir.join("manifest.json");
    let summary_path = candidate_dir.join("summary.txt");
    std::fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
    std::fs::write(&summary_path, summary).unwrap();
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();

    completion_submit::run(&wg_dir, "real-pi-report", &manifest_path, &summary_path).unwrap();
    completion_done::run(&wg_dir, "real-pi-report", "refs/heads/main").unwrap();

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("real-pi-report").unwrap();
    assert_eq!(task.status, Status::Done);
    let candidate = task.completion_candidate.as_ref().unwrap();
    assert!(candidate.flip_receipt.is_some());
    assert!(candidate.eval_receipt.is_some());
    assert!(!wg_dir.join("finalization").exists());
    assert!(!wg_dir.join("worker-control/transactions").exists());
}

#[test]
fn ten_concurrent_attempts_use_one_immutable_review_and_done_authority() {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let candidate_dir = project.join("candidates");
    std::fs::create_dir_all(&wg_dir).unwrap();
    std::fs::create_dir_all(&candidate_dir).unwrap();

    let store = completion_submit::store(&wg_dir).unwrap();
    let mut graph = WorkGraph::new();
    let mut candidates = Vec::new();

    for index in 0..10 {
        let id = format!("canary-{index}");
        let contract = contract_for(index);
        let task = Task {
            id: id.clone(),
            title: format!("Canary attempt {index}"),
            description: Some(format!(
                "Produce immutable output {index}.\n\n## Validation\nResolve and review exact bytes."
            )),
            status: Status::InProgress,
            assigned: Some(format!("agent-{index}")),
            completion_contract: contract,
            ..Task::default()
        };
        let requirements = requirements_digest(&task).unwrap();
        let summary_bytes = format!("completed canary attempt {index}\n").into_bytes();
        let output = store
            .put_bytes(
                format!("immutable output {index}\n").as_bytes(),
                "text/plain",
            )
            .unwrap();
        let evidence = store
            .evidence_from_bytes(
                format!("validation passed {index}\n").as_bytes(),
                "canary-validation",
                "text/plain",
            )
            .unwrap();
        let manifest = CompletionManifest {
            manifest_version: COMPLETION_MANIFEST_VERSION,
            task_id: id.clone(),
            generation: task.lifecycle.generation,
            completion_contract: manifest_contract(contract),
            requirements_digest: requirements,
            source_revision: format!("canary-session:{index}"),
            outputs: vec![OutputRef::Artifact(output.clone())],
            validation_evidence: vec![evidence],
            worker_summary_digest: ContentDigest::of_bytes(&summary_bytes),
        };
        let path = candidate_dir.join(&id);
        std::fs::create_dir_all(&path).unwrap();
        let manifest_path = path.join("manifest.json");
        let summary_path = path.join("summary.txt");
        std::fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
        std::fs::write(&summary_path, summary_bytes).unwrap();

        let (flip, eval, expected) = match index {
            6 => (
                Script::Reject,
                Script::Pass,
                worksgood::completion_review::ReviewValveStatus::FlipRejected,
            ),
            7 => (
                Script::Pass,
                Script::Reject,
                worksgood::completion_review::ReviewValveStatus::EvalRejected,
            ),
            8 => (
                Script::Unavailable,
                Script::Pass,
                worksgood::completion_review::ReviewValveStatus::ReviewUnavailable,
            ),
            9 => {
                let name = output.content_digest.as_str().strip_prefix("b3:").unwrap();
                std::fs::remove_file(wg_dir.join("completion/v3/objects").join(name)).unwrap();
                (
                    Script::Pass,
                    Script::Pass,
                    worksgood::completion_review::ReviewValveStatus::IncompleteEvidence,
                )
            }
            _ => (
                Script::Pass,
                Script::Pass,
                worksgood::completion_review::ReviewValveStatus::Accepted,
            ),
        };
        candidates.push(Candidate {
            id,
            manifest: manifest_path,
            summary: summary_path,
            flip,
            eval,
            expected,
        });
        graph.add_node(Node::Task(task));
    }
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();

    let barrier = Arc::new(Barrier::new(candidates.len()));
    let mut threads = Vec::new();
    for candidate in candidates {
        let barrier = barrier.clone();
        let wg_dir = wg_dir.clone();
        threads.push(std::thread::spawn(move || {
            let mut flip = ScriptedReviewer::new("pi:canary-flip", candidate.flip);
            let mut eval = ScriptedReviewer::new("codex:canary-eval", candidate.eval);
            barrier.wait();
            let outcome = completion_submit::run_with_reviewers(
                &wg_dir,
                &candidate.id,
                &candidate.manifest,
                &candidate.summary,
                &mut flip,
                &mut eval,
            )
            .unwrap();
            assert_eq!(outcome.status, candidate.expected);
            if outcome.status == worksgood::completion_review::ReviewValveStatus::Accepted {
                completion_done::run(&wg_dir, &candidate.id, "refs/heads/main").unwrap();
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    for index in 0..6 {
        let task = graph.get_task(&format!("canary-{index}")).unwrap();
        assert_eq!(task.status, Status::Done);
        assert!(task.completion_receipt.is_some());
        let candidate = task.completion_candidate.as_ref().unwrap();
        assert!(candidate.flip_receipt.is_some());
        assert!(candidate.eval_receipt.is_some());
    }
    for index in 6..10 {
        let task = graph.get_task(&format!("canary-{index}")).unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task.completion_receipt.is_none());
    }
    assert!(
        !wg_dir.join("finalization").exists(),
        "canary must not create FinalizationStore authority"
    );
    assert!(
        !wg_dir.join("worker-control/transactions").exists(),
        "canary must not create SaveTransaction authority"
    );

    let evidence = serde_json::json!({
        "canary_version": 1,
        "attempts": 10,
        "accepted_and_done": 6,
        "flip_rejected": 1,
        "eval_rejected": 1,
        "review_unavailable": 1,
        "incomplete_evidence": 1,
        "legacy_finalization_created": false,
        "legacy_save_transaction_created": false
    });
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
    std::fs::create_dir_all(&target_root).unwrap();
    let evidence_path = target_root.join("worker-owned-completion-canary.json");
    std::fs::write(evidence_path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
}
