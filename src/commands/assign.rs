use anyhow::{Context, Result};
use std::path::Path;
use worksgood::agency;
use worksgood::agency::composition_rules::{
    CompositionRulesOverlay, default_overlay_path, load_composition_rules,
};
use worksgood::config::Config;
use worksgood::parser::{load_graph, modify_graph};

use super::graph_path;

/// Load the composition-rules overlay from `~/.agency/composition-rules.csv`
/// (re-reading on every assignment so edits take effect without daemon
/// restart). Empty overlay when the file is absent or malformed.
fn load_overlay() -> CompositionRulesOverlay {
    let Some(path) = default_overlay_path() else {
        return CompositionRulesOverlay::default();
    };
    match load_composition_rules(&path) {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "Warning: failed to read composition rules from {}: {}",
                path.display(),
                e
            );
            CompositionRulesOverlay::default()
        }
    }
}

/// Bucket an `agency::Agent`'s role into a composition-rules `agent_type`
/// using the role's well-known name (Assigner / Evaluator / Evolver /
/// Agent Creator) or the role's typed scope on its components.
fn agent_type_for_role(role_name: &str) -> &'static str {
    match role_name {
        "Assigner" => "assigner",
        "Evaluator" => "evaluator",
        "Evolver" => "evolver",
        "Agent Creator" | "AgentCreator" => "agent_creator",
        _ => "task",
    }
}

/// Apply composition-rules caps to filter an agent pool down to those whose
/// role component count is within the cap for the agent's `agent_type`.
///
/// If no rule applies (or the cap is `None`), every agent passes through.
/// If applying the cap would empty the pool, the unfiltered pool is
/// returned with a warning printed — the caller still needs *some* agent
/// to assign, and silently failing assignment is worse than violating a
/// (possibly stale) cap.
fn apply_caps(
    overlay: &CompositionRulesOverlay,
    agents: &[agency::Agent],
    roles_dir: &Path,
) -> Vec<agency::Agent> {
    let mut filtered: Vec<agency::Agent> = Vec::with_capacity(agents.len());
    let mut dropped = Vec::new();

    for agent in agents {
        let role = match agency::find_role_by_prefix(roles_dir, &agent.role_id) {
            Ok(r) => r,
            Err(_) => {
                // Role missing — keep the agent; cap doesn't apply.
                filtered.push(agent.clone());
                continue;
            }
        };
        let agent_type = agent_type_for_role(&role.name);
        let Some(rule) = overlay.rule_for(agent_type) else {
            filtered.push(agent.clone());
            continue;
        };
        if rule.role_components_within_cap(role.component_ids.len()) {
            filtered.push(agent.clone());
        } else {
            dropped.push(format!(
                "{} (role '{}' has {} components > cap {})",
                agency::short_hash(&agent.id),
                role.name,
                role.component_ids.len(),
                rule.max_role_components.unwrap_or(0),
            ));
        }
    }

    if filtered.is_empty() && !agents.is_empty() {
        eprintln!(
            "Warning: composition-rules cap would block every candidate agent ({} dropped: {}). \
             Falling back to unfiltered pool.",
            dropped.len(),
            dropped.join(", ")
        );
        return agents.to_vec();
    }
    if !dropped.is_empty() {
        eprintln!(
            "[assign] composition-rules cap dropped {} agent(s): {}",
            dropped.len(),
            dropped.join(", ")
        );
    }
    filtered
}

/// Record an evaluation against the assigner special agent's performance.
///
/// When auto_evaluate is enabled and an assigner_agent is configured, this
/// creates an evaluation entry for the assignment itself (source = "system"),
/// recording against the assigner agent entity so it accumulates performance
/// history. The actual quality signal comes later from the agent's task
/// evaluation, but recording the event here lets the system attribute
/// downstream scores back to the assignment decision via the 6-step cascade.
fn record_assigner_evaluation(
    agency_dir: &Path,
    task_id: &str,
    _assigned_agent: &agency::Agent,
    config: &Config,
) {
    if !config.agency.auto_evaluate {
        return;
    }

    // Resolve the assigner special agent from config
    let assigner_agent = match config.agency.assigner_agent {
        Some(ref hash) => {
            let agents_dir = agency_dir.join("cache/agents");
            match agency::find_agent_by_prefix(&agents_dir, hash) {
                Ok(agent) => agent,
                Err(_) => return, // No assigner agent found — skip recording
            }
        }
        None => return, // No assigner agent configured
    };

    let assign_task_id = format!(".assign-{}", task_id);
    let eval = agency::Evaluation {
        id: format!("eval-assign-{}", task_id),
        task_id: assign_task_id,
        agent_id: assigner_agent.id.clone(),
        role_id: assigner_agent.role_id.clone(),
        tradeoff_id: assigner_agent.tradeoff_id.clone(),
        // Placeholder score — actual quality will be determined by downstream
        // evaluation. The assigner's "score" is updated
        // retrospectively when the assigned agent's task completes.
        score: 0.5,
        dimensions: std::collections::HashMap::new(),
        notes: format!(
            "Assignment recorded for task '{}'. Awaiting downstream evaluation.",
            task_id
        ),
        evaluator: "system".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        model: None,
        source: "system".to_string(),
        loop_iteration: 0,
    };

    if let Err(e) = agency::record_evaluation(&eval, agency_dir) {
        eprintln!(
            "Warning: failed to record assigner evaluation for '{}': {}",
            task_id, e
        );
    }
}

/// `wg assign <task-id> <agent-hash>`  — explicitly assign agent to task
/// `wg assign <task-id> --auto`        — automatically select an agent using LLM
/// `wg assign <task-id> --clear`       — remove agent assignment
pub fn run(
    dir: &Path,
    task_id: &str,
    agent_hash: Option<&str>,
    clear: bool,
    auto: bool,
) -> Result<()> {
    let path = graph_path(dir);

    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    if clear {
        return run_clear(dir, &path, task_id);
    }

    if auto {
        return run_auto_assign(dir, &path, task_id);
    }

    match agent_hash {
        Some(hash) => run_explicit_assign(dir, &path, task_id, hash),
        None => {
            anyhow::bail!(
                "Usage: wg assign <task-id> <agent-hash>\n\
                 Or use --auto for automatic assignment.\n\
                 Or use --clear to remove assignment."
            );
        }
    }
}

/// Automatically select and assign an agent using LLM.
fn run_auto_assign(dir: &Path, path: &Path, task_id: &str) -> Result<()> {
    let agency_dir = dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");

    // Load the graph to verify the task exists and get task details
    let graph = load_graph(path).context("Failed to load graph")?;
    let task = graph.get_task_or_err(task_id)?;

    let config = Config::load_or_default(dir);

    // Try Agency assignment if configured
    if config.agency.assignment_source.as_deref() == Some("agency")
        && config.agency.agency_server_url.is_some()
    {
        let task_title = &task.title;
        let task_desc = task.description.as_deref().unwrap_or("");
        match agency::request_agency_assignment(task_title, task_desc, &config.agency) {
            Ok(response) => {
                eprintln!(
                    "[assign] Agency assignment for '{}': agency_task_id={}",
                    task_id, response.agency_task_id,
                );

                // Save assignment record with Agency source
                let assignments_dir = agency_dir.join("assignments");
                let record = agency::TaskAssignmentRecord {
                    task_id: task_id.to_string(),
                    agent_id: String::new(),
                    composition_id: String::new(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    mode: agency::AssignmentMode::Learning(agency::AssignmentExperiment {
                        base_composition: None,
                        dimension: agency::ExperimentDimension::NovelComposition,
                        bizarre_ideation: false,
                        ucb_scores: std::collections::HashMap::new(),
                    }),
                    agency_task_id: Some(response.agency_task_id.clone()),
                    assignment_source: agency::AssignmentSource::Agency {
                        agency_task_id: response.agency_task_id,
                    },
                };
                if let Err(e) = agency::save_assignment_record(&record, &assignments_dir) {
                    eprintln!(
                        "Warning: failed to save assignment record for '{}': {}",
                        task_id, e
                    );
                }

                println!(
                    "Assigned task '{}' via Agency (prompt rendered externally)",
                    task_id
                );
                return Ok(());
            }
            Err(e) => {
                eprintln!(
                    "Warning: Agency assignment failed ({}), falling back to native",
                    e
                );
                // Fall through to native assignment
            }
        }
    }

    // Load all available agents
    let all_agents = agency::load_all_agents_or_warn(&agents_dir);

    if all_agents.is_empty() {
        anyhow::bail!(
            "No agents available for automatic assignment. \
             Use 'wg agent create' to create agents first."
        );
    }

    // Apply composition-rules caps from ~/.agency/composition-rules.csv
    // (re-read on every assignment so edits take effect without restart).
    let overlay = load_overlay();
    let roles_dir = agency_dir.join("cache/roles");
    let components_dir = agency_dir.join("primitives/components");
    let all_agents = apply_caps(&overlay, &all_agents, &roles_dir);

    // Structural pool separation: a normal work task (anything that is NOT
    // an evaluation/review primitive — `.evaluate-*` / `.flip-*` / `.assign-*`
    // scaffold, or tagged `review`/`evaluation`) draws its candidates from the
    // **work pool only** — system evaluation agents (Reviewer / Evaluator /
    // Assigner / Evolver / Agent Creator) are excluded *before* the max-score
    // pick, regardless of their historical usage or score. Evaluation/review
    // primitives keep the full pool (system agents are the correct candidates
    // there). See `assignment_eligibility` and task `make-evaluator-and`.
    //
    // If the work pool is empty for a work task, we do NOT silently fall back
    // to a system agent — we try a default implementation-capable worker
    // first, and if none exists we fail loudly with a configuration error so
    // the operator creates one rather than running an evaluator on a work
    // task.
    let task_uses_work_pool = match graph.get_task(task_id) {
        Some(t) => worksgood::assignment_eligibility::task_uses_work_pool(t),
        None => true,
    };
    let pool: Vec<agency::Agent> = if task_uses_work_pool {
        let work_pool: Vec<agency::Agent> =
            worksgood::assignment_eligibility::filter_work_pool_agents(
                &all_agents,
                &roles_dir,
                &components_dir,
            )
            .into_iter()
            .cloned()
            .collect();
        if work_pool.is_empty() {
            // No work agent available — try a default implementation-capable
            // fallback before refusing, but NEVER silently pick a system
            // evaluation agent.
            if let Some(fb) = worksgood::assignment_eligibility::pick_implementation_capable_agent(
                &all_agents,
                &roles_dir,
                &components_dir,
            ) {
                eprintln!(
                    "[assign] POOL SEPARATION: task '{}' needs a work agent but the work \
                     pool is empty — falling back to the default implementation-capable \
                     worker '{}' ({}).",
                    task_id,
                    fb.name,
                    agency::short_hash(&fb.id),
                );
                vec![fb.clone()]
            } else {
                anyhow::bail!(
                    "No implementation-capable work agent available for task '{}' \
                     (its work pool is empty and no system evaluation agent may be \
                     auto-picked). Create one with `wg agent create` and a work role \
                     (e.g. Programmer) — this is a configuration error, not a transient \
                     one.",
                    task_id,
                );
            }
        } else {
            work_pool
        }
    } else {
        // Evaluation/review primitive — system agents are the correct pool.
        all_agents.clone()
    };

    // Select the agent with the highest performance score, defaulting to the first agent
    let selected_agent = pool
        .iter()
        .max_by(|a, b| {
            let a_score = a.performance.avg_score.unwrap_or(0.0);
            let b_score = b.performance.avg_score.unwrap_or(0.0);
            a_score
                .partial_cmp(&b_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or_else(|| anyhow::anyhow!("No agents found"))?
        .id
        .clone();

    eprintln!(
        "[assign] Auto-selecting agent: {} for task '{}'",
        agency::short_hash(&selected_agent),
        task_id
    );

    // Perform the explicit assignment with the selected agent
    run_explicit_assign(dir, path, task_id, &selected_agent)
}

/// Explicitly assign an agent (by hash or prefix) to a task.
fn run_explicit_assign(dir: &Path, path: &Path, task_id: &str, agent_hash: &str) -> Result<()> {
    let agency_dir = dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");

    // Resolve agent by prefix
    let agent = agency::find_agent_by_prefix(&agents_dir, agent_hash).with_context(|| {
        let available = list_available_agent_ids(&agents_dir);
        let hint = if available.is_empty() {
            "No agents defined. Use 'wg agent create' to create one.".to_string()
        } else {
            format!("Available agents: {}", available.join(", "))
        };
        format!("No agent matching '{}'. {}", agent_hash, hint)
    })?;

    // Structural pool separation (explicit pin): a human pin always wins,
    // but warn LOUDLY when the pinned agent is a system evaluation persona
    // (Reviewer / Evaluator / Assigner / Evolver / Agent Creator) for a normal
    // work task — that is a role/pool mismatch. Evaluation/review primitives
    // (`.evaluate-*` / `.flip-*` / tagged `review`) keep their system agents
    // without warning. See `assignment_eligibility` and task
    // `make-evaluator-and`.
    let graph = load_graph(path).ok();
    if let Some(task) = graph.as_ref().and_then(|g| g.get_task(task_id)) {
        if worksgood::assignment_eligibility::task_uses_work_pool(task) {
            let roles_dir = agency_dir.join("cache/roles");
            let components_dir = agency_dir.join("primitives/components");
            if let Ok(role) = agency::find_role_by_prefix(&roles_dir, &agent.role_id) {
                let comp_names = worksgood::assignment_eligibility::resolve_role_component_names(
                    &role,
                    &components_dir,
                );
                if worksgood::assignment_eligibility::role_is_system_evaluation_with_components(
                    &role,
                    &comp_names,
                ) {
                    eprintln!(
                        "[assign] POOL MISMATCH WARNING (explicit pin kept): task '{}' is a \
                         normal work task and must use the work pool, but pinned agent '{}' \
                         has system role '{}' ({}), which is an evaluation/review/agency \
                         persona. This is a role/pool mismatch — consider pinning an \
                         implementation-capable worker instead.",
                        task_id,
                        agent.name,
                        role.name,
                        agency::short_hash(&agent.id),
                    );
                }
            }
        }
    }

    let agent_id_clone = agent.id.clone();
    let task_id_owned = task_id.to_string();
    let mut error: Option<anyhow::Error> = None;
    modify_graph(path, |graph| {
        let task = match graph.get_task_mut(&task_id_owned) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id_owned));
                return false;
            }
        };
        task.agent = Some(agent_id_clone.clone());
        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }
    super::notify_graph_changed(dir);

    // Record operation
    let config = Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "assign",
        Some(task_id),
        None,
        serde_json::json!({ "agent_hash": agent.id, "role_id": agent.role_id }),
        config.log.rotation_threshold,
    );

    // Update preliminary TaskAssignmentRecord (created by coordinator) with actual agent info.
    // If no preliminary record exists, create a basic Learning one.
    let assignments_dir = agency_dir.join("assignments");
    let record = match agency::load_assignment_record_by_task(&assignments_dir, task_id) {
        Ok(mut existing) => {
            existing.agent_id = agent.id.clone();
            existing.composition_id = agent.id.clone();
            existing
        }
        Err(_) => {
            // No preliminary record — create a basic one
            agency::TaskAssignmentRecord {
                task_id: task_id.to_string(),
                agent_id: agent.id.clone(),
                composition_id: agent.id.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                mode: agency::AssignmentMode::Learning(agency::AssignmentExperiment {
                    base_composition: None,
                    dimension: agency::ExperimentDimension::NovelComposition,
                    bizarre_ideation: false,
                    ucb_scores: std::collections::HashMap::new(),
                }),
                agency_task_id: None,
                assignment_source: agency::AssignmentSource::Native,
            }
        }
    };
    if let Err(e) = agency::save_assignment_record(&record, &assignments_dir) {
        eprintln!(
            "Warning: failed to save assignment record for '{}': {}",
            task_id, e
        );
    }

    // Record assigner evaluation for downstream attribution
    record_assigner_evaluation(&agency_dir, task_id, &agent, &config);

    // Resolve role/tradeoff names for display
    let roles_dir = agency_dir.join("cache/roles");
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");

    let role_name = agency::find_role_by_prefix(&roles_dir, &agent.role_id)
        .map(|r| r.name)
        .unwrap_or_else(|_| "(not found)".to_string());
    let tradeoff_name = agency::find_tradeoff_by_prefix(&tradeoffs_dir, &agent.tradeoff_id)
        .map(|t| t.name)
        .unwrap_or_else(|_| "(not found)".to_string());

    println!("Assigned agent to task '{}':", task_id);
    println!(
        "  Agent:      {} ({})",
        agent.name,
        agency::short_hash(&agent.id)
    );
    println!(
        "  Role:       {} ({})",
        role_name,
        agency::short_hash(&agent.role_id)
    );
    println!(
        "  Tradeoff:   {} ({})",
        tradeoff_name,
        agency::short_hash(&agent.tradeoff_id)
    );

    Ok(())
}

/// Clear the agent assignment from a task.
fn run_clear(dir: &Path, path: &Path, task_id: &str) -> Result<()> {
    let task_id_owned = task_id.to_string();
    let mut error: Option<anyhow::Error> = None;
    let mut prev_agent: Option<String> = None;
    modify_graph(path, |graph| {
        let task = match graph.get_task_mut(&task_id_owned) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id_owned));
                return false;
            }
        };
        prev_agent = task.agent.clone();
        task.agent = None;
        true
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }
    super::notify_graph_changed(dir);

    // Record operation
    let config = worksgood::config::Config::load_or_default(dir);
    let _ = worksgood::provenance::record(
        dir,
        "assign",
        Some(task_id),
        None,
        serde_json::json!({ "action": "clear", "prev_agent": prev_agent }),
        config.log.rotation_threshold,
    );

    if prev_agent.is_some() {
        println!("Cleared agent from task '{}'", task_id);
    } else {
        println!("Task '{}' had no agent assigned (no change)", task_id);
    }
    Ok(())
}

/// List available agent short IDs from the agents directory.
fn list_available_agent_ids(dir: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                ids.push(agency::short_hash(stem).to_string());
            }
        }
    }
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::tempdir;
    use worksgood::agency::{Lineage, PerformanceRecord};
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn setup_workgraph(dir: &Path, tasks: Vec<Task>) {
        fs::create_dir_all(dir).unwrap();
        let path = graph_path(dir);
        let mut graph = WorkGraph::new();
        for task in tasks {
            graph.add_node(Node::Task(task));
        }
        save_graph(&graph, &path).unwrap();
    }

    /// Set up agency with test entities, returning (agent_id, role_id, tradeoff_id).
    fn setup_agency(dir: &Path) -> (String, String, String) {
        let agency_dir = dir.join("agency");
        agency::init(&agency_dir).unwrap();

        let role = agency::build_role(
            "Implementer",
            "Writes code",
            vec!["rust".to_string()],
            "Working code",
        );
        let role_id = role.id.clone();
        agency::save_role(&role, &agency_dir.join("cache/roles")).unwrap();

        let mut tradeoff = agency::build_tradeoff(
            "Quality First",
            "Prioritise correctness",
            vec!["Slower delivery".to_string()],
            vec!["Skipping tests".to_string()],
        );
        tradeoff.performance.task_count = 2;
        tradeoff.performance.avg_score = Some(0.9);
        let tradeoff_id = tradeoff.id.clone();
        agency::save_tradeoff(&tradeoff, &agency_dir.join("primitives/tradeoffs")).unwrap();

        // Create an agent for this role+tradeoff pair
        let agent_id = agency::content_hash_agent(&role_id, &tradeoff_id);
        let agent = agency::Agent {
            id: agent_id.clone(),
            role_id: role_id.clone(),
            tradeoff_id: tradeoff_id.clone(),
            name: "test-agent".to_string(),
            performance: PerformanceRecord::default(),
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
            deployment_history: vec![],
            staleness_flags: vec![],
        };
        agency::save_agent(&agent, &agency_dir.join("cache/agents")).unwrap();

        (agent_id, role_id, tradeoff_id)
    }

    #[test]
    fn test_assign_explicit_agent_hash() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (agent_id, _role_id, _tradeoff_id) = setup_agency(dir_path);

        let result = run(dir_path, "t1", Some(&agent_id), false, false);
        assert!(result.is_ok(), "assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.agent, Some(agent_id));
    }

    #[test]
    fn test_assign_by_prefix() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (agent_id, _role_id, _tradeoff_id) = setup_agency(dir_path);

        // Use 8-char prefix instead of full hash
        let prefix = &agent_id[..8];
        let result = run(dir_path, "t1", Some(prefix), false, false);
        assert!(
            result.is_ok(),
            "assign by prefix failed: {:?}",
            result.err()
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(task.agent, Some(agent_id));
    }

    #[test]
    fn test_assign_clear() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("t1", "Test task");
        task.agent = Some("some-agent-hash".to_string());
        setup_workgraph(dir_path, vec![task]);

        let result = run(dir_path, "t1", None, true, false);
        assert!(result.is_ok());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert!(task.agent.is_none());
    }

    #[test]
    fn test_assign_nonexistent_task() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![]);
        let (agent_id, _, _) = setup_agency(dir_path);

        let result = run(dir_path, "nonexistent", Some(&agent_id), false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_assign_nonexistent_agent() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        setup_agency(dir_path);

        let result = run(dir_path, "t1", Some("nonexistent"), false, false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No agent matching 'nonexistent'"));
    }

    #[test]
    fn test_assign_no_args_fails() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);

        let result = run(dir_path, "t1", None, false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Usage:"));
    }

    #[test]
    fn test_clear_no_agent_is_noop() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);

        let result = run(dir_path, "t1", None, true, false);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Special agent evaluation recording tests
    // -----------------------------------------------------------------------

    /// Set up a full agency with the assigner special agent composed from
    /// real starters, matching the `wg agency init` pathway. Returns
    /// (actor_agent_id, assigner_agent_id).
    fn setup_agency_with_assigner(dir: &Path) -> (String, String) {
        let agency_dir = dir.join("agency");
        agency::seed_starters(&agency_dir).unwrap();

        let agents_dir = agency_dir.join("cache/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Create the actor agent (assigned to the task)
        let (actor_id, _role_id, _tradeoff_id) = setup_agency(dir);

        // Compose the assigner special agent from starter primitives
        let special_roles = agency::special_agent_roles();
        let special_tradeoffs = agency::special_agent_tradeoffs();
        let assigner_role = special_roles.iter().find(|r| r.name == "Assigner").unwrap();
        let assigner_tradeoff = special_tradeoffs
            .iter()
            .find(|t| t.name == "Assigner Balanced")
            .unwrap();

        let assigner_id = agency::content_hash_agent(&assigner_role.id, &assigner_tradeoff.id);
        let assigner_path = agents_dir.join(format!("{}.yaml", assigner_id));
        if !assigner_path.exists() {
            let assigner_agent = agency::Agent {
                id: assigner_id.clone(),
                role_id: assigner_role.id.clone(),
                tradeoff_id: assigner_tradeoff.id.clone(),
                name: "Default Assigner".to_string(),
                performance: PerformanceRecord::default(),
                lineage: Lineage::default(),
                capabilities: vec![],
                rate: None,
                capacity: None,
                trust_level: Default::default(),
                contact: None,
                executor: "claude".to_string(),
                preferred_model: None,
                preferred_provider: None,
                attractor_weight: 0.5,
                deployment_history: vec![],
                staleness_flags: vec![],
            };
            agency::save_agent(&assigner_agent, &agents_dir).unwrap();
        }

        // Configure the assigner_agent in config with auto_evaluate enabled
        let mut config = Config::load_or_default(dir);
        config.agency.auto_evaluate = true;
        config.agency.assigner_agent = Some(assigner_id.clone());
        config.save(dir).unwrap();

        (actor_id, assigner_id)
    }

    /// (1) Simulate an inline assign execution and verify it succeeds.
    #[test]
    fn test_assign_records_assigner_evaluation() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        // Run assign — this triggers record_assigner_evaluation internally
        let result = run(dir_path, "t1", Some(&actor_id), false, false);
        assert!(result.is_ok(), "assign failed: {:?}", result.err());

        // Verify the evaluation JSON file was created
        let evals_dir = dir_path.join("agency/evaluations");
        let eval_files: Vec<_> = fs::read_dir(&evals_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("eval-.assign-t1-")
            })
            .collect();
        assert_eq!(
            eval_files.len(),
            1,
            "Expected exactly one evaluation file for assign-t1, got {}",
            eval_files.len()
        );

        // Load and verify the evaluation contents
        let eval = agency::load_evaluation(&eval_files[0].path()).unwrap();
        assert_eq!(eval.task_id, ".assign-t1");
        assert_eq!(
            eval.agent_id, assigner_id,
            "Evaluation should be recorded against the assigner agent"
        );
        assert_eq!(eval.source, "system");
        assert_eq!(eval.score, 0.5, "Placeholder score should be 0.5");
    }

    /// (2) Verify the Evaluation is recorded against the assigner agent hash,
    /// not the actor agent.
    #[test]
    fn test_evaluation_recorded_against_assigner_not_actor() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();

        // Load the assigner agent and verify it has the evaluation
        let agents_dir = dir_path.join("agency/cache/agents");
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(
            assigner.performance.evaluations.len(),
            1,
            "Assigner agent should have exactly 1 evaluation"
        );
        assert_eq!(assigner.performance.evaluations[0].task_id, ".assign-t1");

        // The actor agent should NOT have any evaluation from this assignment
        let actor = agency::find_agent_by_prefix(&agents_dir, &actor_id).unwrap();
        assert_eq!(
            actor.performance.evaluations.len(),
            0,
            "Actor agent should NOT have evaluations from assigner recording"
        );
    }

    /// (3) Verify the assigner's PerformanceRecord.task_count increments.
    #[test]
    fn test_assigner_task_count_increments() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![
                make_task("t1", "First task"),
                make_task("t2", "Second task"),
                make_task("t3", "Third task"),
            ],
        );
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        let agents_dir = dir_path.join("agency/cache/agents");

        // Before any assignments
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(assigner.performance.task_count, 0);

        // First assignment
        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(
            assigner.performance.task_count, 1,
            "task_count should be 1 after first assign"
        );

        // Second assignment
        run(dir_path, "t2", Some(&actor_id), false, false).unwrap();
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(
            assigner.performance.task_count, 2,
            "task_count should be 2 after second assign"
        );

        // Third assignment
        run(dir_path, "t3", Some(&actor_id), false, false).unwrap();
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(
            assigner.performance.task_count, 3,
            "task_count should be 3 after third assign"
        );

        // Verify avg_score is 0.5 (all assignments use placeholder score 0.5)
        assert!(
            (assigner.performance.avg_score.unwrap() - 0.5).abs() < 1e-10,
            "All assignments use placeholder 0.5, avg should be 0.5"
        );
    }

    /// (4) Verify score propagates through the 6-step cascade to the
    /// assigner's role components.
    ///
    /// The 6-step cascade in record_evaluation:
    ///   1. Save evaluation JSON
    ///   2. Update agent performance
    ///   3. Update role performance
    ///   4. Update tradeoff performance
    ///   5. Propagate to each role component
    ///   6. Propagate to the role's desired outcome
    #[test]
    fn test_score_propagates_through_cascade_to_components() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        // Run assign to trigger the cascade
        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();

        let agency_dir = dir_path.join("agency");
        let agents_dir = agency_dir.join("cache/agents");
        let roles_dir = agency_dir.join("cache/roles");
        let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
        let components_dir = agency_dir.join("primitives/components");
        let outcomes_dir = agency_dir.join("primitives/outcomes");

        // Step 2: Agent performance updated
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(assigner.performance.task_count, 1);
        assert!((assigner.performance.avg_score.unwrap() - 0.5).abs() < 1e-10);

        // Step 3: Role performance updated
        let role = agency::find_role_by_prefix(&roles_dir, &assigner.role_id).unwrap();
        assert_eq!(
            role.performance.task_count, 1,
            "Role should have task_count=1 after cascade"
        );
        assert!((role.performance.avg_score.unwrap() - 0.5).abs() < 1e-10);
        // Role's context_id should be the tradeoff_id
        assert_eq!(
            role.performance.evaluations[0].context_id, assigner.tradeoff_id,
            "Role eval context_id should be tradeoff_id"
        );

        // Step 4: Tradeoff performance updated
        let tradeoff =
            agency::find_tradeoff_by_prefix(&tradeoffs_dir, &assigner.tradeoff_id).unwrap();
        assert_eq!(
            tradeoff.performance.task_count, 1,
            "Tradeoff should have task_count=1 after cascade"
        );
        assert!((tradeoff.performance.avg_score.unwrap() - 0.5).abs() < 1e-10);
        // Tradeoff's context_id should be the role_id
        assert_eq!(
            tradeoff.performance.evaluations[0].context_id, assigner.role_id,
            "Tradeoff eval context_id should be role_id"
        );

        // Step 5: Each role component's performance updated
        let assigner_comps = agency::assigner_components();
        assert!(
            !role.component_ids.is_empty(),
            "Assigner role should have components"
        );
        for comp_id in &role.component_ids {
            let component = agency::find_component_by_prefix(&components_dir, comp_id).unwrap();
            assert_eq!(
                component.performance.task_count,
                1,
                "Component '{}' ({}) should have task_count=1 after cascade",
                component.name,
                agency::short_hash(&component.id)
            );
            assert!(
                (component.performance.avg_score.unwrap() - 0.5).abs() < 1e-10,
                "Component '{}' avg_score should be 0.5",
                component.name
            );
            // Component's context_id should be the role_id
            assert_eq!(
                component.performance.evaluations[0].context_id, assigner.role_id,
                "Component '{}' context_id should be role_id",
                component.name
            );
        }
        // Verify all expected assigner components were touched
        assert_eq!(
            role.component_ids.len(),
            assigner_comps.len(),
            "Role should reference all {} assigner components",
            assigner_comps.len()
        );

        // Step 6: Desired outcome performance updated
        assert!(
            !role.outcome_id.is_empty(),
            "Assigner role should have an outcome_id"
        );
        let outcome = agency::find_outcome_by_prefix(&outcomes_dir, &role.outcome_id).unwrap();
        assert_eq!(
            outcome.performance.task_count, 1,
            "Outcome should have task_count=1 after cascade"
        );
        assert!(
            (outcome.performance.avg_score.unwrap() - 0.5).abs() < 1e-10,
            "Outcome avg_score should be 0.5"
        );
        // Outcome's context_id should be the agent_id
        assert_eq!(
            outcome.performance.evaluations[0].context_id, assigner.id,
            "Outcome eval context_id should be agent_id"
        );
    }

    /// Verify no evaluation is recorded when auto_evaluate is disabled.
    #[test]
    fn test_no_evaluation_when_auto_evaluate_disabled() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        // Disable auto_evaluate
        let mut config = Config::load_or_default(dir_path);
        config.agency.auto_evaluate = false;
        config.save(dir_path).unwrap();

        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();

        // Assigner should have no evaluations
        let agents_dir = dir_path.join("agency/cache/agents");
        let assigner = agency::find_agent_by_prefix(&agents_dir, &assigner_id).unwrap();
        assert_eq!(
            assigner.performance.task_count, 0,
            "No evaluation should be recorded when auto_evaluate is disabled"
        );
    }

    /// Verify no evaluation is recorded when no assigner_agent is configured.
    #[test]
    #[serial]
    fn test_no_evaluation_when_no_assigner_configured() {
        // Isolate from global config (~/.wg/config.toml) which may
        // set assigner_agent — that value leaks through config merge when
        // the local config omits it (skip_serializing_if on Option::None).
        let saved_home = std::env::var("HOME").ok();
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        unsafe { std::env::set_var("HOME", dir_path) };
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (actor_id, _assigner_id) = setup_agency_with_assigner(dir_path);

        // Remove assigner_agent from config
        let mut config = Config::load_or_default(dir_path);
        config.agency.assigner_agent = None;
        config.save(dir_path).unwrap();

        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();
        if let Some(h) = saved_home {
            unsafe { std::env::set_var("HOME", h) };
        }

        // No evaluation files should be created for assign-t1
        let evals_dir = dir_path.join("agency/evaluations");
        let eval_files: Vec<_> = fs::read_dir(&evals_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("eval-.assign-t1-")
            })
            .collect();
        assert_eq!(
            eval_files.len(),
            0,
            "No evaluation should be recorded when assigner_agent is not configured"
        );
    }

    /// Verify multiple assignments accumulate correctly with the cascade.
    #[test]
    fn test_multiple_assignments_cascade_accumulates() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(
            dir_path,
            vec![make_task("t1", "Task one"), make_task("t2", "Task two")],
        );
        let (actor_id, assigner_id) = setup_agency_with_assigner(dir_path);

        run(dir_path, "t1", Some(&actor_id), false, false).unwrap();
        run(dir_path, "t2", Some(&actor_id), false, false).unwrap();

        let agency_dir = dir_path.join("agency");

        // Agent should have 2 evaluations
        let assigner =
            agency::find_agent_by_prefix(&agency_dir.join("cache/agents"), &assigner_id).unwrap();
        assert_eq!(assigner.performance.task_count, 2);
        assert_eq!(assigner.performance.evaluations.len(), 2);

        // Role should also have 2
        let role = agency::find_role_by_prefix(&agency_dir.join("cache/roles"), &assigner.role_id)
            .unwrap();
        assert_eq!(role.performance.task_count, 2);

        // Each component should have 2
        for comp_id in &role.component_ids {
            let comp = agency::find_component_by_prefix(
                &agency_dir.join("primitives/components"),
                comp_id,
            )
            .unwrap();
            assert_eq!(
                comp.performance.task_count, 2,
                "Component '{}' should have task_count=2 after 2 assignments",
                comp.name
            );
        }

        // Outcome should have 2
        let outcome = agency::find_outcome_by_prefix(
            &agency_dir.join("primitives/outcomes"),
            &role.outcome_id,
        )
        .unwrap();
        assert_eq!(outcome.performance.task_count, 2);
    }

    // -----------------------------------------------------------------------
    // Pool-separation regression tests (make-evaluator-and; supersedes the
    // prevent-evaluator-reviewer heuristic). These assert STRUCTURAL pool
    // separation — system evaluation agents (Reviewer / Evaluator / Assigner
    // / Evolver / Agent Creator) are excluded from the work-task candidate
    // set regardless of score / historical usage / task wording, not merely
    // filtered by verb guessing.
    // -----------------------------------------------------------------------

    /// Seed starter roles (Programmer + Reviewer) and create one agent per
    /// role, returning (programmer_agent_id, reviewer_agent_id). The
    /// Programmer agent is given a higher score so the max-score heuristic
    /// would pick it even without the guard — tests below flip the scores to
    /// force the guard to be the deciding factor.
    fn setup_programmer_and_reviewer(dir: &Path) -> (String, String) {
        let agency_dir = dir.join("agency");
        agency::seed_starters(&agency_dir).unwrap();

        let roles_dir = agency_dir.join("cache/roles");
        let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
        let agents_dir = agency_dir.join("cache/agents");

        // Find the seeded Programmer and Reviewer roles.
        let all_roles = agency::load_all_roles(&roles_dir).unwrap_or_default();
        let programmer_role = all_roles
            .iter()
            .find(|r| r.name == "Programmer")
            .unwrap()
            .clone();
        let reviewer_role = all_roles
            .iter()
            .find(|r| r.name == "Reviewer")
            .unwrap()
            .clone();

        // A single shared tradeoff.
        let tradeoff = agency::build_tradeoff(
            "Careful",
            "Prioritise correctness",
            vec!["Slow".to_string()],
            vec!["Unreliable".to_string()],
        );
        agency::save_tradeoff(&tradeoff, &tradeoffs_dir).unwrap();

        let make_agent = |role: &agency::Role, name: &str, score: Option<f64>| -> String {
            let id = agency::content_hash_agent(&role.id, &tradeoff.id);
            let mut perf = PerformanceRecord::default();
            perf.avg_score = score;
            perf.task_count = if score.is_some() { 1 } else { 0 };
            let agent = agency::Agent {
                id: id.clone(),
                role_id: role.id.clone(),
                tradeoff_id: tradeoff.id.clone(),
                name: name.to_string(),
                performance: perf,
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
                deployment_history: vec![],
                staleness_flags: vec![],
            };
            agency::save_agent(&agent, &agents_dir).unwrap();
            id
        };

        let prog_id = make_agent(&programmer_role, "prog-agent", Some(0.5));
        let rev_id = make_agent(&reviewer_role, "review-agent", Some(0.99));
        (prog_id, rev_id)
    }

    /// Regression: an implementation task with concrete deliverables + build
    /// wording MUST NOT be auto-assigned to a reviewer-only agent, even when
    /// the reviewer has a higher score than every implementation agent.
    #[test]
    fn auto_assign_impl_task_skips_reviewer_for_programmer() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("build-real-async", "Build the real async runtime");
        task.deliverables = vec!["src/async.rs".to_string()];
        task.exec_mode = Some("full".to_string());
        setup_workgraph(dir_path, vec![task]);
        let (prog_id, _rev_id) = setup_programmer_and_reviewer(dir_path);

        // Reviewer has score 0.99 > Programmer 0.5; without the guard the
        // max-score pick would be the reviewer. The guard must filter it out.
        let result = run(dir_path, "build-real-async", None, false, true);
        assert!(result.is_ok(), "auto-assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("build-real-async").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(prog_id.as_str()),
            "implementation task must be assigned to the programmer, not the reviewer"
        );
    }

    /// Regression: an `.evaluate-*` task still routes to the evaluator role —
    /// the guard must not block evaluator assignment for system evaluation
    /// tasks.
    #[test]
    fn evaluate_task_still_routes_to_evaluator() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // An .evaluate-* scaffold task.
        let task = make_task(".evaluate-foo", "Evaluate foo");
        setup_workgraph(dir_path, vec![task]);
        // Build an Evaluator agent (special role) with a high score and a
        // Programmer agent with a low score. The guard must NOT filter the
        // evaluator out for an .evaluate-* task.
        let agency_dir = dir_path.join("agency");
        agency::seed_starters(&agency_dir).unwrap();
        let roles_dir = agency_dir.join("cache/roles");
        let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
        let agents_dir = agency_dir.join("cache/agents");

        let evaluator_role = agency::special_agent_roles()
            .into_iter()
            .find(|r| r.name == "Evaluator")
            .unwrap();
        agency::save_role(&evaluator_role, &roles_dir).unwrap();
        let programmer_role = agency::load_all_roles(&roles_dir)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.name == "Programmer")
            .unwrap();
        let tradeoff = agency::build_tradeoff(
            "Careful",
            "x",
            vec!["Slow".to_string()],
            vec!["Bad".to_string()],
        );
        agency::save_tradeoff(&tradeoff, &tradeoffs_dir).unwrap();

        let make_agent = |role: &agency::Role, name: &str, score: Option<f64>| -> String {
            let id = agency::content_hash_agent(&role.id, &tradeoff.id);
            let mut perf = PerformanceRecord::default();
            perf.avg_score = score;
            perf.task_count = if score.is_some() { 1 } else { 0 };
            let agent = agency::Agent {
                id: id.clone(),
                role_id: role.id.clone(),
                tradeoff_id: tradeoff.id.clone(),
                name: name.to_string(),
                performance: perf,
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
                deployment_history: vec![],
                staleness_flags: vec![],
            };
            agency::save_agent(&agent, &agents_dir).unwrap();
            id
        };
        let eval_id = make_agent(&evaluator_role, "eval-agent", Some(0.99));
        let _prog_id = make_agent(&programmer_role, "prog-agent", Some(0.1));

        let result = run(dir_path, ".evaluate-foo", None, false, true);
        assert!(result.is_ok(), "auto-assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task(".evaluate-foo").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(eval_id.as_str()),
            ".evaluate-* task must still route to the evaluator role"
        );
    }

    /// Regression: explicit human pinning to a valid implementation agent still
    /// works (no warning, assignment proceeds).
    #[test]
    fn explicit_pin_to_programmer_on_impl_task_works() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("build-real-async", "Build the real async runtime");
        task.deliverables = vec!["src/async.rs".to_string()];
        setup_workgraph(dir_path, vec![task]);
        let (prog_id, _rev_id) = setup_programmer_and_reviewer(dir_path);

        let result = run(dir_path, "build-real-async", Some(&prog_id), false, false);
        assert!(result.is_ok(), "explicit pin failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("build-real-async").unwrap();
        assert_eq!(task.agent.as_deref(), Some(prog_id.as_str()));
    }

    /// Regression: explicit human pinning to a REVIEWER for an implementation
    /// task still proceeds (human wins) — the guard only warns, it does not
    /// block explicit pins.
    #[test]
    fn explicit_pin_to_reviewer_on_impl_task_still_assigns() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let mut task = make_task("register-seed", "Register refreshed e97 seed latest");
        task.exec_mode = Some("full".to_string());
        setup_workgraph(dir_path, vec![task]);
        let (_prog_id, rev_id) = setup_programmer_and_reviewer(dir_path);

        // Human explicitly pinned the reviewer — must still assign (warn only).
        let result = run(dir_path, "register-seed", Some(&rev_id), false, false);
        assert!(
            result.is_ok(),
            "explicit reviewer pin failed: {:?}",
            result.err()
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("register-seed").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(rev_id.as_str()),
            "explicit human pin must win even on a guard mismatch"
        );
    }

    /// Regression for the retry-after-evaluator failure mode: when the prior
    /// attempt picked a reviewer (evaluator-style no-op behavior) for an
    /// implementation task, the guard's fallback picker must return an
    /// implementation-capable agent from the pool so the dispatcher can mutate
    /// the assignment on retry. This exercises the same primitive the
    /// dispatcher guard calls.
    #[test]
    fn retry_after_evaluator_no_op_picks_implementation_agent() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // No graph needed — this tests the guard primitive directly.
        let (_prog_id, _rev_id) = setup_programmer_and_reviewer(dir_path);
        let agency_dir = dir_path.join("agency");
        let agents_dir = agency_dir.join("cache/agents");
        let roles_dir = agency_dir.join("cache/roles");
        let components_dir = agency_dir.join("primitives/components");

        let all_agents = agency::load_all_agents_or_warn(&agents_dir);
        assert!(all_agents.len() >= 2, "expected >=2 agents");

        let pick = worksgood::assignment_eligibility::pick_implementation_capable_agent(
            &all_agents,
            &roles_dir,
            &components_dir,
        );
        let pick = pick.expect("a fallback implementation agent must exist");
        let role = agency::find_role_by_prefix(&roles_dir, &pick.role_id).unwrap();
        assert_eq!(
            role.name, "Programmer",
            "retry fallback must be the implementation-capable Programmer, not the Reviewer"
        );
    }

    // -----------------------------------------------------------------------
    // Pool-separation regression tests (make-evaluator-and)
    // -----------------------------------------------------------------------

    /// Acceptance #1 + #3: when a Reviewer has the HIGHEST historical usage
    /// / score in the pool, a normal implementation task is still assigned to
    /// an implementation-capable worker (Programmer), never the Reviewer.
    /// The gate is structural pool separation, not verb guessing.
    #[test]
    fn auto_assign_impl_task_skips_reviewer_even_when_reviewer_score_highest() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // An implementation task — but the structural guarantee does not even
        // depend on the verbs; the work pool excludes the Reviewer regardless.
        let mut task = make_task("build-real-async", "Build the real async runtime");
        task.deliverables = vec!["src/async.rs".to_string()];
        task.exec_mode = Some("full".to_string());
        setup_workgraph(dir_path, vec![task]);
        let (prog_id, _rev_id) = setup_programmer_and_reviewer(dir_path);

        // The Reviewer agent has the higher score (0.99 > 0.5); the guard
        // must still pick the Programmer.
        let result = run(dir_path, "build-real-async", None, false, true);
        assert!(result.is_ok(), "auto-assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("build-real-async").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(prog_id.as_str()),
            "highest-score reviewer must NOT be picked for an impl task"
        );
    }

    /// Acceptance #3: a NEUTRAL work task (no implementation verbs, no review
    /// tags, no deliverables) still must NOT pick a system evaluation agent,
    /// even when the Reviewer has the highest score / historical usage. The
    /// pool split is keyed on task KIND (work vs primitive), not verb guessing.
    #[test]
    fn neutral_work_task_skips_reviewer_even_without_impl_verbs() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        // A neutral work task — title says nothing about implementation,
        // no deliverables, no tags. Under the old verb-guessing guard this
        // would NOT have been flagged; under pool separation it must still
        // exclude the system Reviewer.
        let task = make_task("t1", "Triage incoming issues");
        setup_workgraph(dir_path, vec![task]);
        let (prog_id, _rev_id) = setup_programmer_and_reviewer(dir_path);

        // Reviewer score 0.99 > Programmer 0.5; without pool separation the
        // max-score pick would land on the Reviewer.
        let result = run(dir_path, "t1", None, false, true);
        assert!(result.is_ok(), "auto-assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(prog_id.as_str()),
            "neutral work task must pick the work agent, not the highest-score Reviewer"
        );
    }

    /// Acceptance #1 for the Evaluator meta persona: a normal work task must
    /// not pick an Evaluator even when it has the highest score. This mirrors
    /// the Reviewer case for the Evaluator system role.
    #[test]
    fn neutral_work_task_skips_evaluator_meta_persona() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let task = make_task("t1", "Organise the intake board");
        setup_workgraph(dir_path, vec![task]);

        // Build an Evaluator agent (special role) with a high score and a
        // Programmer agent with a low score — the Evaluator must be excluded
        // from the work pool.
        let agency_dir = dir_path.join("agency");
        agency::seed_starters(&agency_dir).unwrap();
        let roles_dir = agency_dir.join("cache/roles");
        let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
        let agents_dir = agency_dir.join("cache/agents");

        let evaluator_role = agency::special_agent_roles()
            .into_iter()
            .find(|r| r.name == "Evaluator")
            .unwrap();
        agency::save_role(&evaluator_role, &roles_dir).unwrap();
        let programmer_role = agency::load_all_roles(&roles_dir)
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.name == "Programmer")
            .unwrap();
        let tradeoff = agency::build_tradeoff(
            "Careful",
            "x",
            vec!["Slow".to_string()],
            vec!["Bad".to_string()],
        );
        agency::save_tradeoff(&tradeoff, &tradeoffs_dir).unwrap();

        let make_agent = |role: &agency::Role, name: &str, score: Option<f64>| -> String {
            let id = agency::content_hash_agent(&role.id, &tradeoff.id);
            let mut perf = PerformanceRecord::default();
            perf.avg_score = score;
            perf.task_count = if score.is_some() { 1 } else { 0 };
            let agent = agency::Agent {
                id: id.clone(),
                role_id: role.id.clone(),
                tradeoff_id: tradeoff.id.clone(),
                name: name.to_string(),
                performance: perf,
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
                deployment_history: vec![],
                staleness_flags: vec![],
            };
            agency::save_agent(&agent, &agents_dir).unwrap();
            id
        };
        let _eval_id = make_agent(&evaluator_role, "eval-agent", Some(0.99));
        let prog_id = make_agent(&programmer_role, "prog-agent", Some(0.1));

        let result = run(dir_path, "t1", None, false, true);
        assert!(result.is_ok(), "auto-assign failed: {:?}", result.err());

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(prog_id.as_str()),
            "Evaluator meta persona must NOT be picked for a neutral work task"
        );
    }

    /// Acceptance #4: explicit human pin to a Reviewer on a NEUTRAL work task
    /// (no impl verbs) still assigns (human wins) but the pool-mismatch warning
    /// fires — structural separation applies to neutral tasks too, not only
    /// implementation-flavoured ones.
    #[test]
    fn explicit_pin_to_reviewer_on_neutral_task_warns_but_assigns() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        let task = make_task("t1", "Triage incoming issues");
        setup_workgraph(dir_path, vec![task]);
        let (_prog_id, rev_id) = setup_programmer_and_reviewer(dir_path);

        let result = run(dir_path, "t1", Some(&rev_id), false, false);
        assert!(
            result.is_ok(),
            "explicit reviewer pin failed: {:?}",
            result.err()
        );

        let path = graph_path(dir_path);
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("t1").unwrap();
        assert_eq!(
            task.agent.as_deref(),
            Some(rev_id.as_str()),
            "explicit human pin must win even on a pool mismatch"
        );
    }
}
