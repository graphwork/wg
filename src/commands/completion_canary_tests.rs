use super::{completion_done, completion_finish, completion_submit};
use std::sync::{Arc, Barrier};
use tempfile::tempdir;
use worksgood::completion_manifest::{
    COMPLETION_MANIFEST_VERSION, CompletionManifest, ContentDigest, OutputRef,
};
use worksgood::completion_review::{
    ManifestReviewer, ReviewValveStatus, ReviewerKind, ReviewerUnavailable, SemanticReview,
    SemanticVerdict,
};
use worksgood::completion_task::requirements_digest;
use worksgood::graph::{
    CompletionContract, CompletionRepairDisposition, Node, Status, Task, WaitCondition, WaitSpec,
    WorkGraph,
};
use worksgood::lifecycle::AttemptRef;
use worksgood::parser::{load_graph, save_graph};
use worksgood::simple_land::CompletionContract as ManifestContract;

#[derive(Clone, Copy)]
enum Script {
    Pass,
    Reject,
    RejectWith(&'static str),
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
        binding: Option<&worksgood::completion_review::CompletionReviewBinding>,
        artifact_store: &worksgood::completion_manifest::CompletionArtifactStore,
    ) -> Result<SemanticReview, ReviewerUnavailable> {
        match self.script {
            Script::Pass => Ok(SemanticReview {
                verdict: SemanticVerdict::Pass,
                findings: Vec::new(),
                flip_proof: (kind == ReviewerKind::Flip).then(|| {
                    super::completion_test_support::test_flip_proof(
                        artifact_store,
                        bundle,
                        binding.expect("FLIP canary binding"),
                        &self.route,
                        SemanticVerdict::Pass,
                        &[],
                    )
                }),
            }),
            Script::Reject => {
                let findings = vec![worksgood::completion_review::ReviewFinding::new(
                    "canary.rejected",
                    "scripted semantic rejection",
                )];
                Ok(SemanticReview {
                    verdict: SemanticVerdict::Reject,
                    flip_proof: (kind == ReviewerKind::Flip).then(|| {
                        super::completion_test_support::test_flip_proof(
                            artifact_store,
                            bundle,
                            binding.expect("FLIP canary binding"),
                            &self.route,
                            SemanticVerdict::Reject,
                            &findings,
                        )
                    }),
                    findings,
                })
            }
            Script::RejectWith(code) => {
                let findings = vec![worksgood::completion_review::ReviewFinding::new(
                    code,
                    "scripted semantic rejection",
                )];
                Ok(SemanticReview {
                    verdict: SemanticVerdict::Reject,
                    flip_proof: (kind == ReviewerKind::Flip).then(|| {
                        super::completion_test_support::test_flip_proof(
                            artifact_store,
                            bundle,
                            binding.expect("FLIP canary binding"),
                            &self.route,
                            SemanticVerdict::Reject,
                            &findings,
                        )
                    }),
                    findings,
                })
            }
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
fn failed_required_check_repairs_in_same_attempt_and_publishes_once() {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(project)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    git(&["add", "base.txt"]);
    git(&["commit", "-qm", "base"]);
    std::fs::write(project.join("report.txt"), "completion repair report\n").unwrap();

    let mut task = Task {
        id: "same-worker-repair".into(),
        title: "Repair one required check".into(),
        description: Some("## Validation\nThe exact configured gate must pass.".into()),
        status: Status::InProgress,
        assigned: Some("repair-worker".into()),
        completion_contract: CompletionContract::Report,
        artifacts: vec![project.join("report.txt").display().to_string()],
        validation_commands: vec!["test -f gate.ok".into()],
        ..Task::default()
    };
    task.lifecycle.fence = 7;
    task.lifecycle.attempt_sequence = 1;
    task.lifecycle.current_attempt = Some(AttemptRef {
        id: "attempt-0-1".into(),
        generation: 0,
        fence: 7,
        actor_id: "repair-worker".into(),
        disposition: None,
    });
    let source_tuple = task.lifecycle.current_attempt.clone();
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();

    let first =
        completion_finish::run_at(&wg_dir, "same-worker-repair", "refs/heads/main", project)
            .unwrap_err();
    assert!(first.to_string().contains("evidence="));
    let failed = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let failed = failed.get_task("same-worker-repair").unwrap();
    assert_eq!(failed.status, Status::InProgress);
    assert_eq!(failed.assigned.as_deref(), Some("repair-worker"));
    assert_eq!(failed.lifecycle.current_attempt, source_tuple);
    let repair = failed.completion_repair.as_ref().unwrap();
    assert_eq!(
        repair.disposition,
        worksgood::graph::CompletionRepairDisposition::Repairing
    );
    assert!(repair.evidence.content_digest.as_str().starts_with("b3:"));

    std::fs::write(project.join("gate.ok"), "repaired\n").unwrap();
    completion_finish::run_at(&wg_dir, "same-worker-repair", "refs/heads/main", project).unwrap();
    // Lost-response replay verifies the exact selected candidate/publication;
    // it must not append a second terminal receipt/log.
    completion_finish::run_at(&wg_dir, "same-worker-repair", "refs/heads/main", project).unwrap();

    let completed = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let completed = completed.get_task("same-worker-repair").unwrap();
    assert_eq!(completed.status, Status::Done);
    assert!(completed.completion_repair.is_none());
    assert_eq!(
        completed
            .log
            .iter()
            .filter(|entry| entry
                .message
                .starts_with("Done derived from exact reviewed manifest"))
            .count(),
        1
    );
}

#[test]
fn owned_smoke_failure_enters_same_worker_repair() {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let smoke_dir = wg_dir.join("tests/smoke");
    std::fs::create_dir_all(&smoke_dir).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(project)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    git(&["add", "base.txt"]);
    git(&["commit", "-qm", "base"]);
    std::fs::write(smoke_dir.join("fail.sh"), "#!/bin/sh\nexit 6\n").unwrap();
    std::fs::write(
        smoke_dir.join("manifest.toml"),
        r#"[[scenario]]
name = "owned-failure"
script = "fail.sh"
owners = ["smoke-repair"]
timeout_seconds = 10
"#,
    )
    .unwrap();

    let mut task = Task {
        id: "smoke-repair".into(),
        title: "Repair owned smoke".into(),
        status: Status::InProgress,
        assigned: Some("repair-worker".into()),
        completion_contract: CompletionContract::Report,
        ..Task::default()
    };
    task.lifecycle.fence = 9;
    task.lifecycle.current_attempt = Some(AttemptRef {
        id: "attempt-0-1".into(),
        generation: 0,
        fence: 9,
        actor_id: "repair-worker".into(),
        disposition: None,
    });
    let source_tuple = task.lifecycle.current_attempt.clone();
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();

    let error = completion_finish::run_at_with_smoke(
        &wg_dir,
        "smoke-repair",
        "refs/heads/main",
        project,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("evidence="));
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("smoke-repair").unwrap();
    assert_eq!(task.status, Status::InProgress);
    assert_eq!(task.assigned.as_deref(), Some("repair-worker"));
    assert_eq!(task.lifecycle.current_attempt, source_tuple);
    let repair = task.completion_repair.as_ref().unwrap();
    assert_eq!(repair.reason_code, "deterministic-check-failed");
    assert_eq!(repair.exit_category, "nonzero-exit");
    assert!(repair.command.contains("owned smoke gate"));
    assert!(repair.evidence.content_digest.as_str().starts_with("b3:"));
}

#[test]
fn neutral_optional_check_is_captured_without_mutating_required_authority() {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let candidate_dir = project.join("candidate");
    std::fs::create_dir_all(&wg_dir).unwrap();
    std::fs::create_dir_all(&candidate_dir).unwrap();
    std::fs::write(
        wg_dir.join("config.toml"),
        "[agency]\ncompletion_review_strict = false\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(project)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(project.join("answer.txt"), "forty two\n").unwrap();
    git(&["add", "answer.txt"]);
    git(&["commit", "-qm", "neutral report"]);

    let mut task = Task {
        id: "neutral-report".into(),
        title: "Produce a neutral text report".into(),
        description: Some(
            "Produce answer.txt.\n\n## Validation\n- [ ] answer contains two words.\n\n## Coordination\nSend status quickly."
                .into(),
        ),
        status: Status::InProgress,
        assigned: Some("neutral-worker".into()),
        completion_contract: CompletionContract::Report,
        artifacts: vec![project.join("answer.txt").display().to_string()],
        ..Task::default()
    };
    task.lifecycle.fence = 4;
    task.lifecycle.current_attempt = Some(AttemptRef {
        id: "attempt-0-1".into(),
        generation: 0,
        fence: 4,
        actor_id: "neutral-worker".into(),
        disposition: None,
    });
    let captured = worksgood::completion_validation::capture_validation(
        &task,
        "test \"$(wc -w < answer.txt)\" -eq 2",
        0,
        worksgood::completion_validation::ValidationPurpose::Optional,
        project,
    )
    .unwrap();
    assert!(captured.exit.success);
    let optional = completion_finish::store_validation_evidence(
        &wg_dir,
        &captured,
        worksgood::completion_validation::OPTIONAL_VALIDATION_EVIDENCE_KIND,
    )
    .unwrap();
    assert!(task.validation_commands.is_empty());

    let store = completion_submit::store(&wg_dir).unwrap();
    let output = store
        .put_file(&project.join("answer.txt"), "text/plain")
        .unwrap();
    let summary = b"neutral report complete\n";
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        completion_contract: ManifestContract::Report,
        requirements_digest: requirements_digest(&task).unwrap(),
        source_revision: captured.repository.before_head_oid.clone(),
        outputs: vec![OutputRef::Artifact(output)],
        validation_evidence: vec![optional],
        worker_summary_digest: ContentDigest::of_bytes(summary),
    };
    let manifest_path = candidate_dir.join("manifest.json");
    let summary_path = candidate_dir.join("summary.txt");
    std::fs::write(&manifest_path, manifest.canonical_bytes().unwrap()).unwrap();
    std::fs::write(&summary_path, summary).unwrap();
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, wg_dir.join("graph.jsonl")).unwrap();

    let mut flip = ScriptedReviewer::new("pi:neutral-flip", Script::Reject);
    let mut eval = ScriptedReviewer::new("pi:neutral-eval", Script::Pass);
    let outcome = completion_submit::run_with_reviewers(
        &wg_dir,
        "neutral-report",
        &manifest_path,
        &summary_path,
        &mut flip,
        &mut eval,
    )
    .unwrap();
    assert_eq!(
        outcome.status,
        worksgood::completion_review::ReviewValveStatus::FlipRejected
    );
    completion_done::run(&wg_dir, "neutral-report", "refs/heads/main").unwrap();
    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let completed = graph.get_task("neutral-report").unwrap();
    assert_eq!(completed.status, Status::Done);
    assert!(completed.validation_commands.is_empty());
    let receipt = completed.completion_receipt.as_ref().unwrap();
    let receipt = std::fs::read(
        wg_dir
            .join("completion/v3/objects")
            .join(receipt.trim_start_matches("b3:")),
    )
    .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
    assert_eq!(receipt["review_policy"], "advisory");
    assert_eq!(receipt["semantic_outcome"], "advisory-findings");
}

#[test]
fn ten_concurrent_attempts_use_one_immutable_review_and_done_authority() {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let candidate_dir = project.join("candidates");
    std::fs::create_dir_all(&wg_dir).unwrap();
    std::fs::create_dir_all(&candidate_dir).unwrap();
    std::fs::write(
        wg_dir.join("config.toml"),
        "[agency]\ncompletion_review_strict = true\n",
    )
    .unwrap();
    // Real projects establish one graph identity before concurrent review
    // attempts. Pin it here so the canary exercises review concurrency rather
    // than racing first-use graph bootstrap.
    worksgood::worker_control::load_or_create_graph_identity(&wg_dir).unwrap();

    let store = completion_submit::store(&wg_dir).unwrap();
    let mut graph = WorkGraph::new();
    let mut candidates = Vec::new();

    for index in 0..10 {
        let id = format!("canary-{index}");
        let contract = contract_for(index);
        let actor_id = format!("agent-{index}");
        let mut task = Task {
            id: id.clone(),
            title: format!("Canary attempt {index}"),
            description: Some(format!(
                "Produce immutable output {index}.\n\n## Validation\nResolve and review exact bytes."
            )),
            status: Status::InProgress,
            assigned: Some(actor_id.clone()),
            completion_contract: contract,
            ..Task::default()
        };
        task.lifecycle.fence = index as u64 + 1;
        task.lifecycle.attempt_sequence = 1;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: format!("attempt-0-{}", index + 1),
            generation: 0,
            fence: task.lifecycle.fence,
            actor_id,
            disposition: None,
        });
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
            let findings = completion_submit::store(&wg_dir)
                .unwrap()
                .read_artifact(&outcome.flip.findings_object, 16 * 1024)
                .unwrap();
            assert_eq!(
                outcome.status,
                candidate.expected,
                "unexpected review outcome for {}: {}\n{outcome:#?}",
                candidate.id,
                String::from_utf8_lossy(&findings)
            );
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
    let flip_candidate = graph
        .get_task("canary-6")
        .unwrap()
        .completion_candidate
        .clone()
        .unwrap();
    let eval_candidate = graph
        .get_task("canary-7")
        .unwrap()
        .completion_candidate
        .clone()
        .unwrap();
    drop(graph);

    // Both semantic valves use the existing explicit completion-attention
    // surface. Requesting help records no acceptance and calls no reviewer.
    super::fail::run_with_intent(
        &wg_dir,
        "canary-6",
        Some("operator should decide the exact repair boundary"),
        None,
        Some("request-help"),
    )
    .unwrap();
    super::fail::run_with_intent(
        &wg_dir,
        "canary-7",
        Some("operator should approve one missing exact check"),
        None,
        Some("request-contract-correction"),
    )
    .unwrap();
    // Lost-response replay is idempotent.
    super::fail::run_with_intent(
        &wg_dir,
        "canary-7",
        Some("operator should approve one missing exact check"),
        None,
        Some("request-contract-correction"),
    )
    .unwrap();

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    for (id, reviewer, request, candidate) in [
        (
            "canary-6",
            ReviewerKind::Flip,
            "scope-approval-required",
            flip_candidate,
        ),
        (
            "canary-7",
            ReviewerKind::Eval,
            "contract-correction-required",
            eval_candidate,
        ),
    ] {
        let task = graph.get_task(id).unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert!(task.completion_receipt.is_none());
        assert_eq!(task.completion_candidate.as_ref(), Some(&candidate));
        let binding = candidate.review_binding.as_ref().unwrap();
        let repair = task.completion_repair.as_ref().unwrap();
        let semantic = repair.semantic_review.as_ref().unwrap();
        assert_eq!(repair.task_id, id);
        assert_eq!(repair.generation, binding.generation);
        assert_eq!(repair.attempt_id, binding.attempt_id);
        assert_eq!(repair.fence, binding.attempt_fence);
        assert_eq!(repair.candidate_identity, candidate.manifest.content_digest);
        assert_eq!(repair.reason_code, request);
        assert_eq!(semantic.reviewer_kind, reviewer);
        assert_eq!(semantic.candidate_sequence, binding.candidate_sequence);
        assert_eq!(semantic.review_receipt, repair.evidence.content_digest);
        assert_eq!(repair.exit_category, "semantic-rejection");
        assert!(repair.attention_event_id.is_some());
    }
    assert_eq!(
        graph
            .get_task("canary-7")
            .unwrap()
            .log
            .iter()
            .filter(|entry| entry.message.contains("NeedsAttention event="))
            .count(),
        1,
        "same request replay must retain one attention item"
    );
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

/// Build a minimal strict-mode Report fixture whose immutable candidate can be
/// rejected by the review valve without any model or network call.
fn recovery_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = tempdir().unwrap();
    let project = temp.path();
    let wg_dir = project.join(".wg");
    let candidate_dir = project.join("candidate");
    std::fs::create_dir_all(&wg_dir).unwrap();
    std::fs::create_dir_all(&candidate_dir).unwrap();
    std::fs::write(
        wg_dir.join("config.toml"),
        "[agency]\ncompletion_review_strict = true\ngate_max_attempts = 2\n",
    )
    .unwrap();
    let task = Task {
        id: "recovery-report".to_string(),
        title: "Recoverable review rejection".to_string(),
        description: Some(
            "Publish the exact report bytes.\n\n## Validation\nVerify the report bytes."
                .to_string(),
        ),
        status: Status::InProgress,
        assigned: Some("recovery-agent".to_string()),
        session_id: Some("canary-session".to_string()),
        completion_contract: CompletionContract::Report,
        ..Task::default()
    };
    let mut task = task;
    task.lifecycle.fence = 1;
    task.lifecycle.attempt_sequence = 1;
    task.lifecycle.current_attempt = Some(AttemptRef {
        id: "attempt-0-1".to_string(),
        generation: 0,
        fence: 1,
        actor_id: "recovery-agent".to_string(),
        disposition: None,
    });
    let store = completion_submit::store(&wg_dir).unwrap();
    let output = store.put_bytes(b"candidate bytes\n", "text/plain").unwrap();
    let evidence = store
        .evidence_from_bytes(b"validation ok\n", "validation", "text/plain")
        .unwrap();
    let summary = b"recovery summary\n";
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        completion_contract: ManifestContract::Report,
        requirements_digest: requirements_digest(&task).unwrap(),
        source_revision: "recovery-fixture".to_string(),
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
    (temp, wg_dir, manifest_path, summary_path)
}

fn reject_eval(
    wg_dir: &std::path::Path,
    manifest_path: &std::path::Path,
    summary_path: &std::path::Path,
    code: &'static str,
) -> (
    ReviewValveStatus,
    worksgood::completion_review::ReviewValveOutcome,
) {
    let mut flip = ScriptedReviewer::new("pi:canary/flip", Script::Pass);
    let mut eval = ScriptedReviewer::new("pi:canary/eval", Script::RejectWith(code));
    let outcome = completion_submit::run_with_reviewers(
        wg_dir,
        "recovery-report",
        manifest_path,
        summary_path,
        &mut flip,
        &mut eval,
    )
    .unwrap();
    (outcome.status, outcome)
}

#[test]
fn recoverable_rejection_resumes_same_node_with_bounded_checkpoint() {
    let (_temp, wg_dir, manifest_path, summary_path) = recovery_fixture();
    let (status, outcome) = reject_eval(
        &wg_dir,
        &manifest_path,
        &summary_path,
        "eval.substantive-gap",
    );
    assert_eq!(status, ReviewValveStatus::EvalRejected);
    let handled =
        completion_submit::handle_semantic_rejection(&wg_dir, "recovery-report", &outcome).unwrap();
    assert!(
        handled,
        "recoverable rejection must park for in-place recovery"
    );

    let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("recovery-report").unwrap();
    assert_eq!(task.status, Status::Waiting);
    assert_eq!(task.session_id.as_deref(), Some("canary-session"));
    assert!(task.completion_blocker.is_none());
    let repair = task.completion_repair.as_ref().unwrap();
    assert_eq!(repair.disposition, CompletionRepairDisposition::Repairing);
    assert_eq!(repair.recovery_round, Some(1));
    assert_eq!(repair.opportunities_used, 1);
    assert_eq!(repair.opportunity_limit, 2);
    assert_eq!(repair.failed_candidates.len(), 1);
    assert!(repair.semantic_review.is_some());

    match task.wait_condition.as_ref().expect("resumable wait") {
        WaitSpec::All(conditions) => match conditions.as_slice() {
            [WaitCondition::Timer { resume_after }] => {
                assert!(
                    resume_after
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .is_ok()
                );
            }
            other => panic!("expected one Timer condition, got {other:?}"),
        },
        other => panic!("expected WaitSpec::All, got {other:?}"),
    }
    let checkpoint = task.checkpoint.as_deref().expect("bounded checkpoint");
    assert!(checkpoint.contains("untrusted reviewer output"));
    assert!(checkpoint.contains("eval.substantive-gap"));
    assert!(checkpoint.contains("wg done recovery-report"));
    assert!(checkpoint.contains("RECOVERY ROUND 1"));
    assert_ne!(task.status, Status::Done);
    assert_ne!(
        task.completion_disposition,
        Some(worksgood::graph::CompletionDisposition::Landed)
    );
}

#[test]
fn each_irrecoverable_class_fails_closed_to_needs_attention() {
    for (code, expected) in [
        ("eval.gate-weakening", "gate-weakening"),
        ("eval.scope-ambiguity", "scope-ambiguity"),
        ("eval.missing-authority", "missing-authority"),
        ("eval.defect", "defect-or-incomplete-implementation"),
    ] {
        let (_temp, wg_dir, manifest_path, summary_path) = recovery_fixture();
        let (status, outcome) = reject_eval(&wg_dir, &manifest_path, &summary_path, code);
        assert_eq!(status, ReviewValveStatus::EvalRejected, "{code}");
        let handled =
            completion_submit::handle_semantic_rejection(&wg_dir, "recovery-report", &outcome)
                .unwrap();
        assert!(handled, "{code}");
        let graph = load_graph(wg_dir.join("graph.jsonl")).unwrap();
        let task = graph.get_task("recovery-report").unwrap();
        assert_eq!(task.status, Status::Waiting, "{code}");
        let repair = task.completion_repair.as_ref().unwrap();
        assert_eq!(
            repair.disposition,
            CompletionRepairDisposition::NeedsAttention,
            "{code}"
        );
        assert_eq!(repair.reason_code, expected, "{code}");
        assert_eq!(
            task.completion_blocker.as_ref().map(|blocker| blocker.kind),
            Some(worksgood::graph::CompletionBlockerKind::NeedsReview),
            "{code}"
        );
        assert_ne!(task.status, Status::Done, "{code}");
    }
}
