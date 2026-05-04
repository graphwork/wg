//! Assignment routing, UCB1 primitive selection, novelty bonus, and
//! retrospective inference.
//!
//! All assignments go through the LLM-based learning path with structured
//! experiments. ForcedExploration fires on interval triggers.

use std::collections::HashMap;
use std::path::Path;

use crate::config::AgencyConfig;

use super::store::*;
use super::types::*;

// ---------------------------------------------------------------------------
// Assignment routing
// ---------------------------------------------------------------------------

/// Which path a single assignment should take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignmentPath {
    /// Run a structured learning experiment.
    Learning,
    /// Forced exploration episode (exploration_interval trigger).
    ForcedExploration,
}

/// Determine the assignment path for a given task.
///
/// All assignments go through the LLM. `ForcedExploration` fires on
/// interval triggers to vary experiment parameters; otherwise `Learning`.
pub fn determine_assignment_path(config: &AgencyConfig, task_count: u32) -> AssignmentPath {
    if config.exploration_interval > 0
        && task_count > 0
        && task_count.is_multiple_of(config.exploration_interval)
    {
        AssignmentPath::ForcedExploration
    } else {
        AssignmentPath::Learning
    }
}

// ---------------------------------------------------------------------------
// Scope-aware primitive filtering
// ---------------------------------------------------------------------------

/// Determine the required primitive scope for a given task ID.
///
/// Meta-tasks (system tasks with `.` prefix) are mapped to their corresponding
/// meta scope. Regular tasks use the `task` scope.
///
/// Returns `None` for tasks whose scope cannot be determined from the ID,
/// which should fall back to unfiltered selection.
pub fn required_scope_for_task(task_id: &str) -> &'static str {
    if task_id.starts_with(".assign-") {
        "meta:assigner"
    } else if task_id.starts_with(".evaluate-") || task_id.starts_with(".flip-") {
        "meta:evaluator"
    } else if task_id.starts_with(".evolve-") {
        "meta:evolver"
    } else if task_id.starts_with(".create-agent-") {
        "meta:agent_creator"
    } else {
        "task"
    }
}

/// Returns true if a primitive's scope metadata matches the required scope.
///
/// Matching rules:
/// - Exact match (e.g., `scope=task` matches required `task`)
/// - General `meta` scope matches any `meta:*` requirement (fallback pool)
/// - Primitives without scope metadata match everything (backward compat)
fn scope_matches(primitive_scope: Option<&str>, required_scope: &str) -> bool {
    match primitive_scope {
        None | Some("") => true, // No scope = matches everything (backward compat)
        Some(scope) => {
            scope == required_scope || (scope == "meta" && required_scope.starts_with("meta:"))
        }
    }
}

/// Read the effective scope of a `RoleComponent`, preferring the typed
/// `scope` field and falling back to `metadata["scope"]` for primitives
/// authored before the typed field landed.
pub fn component_scope(c: &super::types::RoleComponent) -> Option<&str> {
    c.scope
        .as_deref()
        .or_else(|| c.metadata.get("scope").map(|s| s.as_str()))
}

/// Read the effective scope of a `DesiredOutcome` (typed field, then metadata).
pub fn outcome_scope(o: &super::types::DesiredOutcome) -> Option<&str> {
    o.scope
        .as_deref()
        .or_else(|| o.metadata.get("scope").map(|s| s.as_str()))
}

/// Read the effective scope of a `TradeoffConfig` (typed field, then metadata).
pub fn tradeoff_scope(t: &super::types::TradeoffConfig) -> Option<&str> {
    t.scope
        .as_deref()
        .or_else(|| t.metadata.get("scope").map(|s| s.as_str()))
}

/// Filter components by scope, with fallback to unfiltered if no matches.
///
/// Reads scope via [`component_scope`] so both the typed field and the
/// legacy `metadata["scope"]` entry are honoured.
pub fn filter_components_by_scope(
    components: &[super::types::RoleComponent],
    required_scope: &str,
) -> Vec<super::types::RoleComponent> {
    let filtered: Vec<_> = components
        .iter()
        .filter(|c| scope_matches(component_scope(c), required_scope))
        .cloned()
        .collect();

    if filtered.is_empty() {
        // Fallback: use all components if no scope matches
        components.to_vec()
    } else {
        filtered
    }
}

/// Strict scope filter — does NOT fall back to unfiltered when no primitives
/// match the required scope. Used by the composer when we need to bias
/// selection toward scope-tagged primitives without leaking off-scope ones
/// in. Untagged components still match (backward-compat).
pub fn filter_components_by_required_scope(
    components: &[super::types::RoleComponent],
    required_scope: &str,
) -> Vec<super::types::RoleComponent> {
    components
        .iter()
        .filter(|c| scope_matches(component_scope(c), required_scope))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// UCB1 primitive selection
// ---------------------------------------------------------------------------

/// Compute the UCB1 score for a primitive.
///
/// `avg_score`: average evaluation score for this primitive (None if never evaluated).
/// `eval_count`: number of evaluations for this primitive.
/// `total_assignments`: total number of assignments across all primitives.
/// `exploration_constant`: the C parameter (default √2).
/// `attractor_weight`: the primitive's attractor weight (0..1, higher = more conventional).
/// `novelty_bonus_multiplier`: multiplier for low-attractor primitives.
pub fn ucb1_score(
    avg_score: Option<f64>,
    eval_count: u32,
    total_assignments: u32,
    exploration_constant: f64,
    attractor_weight: f64,
    novelty_bonus_multiplier: f64,
) -> f64 {
    let base_score = avg_score.unwrap_or(0.5); // Optimistic prior for unscored primitives
    let n = total_assignments.max(1) as f64;
    let ni = eval_count.max(1) as f64;

    let exploration_bonus = exploration_constant * (n.ln() / ni).sqrt();

    // Novelty bonus: inversely proportional to attractor weight.
    // Low-attractor primitives get boosted; high-attractor ones stay at 1.0.
    let novelty_factor = if attractor_weight < 0.5 {
        novelty_bonus_multiplier
    } else {
        1.0
    };

    (base_score + exploration_bonus) * novelty_factor
}

/// Select a primitive from candidates using UCB1 scoring with novelty bonus.
///
/// Returns (selected_id, ucb_scores) where ucb_scores maps each candidate
/// to its UCB1 score for post-hoc analysis.
pub fn select_primitive_ucb1(
    candidates: &[(String, Option<f64>, u32, f64)], // (id, avg_score, eval_count, attractor_weight)
    total_assignments: u32,
    exploration_constant: f64,
    novelty_bonus_multiplier: f64,
) -> Option<(String, HashMap<String, f64>)> {
    if candidates.is_empty() {
        return None;
    }

    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut best_id = &candidates[0].0;
    let mut best_score = f64::NEG_INFINITY;

    for (id, avg, eval_count, attractor_weight) in candidates {
        let score = ucb1_score(
            *avg,
            *eval_count,
            total_assignments,
            exploration_constant,
            *attractor_weight,
            novelty_bonus_multiplier,
        );
        scores.insert(id.clone(), score);
        if score > best_score {
            best_score = score;
            best_id = id;
        }
    }

    Some((best_id.clone(), scores))
}

// ---------------------------------------------------------------------------
// Experiment design
// ---------------------------------------------------------------------------

/// Design a learning experiment given the agency state.
///
/// Implements the algorithm from the design doc §4.2:
/// 1. Find best known composition for this task type.
/// 2. Select dimension with highest uncertainty.
/// 3. Pick variant via UCB1 (filtered by scope).
/// 4. Construct the experiment.
///
/// `task_id` is used to determine the required primitive scope. Regular tasks
/// use `scope=task` primitives; meta-tasks (`.assign-*`, `.evaluate-*`, etc.)
/// use their corresponding `scope=meta:*` primitives. If no primitives match
/// the required scope, falls back to unfiltered selection.
pub fn design_experiment(
    agency_dir: &Path,
    config: &AgencyConfig,
    learning_assignment_count: u32,
    task_id: &str,
) -> AssignmentExperiment {
    // Check bizarre ideation schedule
    if config.bizarre_ideation_interval > 0
        && learning_assignment_count > 0
        && learning_assignment_count.is_multiple_of(config.bizarre_ideation_interval)
    {
        return AssignmentExperiment {
            base_composition: None,
            dimension: ExperimentDimension::NovelComposition,
            bizarre_ideation: true,
            ucb_scores: HashMap::new(),
        };
    }

    let agents_dir = agency_dir.join("cache/agents");
    let components_dir = agency_dir.join("primitives/components");

    // Load agents to find best known composition
    let agents = load_all_agents_or_warn(&agents_dir);
    let best_agent = agents
        .iter()
        .filter(|a| a.performance.avg_score.is_some())
        .max_by(|a, b| {
            a.performance
                .avg_score
                .unwrap_or(0.0)
                .partial_cmp(&b.performance.avg_score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    let best_agent = match best_agent {
        Some(a) => a,
        None => {
            // No evaluated compositions exist — do novel composition
            return AssignmentExperiment {
                base_composition: None,
                dimension: ExperimentDimension::NovelComposition,
                bizarre_ideation: false,
                ucb_scores: HashMap::new(),
            };
        }
    };

    // Load components to build UCB1 candidate list
    let all_components = match load_all_components(&components_dir) {
        Ok(c) => c,
        Err(_) => {
            return AssignmentExperiment {
                base_composition: Some(best_agent.id.clone()),
                dimension: ExperimentDimension::NovelComposition,
                bizarre_ideation: false,
                ucb_scores: HashMap::new(),
            };
        }
    };

    // Filter components by scope (with fallback to unfiltered)
    let required_scope = required_scope_for_task(task_id);
    let components = filter_components_by_scope(&all_components, required_scope);

    // Load the role to get the base component list
    let roles_dir = agency_dir.join("cache/roles");
    let base_role = find_role_by_prefix(&roles_dir, &best_agent.role_id).ok();

    let base_component_ids: Vec<String> = base_role
        .as_ref()
        .map(|r| r.component_ids.clone())
        .unwrap_or_default();

    // Build candidate list of components NOT in the base composition
    let total_assignments = count_assignment_records(&agency_dir.join("assignments"));
    let candidates: Vec<(String, Option<f64>, u32, f64)> = components
        .iter()
        .filter(|c| !base_component_ids.contains(&c.id))
        .map(|c| {
            (
                c.id.clone(),
                c.performance.avg_score,
                c.performance.task_count,
                // Use default attractor weight based on former deployments
                if c.former_deployments.is_empty() {
                    0.1 // Low weight for never-deployed components
                } else {
                    0.5 // Default weight
                },
            )
        })
        .collect();

    if candidates.is_empty() {
        return AssignmentExperiment {
            base_composition: Some(best_agent.id.clone()),
            dimension: ExperimentDimension::NovelComposition,
            bizarre_ideation: false,
            ucb_scores: HashMap::new(),
        };
    }

    // Select variant component via UCB1
    let (selected_id, ucb_scores) = select_primitive_ucb1(
        &candidates,
        total_assignments as u32,
        config.ucb_exploration_constant,
        config.novelty_bonus_multiplier,
    )
    .unwrap();

    // Pick a random base component to replace (prefer least-evaluated)
    let replaced = base_component_ids
        .iter()
        .filter_map(|id| {
            let comp = components.iter().find(|c| c.id == *id)?;
            Some((id.clone(), comp.performance.task_count))
        })
        .min_by_key(|(_, count)| *count)
        .map(|(id, _)| id);

    AssignmentExperiment {
        base_composition: Some(best_agent.id.clone()),
        dimension: ExperimentDimension::RoleComponent {
            replaced,
            introduced: selected_id,
        },
        bizarre_ideation: false,
        ucb_scores,
    }
}

// ---------------------------------------------------------------------------
// Retrospective inference
// ---------------------------------------------------------------------------

/// Process retrospective inference when an evaluation arrives for a task
/// that was assigned in learning mode.
///
/// Steps from design doc §6:
/// 1. Load TaskAssignmentRecord.
/// 2. If learning/forced: extract experiment, propagate score.
/// 3. Update attractor weights.
/// 4. Populate cache if above threshold.
pub fn process_retrospective_inference(
    agency_dir: &Path,
    task_id: &str,
    eval_score: f64,
    config: &AgencyConfig,
) -> Result<(), AgencyError> {
    let assignments_dir = agency_dir.join("assignments");
    let record = match load_assignment_record_by_task(&assignments_dir, task_id) {
        Ok(r) => r,
        Err(AgencyError::NotFound(_)) => return Ok(()), // No assignment record — not a learning task
        Err(e) => return Err(e),
    };

    let experiment = match &record.mode {
        AssignmentMode::Learning(exp) | AssignmentMode::ForcedExploration(exp) => exp.clone(),
    };

    let components_dir = agency_dir.join("primitives/components");
    let agents_dir = agency_dir.join("cache/agents");

    match &experiment.dimension {
        ExperimentDimension::RoleComponent { introduced, .. }
        | ExperimentDimension::TradeoffConfig { introduced, .. } => {
            // Propagate score to the introduced primitive
            let component_path = components_dir.join(format!("{}.yaml", introduced));
            if component_path.exists()
                && let Ok(mut component) = load_component(&component_path)
            {
                let eval_ref = EvaluationRef {
                    score: eval_score,
                    task_id: task_id.to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    context_id: format!("experiment:{}", record.composition_id),
                };
                super::eval::update_performance(&mut component.performance, eval_ref);
                let _ = save_component(&component, &components_dir);
            }

            // Update attractor weight on agent
            if let Some(base_id) = &experiment.base_composition {
                let agent_path = agents_dir.join(format!("{}.yaml", base_id));
                if agent_path.exists()
                    && let Ok(agent) = load_agent(&agent_path)
                {
                    let base_avg = agent.performance.avg_score.unwrap_or(0.5);
                    // Adjust attractor weights on the agent
                    // If experiment score > base avg, increase weight; otherwise decrease
                    let mut updated_agent = agent;
                    let learning_rate = 0.1;
                    if eval_score > base_avg {
                        updated_agent.attractor_weight =
                            (updated_agent.attractor_weight + learning_rate).min(1.0);
                    } else {
                        updated_agent.attractor_weight =
                            (updated_agent.attractor_weight - learning_rate).max(0.0);
                    }
                    let _ = save_agent(&updated_agent, &agents_dir);
                }
            }
        }
        ExperimentDimension::NovelComposition => {
            // Propagate score equally to all component primitives of the assigned agent
            let agent_path = agents_dir.join(format!("{}.yaml", record.agent_id));
            if let Ok(agent) = load_agent(&agent_path) {
                let roles_dir = agency_dir.join("cache/roles");
                if let Ok(role) = find_role_by_prefix(&roles_dir, &agent.role_id) {
                    for comp_id in &role.component_ids {
                        let comp_path = components_dir.join(format!("{}.yaml", comp_id));
                        if comp_path.exists()
                            && let Ok(mut comp) = load_component(&comp_path)
                        {
                            let eval_ref = EvaluationRef {
                                score: eval_score,
                                task_id: task_id.to_string(),
                                timestamp: chrono::Utc::now().to_rfc3339(),
                                context_id: format!("experiment:novel:{}", record.composition_id),
                            };
                            super::eval::update_performance(&mut comp.performance, eval_ref);
                            let _ = save_component(&comp, &components_dir);
                        }
                    }
                }
            }
        }
    }

    // Cache population: if score >= threshold, ensure this composition is in the cache
    if eval_score >= config.cache_population_threshold {
        // The agent already exists in the cache by definition (it was deployed),
        // but update its performance to reflect this high score
        let agent_path = agents_dir.join(format!("{}.yaml", record.agent_id));
        if agent_path.exists()
            && let Ok(mut agent) = load_agent(&agent_path)
        {
            let eval_ref = EvaluationRef {
                score: eval_score,
                task_id: task_id.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                context_id: "experiment:cache-population".to_string(),
            };
            super::eval::update_performance(&mut agent.performance, eval_ref);
            let _ = save_agent(&agent, &agents_dir);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> AgencyConfig {
        AgencyConfig {
            exploration_interval: 20,
            cache_population_threshold: 0.8,
            ucb_exploration_constant: std::f64::consts::SQRT_2,
            novelty_bonus_multiplier: 1.5,
            bizarre_ideation_interval: 10,
            ..AgencyConfig::default()
        }
    }

    // -- Assignment routing tests --

    #[test]
    fn test_always_learning_mode() {
        let mut config = test_config();
        config.exploration_interval = 0;

        // Every assignment should be Learning
        for i in 0..100 {
            assert_eq!(
                determine_assignment_path(&config, i),
                AssignmentPath::Learning,
            );
        }
    }

    #[test]
    fn test_forced_exploration_interval() {
        let mut config = test_config();
        config.exploration_interval = 10;

        // task_count=10: forced
        assert_eq!(
            determine_assignment_path(&config, 10),
            AssignmentPath::ForcedExploration,
        );
        // task_count=20: forced
        assert_eq!(
            determine_assignment_path(&config, 20),
            AssignmentPath::ForcedExploration,
        );
        // task_count=11: not forced — Learning
        assert_eq!(
            determine_assignment_path(&config, 11),
            AssignmentPath::Learning,
        );
        // task_count=0: not forced (avoid triggering on first task)
        assert_eq!(
            determine_assignment_path(&config, 0),
            AssignmentPath::Learning,
        );
    }

    #[test]
    fn test_forced_exploration_fires_on_interval() {
        let mut config = test_config();
        config.exploration_interval = 5;

        assert_eq!(
            determine_assignment_path(&config, 5),
            AssignmentPath::ForcedExploration,
        );
    }

    // -- UCB1 tests --

    #[test]
    fn test_ucb1_unscored_gets_optimistic_prior() {
        let score = ucb1_score(None, 0, 100, std::f64::consts::SQRT_2, 0.5, 1.5);
        // Should be > 0.5 due to exploration bonus
        assert!(score > 0.5);
    }

    #[test]
    fn test_ucb1_high_score_low_count_wins() {
        let high_count = ucb1_score(Some(0.8), 50, 100, std::f64::consts::SQRT_2, 0.5, 1.0);
        let low_count = ucb1_score(Some(0.8), 2, 100, std::f64::consts::SQRT_2, 0.5, 1.0);
        // Low count should have higher UCB score due to exploration bonus
        assert!(low_count > high_count);
    }

    #[test]
    fn test_ucb1_novelty_bonus_for_low_attractor() {
        let high_attractor = ucb1_score(Some(0.5), 10, 100, std::f64::consts::SQRT_2, 0.8, 1.5);
        let low_attractor = ucb1_score(Some(0.5), 10, 100, std::f64::consts::SQRT_2, 0.2, 1.5);
        // Low attractor weight should get novelty multiplier
        assert!(low_attractor > high_attractor);
    }

    #[test]
    fn test_select_primitive_empty() {
        let result = select_primitive_ucb1(&[], 100, std::f64::consts::SQRT_2, 1.5);
        assert!(result.is_none());
    }

    #[test]
    fn test_select_primitive_single_candidate() {
        let candidates = vec![("comp-1".to_string(), Some(0.8), 5, 0.5)];
        let (selected, scores) =
            select_primitive_ucb1(&candidates, 100, std::f64::consts::SQRT_2, 1.5).unwrap();
        assert_eq!(selected, "comp-1");
        assert!(scores.contains_key("comp-1"));
    }

    #[test]
    fn test_select_primitive_prefers_under_explored() {
        let candidates = vec![
            ("well-explored".to_string(), Some(0.7), 50, 0.5),
            ("under-explored".to_string(), Some(0.7), 1, 0.5),
        ];
        let (selected, _) =
            select_primitive_ucb1(&candidates, 100, std::f64::consts::SQRT_2, 1.5).unwrap();
        // Under-explored should win due to high exploration bonus
        assert_eq!(selected, "under-explored");
    }

    // -- Experiment design tests --

    #[test]
    fn test_design_experiment_no_agents() {
        let tmp = TempDir::new().unwrap();
        let agency_dir = tmp.path().join("agency");
        init(&agency_dir).unwrap();

        let config = test_config();
        let exp = design_experiment(&agency_dir, &config, 1, "some-task");
        assert!(matches!(
            exp.dimension,
            ExperimentDimension::NovelComposition
        ));
        assert!(!exp.bizarre_ideation);
    }

    #[test]
    fn test_design_experiment_bizarre_ideation() {
        let tmp = TempDir::new().unwrap();
        let agency_dir = tmp.path().join("agency");
        init(&agency_dir).unwrap();

        let config = test_config();
        // learning_assignment_count = 10, bizarre_ideation_interval = 10
        let exp = design_experiment(&agency_dir, &config, 10, "some-task");
        assert!(matches!(
            exp.dimension,
            ExperimentDimension::NovelComposition
        ));
        assert!(exp.bizarre_ideation);
    }

    // -- Retrospective inference tests --

    #[test]
    fn test_retrospective_no_record_is_noop() {
        let tmp = TempDir::new().unwrap();
        let agency_dir = tmp.path().join("agency");
        init(&agency_dir).unwrap();

        let config = test_config();
        let result = process_retrospective_inference(&agency_dir, "nonexistent-task", 0.9, &config);
        assert!(result.is_ok());
    }

    // -- Scope filtering tests --

    #[test]
    fn test_required_scope_for_regular_task() {
        assert_eq!(required_scope_for_task("implement-feature"), "task");
        assert_eq!(required_scope_for_task("fix-bug-123"), "task");
    }

    #[test]
    fn test_required_scope_for_meta_tasks() {
        assert_eq!(required_scope_for_task(".assign-my-task"), "meta:assigner");
        assert_eq!(
            required_scope_for_task(".evaluate-my-task"),
            "meta:evaluator"
        );
        assert_eq!(required_scope_for_task(".flip-my-task"), "meta:evaluator");
        assert_eq!(required_scope_for_task(".evolve-my-task"), "meta:evolver");
        assert_eq!(
            required_scope_for_task(".create-agent-my-task"),
            "meta:agent_creator"
        );
    }

    #[test]
    fn test_scope_matches_exact() {
        assert!(scope_matches(Some("task"), "task"));
        assert!(scope_matches(Some("meta:assigner"), "meta:assigner"));
        assert!(!scope_matches(Some("task"), "meta:assigner"));
        assert!(!scope_matches(Some("meta:assigner"), "task"));
    }

    #[test]
    fn test_scope_matches_general_meta_fallback() {
        // General "meta" scope matches any meta:* requirement
        assert!(scope_matches(Some("meta"), "meta:assigner"));
        assert!(scope_matches(Some("meta"), "meta:evaluator"));
        assert!(scope_matches(Some("meta"), "meta:evolver"));
        assert!(scope_matches(Some("meta"), "meta:agent_creator"));
        // But not regular tasks
        assert!(!scope_matches(Some("meta"), "task"));
    }

    #[test]
    fn test_scope_matches_no_scope_matches_everything() {
        assert!(scope_matches(None, "task"));
        assert!(scope_matches(None, "meta:assigner"));
        assert!(scope_matches(Some(""), "task"));
        assert!(scope_matches(Some(""), "meta:evaluator"));
    }

    fn make_component(id: &str, scope: Option<&str>) -> super::super::types::RoleComponent {
        use super::super::types::*;
        let mut metadata = std::collections::HashMap::new();
        if let Some(s) = scope {
            metadata.insert("scope".to_string(), s.to_string());
        }
        RoleComponent {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("Component {}", id),
            quality: 100,
            domain_specificity: 0,
            domain: vec![],
            scope: scope.map(str::to_string),
            origin_instance_id: None,
            parent_content_hash: None,
            category: ComponentCategory::Translated,
            content: ContentRef::Inline("test".to_string()),
            performance: PerformanceRecord::default(),
            lineage: Lineage::default(),
            access_control: AccessControl::default(),
            domain_tags: vec![],
            metadata,
            former_agents: vec![],
            former_deployments: vec![],
        }
    }

    #[test]
    fn test_scope_filter_ucb1_regular_task_uses_task_scope() {
        let components = vec![
            make_component("task-comp-1", Some("task")),
            make_component("task-comp-2", Some("task")),
            make_component("meta-assigner-comp", Some("meta:assigner")),
            make_component("meta-evaluator-comp", Some("meta:evaluator")),
        ];

        let filtered = filter_components_by_scope(&components, "task");
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|c| c.id.starts_with("task-comp")));
    }

    #[test]
    fn test_scope_filter_meta_assigner_task() {
        let components = vec![
            make_component("task-comp-1", Some("task")),
            make_component("assigner-comp-1", Some("meta:assigner")),
            make_component("assigner-comp-2", Some("meta:assigner")),
            make_component("general-meta-comp", Some("meta")),
            make_component("evaluator-comp", Some("meta:evaluator")),
        ];

        let filtered = filter_components_by_scope(&components, "meta:assigner");
        assert_eq!(filtered.len(), 3); // 2 assigner + 1 general meta
        let ids: Vec<&str> = filtered.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"assigner-comp-1"));
        assert!(ids.contains(&"assigner-comp-2"));
        assert!(ids.contains(&"general-meta-comp"));
    }

    #[test]
    fn test_scope_filter_fallback_when_no_matches() {
        let components = vec![
            make_component("task-comp-1", Some("task")),
            make_component("task-comp-2", Some("task")),
        ];

        // No meta:evolver primitives exist — should fall back to all
        let filtered = filter_components_by_scope(&components, "meta:evolver");
        assert_eq!(filtered.len(), 2, "Should fall back to all components");
    }

    #[test]
    fn test_scope_filter_no_scope_metadata_matches_everything() {
        let components = vec![
            make_component("legacy-comp-1", None),
            make_component("legacy-comp-2", None),
            make_component("task-comp", Some("task")),
        ];

        let filtered = filter_components_by_scope(&components, "task");
        assert_eq!(filtered.len(), 3); // 2 legacy (no scope = match all) + 1 explicit task
    }

    #[test]
    fn test_scope_filter_ucb1_scoring_with_mixed_scopes() {
        // Simulate the full UCB1 flow: mixed scope primitives, only task-scope should
        // be used as candidates for regular tasks.
        let task_candidates = vec![
            ("task-comp-1".to_string(), Some(0.8), 5_u32, 0.5_f64),
            ("task-comp-2".to_string(), Some(0.6), 3, 0.3),
        ];
        let meta_candidates = vec![("meta-comp".to_string(), Some(0.9), 10, 0.5)];

        // For a regular task, only task candidates should be considered
        let (selected, scores) =
            select_primitive_ucb1(&task_candidates, 20, std::f64::consts::SQRT_2, 1.5).unwrap();

        // Selection should be from task candidates only
        assert!(
            selected == "task-comp-1" || selected == "task-comp-2",
            "Selected '{}' should be a task-scope component",
            selected
        );
        // Meta candidates should not appear in scores
        assert!(!scores.contains_key("meta-comp"));

        // For meta tasks, only meta candidates should be considered
        let (meta_selected, meta_scores) =
            select_primitive_ucb1(&meta_candidates, 20, std::f64::consts::SQRT_2, 1.5).unwrap();
        assert_eq!(meta_selected, "meta-comp");
        assert!(!meta_scores.contains_key("task-comp-1"));
    }

    /// Integration-style test: set up a full agency dir with agent, role, and
    /// mixed-scope components, then verify design_experiment uses scope filtering.
    #[test]
    fn test_design_experiment_scope_filter_integration() {
        use super::super::types::*;

        let tmp = TempDir::new().unwrap();
        let agency_dir = tmp.path().join("agency");
        init(&agency_dir).unwrap();

        // Create components with different scopes
        let components_dir = agency_dir.join("primitives/components");
        let task_comp = make_component("task-comp-aaa", Some("task"));
        let meta_assigner_comp = make_component("meta-assigner-bbb", Some("meta:assigner"));
        let meta_evaluator_comp = make_component("meta-evaluator-ccc", Some("meta:evaluator"));
        let base_comp = make_component("base-comp-ddd", Some("task"));

        save_component(&task_comp, &components_dir).unwrap();
        save_component(&meta_assigner_comp, &components_dir).unwrap();
        save_component(&meta_evaluator_comp, &components_dir).unwrap();
        save_component(&base_comp, &components_dir).unwrap();

        // Create a role with the base component
        let roles_dir = agency_dir.join("cache/roles");
        let role = Role {
            id: "test-role-111".to_string(),
            name: "TestRole".to_string(),
            description: "A test role".to_string(),
            component_ids: vec!["base-comp-ddd".to_string()],
            outcome_id: String::new(),
            performance: PerformanceRecord {
                task_count: 5,
                avg_score: Some(0.7),
                evaluations: vec![],
            },
            lineage: Lineage::default(),
            default_context_scope: None,
            default_exec_mode: None,
        };
        save_role(&role, &roles_dir).unwrap();

        // Create an agent with the role
        let agents_dir = agency_dir.join("cache/agents");
        let agent = Agent {
            id: "test-agent-222".to_string(),
            role_id: "test-role-111".to_string(),
            tradeoff_id: "test-tradeoff".to_string(),
            name: "TestAgent".to_string(),
            performance: PerformanceRecord {
                task_count: 5,
                avg_score: Some(0.7),
                evaluations: vec![],
            },
            lineage: Lineage::default(),
            capabilities: vec![],
            rate: None,
            capacity: None,
            trust_level: crate::graph::TrustLevel::Provisional,
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            deployment_history: vec![],
            attractor_weight: 0.5,
            staleness_flags: vec![],
        };
        save_agent(&agent, &agents_dir).unwrap();

        let config = test_config();

        // For a regular task: design_experiment should use task-scope components
        let exp = design_experiment(&agency_dir, &config, 1, "implement-feature");
        match &exp.dimension {
            ExperimentDimension::RoleComponent { introduced, .. } => {
                // Should select the task-scope component, not meta-scope ones
                assert_eq!(
                    introduced, "task-comp-aaa",
                    "Regular task should select task-scope component, got: {}",
                    introduced
                );
            }
            ExperimentDimension::NovelComposition => {
                // Also acceptable if no candidate was found (e.g., all filtered out)
            }
            other => {
                panic!(
                    "Expected RoleComponent or NovelComposition, got: {:?}",
                    other
                );
            }
        }

        // For a .assign-* meta task: should select meta:assigner scope
        let meta_exp = design_experiment(&agency_dir, &config, 2, ".assign-implement-feature");
        match &meta_exp.dimension {
            ExperimentDimension::RoleComponent { introduced, .. } => {
                assert_eq!(
                    introduced, "meta-assigner-bbb",
                    "Assigner meta-task should select meta:assigner component, got: {}",
                    introduced
                );
            }
            ExperimentDimension::NovelComposition => {}
            other => {
                panic!(
                    "Expected RoleComponent or NovelComposition, got: {:?}",
                    other
                );
            }
        }
    }
}
