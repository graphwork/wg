//! Lightweight LLM-based assignment: replaces full Claude Code sessions with a single API call.
//!
//! Pattern follows `triage.rs`: build prompt → call `run_lightweight_llm_call` → parse JSON → apply.

use anyhow::{Context, Result};

use worksgood::agency::{self, Agent, EvaluationRef, short_hash};
use worksgood::config::{Config, DispatchRole};
use worksgood::graph::{Task, TokenUsage, WorkGraph, is_system_task};

/// History partition used for assignment evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentHistoryClass {
    ActualWork,
    SystemAgency,
}

impl AssignmentHistoryClass {
    pub(crate) fn label(self) -> &'static str {
        match self {
            AssignmentHistoryClass::ActualWork => "actual_work",
            AssignmentHistoryClass::SystemAgency => "system_agency",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ScopedPerformance {
    pub avg_score: Option<f64>,
    pub task_count: u32,
}

/// Classify a graph task for assignment-history statistics.
pub(crate) fn classify_task_history(task: &Task) -> AssignmentHistoryClass {
    if task_history_id_is_system(&task.id) {
        AssignmentHistoryClass::SystemAgency
    } else {
        AssignmentHistoryClass::ActualWork
    }
}

/// Select the history bucket that should inform assignment for `task`.
pub(crate) fn history_class_for_assignment(task: &Task) -> AssignmentHistoryClass {
    if worksgood::assignment_eligibility::task_uses_work_pool(task) {
        AssignmentHistoryClass::ActualWork
    } else {
        AssignmentHistoryClass::SystemAgency
    }
}

fn classify_history_ref(
    eval_ref: &EvaluationRef,
    graph: Option<&WorkGraph>,
) -> AssignmentHistoryClass {
    if let Some(task) = graph.and_then(|g| g.get_task(&eval_ref.task_id)) {
        classify_task_history(task)
    } else if task_history_id_is_system(&eval_ref.task_id) {
        AssignmentHistoryClass::SystemAgency
    } else {
        AssignmentHistoryClass::ActualWork
    }
}

fn task_history_id_is_system(task_id: &str) -> bool {
    is_system_task(task_id)
}

/// Compute an agent's performance using only the selected history class.
pub(crate) fn scoped_performance_for_agent(
    agent: &Agent,
    graph: Option<&WorkGraph>,
    history_class: AssignmentHistoryClass,
) -> ScopedPerformance {
    let scores: Vec<f64> = agent
        .performance
        .evaluations
        .iter()
        .filter(|eval_ref| classify_history_ref(eval_ref, graph) == history_class)
        .map(|eval_ref| eval_ref.score)
        .filter(|score| score.is_finite())
        .collect();

    if scores.is_empty() {
        if history_class == AssignmentHistoryClass::SystemAgency
            && agent.performance.evaluations.is_empty()
            && agent.performance.task_count > 0
        {
            return ScopedPerformance {
                avg_score: agent.performance.avg_score,
                task_count: agent.performance.task_count,
            };
        }
        return ScopedPerformance::default();
    }

    let sum: f64 = scores.iter().sum();
    let avg = sum / scores.len() as f64;
    ScopedPerformance {
        avg_score: avg.is_finite().then_some(avg),
        task_count: scores.len() as u32,
    }
}

/// Placement decision: dependency edges to add to the source task.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct PlacementDecision {
    /// Task IDs to add as `after` dependencies (task runs after these).
    #[serde(default)]
    pub after: Vec<String>,
    /// Task IDs to add as `before` dependencies (task runs before these).
    #[serde(default)]
    pub before: Vec<String>,
}

/// Parsed assignment decision from the LLM.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AssignmentVerdict {
    /// Hash (or prefix) of the agent to assign. Use "new:<role_id>:<tradeoff_id>" to create.
    pub agent_hash: String,
    /// Execution weight: "shell", "bare", "light", or "full".
    #[serde(default)]
    pub exec_mode: Option<String>,
    /// Context scope: "clean", "task", "graph", or "full".
    #[serde(default)]
    pub context_scope: Option<String>,
    /// Brief explanation of the decision.
    #[serde(default)]
    pub reason: String,
    /// When true, the assigner signals that no good match was found and the
    /// primitive store should be expanded via the creator agent.
    #[serde(default)]
    pub create_needed: bool,
    /// Optional placement decision: dependency edges to add to the source task.
    /// When null or absent, no placement changes are made.
    #[serde(default)]
    pub placement: Option<PlacementDecision>,
}

/// Pre-gathered agent catalog entry for prompt rendering.
struct AgentEntry {
    hash: String,
    name: String,
    role_name: String,
    role_skills: Vec<String>,
    tradeoff_name: String,
    avg_score: Option<f64>,
    task_count: u32,
    history_class: AssignmentHistoryClass,
    capabilities: Vec<String>,
    _staleness_flags: Vec<String>,
}

/// Build the agent catalog for the assignment prompt.
fn build_agent_catalog(
    agents: &[Agent],
    roles_dir: &std::path::Path,
    tradeoffs_dir: &std::path::Path,
    graph: Option<&WorkGraph>,
    history_class: AssignmentHistoryClass,
) -> Vec<AgentEntry> {
    agents
        .iter()
        .filter(|a| !a.is_human() && a.staleness_flags.is_empty())
        .map(|a| {
            let role = agency::find_role_by_prefix(roles_dir, &a.role_id).ok();
            let tradeoff = agency::find_tradeoff_by_prefix(tradeoffs_dir, &a.tradeoff_id).ok();
            let scoped = scoped_performance_for_agent(a, graph, history_class);
            let role_skills = role
                .as_ref()
                .map(|r| r.component_ids.to_vec())
                .unwrap_or_default();
            AgentEntry {
                hash: short_hash(&a.id).to_string(),
                name: a.name.clone(),
                role_name: role.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
                role_skills,
                tradeoff_name: tradeoff.map(|t| t.name.clone()).unwrap_or_default(),
                avg_score: scoped.avg_score,
                task_count: scoped.task_count,
                history_class,
                capabilities: a.capabilities.clone(),
                _staleness_flags: a
                    .staleness_flags
                    .iter()
                    .map(|f| format!("{:?}", f))
                    .collect(),
            }
        })
        .collect()
}

/// Render the agent catalog as a compact text block for the prompt.
fn render_agent_catalog(entries: &[AgentEntry]) -> String {
    if entries.is_empty() {
        return "No agents available.\n".to_string();
    }
    let mut out = String::new();
    for e in entries {
        out.push_str(&format!(
            "- **{}** (hash: {}): role={}, tradeoff={}, history={}, score={}, tasks={}{}{}\n",
            e.name,
            e.hash,
            e.role_name,
            e.tradeoff_name,
            e.history_class.label(),
            e.avg_score
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "none".to_string()),
            e.task_count,
            if e.capabilities.is_empty() {
                String::new()
            } else {
                format!(", capabilities=[{}]", e.capabilities.join(", "))
            },
            if e.role_skills.is_empty() {
                String::new()
            } else {
                format!(", role_components=[{}]", e.role_skills.join(", "))
            },
        ));
    }
    out
}

/// Build the full assignment prompt for the lightweight LLM call.
///
/// When `active_tasks_context` is non-empty, the prompt includes an "Active Tasks"
/// section and placement instructions, asking the LLM to also decide dependency
/// edges for the source task.
pub(crate) fn build_assignment_prompt(
    task: &Task,
    mode_context: &str,
    agent_catalog: &str,
    history_class: AssignmentHistoryClass,
    underspec_warning: Option<&str>,
    active_tasks_context: &str,
    executor_type: &str,
) -> String {
    let task_id = &task.id;
    let task_title = &task.title;
    let task_desc = task.description.as_deref().unwrap_or("(no description)");
    let task_skills = if task.skills.is_empty() {
        "(none)".to_string()
    } else {
        task.skills.join(", ")
    };
    let task_tags = if task.tags.is_empty() {
        "(none)".to_string()
    } else {
        task.tags.join(", ")
    };
    let task_deps = if task.after.is_empty() {
        "(none)".to_string()
    } else {
        task.after.join(", ")
    };
    let context_scope_note = task
        .context_scope
        .as_ref()
        .map(|s| format!("\n- **Pre-set context scope:** {}", s))
        .unwrap_or_default();

    let underspec = underspec_warning.unwrap_or("");

    let placement_section = if active_tasks_context.is_empty() {
        String::new()
    } else {
        format!(
            r#"
## Active Tasks (for placement)

{active_tasks_context}
## Placement Instructions

In addition to agent assignment, decide whether this task needs additional dependency edges.
Look at the task's existing dependencies and the active tasks above. If the task should
run after or before any active tasks (beyond its current deps), include a `placement` field.
Only add edges that are clearly needed — do NOT add edges to system tasks (.assign-*, .flip-*, .evaluate-*).
If no placement changes are needed, set `placement` to null.
"#
        )
    };

    let placement_json = if active_tasks_context.is_empty() {
        String::new()
    } else {
        r#",
  "placement": null | {"after": ["task-id-1"], "before": ["task-id-2"]}"#
            .to_string()
    };

    let valid_modes = worksgood::config::ExecMode::valid_for_executor(executor_type);
    let valid_modes_str: Vec<String> = valid_modes.iter().map(|m| m.to_string()).collect();
    let valid_modes_list = valid_modes_str.join(", ");
    let valid_modes_pipe = valid_modes_str.join("|");

    let exec_mode_descriptions: String = valid_modes
        .iter()
        .map(|m| match m {
            worksgood::config::ExecMode::Shell => {
                "- **shell**: Task has exec command, no LLM needed."
            }
            worksgood::config::ExecMode::Bare => {
                "- **bare**: Pure reasoning, synthesis, no file access needed."
            }
            worksgood::config::ExecMode::Light => {
                "- **light**: Read-only file access (research, review, exploration)."
            }
            worksgood::config::ExecMode::Full => {
                "- **full**: Modifies files (implementation, debugging, refactoring, test writing). Default if unsure."
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are an agent assignment system. Given a task and available agents, select the best agent and configure execution parameters.

## Task
- **ID:** {task_id}
- **Title:** {task_title}
- **Description:** {task_desc}
- **Skills:** {task_skills}
- **Tags:** {task_tags}
- **Dependencies:** {task_deps}{context_scope_note}
{underspec}
{mode_context}
## Available Agents

{agent_catalog}
## Assignment History Class

Using `{history_class}` history for candidate scores and task counts. Normal work tasks ignore
`.assign-*`, `.flip-*`, `.evaluate-*`, `agency`, and reviewer/evaluator system history.

## Decision Criteria

1. **Role fit**: Agent's role skills should overlap with task requirements.
2. **Tradeoff fit**: Agent's operational style should match task nature (Careful for correctness-critical, Fast for routine, Thorough for complex).
3. **Performance**: Prefer agents with higher avg_score in the selected history class only.
4. **Capabilities**: Match agent capabilities to task tags/skills.
5. **Cold start**: When agents have no scores, match on role and spread work across untested agents.

## System Configuration
- **Available executor:** {executor_type}
- **Valid exec_modes:** {valid_modes_list}
- Do NOT use exec_modes outside this list. They will cause spawn failures.

## exec_mode Selection
{exec_mode_descriptions}

## context_scope Selection
- **clean**: Self-contained computation/writing, no WG interaction needed.
- **task**: Standard implementation (default if unsure).
- **graph**: Integration tasks spanning multiple components (3+ dependencies).
- **full**: Meta-tasks about WG itself.
{placement_section}
## Response

Respond with ONLY a JSON object (no markdown fences, no commentary):

{{
  "agent_hash": "<hash prefix of selected agent>",
  "exec_mode": "<{valid_modes_pipe}>",
  "context_scope": "<clean|task|graph|full>",
  "reason": "<one-sentence explanation>",
  "create_needed": false{placement_json}
}}

Always pick the closest match — never fail to assign. If no agent is a good fit
(the task requires capabilities not represented by any existing agent), still assign
the best available but set `"create_needed": true` to signal that new agent types
should be created for future tasks like this."#,
        history_class = history_class.label()
    )
}

/// Run the lightweight assignment LLM call and parse the verdict.
/// Returns the assignment verdict and any token usage from the LLM call.
///
/// When `active_tasks_context` is non-empty, the prompt includes placement
/// instructions and the verdict may contain a `placement` field with dependency
/// edges to apply to the source task.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_lightweight_assignment(
    config: &Config,
    task: &Task,
    agents: &[Agent],
    roles_dir: &std::path::Path,
    tradeoffs_dir: &std::path::Path,
    mode_context: &str,
    underspec_warning: Option<&str>,
    active_tasks_context: &str,
    graph: Option<&WorkGraph>,
) -> Result<(AssignmentVerdict, Option<TokenUsage>)> {
    // Assignment inference shares the agency one-shot hard deadline, not the
    // short triage budget. Its inline supervisor heartbeats independently.
    let timeout_secs = config.agency.inference_timeout_secs();
    let history_class = history_class_for_assignment(task);

    let catalog_entries =
        build_agent_catalog(agents, roles_dir, tradeoffs_dir, graph, history_class);
    let catalog_text = render_agent_catalog(&catalog_entries);

    let executor_type = config.coordinator.effective_executor();
    let prompt = build_assignment_prompt(
        task,
        mode_context,
        &catalog_text,
        history_class,
        underspec_warning,
        active_tasks_context,
        &executor_type,
    );
    eprintln!(
        "[assignment] history_class={} task='{}' candidates={} (candidate score/task_count evidence filtered to this class)",
        history_class.label(),
        task.id,
        catalog_entries.len()
    );

    let result = worksgood::service::llm::run_lightweight_llm_call(
        config,
        DispatchRole::Assigner,
        &prompt,
        timeout_secs,
    )
    .context("Assignment LLM call failed")?;

    let token_usage = result.token_usage;

    // Parse JSON verdict from output (reuse triage JSON extraction logic)
    let json_str = extract_assignment_json(&result.text).ok_or_else(|| {
        anyhow::anyhow!(
            "No valid JSON found in assignment output: {}",
            &result.text[..result.text.len().min(200)]
        )
    })?;

    let mut verdict: AssignmentVerdict = serde_json::from_str(&json_str)
        .with_context(|| format!("Failed to parse assignment JSON: {}", json_str))?;

    // Validate exec_mode against the configured executor
    verdict.exec_mode = Some(validate_exec_mode(
        verdict.exec_mode.as_deref(),
        &executor_type,
    ));

    // Validate context_scope
    if let Some(ref scope) = verdict.context_scope {
        match scope.as_str() {
            "clean" | "task" | "graph" | "full" => {}
            other => {
                eprintln!(
                    "[assignment] Warning: invalid context_scope '{}', defaulting to 'task'",
                    other
                );
            }
        }
    }

    Ok((verdict, token_usage))
}

/// Build a compact list of active (non-terminal, non-paused, non-system) tasks
/// for placement context. Only includes task IDs and titles to keep the prompt slim.
pub(crate) fn build_active_tasks_context(graph: &WorkGraph, exclude_task_id: &str) -> String {
    let active_tasks: Vec<_> = graph
        .tasks()
        .filter(|t| {
            !t.status.is_terminal()
                && !t.paused
                && !is_system_task(&t.id)
                && t.id != exclude_task_id
        })
        .collect();

    if active_tasks.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for t in &active_tasks {
        out.push_str(&format!("- {} ({})\n", t.id, t.title));
    }
    out
}

/// Validate that exec_mode is compatible with the configured executor.
/// Returns the validated mode string. If the mode is incompatible or invalid,
/// overrides to the safe default for that executor and logs a warning.
pub(crate) fn validate_exec_mode(mode: Option<&str>, executor_type: &str) -> String {
    use worksgood::config::ExecMode;

    let default = ExecMode::default_for_executor(executor_type);

    let mode_str = match mode {
        Some(m) => m,
        None => return default.to_string(),
    };

    // Parse the mode string
    let parsed: ExecMode = match mode_str.parse() {
        Ok(m) => m,
        Err(_) => {
            eprintln!(
                "[assignment] Warning: invalid exec_mode '{}'. Overriding to '{}' for executor '{}'.",
                mode_str, default, executor_type
            );
            return default.to_string();
        }
    };

    // Check compatibility with the executor
    if !parsed.is_valid_for_executor(executor_type) {
        eprintln!(
            "[assignment] Warning: exec_mode '{}' is incompatible with executor '{}'. Overriding to '{}'.",
            mode_str, executor_type, default
        );
        return default.to_string();
    }

    mode_str.to_string()
}

/// Extract a JSON object from potentially noisy LLM output.
fn extract_assignment_json(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // Strip markdown code fences
    if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        if serde_json::from_str::<serde_json::Value>(inner).is_ok() {
            return Some(inner.to_string());
        }
    }

    // Find first { to last }
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed.rfind('}')
        && start <= end
    {
        let candidate = &trimmed[start..=end];
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            return Some(candidate.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::agency::{Lineage, PerformanceRecord};
    use worksgood::graph::{Node, Status, Task};

    fn eval_ref(task_id: &str, score: f64) -> EvaluationRef {
        EvaluationRef {
            score,
            task_id: task_id.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            context_id: "role".to_string(),
        }
    }

    fn agent_with_evals(name: &str, evals: Vec<EvaluationRef>) -> Agent {
        let avg_score = agency::recalculate_avg_score(&evals);
        Agent {
            id: format!("agent-{name}"),
            role_id: "role-programmer".to_string(),
            tradeoff_id: "tradeoff-careful".to_string(),
            name: name.to_string(),
            performance: PerformanceRecord {
                task_count: evals.len() as u32,
                avg_score,
                evaluations: evals,
            },
            lineage: Lineage::default(),
            capabilities: Vec::new(),
            rate: None,
            capacity: None,
            trust_level: Default::default(),
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            attractor_weight: 1.0,
            deployment_history: Vec::new(),
            staleness_flags: Vec::new(),
        }
    }

    fn graph_with_history_tasks() -> WorkGraph {
        let mut graph = WorkGraph::new();
        let mut actual = Task {
            id: "impl-success".to_string(),
            title: "Implement feature".to_string(),
            status: Status::Done,
            ..Default::default()
        };
        actual.tags = vec!["implementation".to_string()];
        graph.add_node(Node::Task(actual));

        for id in [
            ".assign-impl-success",
            ".flip-impl-success",
            ".evaluate-impl-success",
        ] {
            graph.add_node(Node::Task(Task {
                id: id.to_string(),
                title: id.to_string(),
                status: Status::Done,
                ..Default::default()
            }));
        }

        let mut agency = Task {
            id: "agency-review-pass".to_string(),
            title: "Review agency output".to_string(),
            status: Status::Done,
            ..Default::default()
        };
        agency.tags = vec!["agency".to_string(), "review".to_string()];
        graph.add_node(Node::Task(agency));
        graph
    }

    #[test]
    fn test_extract_assignment_json_plain() {
        let input = r#"{"agent_hash": "abc123", "exec_mode": "full", "context_scope": "task", "reason": "best match"}"#;
        let result = extract_assignment_json(input).unwrap();
        let parsed: AssignmentVerdict = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.agent_hash, "abc123");
        assert_eq!(parsed.exec_mode.as_deref(), Some("full"));
    }

    #[test]
    fn test_extract_assignment_json_with_fences() {
        let input = "```json\n{\"agent_hash\": \"abc\", \"reason\": \"ok\"}\n```";
        let result = extract_assignment_json(input).unwrap();
        let parsed: AssignmentVerdict = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.agent_hash, "abc");
    }

    #[test]
    fn test_extract_assignment_json_garbage() {
        assert!(extract_assignment_json("no json here").is_none());
    }

    #[test]
    fn test_build_assignment_prompt_contains_task_info() {
        let task = Task {
            id: "test-task".to_string(),
            title: "Fix the bug".to_string(),
            description: Some("There is a bug in foo.rs".to_string()),
            status: Status::Open,
            skills: vec!["rust".to_string()],
            tags: vec!["implementation".to_string()],
            ..Default::default()
        };
        let prompt = build_assignment_prompt(
            &task,
            "## Mode\nPerformance",
            "- Agent1 (hash: abc)\n",
            AssignmentHistoryClass::ActualWork,
            None,
            "",
            "claude",
        );
        assert!(prompt.contains("test-task"));
        assert!(prompt.contains("Fix the bug"));
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("Agent1"));
        assert!(prompt.contains("Performance"));
    }

    #[test]
    fn test_render_agent_catalog_empty() {
        let result = render_agent_catalog(&[]);
        assert!(result.contains("No agents"));
    }

    #[test]
    fn test_render_agent_catalog_entries() {
        let entries = vec![AgentEntry {
            hash: "abc12345".to_string(),
            name: "TestAgent".to_string(),
            role_name: "Programmer".to_string(),
            role_skills: vec!["coding".to_string()],
            tradeoff_name: "Careful".to_string(),
            avg_score: Some(0.85),
            task_count: 10,
            history_class: AssignmentHistoryClass::ActualWork,
            capabilities: vec!["rust".to_string()],
            _staleness_flags: vec![],
        }];
        let result = render_agent_catalog(&entries);
        assert!(result.contains("TestAgent"));
        assert!(result.contains("abc12345"));
        assert!(result.contains("history=actual_work"));
        assert!(result.contains("0.85"));
        assert!(result.contains("rust"));
    }

    #[test]
    fn actual_work_assignment_stats_ignore_system_agency_history() {
        let graph = graph_with_history_tasks();
        let evaluator = agent_with_evals(
            "Default Evaluator",
            vec![
                eval_ref(".assign-impl-success", 1.0),
                eval_ref(".flip-impl-success", 1.0),
                eval_ref(".evaluate-impl-success", 1.0),
            ],
        );
        let programmer =
            agent_with_evals("Careful Programmer", vec![eval_ref("impl-success", 0.82)]);

        let evaluator_actual = scoped_performance_for_agent(
            &evaluator,
            Some(&graph),
            AssignmentHistoryClass::ActualWork,
        );
        let programmer_actual = scoped_performance_for_agent(
            &programmer,
            Some(&graph),
            AssignmentHistoryClass::ActualWork,
        );

        assert_eq!(evaluator_actual, ScopedPerformance::default());
        assert_eq!(programmer_actual.task_count, 1);
        assert_eq!(programmer_actual.avg_score, Some(0.82));
        assert!(
            programmer_actual.avg_score.unwrap_or(0.0) > evaluator_actual.avg_score.unwrap_or(0.0),
            "actual-work ranking should surface the programmer over system-only evaluator"
        );
    }

    #[test]
    fn system_assignment_stats_keep_system_agency_history() {
        let graph = graph_with_history_tasks();
        let evaluator = agent_with_evals(
            "Default Evaluator",
            vec![
                eval_ref(".assign-impl-success", 0.9),
                eval_ref(".evaluate-impl-success", 1.0),
            ],
        );
        let programmer =
            agent_with_evals("Careful Programmer", vec![eval_ref("impl-success", 0.82)]);

        let evaluator_system = scoped_performance_for_agent(
            &evaluator,
            Some(&graph),
            AssignmentHistoryClass::SystemAgency,
        );
        let programmer_system = scoped_performance_for_agent(
            &programmer,
            Some(&graph),
            AssignmentHistoryClass::SystemAgency,
        );

        assert_eq!(evaluator_system.task_count, 2);
        assert!((evaluator_system.avg_score.unwrap() - 0.95).abs() < f64::EPSILON);
        assert_eq!(programmer_system, ScopedPerformance::default());
    }

    #[test]
    fn real_work_prompt_catalog_excludes_system_task_scores() {
        let graph = graph_with_history_tasks();
        let evaluator = agent_with_evals(
            "Default Evaluator",
            vec![
                eval_ref(".assign-impl-success", 1.0),
                eval_ref(".flip-impl-success", 1.0),
                eval_ref(".evaluate-impl-success", 1.0),
            ],
        );
        let programmer =
            agent_with_evals("Careful Programmer", vec![eval_ref("impl-success", 0.82)]);
        let entries = build_agent_catalog(
            &[evaluator, programmer],
            std::path::Path::new("/missing/roles"),
            std::path::Path::new("/missing/tradeoffs"),
            Some(&graph),
            AssignmentHistoryClass::ActualWork,
        );
        let catalog = render_agent_catalog(&entries);

        assert!(catalog.contains("Default Evaluator"));
        assert!(catalog.contains("history=actual_work, score=none, tasks=0"));
        assert!(catalog.contains("Careful Programmer"));
        assert!(catalog.contains("history=actual_work, score=0.82, tasks=1"));

        let task = Task {
            id: "build-widget".to_string(),
            title: "Build widget".to_string(),
            status: Status::Open,
            tags: vec!["implementation".to_string()],
            ..Default::default()
        };
        let prompt = build_assignment_prompt(
            &task,
            "",
            &catalog,
            AssignmentHistoryClass::ActualWork,
            None,
            "",
            "claude",
        );
        assert!(prompt.contains("Using `actual_work` history"));
        assert!(prompt.contains("Normal work tasks ignore"));
        assert!(prompt.contains("history=actual_work, score=none, tasks=0"));
        assert!(!prompt.contains("history=actual_work, score=1.00"));
    }

    #[test]
    fn label_tagged_tasks_count_as_actual_work_history() {
        let graph = graph_with_history_tasks();
        let evaluator = agent_with_evals(
            "Default Evaluator",
            vec![eval_ref("agency-review-pass", 1.0)],
        );

        let actual = scoped_performance_for_agent(
            &evaluator,
            Some(&graph),
            AssignmentHistoryClass::ActualWork,
        );
        let system = scoped_performance_for_agent(
            &evaluator,
            Some(&graph),
            AssignmentHistoryClass::SystemAgency,
        );

        assert_eq!(actual.task_count, 1);
        assert_eq!(actual.avg_score, Some(1.0));
        assert_eq!(system, ScopedPerformance::default());
    }

    #[test]
    fn test_build_assignment_prompt_includes_placement_when_active_tasks() {
        let task = Task {
            id: "test-task".to_string(),
            title: "Fix the bug".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let active_ctx = "- other-task (Other Task)\n- third-task (Third Task)\n";
        let prompt = build_assignment_prompt(
            &task,
            "",
            "- Agent1 (hash: abc)\n",
            AssignmentHistoryClass::ActualWork,
            None,
            active_ctx,
            "claude",
        );
        assert!(prompt.contains("Active Tasks"));
        assert!(prompt.contains("other-task"));
        assert!(prompt.contains("Placement Instructions"));
        assert!(prompt.contains("\"placement\""));
    }

    #[test]
    fn test_build_assignment_prompt_no_placement_when_no_active_tasks() {
        let task = Task {
            id: "test-task".to_string(),
            title: "Fix the bug".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let prompt = build_assignment_prompt(
            &task,
            "",
            "- Agent1 (hash: abc)\n",
            AssignmentHistoryClass::ActualWork,
            None,
            "",
            "claude",
        );
        assert!(!prompt.contains("Active Tasks"));
        assert!(!prompt.contains("Placement Instructions"));
    }

    #[test]
    fn test_verdict_with_placement_deserialization() {
        let json = r#"{
            "agent_hash": "abc123",
            "exec_mode": "full",
            "context_scope": "task",
            "reason": "best match",
            "create_needed": false,
            "placement": {"after": ["dep-a", "dep-b"], "before": ["dep-c"]}
        }"#;
        let verdict: AssignmentVerdict = serde_json::from_str(json).unwrap();
        assert_eq!(verdict.agent_hash, "abc123");
        let placement = verdict.placement.unwrap();
        assert_eq!(placement.after, vec!["dep-a", "dep-b"]);
        assert_eq!(placement.before, vec!["dep-c"]);
    }

    #[test]
    fn test_verdict_without_placement_deserialization() {
        let json = r#"{
            "agent_hash": "abc123",
            "reason": "ok"
        }"#;
        let verdict: AssignmentVerdict = serde_json::from_str(json).unwrap();
        assert!(verdict.placement.is_none());
    }

    #[test]
    fn test_verdict_null_placement_deserialization() {
        let json = r#"{
            "agent_hash": "abc123",
            "reason": "ok",
            "placement": null
        }"#;
        let verdict: AssignmentVerdict = serde_json::from_str(json).unwrap();
        assert!(verdict.placement.is_none());
    }

    #[test]
    fn test_build_assignment_prompt_contains_executor_info_claude() {
        let task = Task {
            id: "test-task".to_string(),
            title: "Some task".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let prompt = build_assignment_prompt(
            &task,
            "",
            "- Agent1\n",
            AssignmentHistoryClass::ActualWork,
            None,
            "",
            "claude",
        );
        // Should mention the configured executor
        assert!(prompt.contains("Available executor:** claude"));
        // Should list valid modes for claude (bare, light, full — NOT shell)
        assert!(prompt.contains("bare, light, full"));
        assert!(!prompt.contains("Valid exec_modes:** shell"));
        // The exec_mode enum in the JSON response should only show valid options
        assert!(prompt.contains("bare|light|full"));
        assert!(!prompt.contains("shell|bare|light|full"));
        // Should warn against using invalid modes
        assert!(prompt.contains("Do NOT use exec_modes outside this list"));
    }

    #[test]
    fn test_build_assignment_prompt_contains_executor_info_shell() {
        let task = Task {
            id: "test-task".to_string(),
            title: "Shell task".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        let prompt = build_assignment_prompt(
            &task,
            "",
            "- Agent1\n",
            AssignmentHistoryClass::ActualWork,
            None,
            "",
            "shell",
        );
        // Should mention shell executor
        assert!(prompt.contains("Available executor:** shell"));
        // Should only list shell as valid mode
        assert!(prompt.contains("Valid exec_modes:** shell"));
        // Should NOT include bare/light/full in the valid modes list
        assert!(!prompt.contains("bare, light, full"));
    }

    #[test]
    fn test_exec_mode_validation() {
        // Valid mode for claude executor — no override
        assert_eq!(validate_exec_mode(Some("full"), "claude"), "full");
        assert_eq!(validate_exec_mode(Some("bare"), "claude"), "bare");
        assert_eq!(validate_exec_mode(Some("light"), "claude"), "light");

        // shell is incompatible with claude executor — override to full
        assert_eq!(validate_exec_mode(Some("shell"), "claude"), "full");

        // Valid mode for shell executor
        assert_eq!(validate_exec_mode(Some("shell"), "shell"), "shell");

        // bare/light/full are incompatible with shell executor — override to shell
        assert_eq!(validate_exec_mode(Some("full"), "shell"), "shell");
        assert_eq!(validate_exec_mode(Some("bare"), "shell"), "shell");
        assert_eq!(validate_exec_mode(Some("light"), "shell"), "shell");

        // Invalid mode string — override to default for executor
        assert_eq!(validate_exec_mode(Some("invalid"), "claude"), "full");
        assert_eq!(validate_exec_mode(Some("invalid"), "shell"), "shell");

        // None — return default for executor
        assert_eq!(validate_exec_mode(None, "claude"), "full");
        assert_eq!(validate_exec_mode(None, "shell"), "shell");

        // native executor — same valid modes as claude
        assert_eq!(validate_exec_mode(Some("full"), "native"), "full");
        assert_eq!(validate_exec_mode(Some("shell"), "native"), "full");
    }
}
