use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use worksgood::config::{Config, ReasoningLevel};
use worksgood::eval_lifecycle::{AgencyStage, EvaluationGateApplicability};
use worksgood::evaluation::bounded::{
    EvaluationLaneStatus, load_lane_status, run_one_pending, status_path,
};
use worksgood::evaluation::{
    EvaluationPolicySnapshot, EvaluationProduct, EvaluationRecord, EvaluationRouteCall,
    EvaluationRouteSnapshot, EvaluationState, SourceCandidateRef,
};
use worksgood::finalization::{
    CandidateBinding, CandidateDescriptor, ContentManifest, ManifestEntry, ValidationResult,
};
use worksgood::graph::{LogEntry, Node, Status, Task, WorkGraph};
use worksgood::parser::{load_graph, save_graph};

fn fixture_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-pi-bounded")
}

struct EnvGuard {
    key: &'static str,
    old: Option<OsString>,
}
impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let old = std::env::var_os(key);
        // SAFETY: every test mutating process environment is serial.
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
    fn remove(key: &'static str) -> Self {
        let old = std::env::var_os(key);
        // SAFETY: every test mutating process environment is serial.
        unsafe { std::env::remove_var(key) };
        Self { key, old }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: every test mutating process environment is serial.
        unsafe {
            match self.old.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn test_env(home: &Path) -> Vec<EnvGuard> {
    test_env_with_bin(home, &fixture_bin())
}

fn test_env_with_bin(home: &Path, bin: &Path) -> Vec<EnvGuard> {
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    vec![
        EnvGuard::set("PATH", std::env::join_paths(paths).unwrap()),
        EnvGuard::set("HOME", home),
        EnvGuard::remove("WG_TASK_ID"),
        EnvGuard::remove("WG_AGENT_ID"),
        EnvGuard::remove("WG_SPAWN_RUN_ID"),
        EnvGuard::remove("PI_SESSION_ID"),
        EnvGuard::remove("PI_SESSION_FILE"),
        EnvGuard::remove("OPENAI_API_KEY"),
        EnvGuard::remove("OPENROUTER_API_KEY"),
        EnvGuard::remove("ANTHROPIC_API_KEY"),
        EnvGuard::remove("AWS_SECRET_ACCESS_KEY"),
    ]
}

fn config(timeout: u64) -> Config {
    let mut config = Config::default();
    config.agency.inference_timeout = Some(timeout);
    config
}

fn git(project: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_candidate_evidence(dir: &Path) {
    let project = dir.parent().unwrap();
    fs::create_dir_all(project.join("src")).unwrap();
    git(project, &["init", "-q", "-b", "main"]);
    git(project, &["config", "user.email", "bounded@test.invalid"]);
    git(project, &["config", "user.name", "Bounded"]);
    fs::write(project.join("src/example.rs"), "pub const VALUE: u8 = 1;\n").unwrap();
    git(project, &["add", "src/example.rs"]);
    git(project, &["commit", "-qm", "base"]);
    let base = git(project, &["rev-parse", "HEAD"]);
    let base_tree = git(project, &["rev-parse", "HEAD^{tree}"]);
    fs::write(project.join("src/example.rs"), "pub const VALUE: u8 = 2;\n").unwrap();
    git(project, &["add", "src/example.rs"]);
    git(project, &["commit", "-qm", "candidate"]);
    let commit = git(project, &["rev-parse", "HEAD"]);
    let tree = git(project, &["rev-parse", "HEAD^{tree}"]);
    let blob = git(project, &["rev-parse", "HEAD:src/example.rs"]);

    let objects = dir.join("finalization/objects");
    fs::create_dir_all(&objects).unwrap();
    let binding = CandidateBinding {
        candidate_id: "wgcid:v1:blake3:candidate".into(),
        commit_oid: commit.clone(),
        tree_oid: tree.clone(),
        manifest_cid: "wgcid:v1:blake3:manifest".into(),
        delta_manifest_cid: "wgcid:v1:blake3:delta".into(),
    };
    let descriptor = CandidateDescriptor {
        schema_version: 1,
        candidate_id: binding.candidate_id.clone(),
        candidate_version: 1,
        task_id: "source".into(),
        generation: 3,
        attempt_id: "attempt-3-9".into(),
        attempt_fence: 9,
        process_epoch: 1,
        terminal_reservation_id: "terminal".into(),
        quiescence_receipt_cid: "quiet".into(),
        rescue_id: "rescue".into(),
        worktree_id: "agent-source".into(),
        worktree_lease_epoch: 1,
        base_commit_oid: base.clone(),
        base_tree_oid: base_tree,
        worker_head_oid: commit.clone(),
        candidate_commit_oid: commit.clone(),
        candidate_tree_oid: tree.clone(),
        content_manifest_cid: binding.manifest_cid.clone(),
        delta_manifest_cid: binding.delta_manifest_cid.clone(),
        validation_policy_cid: "validation-policy".into(),
        evaluation_policy: "bounded-test".into(),
        merge_policy_cid: "merge-policy".into(),
        route_snapshot_cid: "route".into(),
        immutable_ref: "refs/wg/candidates/source".into(),
        created_at: "2026-07-28T00:00:00Z".into(),
        binding: binding.clone(),
    };
    let content = fs::read(project.join("src/example.rs")).unwrap();
    let manifest = ContentManifest {
        schema_version: 1,
        tree_oid: tree.clone(),
        entries: vec![ManifestEntry {
            path: "src/example.rs".into(),
            git_mode: "100644".into(),
            kind: "blob".into(),
            git_object_oid: blob,
            blake3_content_digest: format!("wgcid:v1:blake3:{}", blake3::hash(&content).to_hex()),
            size: content.len() as u64,
        }],
    };
    let validation = ValidationResult {
        result_id: "wgcid:v1:blake3:validation".into(),
        request_id: "validation-request".into(),
        binding,
        policy_cid: "validation-policy".into(),
        materialized_tree_oid: tree,
        materialized_manifest_cid: "wgcid:v1:blake3:manifest".into(),
        passed: true,
        validator_identity: "test-validator".into(),
        created_at: "2026-07-28T00:00:00Z".into(),
    };
    for (cid, bytes) in [
        (
            "wgcid:v1:blake3:candidate",
            serde_json::to_vec(&descriptor).unwrap(),
        ),
        (
            "wgcid:v1:blake3:manifest",
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (
            "wgcid:v1:blake3:delta",
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "base": base,
                "candidate": commit,
            }))
            .unwrap(),
        ),
        (
            "wgcid:v1:blake3:validation",
            serde_json::to_vec(&validation).unwrap(),
        ),
    ] {
        fs::write(objects.join(cid.replace(':', "_")), bytes).unwrap();
    }
}

fn make_graph(dir: &Path, model: &str, applicability: EvaluationGateApplicability) {
    fs::create_dir_all(dir).unwrap();
    write_candidate_evidence(dir);
    let source = SourceCandidateRef {
        task_id: "source".into(),
        generation: 3,
        source_attempt_id: "attempt-3-9".into(),
        source_fence: 9,
        finalization_round: 1,
        candidate_digest: "wgcid:v1:blake3:candidate".into(),
        candidate_manifest_digest: "wgcid:v1:blake3:manifest".into(),
        dependency_revision_digest: "b3:dependencies".into(),
        validation_result_id: "wgcid:v1:blake3:validation".into(),
    };
    let exact_route = if model.starts_with("codex:") || model.starts_with("claude:") {
        model.to_string()
    } else {
        format!("pi:test:{model}")
    };
    let handler = exact_route.split(':').next().unwrap().to_string();
    let route = EvaluationRouteSnapshot {
        adapter: format!("{handler}-evaluation-v1"),
        calls: vec![EvaluationRouteCall {
            stage: AgencyStage::Evaluate,
            exact_route: exact_route.clone(),
            endpoint: None,
            reasoning: Some(ReasoningLevel::Low),
            handler,
            provider: "test".into(),
        }],
        digest: format!("b3:route-{model}"),
    };
    let record = EvaluationRecord {
        schema: 1,
        evaluation_id: format!("eval-{model}"),
        product: EvaluationProduct::Bounded,
        source,
        policy: EvaluationPolicySnapshot {
            product: EvaluationProduct::Bounded,
            applicability,
            threshold: (applicability == EvaluationGateApplicability::Required).then_some(0.7),
            selector: "test".into(),
            digest: "b3:policy".into(),
        },
        route_digest: route.digest.clone(),
        route: Some(route),
        state: EvaluationState::PreparingBundle,
        runner_attempts: vec![],
        attempts: vec![],
        evidence_ids: vec![],
        evidence_manifest_id: None,
        verdict: None,
        deep_report: None,
        prior_deep_reports: Vec::new(),
        consumed_verdict_id: None,
        created_by_event: "event-candidate".into(),
        created_at: "2026-07-28T00:00:00Z".into(),
        diagnostic: None,
    };
    let task = Task {
        id: "source".into(),
        title: "Implement bounded lane test".into(),
        description: Some("Original intent.\n\n## Validation\n- [ ] bounded lane works".into()),
        status: if applicability == EvaluationGateApplicability::Required {
            Status::PendingEval
        } else {
            Status::Done
        },
        model: Some("pi:test:source-model".into()),
        reasoning: Some(ReasoningLevel::High),
        artifacts: vec!["src/example.rs".into()],
        validation_commands: vec!["cargo test bounded".into()],
        log: vec![
            LogEntry {
                timestamp: "2026-07-28T00:00:00Z".into(),
                actor: Some("agent-source".into()),
                user: None,
                message: "Spawned by coordinator --executor pi --model pi:test:source-model".into(),
            },
            LogEntry {
                timestamp: "2026-07-28T00:01:00Z".into(),
                actor: Some("agent-source".into()),
                user: None,
                message: "Declared validation passed".into(),
            },
        ],
        evaluation_records: vec![record],
        ..Task::default()
    };
    let mut graph = WorkGraph::new();
    graph.add_node(Node::Task(task));
    save_graph(&graph, &dir.join("graph.jsonl")).unwrap();
}

fn record(dir: &Path) -> EvaluationRecord {
    load_graph(&dir.join("graph.jsonl"))
        .unwrap()
        .get_task("source")
        .unwrap()
        .evaluation_records[0]
        .clone()
}

#[test]
#[serial]
fn bounded_pi_lane_uses_no_worker_or_worktree() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(&dir, "fake-valid", EvaluationGateApplicability::Advisory);

    let tick = run_one_pending(&dir, &config(5)).unwrap();
    assert!(tick.ran);
    let record = record(&dir);
    assert_eq!(record.state, EvaluationState::Consumed);
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].executor, "pi");
    assert_eq!(record.attempts[0].usage.as_ref().unwrap().input_tokens, 17);
    assert_eq!(record.attempts[0].usage.as_ref().unwrap().cost_usd, 0.0033);
    assert!(
        record
            .evidence_manifest_id
            .as_deref()
            .unwrap()
            .starts_with("wgcid:v1:blake3:")
    );
    assert!(
        !dir.join("agents").exists(),
        "bounded lane entered agent registry surface"
    );
    assert!(
        !dir.join("worktrees").exists(),
        "bounded lane allocated a worktree"
    );
    assert!(
        !dir.join("service/disk").exists(),
        "bounded lane entered build admission/cache surface"
    );
    let args = fs::read_to_string(home.join("fake-pi-invocations.log")).unwrap();
    for flag in [
        "--mode json",
        "--print",
        "--no-tools",
        "-ne",
        "--no-session",
    ] {
        assert!(args.contains(flag), "missing {flag}: {args}");
    }
}

#[test]
#[serial]
fn bounded_pi_verdict_consumed_exactly_once() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(
        &dir,
        "fake-duplicate",
        EvaluationGateApplicability::Advisory,
    );

    assert!(run_one_pending(&dir, &config(5)).unwrap().ran);
    let first = record(&dir);
    assert_eq!(
        first.consumed_verdict_id,
        first.verdict.as_ref().map(|v| v.verdict_id.clone())
    );
    assert!(!run_one_pending(&dir, &config(5)).unwrap().ran);
    let replay = record(&dir);
    assert_eq!(replay.attempts.len(), 1);
    assert_eq!(replay.consumed_verdict_id, first.consumed_verdict_id);
    assert_eq!(replay.verdict, first.verdict);
    assert_eq!(
        fs::read_to_string(home.join("fake-pi-invocations.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
#[serial]
fn sufficient_coding_bounded_fail_is_candidate_bound_advisory_not_rejection() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(
        &dir,
        "fake-semantic-fail",
        EvaluationGateApplicability::Required,
    );

    assert!(run_one_pending(&dir, &config(5)).unwrap().ran);
    let graph = load_graph(&dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("source").unwrap();
    let record = &task.evaluation_records[0];
    assert_eq!(record.state, EvaluationState::Consumed);
    assert_eq!(
        record.verdict.as_ref().unwrap().outcome,
        worksgood::evaluation::BoundedVerdictOutcome::Fail
    );
    assert_eq!(task.status, Status::PendingEval);
    assert_eq!(task.retry_count, 0);
    assert_eq!(task.spawn_failures, 0);
    assert!(
        record
            .diagnostic
            .as_deref()
            .unwrap()
            .contains("coding-structural decisions require exact-candidate deep-readonly-flip")
    );
    assert!(
        !task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.event_kind.contains("acceptance-rejected"))
    );
}

#[test]
#[serial]
fn missing_automatic_validation_receipt_is_evidence_unavailable_before_model() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(&dir, "fake-valid", EvaluationGateApplicability::Required);
    fs::remove_file(dir.join("finalization/objects/wgcid_v1_blake3_validation")).unwrap();

    assert!(run_one_pending(&dir, &config(5)).unwrap().ran);
    let graph = load_graph(&dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("source").unwrap();
    let record = &task.evaluation_records[0];
    assert_eq!(record.state, EvaluationState::RetryBackoff);
    assert!(record.verdict.is_none());
    let failure = record.attempts[0].failure.as_ref().unwrap();
    assert_eq!(
        failure.kind,
        worksgood::evaluation::EvaluationFailureKind::EvidenceUnavailable
    );
    assert!(
        failure
            .safe_evidence_categories
            .contains(&"validation-receipt".into())
    );
    assert_eq!(task.status, Status::PendingEval);
    assert!(!home.join("fake-pi-invocations.log").exists());
}

#[test]
#[serial]
fn model_can_return_structured_insufficient_evidence_without_semantic_verdict() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(
        &dir,
        "fake-insufficient",
        EvaluationGateApplicability::Required,
    );

    assert!(run_one_pending(&dir, &config(5)).unwrap().ran);
    let graph = load_graph(&dir.join("graph.jsonl")).unwrap();
    let task = graph.get_task("source").unwrap();
    let record = &task.evaluation_records[0];
    assert_eq!(record.state, EvaluationState::RetryBackoff);
    assert!(record.verdict.is_none());
    assert!(record.consumed_verdict_id.is_none());
    let failure = record.attempts[0].failure.as_ref().unwrap();
    assert_eq!(
        failure.kind,
        worksgood::evaluation::EvaluationFailureKind::InsufficientEvidence
    );
    assert_eq!(failure.code, "WG-EVAL-INSUFFICIENT-EVIDENCE");
    assert!(failure.reported_usage.is_some());
    assert!(record.evidence_manifest_id.is_some());
    assert_eq!(
        failure.safe_evidence_ids.first(),
        record.evidence_manifest_id.as_ref()
    );
    assert!(
        failure
            .safe_evidence_ids
            .contains(&"candidate-source".into())
    );
    assert_eq!(task.status, Status::PendingEval);
    assert_eq!(task.retry_count, 0);
}

#[test]
#[serial]
fn pi_failure_never_cross_falls_back_executor() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let sentinels = tmp.path().join("sentinels");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&sentinels).unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::copy(fixture_bin().join("pi"), bin.join("pi")).unwrap();
    for name in ["codex", "claude"] {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\ntouch '{}/{}'\nexit 99\n",
                sentinels.display(),
                name
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let _env = test_env_with_bin(&home, &bin);
    let dir = tmp.path().join("wg");
    make_graph(
        &dir,
        "fake-malformed",
        EvaluationGateApplicability::Advisory,
    );

    assert!(run_one_pending(&dir, &config(5)).unwrap().ran);
    let failed = record(&dir);
    assert_eq!(failed.state, EvaluationState::Malformed);
    assert_eq!(
        failed.attempts[0].failure.as_ref().unwrap().code,
        "WG-EVAL-PI-VERDICT-SCHEMA"
    );
    assert!(!sentinels.join("codex").exists());
    assert!(!sentinels.join("claude").exists());
    assert_eq!(
        load_graph(&dir.join("graph.jsonl"))
            .unwrap()
            .get_task("source")
            .unwrap()
            .status,
        Status::Done,
        "advisory adapter failure must not reopen the source"
    );
}

#[test]
#[serial]
fn fake_pi_failure_matrix_captures_timeout_process_route_usage_and_unavailability() {
    for (model, expected_state, expected_code) in [
        (
            "fake-timeout",
            EvaluationState::TimedOut,
            "WG-EVAL-PI-TIMEOUT",
        ),
        (
            "fake-fail",
            EvaluationState::RetryBackoff,
            "WG-EVAL-PI-PROCESS",
        ),
        (
            "fake-route-drift",
            EvaluationState::RouteDrift,
            "WG-EVAL-PI-ROUTE-DRIFT",
        ),
        (
            "fake-reported-error",
            EvaluationState::RetryBackoff,
            "WG-EVAL-PI-REPORTED-ERROR",
        ),
        (
            "codex:fake-unavailable",
            EvaluationState::Unavailable,
            "WG-EVAL-ADAPTER-UNAVAILABLE",
        ),
        (
            "claude:fake-unavailable",
            EvaluationState::Unavailable,
            "WG-EVAL-ADAPTER-UNAVAILABLE",
        ),
    ] {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let _env = test_env(&home);
        let dir = tmp.path().join("wg");
        make_graph(&dir, model, EvaluationGateApplicability::Required);
        let tick = run_one_pending(&dir, &config(1)).unwrap();
        assert!(tick.ran, "{model}");
        let failed = record(&dir);
        assert_eq!(failed.state, expected_state, "{model}");
        let failure = failed.attempts[0].failure.as_ref().unwrap();
        assert_eq!(failure.code, expected_code, "{model}");
        assert!(
            failure.stdout_digest.is_some()
                || model.starts_with("codex:")
                || model.starts_with("claude:"),
            "{model}"
        );
        if model == "fake-reported-error" {
            let usage = failed.attempts[0]
                .usage
                .as_ref()
                .expect("Pi-reported usage survives a reported error");
            assert_eq!(usage.input_tokens, 17);
            assert_eq!(failure.reported_usage.as_ref(), Some(usage));
        }
        assert_eq!(
            load_graph(&dir.join("graph.jsonl"))
                .unwrap()
                .get_task("source")
                .unwrap()
                .status,
            Status::PendingEval,
            "hard-gated failure must remain explicitly awaiting evidence: {model}"
        );
        assert_eq!(
            load_graph(&dir.join("graph.jsonl"))
                .unwrap()
                .get_task("source")
                .unwrap()
                .spawn_failures,
            0
        );
    }
}

#[test]
#[serial]
fn resource_deferral_does_not_charge_attempt_or_source_failure() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _env = test_env(&home);
    let dir = tmp.path().join("wg");
    make_graph(&dir, "fake-valid", EvaluationGateApplicability::Advisory);
    fs::create_dir_all(status_path(&dir).parent().unwrap()).unwrap();
    fs::write(
        status_path(&dir),
        serde_json::to_vec(&EvaluationLaneStatus {
            schema_version: 1,
            active: 1,
            max_concurrency: 1,
            launch_limit_per_minute: 6,
            ..EvaluationLaneStatus::default()
        })
        .unwrap(),
    )
    .unwrap();
    let tick = run_one_pending(&dir, &config(5)).unwrap();
    assert!(tick.deferred);
    let task = load_graph(&dir.join("graph.jsonl"))
        .unwrap()
        .get_task("source")
        .unwrap()
        .clone();
    assert!(task.evaluation_records[0].attempts.is_empty());
    assert_eq!(task.spawn_failures, 0);
    assert_eq!(task.retry_count, 0);
    assert_eq!(load_lane_status(&dir).resource_deferrals, 1);
}

#[test]
fn response_schema_example_is_closed_and_dimensions_are_bounded() {
    let example = serde_json::json!({
        "schema_version": 2,
        "score": 0.9,
        "outcome": "pass",
        "dimensions": BTreeMap::from([("correctness", 0.9)]),
        "summary": "ok"
    });
    assert_eq!(example["schema_version"], 2);
}
