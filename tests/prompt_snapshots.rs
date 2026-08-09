//! Snapshot tests for all prompt generation functions.
//!
//! Uses `insta` to capture generated prompts as golden files.
//! Any change to prompt construction fails the test until explicitly approved
//! via `cargo insta review`.

use worksgood::agency::{
    self, EvaluatorInput, ResolvedSkill, Role, TradeoffConfig, render_evaluator_prompt,
    render_identity_prompt,
};
use worksgood::config::CLAUDE_SONNET_MODEL_ID;
use worksgood::context_scope::ContextScope;
use worksgood::graph::LogEntry;
use worksgood::service::executor::{ScopeContext, TemplateVars, build_prompt};

// ---------------------------------------------------------------------------
// Test data builders
// ---------------------------------------------------------------------------

fn test_role() -> Role {
    agency::build_role(
        "Builder",
        "Builds features from specifications with clean, tested code.",
        vec![
            "rust".to_string(),
            "inline:Write idiomatic Rust.".to_string(),
        ],
        "Working, tested code merged to main.",
    )
}

fn test_tradeoff() -> TradeoffConfig {
    agency::build_tradeoff(
        "Quality First",
        "Prioritise correctness and maintainability over speed.",
        vec![
            "Slower delivery for higher quality".into(),
            "More verbose code for clarity".into(),
        ],
        vec!["Skipping tests".into(), "Ignoring error handling".into()],
    )
}

fn test_skills() -> Vec<ResolvedSkill> {
    vec![
        ResolvedSkill {
            name: "Rust".into(),
            content: "Write idiomatic Rust code with proper error handling.".into(),
        },
        ResolvedSkill {
            name: "Testing".into(),
            content: "Write comprehensive unit and integration tests.".into(),
        },
    ]
}

fn test_log_entries() -> Vec<LogEntry> {
    vec![
        LogEntry {
            timestamp: "2025-01-15T10:00:00Z".into(),
            actor: Some("agent-abc".into()),
            user: None,
            message: "Starting implementation of feature X".into(),
        },
        LogEntry {
            timestamp: "2025-01-15T10:30:00Z".into(),
            actor: None,
            user: None,
            message: "Completed core logic, writing tests".into(),
        },
    ]
}

fn test_template_vars() -> TemplateVars {
    TemplateVars {
        task_id: "test-task-123".into(),
        task_title: "Implement widget factory".into(),
        task_description: "Build a widget factory that produces widgets from specs.".into(),
        task_context: "From prerequisite-task: Widget spec is defined in docs/spec.md".into(),
        task_identity: "## Agent Identity\n\nYou are a Builder agent.".into(),
        bound_session_summary: String::new(),
        working_dir: "/home/user/project".into(),
        skills_preamble: "".into(),
        model: CLAUDE_SONNET_MODEL_ID.into(),
        task_loop_info: "".into(),
        task_verify: None,
        max_child_tasks: 10,
        has_failed_deps: false,
        failed_deps_info: String::new(),
        in_worktree: false,
    }
}

fn test_scope_context() -> ScopeContext {
    ScopeContext {
        worker_control_info: "## Worker Control\n\n- **Effective mode:** `trusted`\n- **Restrictions:** normal local graph coordination is allowed".into(),
        downstream_info: "\n## Downstream Consumers\n\nTasks that depend on your work:\n- **verify-widgets**: \"Verify widget factory output\"".into(),
        tags_skills_info: "\n## Tags & Skills\n- Tags: implementation, rust\n- Skills: rust, testing".into(),
        project_description: "WG: A lightweight work coordination graph for humans and AI agents.".into(),
        graph_summary: "\n## Graph Status\n\n50 tasks — 45 done, 2 in-progress, 3 open".into(),
        full_graph_summary: "\n## Full Graph\n\nDetailed graph with all 50 tasks and their relationships.".into(),
        claude_md_content: "Use WG for task management.\nAlways run tests before marking done.".into(),
        queued_messages: String::new(),
        previous_attempt_context: String::new(),
        wg_guide_content: String::new(),
        discovered_tests: String::new(),
        decomp_guidance: true,
        telegram_available: false,
        native_file_tools: false,
    }
}

// ============================================================================
// render_identity_prompt snapshots
// ============================================================================

#[test]
fn snapshot_identity_prompt_full() {
    let role = test_role();
    let tradeoff = test_tradeoff();
    let skills = test_skills();
    let output = render_identity_prompt(&role, &tradeoff, &skills);
    insta::assert_snapshot!("identity_prompt_full", output);
}

#[test]
fn snapshot_identity_prompt_no_skills() {
    let role = agency::build_role(
        "Reviewer",
        "Reviews code for quality and correctness.",
        vec![],
        "All code reviewed and approved.",
    );
    let tradeoff = test_tradeoff();
    let output = render_identity_prompt(&role, &tradeoff, &[]);
    insta::assert_snapshot!("identity_prompt_no_skills", output);
}

#[test]
fn snapshot_identity_prompt_empty_tradeoffs() {
    let role = test_role();
    let tradeoff = agency::build_tradeoff("Minimal", "Minimal constraints.", vec![], vec![]);
    let skills = test_skills();
    let output = render_identity_prompt(&role, &tradeoff, &skills);
    insta::assert_snapshot!("identity_prompt_empty_tradeoffs", output);
}

#[test]
fn snapshot_identity_prompt_name_only_skills() {
    let role = test_role();
    let tradeoff = test_tradeoff();
    let skills = vec![
        ResolvedSkill {
            name: "rust".into(),
            content: "rust".into(),
        },
        ResolvedSkill {
            name: "testing".into(),
            content: "testing".into(),
        },
    ];
    let output = render_identity_prompt(&role, &tradeoff, &skills);
    insta::assert_snapshot!("identity_prompt_name_only_skills", output);
}

// ============================================================================
// render_evaluator_prompt snapshots
// ============================================================================

#[test]
fn snapshot_evaluator_prompt_full() {
    let role = test_role();
    let tradeoff = test_tradeoff();
    let artifacts = vec![
        "src/widget.rs".to_string(),
        "tests/test_widget.rs".to_string(),
    ];
    let log = test_log_entries();
    let skills = vec!["rust".to_string(), "testing".to_string()];

    let input = EvaluatorInput {
        task_title: "Implement widget factory",
        task_description: Some("Build a widget factory with full test coverage."),
        task_skills: &skills,
        verify: Some("All tests pass. No compiler warnings."),
        agent: None,
        role: Some(&role),
        tradeoff: Some(&tradeoff),
        artifacts: &artifacts,
        log_entries: &log,
        started_at: Some("2025-01-15T10:00:00Z"),
        completed_at: Some("2025-01-15T11:00:00Z"),
        artifact_diff: Some("diff --git a/src/widget.rs\n+pub fn create_widget() {}"),
        evaluator_identity: None,
        downstream_tasks: &[],
        flip_score: None,
        verify_status: None,
        verify_findings: None,
        resolved_outcome_name: None,
        child_tasks: &[],
        constraint_fidelity_score: None,
        constraint_fidelity_unanchored: None,
    };

    let output = render_evaluator_prompt(&input);
    insta::assert_snapshot!("evaluator_prompt_full", output);
}

#[test]
fn snapshot_evaluator_prompt_minimal() {
    let input = EvaluatorInput {
        task_title: "Simple task",
        task_description: None,
        task_skills: &[],
        verify: None,
        agent: None,
        role: None,
        tradeoff: None,
        artifacts: &[],
        log_entries: &[],
        started_at: None,
        completed_at: None,
        artifact_diff: None,
        evaluator_identity: None,
        downstream_tasks: &[],
        flip_score: None,
        verify_status: None,
        verify_findings: None,
        resolved_outcome_name: None,
        child_tasks: &[],
        constraint_fidelity_score: None,
        constraint_fidelity_unanchored: None,
    };

    let output = render_evaluator_prompt(&input);
    insta::assert_snapshot!("evaluator_prompt_minimal", output);
}

#[test]
fn snapshot_evaluator_prompt_with_evaluator_identity() {
    let input = EvaluatorInput {
        task_title: "Feature implementation",
        task_description: Some("Implement the feature."),
        task_skills: &[],
        verify: None,
        agent: None,
        role: None,
        tradeoff: None,
        artifacts: &["output.txt".to_string()],
        log_entries: &[],
        started_at: None,
        completed_at: None,
        artifact_diff: None,
        evaluator_identity: Some(
            "## Custom Evaluator\n\nYou are a specialized code quality evaluator.",
        ),
        downstream_tasks: &[],
        flip_score: None,
        verify_status: None,
        verify_findings: None,
        resolved_outcome_name: None,
        child_tasks: &[],
        constraint_fidelity_score: None,
        constraint_fidelity_unanchored: None,
    };

    let output = render_evaluator_prompt(&input);
    insta::assert_snapshot!("evaluator_prompt_with_identity", output);
}

#[test]
fn snapshot_evaluator_prompt_with_downstream_tasks() {
    let role = test_role();
    let tradeoff = test_tradeoff();
    let artifacts = vec!["src/api.rs".to_string()];
    let log = test_log_entries();
    let skills = vec!["rust".to_string()];
    let downstream = vec![
        (
            "Integrate API client".to_string(),
            "Open".to_string(),
            Some("Wire the API client into the service layer.".to_string()),
        ),
        ("Write API docs".to_string(), "Open".to_string(), None),
    ];

    let input = EvaluatorInput {
        task_title: "Build API client",
        task_description: Some("Implement the HTTP API client for the external service."),
        task_skills: &skills,
        verify: Some("API client compiles and unit tests pass."),
        agent: None,
        role: Some(&role),
        tradeoff: Some(&tradeoff),
        artifacts: &artifacts,
        log_entries: &log,
        started_at: Some("2025-01-15T10:00:00Z"),
        completed_at: Some("2025-01-15T11:30:00Z"),
        artifact_diff: None,
        evaluator_identity: None,
        downstream_tasks: &downstream,
        flip_score: None,
        verify_status: None,
        verify_findings: None,
        resolved_outcome_name: None,
        child_tasks: &[],
        constraint_fidelity_score: None,
        constraint_fidelity_unanchored: None,
    };

    let output = render_evaluator_prompt(&input);
    insta::assert_snapshot!("evaluator_prompt_with_downstream", output);
}

// ============================================================================
// build_prompt snapshots (all context scopes)
// ============================================================================

#[test]
fn shipped_worker_prompt_authority_audit_is_complete() {
    let vars = test_template_vars();
    let ctx = test_scope_context();
    for scope in [
        ContextScope::Clean,
        ContextScope::Task,
        ContextScope::Graph,
        ContextScope::Full,
    ] {
        let prompt = build_prompt(&vars, scope, &ctx);
        assert!(prompt.contains("## Worker Control"), "scope={scope:?}");
        assert!(prompt.contains("Effective mode"), "scope={scope:?}");
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // Scan every shipped Rust/Markdown/TypeScript source that can construct a
    // worker/system prompt. This is intentionally a recursive superset rather
    // than a hand-maintained file list: new prompt builders, dynamic fragments,
    // Pi tools (`wg_run`/`wg_*`), and command spellings enter the audit without
    // requiring this test to know their path first.
    fn scan_sources(
        root: &std::path::Path,
        path: &std::path::Path,
        shipped: &mut String,
        scanned: &mut usize,
        prompt_surfaces: &mut std::collections::BTreeMap<
            String,
            std::collections::BTreeSet<String>,
        >,
        commands: &mut std::collections::BTreeSet<String>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                scan_sources(root, &path, shipped, scanned, prompt_surfaces, commands);
                continue;
            }
            if !matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("rs" | "md" | "ts")
            ) {
                continue;
            }
            *scanned += 1;
            let content = std::fs::read_to_string(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let lines: Vec<&str> = content.lines().collect();
            let mut surface_commands = std::collections::BTreeSet::new();
            for (line_index, line) in lines.iter().enumerate() {
                let trimmed = line.trim_start();
                let source_comment = matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("rs" | "ts")
                ) && (trimmed.starts_with("//")
                    || trimmed.starts_with("///")
                    || trimmed.starts_with("//!"));
                let nearby = lines[line_index.saturating_sub(6)..(line_index + 7).min(lines.len())]
                    .join("\n")
                    .to_ascii_lowercase();
                let prompt_context = [
                    "prompt",
                    "instruction",
                    "you are",
                    "description",
                    "registertool",
                    "worker control",
                ]
                .iter()
                .any(|marker| nearby.contains(marker));
                for command in ["add", "edit", "assign", "reprioritize", "publish", "msg"] {
                    if !source_comment
                        && prompt_context
                        && (line.contains(&format!("wg {command}"))
                            || line.contains(&format!("wg_{command}")))
                    {
                        surface_commands.insert(command.to_string());
                    }
                }
            }
            if relative == "worksgood-pi/src/tools.ts" && content.contains("wg_run") {
                surface_commands.insert("wg_run".into());
            }
            if !surface_commands.is_empty() {
                prompt_surfaces.insert(relative.clone(), surface_commands);
            }
            for (offset, _) in content.match_indices("wg ") {
                let command: String = content[offset + 3..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                    .collect();
                if !command.is_empty() {
                    commands.insert(command);
                }
            }
            shipped.push_str(&format!("\n--- {relative} ---\n{content}"));
        }
    }

    let mut shipped = String::new();
    let mut scanned = 0;
    let mut prompt_surfaces = std::collections::BTreeMap::new();
    let mut commands = std::collections::BTreeSet::new();
    scan_sources(
        root,
        &root.join("src"),
        &mut shipped,
        &mut scanned,
        &mut prompt_surfaces,
        &mut commands,
    );
    scan_sources(
        root,
        &root.join("worksgood-pi/src"),
        &mut shipped,
        &mut scanned,
        &mut prompt_surfaces,
        &mut commands,
    );
    assert!(
        scanned > 100,
        "recursive shipped-source audit unexpectedly small"
    );
    // Discovery is recursive, while classification is explicit and exhaustive:
    // any new source surface containing a cross-task instruction fails until
    // its actual execution authority is named here.
    let expected_authority = std::collections::BTreeMap::from([
        ("src/text/agent_guide.md", "trusted-worker"),
        ("src/commands/show.rs", "operator-hint"),
        ("src/commands/add.rs", "operator-validation"),
        ("src/commands/placement.rs", "system-placement-adapter"),
        ("src/commands/quickstart.rs", "operator-help"),
        ("src/service/executor.rs", "trusted-worker"),
        ("src/commands/spawn/context.rs", "trusted-worker"),
        ("src/commands/service/coordinator_agent.rs", "coordinator"),
        ("worksgood-pi/src/tools.ts", "capability-aware-pi-tool"),
    ]);
    let discovered: std::collections::BTreeSet<&str> =
        prompt_surfaces.keys().map(String::as_str).collect();
    let classified: std::collections::BTreeSet<&str> = expected_authority.keys().copied().collect();
    assert_eq!(
        discovered, classified,
        "cross-task prompt surface classification is incomplete"
    );
    for (surface, surface_commands) in &prompt_surfaces {
        let authority = expected_authority[surface.as_str()];
        assert!(
            !surface_commands.is_empty(),
            "{surface} was discovered without commands"
        );
        let surface_text = std::fs::read_to_string(root.join(surface)).unwrap();
        match authority {
            "trusted-worker" => assert!(
                surface_text.contains("Worker Control")
                    || surface_text.contains("worker_control")
                    || surface_text.contains("capabilities"),
                "{surface} lacks its runtime trusted/scoped/read-only preflight"
            ),
            "capability-aware-pi-tool" => assert!(
                surface_commands.contains("wg_run")
                    && surface_text.contains("name: \"wg_capabilities\"")
                    && surface_text.contains("backend.capabilities"),
                "{surface} lacks Pi capability preflight"
            ),
            "operator-hint"
            | "operator-validation"
            | "operator-help"
            | "coordinator"
            | "system-placement-adapter" => {}
            other => panic!("unrecognized authority class {other} for {surface}"),
        }
    }
    assert!(shipped.contains("wg_run") && shipped.contains("wg_ready"));
    for command in ["capabilities", "add", "edit", "publish", "msg"] {
        assert!(
            commands.contains(command),
            "source-wide audit lost wg {command}"
        );
    }
    for boundary in [
        "Ordinary local workers are trusted participants",
        "stale/reaped attempt is refused",
        "completion remains own-task",
        "worker-control:inbound",
    ] {
        assert!(
            shipped.contains(boundary),
            "prompt inventory lost boundary: {boundary}"
        );
    }

    let quality = std::fs::read_to_string(root.join("docs/designs/quality-pass.md")).unwrap();
    for command in ["wg assign", "wg edit", "wg publish"] {
        assert!(quality.contains(command), "quality prompt lost {command}");
    }
    assert!(shipped.contains("name: \"wg_capabilities\""));
    assert!(shipped.contains("backend.capabilities"));
}

#[test]
fn snapshot_build_prompt_clean_scope() {
    let vars = test_template_vars();
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Clean, &ctx);
    insta::assert_snapshot!("build_prompt_clean", output);
}

#[test]
fn snapshot_build_prompt_task_scope() {
    let vars = test_template_vars();
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Task, &ctx);
    insta::assert_snapshot!("build_prompt_task", output);
}

#[test]
fn snapshot_build_prompt_graph_scope() {
    let vars = test_template_vars();
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Graph, &ctx);
    insta::assert_snapshot!("build_prompt_graph", output);
}

#[test]
fn snapshot_build_prompt_full_scope() {
    let vars = test_template_vars();
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Full, &ctx);
    insta::assert_snapshot!("build_prompt_full", output);
}

#[test]
fn snapshot_build_prompt_with_verify() {
    let mut vars = test_template_vars();
    vars.task_verify =
        Some("- cargo build passes\n- cargo test passes\n- No clippy warnings".into());
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Task, &ctx);
    insta::assert_snapshot!("build_prompt_with_verify", output);
}

#[test]
fn snapshot_build_prompt_with_loop_info() {
    let mut vars = test_template_vars();
    vars.task_loop_info =
        "## Cycle Information\n\nThis task is a cycle header (iteration 2, max 5).".into();
    let ctx = test_scope_context();
    let output = build_prompt(&vars, ContextScope::Task, &ctx);
    insta::assert_snapshot!("build_prompt_with_loop", output);
}
