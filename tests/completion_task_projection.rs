use tempfile::tempdir;
use worksgood::completion_manifest::{
    COMPLETION_MANIFEST_VERSION, CompletionArtifactStore, CompletionManifest, OutputRef,
    ReviewResolver,
};
use worksgood::completion_task::{
    CompletionCandidateRefs, load_submission_bytes, requirements_digest, task_requirements_bytes,
};
use worksgood::graph::{CompletionContract, Node, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, save_graph};
use worksgood::simple_land::CompletionContract as ManifestContract;

fn report_task() -> Task {
    Task {
        id: "report-task".to_string(),
        title: "Write exact report".to_string(),
        description: Some(
            "Produce the reviewed report.\n\n## Validation\nCheck exact bytes.".to_string(),
        ),
        status: Status::InProgress,
        completion_contract: CompletionContract::Report,
        deliverables: vec!["reports/report-task/report.md".to_string()],
        ..Task::default()
    }
}

#[test]
fn task_requirements_bind_contract_generation_and_description() {
    let task = report_task();
    let original = requirements_digest(&task).unwrap();

    let mut changed = task.clone();
    changed.description = Some("changed requirement".to_string());
    assert_ne!(original, requirements_digest(&changed).unwrap());

    let mut changed = task.clone();
    changed.lifecycle.generation += 1;
    assert_ne!(original, requirements_digest(&changed).unwrap());

    let mut changed = task;
    changed.completion_contract = CompletionContract::Explore;
    assert_ne!(original, requirements_digest(&changed).unwrap());
}

#[test]
fn graph_projects_only_immutable_candidate_references() {
    let root = tempdir().unwrap();
    let store = CompletionArtifactStore::open(root.path().join("objects")).unwrap();
    let mut task = report_task();
    let requirements = task_requirements_bytes(&task).unwrap();
    let requirements_ref = store
        .put_bytes(&requirements, "application/vnd.worksgood.requirements+json")
        .unwrap();
    let summary = b"Report generated and validated.";
    let summary_ref = store.put_bytes(summary, "text/plain").unwrap();
    let report = store
        .put_bytes(b"reviewed report\n", "text/markdown")
        .unwrap();
    let evidence = store
        .evidence_from_bytes(b"ok\n", "validation-log", "text/plain")
        .unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        completion_contract: ManifestContract::Report,
        requirements_digest: requirements_digest(&task).unwrap(),
        source_revision: "worker-session:test".to_string(),
        outputs: vec![OutputRef::Artifact(report)],
        validation_evidence: vec![evidence],
        worker_summary_digest: worksgood::completion_manifest::ContentDigest::of_bytes(summary),
    };
    let manifest_ref = store.put_manifest(&manifest).unwrap();
    task.completion_candidate = Some(CompletionCandidateRefs {
        manifest: manifest_ref,
        requirements: requirements_ref,
        worker_summary: summary_ref,
        dependency_outputs: Vec::new(),
        flip_receipt: None,
        eval_receipt: None,
    });

    let graph_path = root.path().join("graph.jsonl");
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &graph_path).unwrap();
    let graph = load_graph(&graph_path).unwrap();
    let task = graph.get_task("report-task").unwrap();
    let (_, loaded_manifest, loaded_requirements, loaded_summary) =
        load_submission_bytes(&store, task).unwrap();
    assert_eq!(loaded_manifest, manifest);
    assert_eq!(loaded_requirements, requirements);
    assert_eq!(loaded_summary, summary);

    ReviewResolver::new(&store)
        .resolve_submission(
            &task.completion_candidate.as_ref().unwrap().manifest,
            &loaded_requirements,
            &loaded_summary,
            &[],
        )
        .unwrap();
}

#[test]
fn changed_graph_requirements_invalidate_candidate_without_mutating_objects() {
    let root = tempdir().unwrap();
    let store = CompletionArtifactStore::open(root.path().join("objects")).unwrap();
    let mut task = report_task();
    let requirements = task_requirements_bytes(&task).unwrap();
    let requirements_ref = store
        .put_bytes(&requirements, "application/vnd.worksgood.requirements+json")
        .unwrap();
    let summary = b"summary";
    let summary_ref = store.put_bytes(summary, "text/plain").unwrap();
    let output = store.put_bytes(b"report", "text/plain").unwrap();
    let evidence = store
        .evidence_from_bytes(b"validated", "validation", "text/plain")
        .unwrap();
    let manifest = CompletionManifest {
        manifest_version: COMPLETION_MANIFEST_VERSION,
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        completion_contract: ManifestContract::Report,
        requirements_digest: requirements_digest(&task).unwrap(),
        source_revision: "session".to_string(),
        outputs: vec![OutputRef::Artifact(output)],
        validation_evidence: vec![evidence],
        worker_summary_digest: worksgood::completion_manifest::ContentDigest::of_bytes(summary),
    };
    task.completion_candidate = Some(CompletionCandidateRefs {
        manifest: store.put_manifest(&manifest).unwrap(),
        requirements: requirements_ref,
        worker_summary: summary_ref,
        dependency_outputs: Vec::new(),
        flip_receipt: None,
        eval_receipt: None,
    });
    task.description = Some("requirements changed after review".to_string());

    let error = load_submission_bytes(&store, &task).unwrap_err();
    assert!(error.to_string().contains("requirements changed"));
}
