//! RED-first regressions for attempt-bound lazy evaluation creation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;
use worksgood::config::{Config, ReasoningLevel};
use worksgood::evaluation::{
    EvaluationProduct, LazyEvaluationSelection, SourceCandidateRef, mint_for_candidate,
};
use worksgood::graph::{Status, Task, WorkGraph};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::{load_graph, save_graph};

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    path
}

fn wg_ok(wg_dir: &Path, args: &[&str]) {
    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .stdin(Stdio::null())
        .output()
        .expect("run wg");
    assert!(
        output.status.success(),
        "wg {args:?} failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn eval_config(flip: bool) -> Config {
    let mut config = Config::default();
    config.agency.auto_assign = false;
    config.agency.auto_evaluate = true;
    config.agency.flip_enabled = flip;
    config.agency.eval_gate_threshold = None;
    config.tiers.fast = Some("pi:test:fake-evaluator".into());
    config.tiers.fast_reasoning = Some(ReasoningLevel::Low);
    config
}

#[test]
fn publish_many_creates_no_evaluation_before_attempt_completion() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    save_graph(&WorkGraph::new(), &wg_dir.join("graph.jsonl")).unwrap();
    fs::write(
        wg_dir.join("config.toml"),
        toml::to_string_pretty(&eval_config(true)).unwrap(),
    )
    .unwrap();

    for n in 0..100 {
        let id = format!("lazy-{n}");
        wg_ok(&wg_dir, &["add", &id, "--id", &id]);
        wg_ok(&wg_dir, &["publish", &id, "--only"]);
    }

    let graph = load_graph(&wg_dir.join("graph.jsonl")).unwrap();
    assert_eq!(
        graph.tasks().filter(|t| !t.id.starts_with('.')).count(),
        100
    );
    assert_eq!(
        graph
            .tasks()
            .filter(|t| t.id.starts_with(".evaluate-") || t.id.starts_with(".flip-"))
            .count(),
        0,
        "publishing must not create graph satellites"
    );
    assert!(graph.tasks().all(|t| t.evaluation_records.is_empty()));
}

fn source_ref(id: &str, attempt_id: &str, fence: u64) -> SourceCandidateRef {
    SourceCandidateRef {
        task_id: id.into(),
        generation: 0,
        source_attempt_id: attempt_id.into(),
        source_fence: fence,
        finalization_round: 1,
        candidate_digest: "wgcid:v1:blake3:candidate".into(),
        candidate_manifest_digest: "wgcid:v1:blake3:manifest".into(),
        dependency_revision_digest: "b3:deps".into(),
        validation_result_id: "wgcid:v1:blake3:validation".into(),
    }
}

fn reserve(task: &mut Task) {
    let request = TransitionRequest::new(
        TransitionKind::AttemptReserved {
            owner_id: Some("worker".into()),
        },
        LifecycleActor {
            kind: ActorKind::Dispatcher,
            id: "test-dispatcher".into(),
        },
        "test-reserved",
        format!("reserve:{}", task.id),
    );
    apply_transition(task, request).unwrap();
}

fn running(task: &mut Task) {
    let request = TransitionRequest::new(
        TransitionKind::AttemptRunning {
            launch_receipt: format!("launch:{}", task.id),
        },
        LifecycleActor {
            kind: ActorKind::Dispatcher,
            id: "test-launcher".into(),
        },
        "test-running",
        format!("running:{}", task.id),
    )
    .expecting(FenceExpectation::current(task));
    apply_transition(task, request).unwrap();
}

fn checkpoint(task: &mut Task, source: &SourceCandidateRef) {
    let request = TransitionRequest::new(
        TransitionKind::CandidateCheckpointed {
            candidate_id: source.candidate_digest.clone(),
            manifest_cid: source.candidate_manifest_digest.clone(),
            validation_result_id: source.validation_result_id.clone(),
            finalization_round: source.finalization_round,
        },
        LifecycleActor {
            kind: ActorKind::Finalizer,
            id: "test-finalizer".into(),
        },
        "test-candidate-checkpointed",
        format!("candidate:{}", source.candidate_digest),
    )
    .expecting(FenceExpectation::current(task))
    .with_evidence(source.candidate_digest.clone())
    .with_evidence(source.candidate_manifest_digest.clone())
    .with_evidence(source.validation_result_id.clone());
    apply_transition(task, request).unwrap();
}

#[test]
fn never_ran_sources_never_evaluate() {
    let config = eval_config(true);
    let make = |id: &str| Task {
        id: id.into(),
        title: id.into(),
        status: Status::Open,
        ..Task::default()
    };
    let apply = |task: &mut Task, kind: TransitionKind, actor: ActorKind, suffix: &str| {
        let mut request = TransitionRequest::new(
            kind,
            LifecycleActor {
                kind: actor,
                id: format!("test-{suffix}"),
            },
            suffix,
            format!("{suffix}:{}", task.id),
        );
        if task
            .lifecycle
            .current_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.disposition.is_none())
        {
            request.expected = FenceExpectation::current(task);
        }
        apply_transition(task, request).unwrap();
    };

    let mut deferred = make("admission-deferral");
    apply(
        &mut deferred,
        TransitionKind::AdmissionDeferred {
            gate: "build-heavy-capacity".into(),
        },
        ActorKind::Dispatcher,
        "admission-deferred",
    );

    let mut launch_failed = make("launch-failure");
    reserve(&mut launch_failed);
    apply(
        &mut launch_failed,
        TransitionKind::AttemptFailed { class: None },
        ActorKind::ProcessObserver,
        "launch-failed",
    );

    let mut cancelled = make("cancelled");
    reserve(&mut cancelled);
    apply(
        &mut cancelled,
        TransitionKind::ReservationCancelled,
        ActorKind::Dispatcher,
        "reservation-cancelled",
    );

    let mut skipped = make("skipped");
    apply(
        &mut skipped,
        TransitionKind::Abandoned,
        ActorKind::Operator,
        "skipped",
    );

    let open = make("open");

    let mut message = make("message");
    apply(
        &mut message,
        TransitionKind::MessageObserved {
            message_id: "msg-1".into(),
        },
        ActorKind::Operator,
        "message-observed",
    );

    let mut reconciliation = make("reconciliation");
    reserve(&mut reconciliation);
    apply(
        &mut reconciliation,
        TransitionKind::ReconciliationIssue {
            issue_id: "dead-agent-no-candidate".into(),
        },
        ActorKind::Reconciler,
        "reconciliation-only",
    );

    for mut task in [
        deferred,
        launch_failed,
        cancelled,
        skipped,
        open,
        message,
        reconciliation,
    ] {
        let id = task.id.clone();
        let attempt = task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "attempt-never-ran".into());
        let source = source_ref(&id, &attempt, task.lifecycle.fence);
        let selection = LazyEvaluationSelection::resolve(&task, &config).unwrap();
        assert!(
            mint_for_candidate(&mut task, &source, &selection, &config).is_err(),
            "{id} must not be eligible"
        );
        assert!(task.evaluation_records.is_empty(), "{id} minted work");
        assert!(
            !worksgood::evaluation::has_authenticated_running_attempt(&task),
            "{id} acquired false running proof"
        );
    }
}

#[test]
fn candidate_completion_mints_selected_products_once() {
    let config = eval_config(false);
    let mut task = Task {
        id: "completed".into(),
        title: "Completed".into(),
        status: Status::Open,
        ..Task::default()
    };
    reserve(&mut task);
    running(&mut task);
    let attempt = task.lifecycle.current_attempt.as_ref().unwrap().id.clone();
    let source = source_ref(&task.id, &attempt, task.lifecycle.fence);
    checkpoint(&mut task, &source);
    let selection = LazyEvaluationSelection::resolve(&task, &config).unwrap();

    let first = mint_for_candidate(&mut task, &source, &selection, &config).unwrap();
    let replay = mint_for_candidate(&mut task, &source, &selection, &config).unwrap();
    assert_eq!(first.created, 1);
    assert_eq!(replay.created, 0);
    assert_eq!(task.evaluation_records.len(), 1);
    assert_eq!(
        task.evaluation_records[0].product,
        EvaluationProduct::Bounded
    );

    let bytes = serde_json::to_vec(&task).unwrap();
    let restarted: Task = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restarted.evaluation_records, task.evaluation_records);
}

#[test]
fn explicit_deep_flip_is_separate_from_bounded_default() {
    let config = eval_config(true);
    let mut task = Task {
        id: "high-risk".into(),
        title: "High risk".into(),
        status: Status::Open,
        ..Task::default()
    };
    reserve(&mut task);
    running(&mut task);
    let attempt = task.lifecycle.current_attempt.as_ref().unwrap().id.clone();
    let source = source_ref(&task.id, &attempt, task.lifecycle.fence);
    checkpoint(&mut task, &source);
    let selection = LazyEvaluationSelection::resolve(&task, &config).unwrap();
    mint_for_candidate(&mut task, &source, &selection, &config).unwrap();

    assert_eq!(task.evaluation_records.len(), 2);
    assert!(
        task.evaluation_records
            .iter()
            .any(|r| r.product == EvaluationProduct::Bounded)
    );
    assert!(
        task.evaluation_records
            .iter()
            .any(|r| r.product == EvaluationProduct::DeepReadonlyFlip)
    );
}

#[test]
fn high_risk_policy_selects_deep_flip_independently() {
    let mut config = eval_config(false);
    config.agency.auto_evaluate = false;
    let task = Task {
        id: "risk-only".into(),
        title: "Risk only".into(),
        tags: vec!["high-risk-evaluation".into()],
        ..Task::default()
    };
    let selection = LazyEvaluationSelection::resolve(&task, &config).unwrap();
    assert!(selection.bounded.is_none());
    assert_eq!(
        selection
            .deep_readonly_flip
            .as_ref()
            .map(|policy| policy.product),
        Some(EvaluationProduct::DeepReadonlyFlip)
    );
    assert_eq!(
        selection
            .deep_readonly_flip
            .as_ref()
            .map(|policy| policy.selector.as_str()),
        Some("deep:high-risk-task-policy")
    );
}

#[test]
fn historical_task_without_records_remains_readable() {
    let task: Task = serde_json::from_str(r#"{"id":"old","title":"Old"}"#).unwrap();
    assert!(task.evaluation_records.is_empty());
}
