//! Publish-time agency prerequisites and legacy evaluation migration.
//!
//! New publication creates only `.assign-<task> → task` when automatic
//! assignment is enabled. Evaluation and deep FLIP are selected lazily from an
//! authenticated candidate-completion event and stored as hidden source-bound
//! records. The explicit satellite builders remain temporarily for historical
//! graph compatibility/tests; production publication and coordinator ticks do
//! not call them.
//!
//! Placement (dependency edge decisions) is merged into assignment; no
//! separate `.place-*` tasks are created.

use chrono::Utc;
use std::path::Path;

use worksgood::config::Config;
use worksgood::graph::{Node, PRIORITY_DEFAULT, Priority, Status, Task, WorkGraph, lower_priority};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};

/// System task prefixes that are eligible for the full agency pipeline.
/// These tasks go through placement, assignment, FLIP, and evaluation like
/// regular tasks — unlike other system tasks (`.evaluate-*`, `.assign-*`,
/// `.flip-*`) which are infrastructure and skip the pipeline.
const PIPELINE_ELIGIBLE_PREFIXES: &[&str] = &[".verify-"];

/// Returns true if a task uses the shell executor (command execution, no LLM).
/// Shell tasks are exempt from the agency pipeline — no .assign-*, .flip-*,
/// or .evaluate-* scaffolding.
pub fn is_shell_task(task: &Task) -> bool {
    task.exec.is_some() || task.exec_mode.as_deref() == Some("shell")
}

/// Returns true if a system task (dot-prefixed) should still go through the
/// agency pipeline. `.verify-*` tasks are the primary example: they need
/// intelligent agent matching via the same placement/assignment/evaluation
/// chain as regular tasks.
pub fn is_pipeline_eligible_system_task(task_id: &str) -> bool {
    PIPELINE_ELIGIBLE_PREFIXES
        .iter()
        .any(|prefix| task_id.starts_with(prefix))
}

/// Calculate the automatic priority for a scaffolded task based on its parent.
///
/// Rules:
/// - .assign-* tasks: inherit parent priority (they gate the parent)
/// - .evaluate-* and .flip-* tasks: parent priority minus one level
/// - Defaults to Normal if parent priority cannot be determined
fn calculate_auto_priority(
    graph: &WorkGraph,
    parent_task_id: &str,
    scaffolding_type: &str,
) -> Priority {
    let parent_task = match graph.get_task(parent_task_id) {
        Some(task) => task,
        None => return PRIORITY_DEFAULT,
    };

    let parent_priority = parent_task.priority;

    match scaffolding_type {
        "assign" => parent_priority,
        "evaluate" | "flip" => lower_priority(parent_priority),
        _ => PRIORITY_DEFAULT,
    }
}

/// Returns true if FLIP should run for a given task.
fn should_run_flip(graph: &WorkGraph, task_id: &str, config: &Config) -> bool {
    let _ = (graph, task_id);
    config.agency.flip_enabled
}

fn plan_satellite(
    graph: &WorkGraph,
    source_task_id: &str,
    satellite_task_id: &str,
    config: &Config,
) -> Option<worksgood::eval_lifecycle::AgencyDispatchPlan> {
    let source = graph.get_task(source_task_id)?;
    match worksgood::eval_lifecycle::build_plan(
        config,
        source,
        satellite_task_id,
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    ) {
        Ok(plan) => Some(plan),
        Err(error) => {
            eprintln!(
                "[eval-scaffold] Cannot create '{}': no canonical agency route plan: {:#}",
                satellite_task_id, error
            );
            None
        }
    }
}

/// Create a `.flip-<task_id>` task in `graph`, blocked by `task_id`.
///
/// Returns `true` if the graph was modified (i.e. the flip task was created).
/// Idempotent: returns `false` if the flip task already exists.
pub fn scaffold_flip_task(graph: &mut WorkGraph, task_id: &str, config: &Config) -> bool {
    let flip_task_id = format!(".flip-{}", task_id);

    // Skip system tasks (unless pipeline-eligible like .verify-*)
    if worksgood::graph::is_system_task(task_id) && !is_pipeline_eligible_system_task(task_id) {
        return false;
    }

    // Idempotency: skip if flip task already exists
    if graph.get_task(&flip_task_id).is_some() {
        return false;
    }

    let Some(flip_plan) = plan_satellite(graph, task_id, &flip_task_id, config) else {
        return false;
    };
    let primary = &flip_plan.calls[0];

    // Calculate auto-priority for flip task
    let priority = calculate_auto_priority(graph, task_id, "flip");

    let flip_task = Task {
        id: flip_task_id.clone(),
        title: format!("FLIP: {}", task_id),
        description: Some(format!(
            "Run FLIP (Fidelity via Latent Intent Probing) evaluation for task '{}'.",
            task_id,
        )),
        status: Status::Open,
        priority,
        after: vec![task_id.to_string()],
        tags: vec!["flip".to_string(), "agency".to_string()],
        exec: Some(format!("wg evaluate run {} --flip", task_id)),
        model: Some(primary.route.clone()),
        provider: Some(primary.system.handler.clone()),
        endpoint: primary.endpoint.clone(),
        reasoning: primary.reasoning,
        agency_dispatch: Some(flip_plan),
        exec_mode: Some("bare".to_string()),
        visibility: "internal".to_string(),
        created_at: Some(Utc::now().to_rfc3339()),
        ..Task::default()
    };

    graph.add_node(Node::Task(flip_task));

    eprintln!(
        "[eval-scaffold] Created FLIP task '{}' blocked by '{}'",
        flip_task_id, task_id,
    );

    true
}

/// Scaffold publish-time agency prerequisites.
///
/// Assignment remains a real pre-execution task because it gates dispatch.
/// Evaluation and FLIP do not: they are selected lazily from a genuine
/// candidate-completion event and stored as hidden source evidence.
///
/// Returns `true` if assignment was created/wired or stale legacy evaluation
/// scaffolding was safely retired.
pub fn scaffold_full_pipeline(
    _dir: &Path,
    graph: &mut WorkGraph,
    task_id: &str,
    task_title: &str,
    config: &Config,
) -> bool {
    // Skip system tasks (unless pipeline-eligible like .verify-*)
    if worksgood::graph::is_system_task(task_id) && !is_pipeline_eligible_system_task(task_id) {
        return false;
    }
    // Skip shell executor tasks — they're commands, not agent work
    if let Some(task) = graph.get_task(task_id)
        && is_shell_task(task)
    {
        return false;
    }

    let assign_task_id = format!(".assign-{}", task_id);

    let mut any_created = false;

    // 1. Create .assign-* task (no deps — runs first via lightweight LLM call)
    // Placement (dependency edge decisions) is handled within the assignment step.
    if config.agency.auto_assign && graph.get_task(&assign_task_id).is_none() {
        let assign_task = Task {
            id: assign_task_id.clone(),
            title: format!("Assign agent for: {}", task_title),
            status: Status::Open,
            after: vec![],
            before: vec![task_id.to_string()],
            tags: vec!["assignment".to_string(), "agency".to_string()],
            exec: Some(format!("wg assign {} --auto", task_id)),
            exec_mode: Some("bare".to_string()),
            visibility: "internal".to_string(),
            created_at: Some(Utc::now().to_rfc3339()),
            ..Task::default()
        };
        graph.add_node(Node::Task(assign_task));
        any_created = true;
        eprintln!(
            "[eval-scaffold] Created assignment task '{}' blocking '{}'",
            assign_task_id, task_id,
        );
    }

    // 2. Wire main task to depend on .assign-* (so it waits for assignment)
    if graph.get_task(&assign_task_id).is_some()
        && let Some(source) = graph.get_task_mut(task_id)
        && !source.after.iter().any(|a| a == &assign_task_id)
    {
        source.after.push(assign_task_id.clone());
        any_created = true;
    }

    // Evaluation/FLIP are deliberately absent here. Publication owns only
    // assignment scaffolding; candidate completion atomically selects and
    // mints hidden attempt-bound records on the source.
    let retired = retire_stale_legacy_satellites(graph, task_id, false);
    if retired > 0 {
        any_created = true;
        eprintln!(
            "[eval-scaffold] Retired {} stale pre-created evaluation satellite(s) for '{}'",
            retired, task_id
        );
    }

    any_created
}

/// Retire unclaimed, evidence-free rows created by the legacy eager pipeline.
/// Claimed/running/terminal rows and historical verdict-bearing rows remain
/// readable and continue through the compatibility path.
pub fn retire_stale_legacy_satellites(
    graph: &mut WorkGraph,
    source_id: &str,
    candidate_completion: bool,
) -> usize {
    let source_eligible = graph.get_task(source_id).is_some_and(|source| {
        candidate_completion
            || (source.status == Status::Open
                && source
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_none_or(|attempt| attempt.disposition.is_some()))
    });
    if !source_eligible {
        return 0;
    }

    let mut retired = 0usize;
    for satellite_id in [
        format!(".flip-{source_id}"),
        format!(".evaluate-{source_id}"),
    ] {
        let safe = graph.get_task(&satellite_id).is_some_and(|satellite| {
            matches!(
                satellite.status,
                Status::Open | Status::Blocked | Status::Waiting
            ) && satellite.assigned.is_none()
                && satellite.started_at.is_none()
                && satellite
                    .evaluation_lifecycle
                    .as_ref()
                    .is_none_or(|lifecycle| {
                        lifecycle.linked_flip_verdict.is_none()
                            && lifecycle.linked_eval_verdict.is_none()
                            && lifecycle.consumed_verdict.is_none()
                    })
        });
        if !safe {
            continue;
        }
        let satellite = graph
            .get_task_mut(&satellite_id)
            .expect("safe check established satellite existence");
        let request = TransitionRequest::new(
            TransitionKind::Abandoned,
            LifecycleActor {
                kind: ActorKind::Operator,
                id: "lazy-evaluation-migration".to_string(),
            },
            "legacy_eager_evaluation_retired",
            format!("retire-eager-evaluation:{satellite_id}"),
        )
        .expecting(FenceExpectation::current(satellite));
        if apply_transition(satellite, request).is_ok() {
            satellite.completed_at = Some(Utc::now().to_rfc3339());
            retired += 1;
        }
    }
    retired
}

/// Scaffold publish-time prerequisites for multiple tasks at once.
/// Returns the number of tasks for which the pipeline was created.
pub fn scaffold_full_pipeline_batch(
    dir: &Path,
    graph: &mut WorkGraph,
    task_ids: &[(String, String)], // (id, title) pairs
    config: &Config,
) -> usize {
    let mut count = 0;
    for (task_id, task_title) in task_ids {
        if scaffold_full_pipeline(dir, graph, task_id, task_title, config) {
            count += 1;
        }
    }
    count
}

/// Create a `.assign-<task_id>` task in `graph` that blocks `task_id`.
///
/// The assign task is created Open with no dependencies (immediately ready).
/// The source task gets `.assign-<task_id>` added to its `after` list,
/// making it blocked until assignment completes.
///
/// Returns `true` if the graph was modified.
/// Idempotent: returns `false` if the assign task already exists.
pub fn scaffold_assign_task(graph: &mut WorkGraph, task_id: &str, task_title: &str) -> bool {
    let assign_task_id = format!(".assign-{}", task_id);

    // Idempotent: skip if assign task already exists
    if graph.get_task(&assign_task_id).is_some() {
        return false;
    }

    // Skip system tasks (unless pipeline-eligible like .verify-*) — no assign for .evaluate, .flip, etc.
    if worksgood::graph::is_system_task(task_id) && !is_pipeline_eligible_system_task(task_id) {
        return false;
    }

    // Skip shell executor tasks — they're commands, not agent work
    if let Some(task) = graph.get_task(task_id)
        && is_shell_task(task)
    {
        return false;
    }

    // Calculate auto-priority for assign task
    let priority = calculate_auto_priority(graph, task_id, "assign");

    let assign_task = Task {
        id: assign_task_id.clone(),
        title: format!("Assign agent for: {}", task_title),
        status: Status::Open,
        priority,
        after: vec![],
        before: vec![task_id.to_string()],
        tags: vec!["assignment".to_string(), "agency".to_string()],
        exec: Some(format!("wg assign {} --auto", task_id)),
        exec_mode: Some("bare".to_string()),
        visibility: "internal".to_string(),
        created_at: Some(Utc::now().to_rfc3339()),
        ..Task::default()
    };

    graph.add_node(Node::Task(assign_task));

    // Add blocking edge: source task depends on .assign-*
    if let Some(source) = graph.get_task_mut(task_id)
        && !source.after.iter().any(|a| a == &assign_task_id)
    {
        source.after.push(assign_task_id.clone());
    }

    eprintln!(
        "[eval-scaffold] Created assignment task '{}' blocking '{}'",
        assign_task_id, task_id,
    );

    true
}

/// Scaffold assign tasks for multiple task IDs at once (batch mode for publish).
/// Returns the number of assign tasks created.
#[allow(dead_code)]
pub fn scaffold_assign_tasks_batch(
    graph: &mut WorkGraph,
    task_ids: &[(String, String)], // (id, title) pairs
) -> usize {
    let mut count = 0;
    for (task_id, task_title) in task_ids {
        if scaffold_assign_task(graph, task_id, task_title) {
            count += 1;
        }
    }
    count
}

/// Create a `.evaluate-<task_id>` task in `graph`, blocked by `task_id`.
///
/// When FLIP is enabled, also creates `.flip-<task_id>` and makes
/// `.evaluate-<task_id>` depend on the flip task instead of the source task
/// directly.
///
/// Returns `true` if the graph was modified (i.e. the eval task was created).
/// Idempotent: returns `false` if the eval task already exists or the source
/// task should not be evaluated.
pub fn scaffold_eval_task(
    dir: &Path,
    graph: &mut WorkGraph,
    task_id: &str,
    task_title: &str,
    config: &Config,
) -> bool {
    let eval_task_id = format!(".evaluate-{}", task_id);

    // Skip system tasks (unless pipeline-eligible like .verify-*)
    if worksgood::graph::is_system_task(task_id) && !is_pipeline_eligible_system_task(task_id) {
        return false;
    }

    // Idempotency: skip if eval task already exists
    if graph.get_task(&eval_task_id).is_some() {
        return false;
    }

    // When FLIP is enabled, scaffold the flip task and make eval depend on it
    let run_flip = should_run_flip(graph, task_id, config);
    let eval_after = if run_flip {
        scaffold_flip_task(graph, task_id, config);
        let flip_task_id = format!(".flip-{}", task_id);
        vec![flip_task_id]
    } else {
        vec![task_id.to_string()]
    };

    // Resolve evaluator agent identity (if configured)
    let evaluator_identity = resolve_evaluator_identity(dir, config);

    let mut desc = String::new();
    if let Some(ref identity) = evaluator_identity {
        desc.push_str(identity);
        desc.push_str("\n\n");
    }
    desc.push_str(&format!(
        "Evaluate the completed task '{}'.\n\n\
         Run `wg evaluate run {}` to produce a structured evaluation.\n\
         This reads the task output from `.wg/output/{}/` and \
         the task definition via `wg show {}`.",
        task_id, task_id, task_id, task_id,
    ));

    let Some(eval_plan) = plan_satellite(graph, task_id, &eval_task_id, config) else {
        return false;
    };
    let primary = &eval_plan.calls[0];

    // Calculate auto-priority for eval task
    let priority = calculate_auto_priority(graph, task_id, "evaluate");

    let eval_task = Task {
        id: eval_task_id.clone(),
        title: format!("Evaluate: {}", task_title),
        description: Some(desc),
        status: Status::Open,
        priority,
        after: eval_after,
        tags: vec!["evaluation".to_string(), "agency".to_string()],
        exec: Some(format!("wg evaluate run {}", task_id)),
        model: Some(primary.route.clone()),
        provider: Some(primary.system.handler.clone()),
        endpoint: primary.endpoint.clone(),
        reasoning: primary.reasoning,
        agency_dispatch: Some(eval_plan),
        agent: config.agency.evaluator_agent.clone(),
        exec_mode: Some("bare".to_string()),
        visibility: "internal".to_string(),
        created_at: Some(Utc::now().to_rfc3339()),
        ..Task::default()
    };

    graph.add_node(Node::Task(eval_task));

    eprintln!(
        "[eval-scaffold] Created evaluation task '{}' blocked by '{}'",
        eval_task_id, task_id,
    );

    true
}

/// Resolve the evaluator agent identity prompt, if an evaluator agent is configured.
fn resolve_evaluator_identity(dir: &Path, config: &Config) -> Option<String> {
    use worksgood::agency::{
        load_agent, load_role, load_tradeoff, render_identity_prompt_rich, resolve_all_components,
        resolve_outcome,
    };

    config
        .agency
        .evaluator_agent
        .as_ref()
        .and_then(|agent_hash| {
            let agency_dir = dir.join("agency");
            let agents_dir = agency_dir.join("cache/agents");
            let agent_path = agents_dir.join(format!("{}.yaml", agent_hash));
            let agent = load_agent(&agent_path).ok()?;
            let roles_dir = agency_dir.join("cache/roles");
            let role_path = roles_dir.join(format!("{}.yaml", agent.role_id));
            let role = load_role(&role_path).ok()?;
            let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
            let tradeoff_path = tradeoffs_dir.join(format!("{}.yaml", agent.tradeoff_id));
            let tradeoff = load_tradeoff(&tradeoff_path).ok()?;
            let workgraph_root = dir;
            let resolved_skills = resolve_all_components(&role, workgraph_root, &agency_dir);
            let outcome = resolve_outcome(&role.outcome_id, &agency_dir);
            Some(render_identity_prompt_rich(
                &role,
                &tradeoff,
                &resolved_skills,
                outcome.as_ref(),
            ))
        })
}

/// Scaffold eval tasks for multiple task IDs at once (batch mode for publish).
/// Returns the number of eval tasks created.
pub fn scaffold_eval_tasks_batch(
    dir: &Path,
    graph: &mut WorkGraph,
    task_ids: &[(String, String)], // (id, title) pairs
    config: &Config,
) -> usize {
    let mut count = 0;
    for (task_id, task_title) in task_ids {
        if scaffold_eval_task(dir, graph, task_id, task_title, config) {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::{Node, Status, Task, WorkGraph};

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            status: Status::Open,
            ..Task::default()
        }
    }

    fn agency_config() -> Config {
        let mut config = Config::default();
        config.tiers.fast = Some("pi:test:agency".to_string());
        config.tiers.fast_reasoning = Some(worksgood::config::ReasoningLevel::Low);
        config
    }

    #[test]
    fn test_scaffold_creates_eval_task() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.flip_enabled = false;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        let modified = scaffold_eval_task(dir.path(), &mut graph, "my-task", "My Task", &config);
        assert!(modified);
        let eval = graph.get_task(".evaluate-my-task").unwrap();
        assert_eq!(eval.title, "Evaluate: My Task");
        assert_eq!(eval.after, vec!["my-task".to_string()]);
        assert!(eval.tags.contains(&"evaluation".to_string()));
        assert!(eval.tags.contains(&"agency".to_string()));
        assert_eq!(eval.exec, Some("wg evaluate run my-task".to_string()));
        assert_eq!(eval.exec_mode, Some("bare".to_string()));
        assert_eq!(eval.visibility, "internal");
    }

    #[test]
    fn test_scaffold_idempotent() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "my-task",
            "My Task",
            &config
        ));
        // Second call should be a no-op
        assert!(!scaffold_eval_task(
            dir.path(),
            &mut graph,
            "my-task",
            "My Task",
            &config
        ));
    }

    #[test]
    fn test_scaffold_evaluation_label_is_inert() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        let mut task = make_task("eval-infra", "Eval Infra");
        task.tags = vec!["evaluation".to_string()];
        graph.add_node(Node::Task(task));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "eval-infra",
            "Eval Infra",
            &config
        ));
        assert!(graph.get_task(".evaluate-eval-infra").is_some());
    }

    #[test]
    fn test_eval_scheduled_label_is_inert() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        let mut task = make_task("old-task", "Old Task");
        task.tags = vec!["eval-scheduled".to_string()];
        graph.add_node(Node::Task(task));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "old-task",
            "Old Task",
            &config
        ));
        assert!(graph.get_task(".evaluate-old-task").is_some());
    }

    #[test]
    fn test_scaffold_does_not_tag_source_task() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        scaffold_eval_task(dir.path(), &mut graph, "my-task", "My Task", &config);

        let source = graph.get_task("my-task").unwrap();
        assert!(!source.tags.contains(&"eval-scheduled".to_string()));
    }

    #[test]
    fn test_scaffold_batch() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("a", "Task A")));
        graph.add_node(Node::Task(make_task("b", "Task B")));
        let mut eval_task = make_task("c", "Eval Task");
        eval_task.tags = vec!["evaluation".to_string()];
        graph.add_node(Node::Task(eval_task));

        let ids = vec![
            ("a".to_string(), "Task A".to_string()),
            ("b".to_string(), "Task B".to_string()),
            ("c".to_string(), "Eval Task".to_string()),
        ];
        let count = scaffold_eval_tasks_batch(dir.path(), &mut graph, &ids, &config);
        assert_eq!(count, 3);
        assert!(graph.get_task(".evaluate-a").is_some());
        assert!(graph.get_task(".evaluate-b").is_some());
        assert!(graph.get_task(".evaluate-c").is_some());
    }

    // --- FLIP scaffolding tests ---

    #[test]
    fn test_scaffold_flip_creates_flip_task() {
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        let modified = scaffold_flip_task(&mut graph, "my-task", &config);
        assert!(modified);

        let flip = graph.get_task(".flip-my-task").unwrap();
        assert_eq!(flip.title, "FLIP: my-task");
        assert_eq!(flip.after, vec!["my-task".to_string()]);
        assert!(flip.tags.contains(&"flip".to_string()));
        assert!(flip.tags.contains(&"agency".to_string()));
        assert_eq!(
            flip.exec,
            Some("wg evaluate run my-task --flip".to_string())
        );
        assert_eq!(flip.exec_mode, Some("bare".to_string()));
        assert_eq!(flip.visibility, "internal");
    }

    #[test]
    fn test_scaffold_flip_idempotent() {
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        assert!(scaffold_flip_task(&mut graph, "my-task", &config));
        // Second call should be a no-op
        assert!(!scaffold_flip_task(&mut graph, "my-task", &config));
    }

    #[test]
    fn test_scaffold_eval_depends_on_flip_when_enabled() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        scaffold_eval_task(dir.path(), &mut graph, "my-task", "My Task", &config);

        // .flip-my-task should exist and depend on my-task
        let flip = graph.get_task(".flip-my-task").unwrap();
        assert_eq!(flip.after, vec!["my-task".to_string()]);

        // .evaluate-my-task should depend on .flip-my-task, NOT my-task
        let eval = graph.get_task(".evaluate-my-task").unwrap();
        assert_eq!(eval.after, vec![".flip-my-task".to_string()]);
    }

    #[test]
    fn test_scaffold_eval_depends_on_source_when_flip_disabled() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.flip_enabled = false;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        scaffold_eval_task(dir.path(), &mut graph, "my-task", "My Task", &config);

        // No .flip-my-task should exist
        assert!(graph.get_task(".flip-my-task").is_none());

        // .evaluate-my-task should depend on my-task directly
        let eval = graph.get_task(".evaluate-my-task").unwrap();
        assert_eq!(eval.after, vec!["my-task".to_string()]);
    }

    #[test]
    fn test_flip_eval_label_does_not_enable_flip() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.flip_enabled = false; // flip_enabled = false globally
        let mut graph = WorkGraph::new();
        let mut task = make_task("my-task", "My Task");
        task.tags = vec!["flip-eval".to_string()];
        graph.add_node(Node::Task(task));

        scaffold_eval_task(dir.path(), &mut graph, "my-task", "My Task", &config);

        assert!(graph.get_task(".flip-my-task").is_none());

        let eval = graph.get_task(".evaluate-my-task").unwrap();
        assert_eq!(eval.after, vec!["my-task".to_string()]);
    }

    #[test]
    fn test_flip_label_is_inert() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        let mut task = make_task("flip-infra", "Flip Infra");
        task.tags = vec!["flip".to_string()];
        graph.add_node(Node::Task(task));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "flip-infra",
            "Flip Infra",
            &config
        ));
        assert!(graph.get_task(".evaluate-flip-infra").is_some());
    }

    #[test]
    fn test_scaffold_does_not_skip_label_tagged_tasks() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        let mut task = make_task("labelled-work", "Normal implementation work");
        task.tags = vec![
            "agency".to_string(),
            "assignment".to_string(),
            "evaluation".to_string(),
            "reviewer".to_string(),
            "placement".to_string(),
        ];
        graph.add_node(Node::Task(task));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "labelled-work",
            "Normal implementation work",
            &config
        ));
        assert!(graph.get_task(".evaluate-labelled-work").is_some());

        assert!(scaffold_assign_task(
            &mut graph,
            "labelled-work",
            "Normal implementation work"
        ));
        assert!(graph.get_task(".assign-labelled-work").is_some());
    }

    // --- Assign scaffolding tests ---

    #[test]
    fn test_scaffold_assign_creates_assign_task() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        let modified = scaffold_assign_task(&mut graph, "my-task", "My Task");
        assert!(modified);

        let assign = graph.get_task(".assign-my-task").unwrap();
        assert_eq!(assign.title, "Assign agent for: My Task");
        assert_eq!(assign.status, Status::Open);
        assert_eq!(assign.before, vec!["my-task".to_string()]);
        assert!(assign.after.is_empty()); // No deps (placement merged into assignment)
        assert!(assign.tags.contains(&"assignment".to_string()));
        assert!(assign.tags.contains(&"agency".to_string()));
        assert_eq!(assign.visibility, "internal");

        // Source task should have .assign-* as a blocker
        let source = graph.get_task("my-task").unwrap();
        assert!(source.after.contains(&".assign-my-task".to_string()));
    }

    #[test]
    fn test_scaffold_assign_no_place_dependency() {
        // .assign-* tasks have no dependencies (placement is handled within the assignment step)
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        let modified = scaffold_assign_task(&mut graph, "my-task", "My Task");
        assert!(modified);

        let assign = graph.get_task(".assign-my-task").unwrap();
        assert!(assign.after.is_empty());
        assert_eq!(assign.before, vec!["my-task".to_string()]);
    }

    #[test]
    fn test_scaffold_assign_idempotent() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        assert!(scaffold_assign_task(&mut graph, "my-task", "My Task"));
        assert!(!scaffold_assign_task(&mut graph, "my-task", "My Task"));
    }

    #[test]
    fn test_scaffold_assign_skips_system_tasks() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(".evaluate-foo", "Eval Foo")));

        assert!(!scaffold_assign_task(
            &mut graph,
            ".evaluate-foo",
            "Eval Foo"
        ));
        assert!(graph.get_task(".assign-.evaluate-foo").is_none());
    }

    #[test]
    fn test_scaffold_assign_ignores_label_tags() {
        let mut graph = WorkGraph::new();
        let mut task = make_task("assign-infra", "Assign Infra");
        task.tags = vec!["assignment".to_string(), "agency".to_string()];
        graph.add_node(Node::Task(task));

        assert!(scaffold_assign_task(
            &mut graph,
            "assign-infra",
            "Assign Infra"
        ));
    }

    #[test]
    fn test_scaffold_assign_batch() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("a", "Task A")));
        graph.add_node(Node::Task(make_task("b", "Task B")));

        let ids = vec![
            ("a".to_string(), "Task A".to_string()),
            ("b".to_string(), "Task B".to_string()),
        ];
        let count = scaffold_assign_tasks_batch(&mut graph, &ids);
        assert_eq!(count, 2);
        assert!(graph.get_task(".assign-a").is_some());
        assert!(graph.get_task(".assign-b").is_some());
    }

    // --- scaffold_full_pipeline tests ---

    #[test]
    fn test_scaffold_full_pipeline_creates_only_assignment_before_completion() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_place = true;
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        let modified = scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);
        assert!(modified);

        assert!(graph.get_task(".assign-foo").is_some());
        assert!(graph.get_task(".flip-foo").is_none());
        assert!(graph.get_task(".evaluate-foo").is_none());
    }

    #[test]
    fn test_scaffold_full_pipeline_wires_all_edges() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_place = true;
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);

        // .assign-foo has no deps (placement is merged into assignment)
        let assign = graph.get_task(".assign-foo").unwrap();
        assert!(assign.after.is_empty());
        assert_eq!(assign.before, vec!["foo".to_string()]);

        // foo depends on .assign-foo
        let foo = graph.get_task("foo").unwrap();
        assert!(foo.after.contains(&".assign-foo".to_string()));

        // Candidate-bound evaluation has no graph edges before completion.
        assert!(graph.get_task(".flip-foo").is_none());
        assert!(graph.get_task(".evaluate-foo").is_none());
    }

    #[test]
    fn test_scaffold_full_pipeline_assign_has_no_deps() {
        // .assign-* tasks never have deps (placement is merged, not a separate step)
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);

        let assign = graph.get_task(".assign-foo").unwrap();
        assert!(assign.after.is_empty());
    }

    #[test]
    fn test_scaffold_full_pipeline_idempotent() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_place = true;
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        assert!(scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            "foo",
            "Foo Task",
            &config
        ));
        // Second call is a no-op because assignment is already wired.
        assert!(!scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            "foo",
            "Foo Task",
            &config
        ));
    }

    #[test]
    fn test_scaffold_full_pipeline_does_not_tag_source_as_eval_scheduled() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);

        let foo = graph.get_task("foo").unwrap();
        assert!(!foo.tags.contains(&"eval-scheduled".to_string()));
    }

    #[test]
    fn test_scaffold_full_pipeline_skips_system_tasks() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(".evaluate-foo", "Eval Foo")));

        let modified =
            scaffold_full_pipeline(dir.path(), &mut graph, ".evaluate-foo", "Eval Foo", &config);
        assert!(!modified);
    }

    #[test]
    fn test_scaffold_full_pipeline_ignores_label_tags() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        let mut graph = WorkGraph::new();
        let mut task = make_task("eval-infra", "Eval Infra");
        task.tags = vec!["evaluation".to_string()];
        graph.add_node(Node::Task(task));

        let modified =
            scaffold_full_pipeline(dir.path(), &mut graph, "eval-infra", "Eval Infra", &config);
        assert!(modified);
        assert!(graph.get_task(".assign-eval-infra").is_some());
        assert!(graph.get_task(".evaluate-eval-infra").is_none());
    }

    #[test]
    fn test_scaffold_full_pipeline_no_place_task_created() {
        // Placement is handled by the assignment step — no separate .place-* tasks
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_place = true;
        config.agency.auto_assign = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);

        assert!(
            graph.get_task(".place-foo").is_none(),
            ".place-* tasks should not be created"
        );
        assert!(
            graph.get_task(".assign-foo").is_some(),
            ".assign-* task should still be created"
        );
    }

    #[test]
    fn test_scaffold_full_pipeline_creates_assign_when_eval_exists() {
        // Regression: if scaffold_eval_task ran first (coordinator path),
        // scaffold_full_pipeline must still create .assign-* tasks.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_place = true;
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("foo", "Foo Task")));

        // Simulate coordinator's scaffold_eval_task running first.
        scaffold_eval_task(dir.path(), &mut graph, "foo", "Foo Task", &config);
        assert!(graph.get_task(".flip-foo").is_some());
        assert!(graph.get_task(".evaluate-foo").is_some());
        let source = graph.get_task("foo").unwrap();
        assert!(!source.tags.contains(&"eval-scheduled".to_string()));

        // Now scaffold_full_pipeline runs (publish path) — must still create
        // .assign-* despite the existing eval/flip tasks.
        let modified = scaffold_full_pipeline(dir.path(), &mut graph, "foo", "Foo Task", &config);
        assert!(
            modified,
            "scaffold_full_pipeline should have created .assign"
        );

        assert!(
            graph.get_task(".assign-foo").is_some(),
            ".assign-foo must exist even when label tags are set"
        );
        assert_eq!(
            graph.get_task(".flip-foo").unwrap().status,
            Status::Abandoned
        );
        assert_eq!(
            graph.get_task(".evaluate-foo").unwrap().status,
            Status::Abandoned
        );
    }

    #[test]
    fn test_verify_task_gets_assignment_but_no_eager_evaluation() {
        // .verify-* tasks are pipeline-eligible for assignment; their own
        // evaluation remains candidate-bound like any other source.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;

        let mut graph = WorkGraph::new();
        let mut verify_task = make_task(".verify-my-task", "Verify: my-task");
        verify_task.tags = vec!["verification".to_string(), "agency".to_string()];
        graph.add_node(Node::Task(verify_task));

        let modified = scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            ".verify-my-task",
            "Verify: my-task",
            &config,
        );
        assert!(modified, "should scaffold pipeline for .verify-* task");

        // .assign-.verify-my-task should exist and block .verify-my-task
        let assign = graph.get_task(".assign-.verify-my-task").unwrap();
        assert!(assign.tags.contains(&"assignment".to_string()));
        assert_eq!(
            assign.exec,
            Some("wg assign .verify-my-task --auto".to_string())
        );
        let verify = graph.get_task(".verify-my-task").unwrap();
        assert!(
            verify
                .after
                .contains(&".assign-.verify-my-task".to_string()),
            ".verify-* should depend on its .assign-* task"
        );

        assert!(graph.get_task(".flip-.verify-my-task").is_none());
        assert!(graph.get_task(".evaluate-.verify-my-task").is_none());
    }

    #[test]
    fn test_non_verify_system_tasks_still_skip_pipeline() {
        // System tasks like .evaluate-*, .flip-*, .assign-* should NOT get the pipeline.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;

        let mut graph = WorkGraph::new();
        let mut eval_task = make_task(".evaluate-my-task", "Evaluate: my-task");
        eval_task.tags = vec!["evaluation".to_string(), "agency".to_string()];
        graph.add_node(Node::Task(eval_task));

        let modified = scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            ".evaluate-my-task",
            "Evaluate: my-task",
            &config,
        );
        assert!(
            !modified,
            "should NOT scaffold pipeline for .evaluate-* task"
        );
        assert!(graph.get_task(".assign-.evaluate-my-task").is_none());
    }

    #[test]
    fn test_verify_assign_task_idempotent() {
        // If .assign-.verify-* already exists, scaffold_full_pipeline should not duplicate it.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;

        let mut graph = WorkGraph::new();
        let mut verify_task = make_task(".verify-t1", "Verify: t1");
        verify_task.tags = vec!["verification".to_string(), "agency".to_string()];
        graph.add_node(Node::Task(verify_task));

        // Pre-create the assign task
        let mut existing_assign = make_task(".assign-.verify-t1", "Pre-existing assign");
        existing_assign.tags = vec!["assignment".to_string(), "agency".to_string()];
        graph.add_node(Node::Task(existing_assign));

        let modified =
            scaffold_full_pipeline(dir.path(), &mut graph, ".verify-t1", "Verify: t1", &config);
        // Existing assignment is wired to the source, but no evaluation row is created.
        assert!(modified);

        // Existing assign should be preserved
        let assign = graph.get_task(".assign-.verify-t1").unwrap();
        assert_eq!(assign.title, "Pre-existing assign");
    }

    #[test]
    fn test_is_pipeline_eligible_system_task() {
        assert!(is_pipeline_eligible_system_task(".verify-my-task"));
        assert!(is_pipeline_eligible_system_task(".verify-feature-x"));
        assert!(!is_pipeline_eligible_system_task(".evaluate-my-task"));
        assert!(!is_pipeline_eligible_system_task(".assign-my-task"));
        assert!(!is_pipeline_eligible_system_task(".flip-my-task"));
        assert!(!is_pipeline_eligible_system_task("regular-task"));
    }

    #[test]
    fn test_scaffold_eval_skips_system_tasks() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();

        // .coordinator-* tasks should NOT get eval scaffolding
        graph.add_node(Node::Task(make_task(
            ".coordinator-test",
            "Coordinator Test",
        )));
        assert!(!scaffold_eval_task(
            dir.path(),
            &mut graph,
            ".coordinator-test",
            "Coordinator Test",
            &config
        ));
        assert!(graph.get_task(".evaluate-.coordinator-test").is_none());
        assert!(graph.get_task(".flip-.coordinator-test").is_none());

        // .archive-* tasks should NOT get eval scaffolding
        graph.add_node(Node::Task(make_task(".archive-test", "Archive Test")));
        assert!(!scaffold_eval_task(
            dir.path(),
            &mut graph,
            ".archive-test",
            "Archive Test",
            &config
        ));
        assert!(graph.get_task(".evaluate-.archive-test").is_none());

        // .compact-* tasks should NOT get eval scaffolding
        graph.add_node(Node::Task(make_task(".compact-0", "Compact")));
        assert!(!scaffold_eval_task(
            dir.path(),
            &mut graph,
            ".compact-0",
            "Compact",
            &config
        ));
        assert!(graph.get_task(".evaluate-.compact-0").is_none());

        // Normal tasks should still get eval scaffolding
        graph.add_node(Node::Task(make_task("normal-task", "Normal Task")));
        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            "normal-task",
            "Normal Task",
            &config
        ));
        assert!(graph.get_task(".evaluate-normal-task").is_some());
    }

    #[test]
    fn test_scaffold_flip_skips_system_tasks() {
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();

        graph.add_node(Node::Task(make_task(
            ".coordinator-test",
            "Coordinator Test",
        )));
        assert!(!scaffold_flip_task(
            &mut graph,
            ".coordinator-test",
            &config
        ));
        assert!(graph.get_task(".flip-.coordinator-test").is_none());

        graph.add_node(Node::Task(make_task(".archive-test", "Archive Test")));
        assert!(!scaffold_flip_task(&mut graph, ".archive-test", &config));
        assert!(graph.get_task(".flip-.archive-test").is_none());

        // Normal tasks should still get FLIP
        graph.add_node(Node::Task(make_task("normal-task", "Normal Task")));
        assert!(scaffold_flip_task(&mut graph, "normal-task", &config));
        assert!(graph.get_task(".flip-normal-task").is_some());
    }

    #[test]
    fn test_scaffold_eval_batch_skips_system_tasks() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("a", "Task A")));
        graph.add_node(Node::Task(make_task(
            ".coordinator-main",
            "Coordinator Main",
        )));
        graph.add_node(Node::Task(make_task(".archive-old", "Archive Old")));

        let ids = vec![
            ("a".to_string(), "Task A".to_string()),
            (
                ".coordinator-main".to_string(),
                "Coordinator Main".to_string(),
            ),
            (".archive-old".to_string(), "Archive Old".to_string()),
        ];
        let count = scaffold_eval_tasks_batch(dir.path(), &mut graph, &ids, &config);
        assert_eq!(count, 1); // Only "a" should get eval scaffolding
        assert!(graph.get_task(".evaluate-a").is_some());
        assert!(graph.get_task(".evaluate-.coordinator-main").is_none());
        assert!(graph.get_task(".evaluate-.archive-old").is_none());
    }

    #[test]
    fn test_verify_tasks_still_get_eval_scaffolding() {
        // .verify-* tasks are pipeline-eligible and SHOULD get eval scaffolding
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task(".verify-my-task", "Verify: my-task")));

        assert!(scaffold_eval_task(
            dir.path(),
            &mut graph,
            ".verify-my-task",
            "Verify: my-task",
            &config
        ));
        assert!(graph.get_task(".evaluate-.verify-my-task").is_some());
    }

    #[test]
    fn agency_skips_system_tasks() {
        // Integration-style: system tasks (.coordinator-*, .archive-*, .compact-*) get
        // no FLIP, no evaluate, and no assign scaffolding via any entry point.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;

        let system_ids = [
            (".coordinator-test", "Coordinator Test"),
            (".archive-test", "Archive Test"),
            (".compact-0", "Compact"),
            (".assign-foo", "Assign Foo"),
            (".flip-foo", "FLIP Foo"),
            (".evaluate-foo", "Evaluate Foo"),
            (".quality-pass-1", "Quality Pass"),
        ];

        let mut graph = WorkGraph::new();
        for (id, title) in &system_ids {
            graph.add_node(Node::Task(make_task(id, title)));
        }
        graph.add_node(Node::Task(make_task("normal-task", "Normal Task")));

        for (id, title) in &system_ids {
            assert!(
                !scaffold_full_pipeline(dir.path(), &mut graph, id, title, &config),
                "scaffold_full_pipeline should skip system task '{}'",
                id
            );
            assert!(
                !scaffold_eval_task(dir.path(), &mut graph, id, title, &config),
                "scaffold_eval_task should skip system task '{}'",
                id
            );
            assert!(
                !scaffold_flip_task(&mut graph, id, &config),
                "scaffold_flip_task should skip system task '{}'",
                id
            );
            assert!(
                !scaffold_assign_task(&mut graph, id, title),
                "scaffold_assign_task should skip system task '{}'",
                id
            );
        }

        // Normal tasks get assignment only; evaluation is lazy.
        assert!(scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            "normal-task",
            "Normal Task",
            &config
        ));
        assert!(graph.get_task(".assign-normal-task").is_some());
        assert!(graph.get_task(".flip-normal-task").is_none());
        assert!(graph.get_task(".evaluate-normal-task").is_none());
    }

    #[test]
    fn test_is_shell_task() {
        // Task with exec set → shell task
        let mut task = make_task("shell-1", "Shell Task");
        task.exec = Some("echo hello".to_string());
        assert!(is_shell_task(&task));

        // Task with exec_mode=shell → shell task
        let mut task2 = make_task("shell-2", "Shell Task 2");
        task2.exec_mode = Some("shell".to_string());
        assert!(is_shell_task(&task2));

        // Regular task → not a shell task
        let task3 = make_task("regular", "Regular Task");
        assert!(!is_shell_task(&task3));

        // Task with exec_mode=full → not a shell task
        let mut task4 = make_task("full", "Full Task");
        task4.exec_mode = Some("full".to_string());
        assert!(!is_shell_task(&task4));
    }

    #[test]
    fn test_shell_task_skips_full_pipeline() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();

        let mut task = make_task("run-tests", "Run Tests");
        task.exec = Some("cargo test".to_string());
        graph.add_node(Node::Task(task));

        let modified =
            scaffold_full_pipeline(dir.path(), &mut graph, "run-tests", "Run Tests", &config);
        assert!(!modified);
        assert!(graph.get_task(".assign-run-tests").is_none());
        assert!(graph.get_task(".flip-run-tests").is_none());
        assert!(graph.get_task(".evaluate-run-tests").is_none());
    }

    #[test]
    fn test_shell_task_skips_assign() {
        let mut graph = WorkGraph::new();

        let mut task = make_task("run-script", "Run Script");
        task.exec = Some("python3 run.py".to_string());
        graph.add_node(Node::Task(task));

        let modified = scaffold_assign_task(&mut graph, "run-script", "Run Script");
        assert!(!modified);
        assert!(graph.get_task(".assign-run-script").is_none());
    }

    #[test]
    fn test_checker_downstream_of_shell_gets_pipeline() {
        // A non-shell task depending on a shell task still gets assignment.
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();

        // Shell task
        let mut shell_task = make_task("run-batch", "Run Batch");
        shell_task.exec = Some("python3 batch.py".to_string());
        graph.add_node(Node::Task(shell_task));

        // Checker task (non-shell, depends on shell task)
        let mut checker = make_task("check-batch", "Check Batch");
        checker.after = vec!["run-batch".to_string()];
        graph.add_node(Node::Task(checker));

        // Shell task should not get pipeline
        let modified_shell =
            scaffold_full_pipeline(dir.path(), &mut graph, "run-batch", "Run Batch", &config);
        assert!(!modified_shell);

        // Checker task gets assignment, never eager evaluation.
        let modified_checker = scaffold_full_pipeline(
            dir.path(),
            &mut graph,
            "check-batch",
            "Check Batch",
            &config,
        );
        assert!(modified_checker);
        assert!(graph.get_task(".assign-check-batch").is_some());
        assert!(graph.get_task(".flip-check-batch").is_none());
        assert!(graph.get_task(".evaluate-check-batch").is_none());
    }

    // --- freeform label inertness tests ---

    #[test]
    fn test_skip_eval_label_does_not_prevent_flip_creation() {
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        let mut task = make_task("pulse-task", "Pulse Task");
        task.tags = vec!["skip-eval".to_string()];
        graph.add_node(Node::Task(task));

        let modified = scaffold_flip_task(&mut graph, "pulse-task", &config);
        assert!(
            modified,
            "freeform labels must not prevent .flip-* creation"
        );
        assert!(graph.get_task(".flip-pulse-task").is_some());
    }

    #[test]
    fn test_skip_eval_label_does_not_prevent_eval_creation() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        let mut task = make_task("pulse-task", "Pulse Task");
        task.tags = vec!["skip-eval".to_string()];
        graph.add_node(Node::Task(task));

        let modified =
            scaffold_eval_task(dir.path(), &mut graph, "pulse-task", "Pulse Task", &config);
        assert!(
            modified,
            "freeform labels must not prevent .evaluate-* creation"
        );
        assert!(graph.get_task(".evaluate-pulse-task").is_some());
        assert!(graph.get_task(".flip-pulse-task").is_some());
    }

    #[test]
    fn test_skip_eval_label_does_not_change_full_pipeline() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        config.agency.flip_enabled = true;
        let mut graph = WorkGraph::new();
        let mut task = make_task("pulse-task", "Pulse Task");
        task.tags = vec!["skip-eval".to_string()];
        graph.add_node(Node::Task(task));

        let modified =
            scaffold_full_pipeline(dir.path(), &mut graph, "pulse-task", "Pulse Task", &config);
        assert!(modified, "freeform labels do not suppress assignment");
        assert!(graph.get_task(".assign-pulse-task").is_some());
        assert!(graph.get_task(".flip-pulse-task").is_none());
        assert!(graph.get_task(".evaluate-pulse-task").is_none());
    }

    #[test]
    fn test_skip_eval_label_in_batch_is_inert() {
        let dir = tempdir().unwrap();
        let config = agency_config();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("normal", "Normal Task")));
        let mut skip_task = make_task("mechanical", "Mechanical Task");
        skip_task.tags = vec!["skip-eval".to_string()];
        graph.add_node(Node::Task(skip_task));

        let ids = vec![
            ("normal".to_string(), "Normal Task".to_string()),
            ("mechanical".to_string(), "Mechanical Task".to_string()),
        ];
        let count = scaffold_eval_tasks_batch(dir.path(), &mut graph, &ids, &config);
        assert_eq!(count, 2, "freeform labels must not suppress eval");
        assert!(graph.get_task(".evaluate-normal").is_some());
        assert!(graph.get_task(".evaluate-mechanical").is_some());
    }

    #[test]
    fn test_assign_tasks_do_not_need_control_tags() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        scaffold_assign_task(&mut graph, "my-task", "My Task");

        let assign = graph.get_task(".assign-my-task").unwrap();
        assert!(
            !assign.tags.contains(&"skip-eval".to_string()),
            ".assign-* tasks are internal by dot-prefixed identity, not labels"
        );
    }

    #[test]
    fn test_assign_tasks_in_full_pipeline_do_not_need_control_tags() {
        let dir = tempdir().unwrap();
        let mut config = agency_config();
        config.agency.auto_assign = true;
        config.agency.auto_evaluate = true;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("my-task", "My Task")));

        scaffold_full_pipeline(dir.path(), &mut graph, "my-task", "My Task", &config);

        let assign = graph.get_task(".assign-my-task").unwrap();
        assert!(
            !assign.tags.contains(&"skip-eval".to_string()),
            ".assign-* tasks created by full pipeline are structural internals"
        );
    }
}
