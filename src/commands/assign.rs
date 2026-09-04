use anyhow::{Context, Result};
use std::path::Path;
use worksgood::adaptive_agency::{
    AdaptiveStore, AssignmentDecisionV1, AssignmentInfrastructureFailureV1, AssignmentIntentV1,
    AssignmentSelectorSnapshotV1,
};
use worksgood::agency;
use worksgood::agency::composition_rules::{
    CompositionRulesOverlay, default_overlay_path, load_composition_rules,
};
use worksgood::config::Config;
use worksgood::identity::canonical_json;
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

fn digest<T: serde::Serialize>(value: &T) -> String {
    format!(
        "b3:{}",
        blake3::hash(&canonical_json(
            &serde_json::to_value(value).unwrap_or_default()
        ))
        .to_hex()
    )
}

fn record_uncomposed_auto_intent(
    dir: &Path,
    task_id: &str,
    class: &str,
    message: &str,
    selector_route: Option<String>,
) -> Result<()> {
    AdaptiveStore::open(dir)?.record_assignment_intent(AssignmentIntentV1 {
        task_id: task_id.to_string(),
        decision: AssignmentDecisionV1::Uncomposed {
            reason: format!("automatic selector failed: {class}"),
        },
        selector: AssignmentSelectorSnapshotV1 {
            kind: "agency-provider".to_string(),
            principal: "configured-assignment-provider".to_string(),
            policy_digest: digest(&("agency-provider-v1", selector_route.as_deref())),
            exact_route: selector_route,
        },
        candidate_scores: std::collections::BTreeMap::new(),
        selected_composition: None,
        failure: Some(AssignmentInfrastructureFailureV1 {
            class: class.to_string(),
            message_digest: digest(&message),
            fallback: "direct-uncomposed-dispatch".to_string(),
        }),
        created_at: chrono::Utc::now().to_rfc3339(),
    })?;
    Ok(())
}

/// `wg assign <task-id> <agent-hash>`  — explicitly bind the next attempt
/// `wg assign <task-id> --auto`        — deterministically rank receipt-backed history
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
        Some(hash) => run_explicit_assign(dir, &path, task_id, hash, None),
        None => {
            anyhow::bail!(
                "Usage: wg assign <task-id> <agent-hash>\n\
                 Or use --auto for automatic assignment.\n\
                 Or use --clear to remove assignment."
            );
        }
    }
}

/// Admission hook for the optional automatic mode. It runs before attempt
/// reservation in this bounded assignment state domain. Any selector failure
/// must already have written an explicit uncomposed intent; admission then
/// continues without an invented identity or route.
pub(crate) fn prepare_automatic_intent_if_configured(dir: &Path, task_id: &str) -> Result<()> {
    let config = Config::load_merged(dir)?;
    if !config.agency.auto_assign {
        return Ok(());
    }
    let path = graph_path(dir);
    let graph = load_graph(&path)?;
    let task = graph.get_task_or_err(task_id)?;
    if task.agent.is_some()
        || AdaptiveStore::open(dir)?
            .assignment_intent(task_id)?
            .is_some()
    {
        return Ok(());
    }
    if let Err(error) = run_auto_assign(dir, &path, task_id) {
        let adaptive = AdaptiveStore::open(dir)?;
        let explicit_uncomposed = adaptive.assignment_intent(task_id)?.is_some_and(|intent| {
            matches!(intent.decision, AssignmentDecisionV1::Uncomposed { .. })
        });
        if !explicit_uncomposed {
            return Err(error);
        }
        eprintln!(
            "[assign] automatic selector failed visibly; continuing under recorded direct-uncomposed fallback: {error:#}"
        );
    }
    Ok(())
}

/// Deterministically rank the eligible roster. No model call is claimed.
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
        let route = config.agency.agency_server_url.clone();
        match agency::request_agency_assignment(task_title, task_desc, &config.agency) {
            Ok(response) => {
                let message = format!(
                    "provider returned agency_task_id={} but no receipt-bound composition",
                    response.agency_task_id
                );
                record_uncomposed_auto_intent(
                    dir,
                    task_id,
                    "malformed_or_unbound_output",
                    &message,
                    route,
                )?;
                anyhow::bail!(
                    "error[WG-ASSIGNMENT-PROVIDER-UNCOMPOSED]: {message}; no identity or route was fabricated. Explicit fallback policy is direct-uncomposed dispatch on the next claim"
                );
            }
            Err(error) => {
                let message = format!("{error:#}");
                record_uncomposed_auto_intent(
                    dir,
                    task_id,
                    "provider_unavailable",
                    &message,
                    route,
                )?;
                anyhow::bail!(
                    "error[WG-ASSIGNMENT-PROVIDER-UNAVAILABLE]: configured assignment provider failed; no silent native reroute occurred. Explicit fallback policy is direct-uncomposed dispatch on the next claim: {message}"
                );
            }
        }
    }

    // Load all available agents
    let all_agents = agency::load_all_agents_or_warn(&agents_dir);

    if all_agents.is_empty() {
        record_uncomposed_auto_intent(
            dir,
            task_id,
            "no_eligible_composition",
            "no agents available for deterministic ranking",
            None,
        )?;
        anyhow::bail!(
            "No agents available for deterministic automatic assignment. The next claim may proceed with an explicit direct-uncomposed receipt; use 'wg agent create' to create agents first."
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

    // Select the agent with the highest performance score in the same history
    // partition the lightweight assigner uses. This keeps agency/system task
    // wins from making evaluator/reviewer personas look experienced for
    // ordinary work.
    let history_class = crate::commands::service::assignment::history_class_for_assignment(task);
    eprintln!(
        "[assign] history_class={} for task '{}' (auto-rank uses only this class)",
        history_class.label(),
        task_id
    );
    let adaptive = AdaptiveStore::open(dir)?;
    let mut ranked = pool
        .iter()
        .map(|agent| {
            let composition = super::adaptive_agency::composition_snapshot(dir, agent)?;
            let delayed_reward = adaptive.reader().mean_reward_for_composition(
                &composition.composition_digest,
                history_class.label(),
            )?;
            let legacy = crate::commands::service::assignment::scoped_performance_for_agent(
                agent,
                Some(&graph),
                history_class,
            )
            .avg_score
            .unwrap_or(0.0);
            Ok((agent, composition, delayed_reward.unwrap_or(legacy)))
        })
        .collect::<Result<Vec<_>>>()?;
    ranked.sort_by(|left, right| {
        right
            .2
            .partial_cmp(&left.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.id.cmp(&right.0.id))
    });
    let (selected, selected_composition, _) = ranked
        .first()
        .ok_or_else(|| anyhow::anyhow!("No agents found"))?;
    let candidate_scores = ranked
        .iter()
        .map(|(_, composition, score)| (composition.composition_digest.clone(), *score))
        .collect::<std::collections::BTreeMap<_, _>>();
    let selected_agent = selected.id.clone();

    eprintln!(
        "[assign] Deterministic receipt-backed ranking selected agent: {} for task '{}'",
        agency::short_hash(&selected_agent),
        task_id
    );

    run_explicit_assign(
        dir,
        path,
        task_id,
        &selected_agent,
        Some((selected_composition.clone(), candidate_scores)),
    )
}

/// Explicitly assign an agent (by hash or prefix) to a task.
fn run_explicit_assign(
    dir: &Path,
    path: &Path,
    task_id: &str,
    agent_hash: &str,
    automatic: Option<(
        worksgood::adaptive_agency::CompositionSnapshotV1,
        std::collections::BTreeMap<String, f64>,
    )>,
) -> Result<()> {
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

    // Preserve the next-attempt intent. The dispatcher/claim path turns it
    // into an attempt-bound immutable receipt; no synthetic assignment task or
    // placeholder quality evaluation is created here.
    let (decision, selector, candidate_scores, selected_composition) =
        if let Some((composition, candidate_scores)) = automatic {
            (
                AssignmentDecisionV1::Automatic {
                    composition_digest: composition.composition_digest.clone(),
                },
                AssignmentSelectorSnapshotV1 {
                    kind: "deterministic-reward-ranking".to_string(),
                    principal: "wg-native-selector".to_string(),
                    policy_digest: digest(&(
                        "deterministic-reward-ranking-v1",
                        crate::commands::service::assignment::history_class_for_assignment(
                            load_graph(path)?.get_task_or_err(task_id)?,
                        )
                        .label(),
                    )),
                    exact_route: None,
                },
                candidate_scores,
                composition,
            )
        } else {
            let composition = super::adaptive_agency::composition_snapshot(dir, &agent)?;
            (
                AssignmentDecisionV1::Explicit {
                    composition_digest: composition.composition_digest.clone(),
                },
                AssignmentSelectorSnapshotV1 {
                    kind: "explicit-task-intent".to_string(),
                    principal: worksgood::current_user(),
                    policy_digest: "explicit-assignment-v1".to_string(),
                    exact_route: None,
                },
                std::collections::BTreeMap::new(),
                composition,
            )
        };
    AdaptiveStore::open(dir)?.record_assignment_intent(AssignmentIntentV1 {
        task_id: task_id.to_string(),
        decision,
        selector,
        candidate_scores,
        selected_composition: Some(selected_composition),
        failure: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    })?;

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
    AdaptiveStore::open(dir)?.clear_assignment_intent(task_id)?;

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
    fn assignment_records_intent_without_placeholder_evaluation() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();
        setup_workgraph(dir_path, vec![make_task("t1", "Test task")]);
        let (agent_id, _, _) = setup_agency(dir_path);
        let mut config = Config::load_or_default(dir_path);
        config.agency.auto_evaluate = true;
        config.save(dir_path).unwrap();

        run(dir_path, "t1", Some(&agent_id), false, false).unwrap();
        let intent = AdaptiveStore::open(dir_path)
            .unwrap()
            .assignment_intent("t1")
            .unwrap()
            .unwrap();
        assert!(matches!(
            intent.decision,
            AssignmentDecisionV1::Explicit { .. }
        ));
        assert_eq!(
            std::fs::read_dir(dir_path.join("agency/evaluations"))
                .map(|entries| entries.filter_map(Result::ok).count())
                .unwrap_or(0),
            0,
            "assignment must not manufacture a placeholder quality score"
        );
    }

    #[test]
    fn provider_failure_records_visible_direct_uncomposed_fallback() {
        let dir = tempdir().unwrap();
        record_uncomposed_auto_intent(
            dir.path(),
            "t1",
            "provider_unavailable",
            "connection refused",
            Some("https://selector.invalid".into()),
        )
        .unwrap();
        let intent = AdaptiveStore::open(dir.path())
            .unwrap()
            .assignment_intent("t1")
            .unwrap()
            .unwrap();
        assert!(matches!(
            intent.decision,
            AssignmentDecisionV1::Uncomposed { .. }
        ));
        assert_eq!(
            intent.failure.as_ref().unwrap().fallback,
            "direct-uncomposed-dispatch"
        );
        assert!(intent.selected_composition.is_none());

        setup_workgraph(dir.path(), vec![make_task("t1", "Provider fallback")]);
        let graph = load_graph(&graph_path(dir.path())).unwrap();
        let receipt = crate::commands::adaptive_agency::prepare_next_attempt_assignment(
            dir.path(),
            graph.get_task("t1").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            receipt.decision,
            AssignmentDecisionV1::Uncomposed { .. }
        ));
        assert_eq!(receipt.attempt_id, "attempt-0-1");
        assert_eq!(receipt.failure.unwrap().class, "provider_unavailable");
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

    // Assignment no longer creates synthetic `.assign-*` evaluations.
    // Quality arrives only as delayed reward from an independent terminal outcome.

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
