//! Edit command for modifying existing tasks

use anyhow::{Context, Result};
use std::path::Path;
use worksgood::config::{Config, DispatchRole, ReasoningLevel};
use worksgood::cycle::{EdgeAddResult, check_edge_addition};
use worksgood::graph::{CycleConfig, LogEntry, Status, parse_delay};
use worksgood::parser::modify_graph;
use worksgood::service::AgentRegistry;

use super::graph_path;

/// A read-only snapshot of the project policy that an unpinned task would use
/// if it were dispatched now. This is audit/display metadata only: the clear
/// command never copies these values onto the task.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RouteInheritancePreview {
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

pub(crate) fn current_route_inheritance(dir: &Path) -> Result<RouteInheritancePreview> {
    let association = worksgood::profile::project::read_association(dir)?;
    let profile = association
        .as_ref()
        .map(|selected| selected.profile.clone())
        .unwrap_or_else(|| "project-config (no selected profile)".to_string());
    let profile_generation = association
        .as_ref()
        .map(|selected| selected.profile_fingerprint.clone());
    let config = Config::load_merged(dir)
        .context("Cannot inspect the project's current route inheritance")?;
    let association_after = worksgood::profile::project::read_association(dir)?;
    if association != association_after {
        anyhow::bail!(
            "Project profile changed while route inheritance was being inspected. No task metadata was changed; run the command again."
        );
    }

    Ok(
        match config.resolve_execution_route_for_role(DispatchRole::TaskAgent) {
            Ok(route) => RouteInheritancePreview {
                profile,
                profile_generation,
                route: Some(route.route),
                handler: Some(route.handler),
                reasoning: Some(route.reasoning.to_string()),
                source: Some(route.source),
                unavailable_reason: None,
            },
            Err(error) => RouteInheritancePreview {
                profile,
                profile_generation,
                route: None,
                handler: None,
                reasoning: None,
                source: None,
                unavailable_reason: Some(error.to_string()),
            },
        },
    )
}

#[derive(Debug, Clone)]
struct ActiveAttemptRoute {
    agent_id: String,
    executor: String,
    model: String,
}

fn recorded_active_attempt(dir: &Path, task_id: &str) -> Result<Option<ActiveAttemptRoute>> {
    let graph = worksgood::parser::load_graph(&graph_path(dir)).context("Failed to load graph")?;
    let task = graph
        .get_task(task_id)
        .ok_or_else(|| anyhow::anyhow!("Task '{}' not found", task_id))?;
    if task.status != Status::InProgress {
        return Ok(None);
    }

    let agent_id = task.assigned.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "Task '{}' is in-progress but has no recorded assigned attempt. Route-pin clear refused because the active route/session cannot be preserved safely.",
            task_id
        )
    })?;
    let registry = AgentRegistry::load(dir)
        .context("Cannot read the active agent registry; route-pin clear made no changes")?;
    let entry = registry.agents.get(agent_id).ok_or_else(|| {
        anyhow::anyhow!(
            "Task '{}' is in-progress but assigned attempt '{}' is absent from the agent registry. Route-pin clear refused because the active route/session cannot be preserved safely.",
            task_id,
            agent_id
        )
    })?;
    if entry.task_id != task_id {
        anyhow::bail!(
            "Task '{}' points at attempt '{}', but the registry records task '{}'. Route-pin clear refused because the active route/session cannot be preserved safely.",
            task_id,
            agent_id,
            entry.task_id
        );
    }
    let model = entry.model.clone().filter(|model| !model.trim().is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "Task '{}' active attempt '{}' has no recorded actual model. Route-pin clear refused because clearing task.model would erase the only exact route record.",
            task_id,
            agent_id
        )
    })?;

    Ok(Some(ActiveAttemptRoute {
        agent_id: agent_id.clone(),
        executor: entry.executor.clone(),
        model,
    }))
}

/// Edit a task's fields
#[allow(clippy::too_many_arguments)]
pub fn run(
    dir: &Path,
    task_id: &str,
    title: Option<&str>,
    description: Option<&str>,
    add_after: &[String],
    remove_after: &[String],
    add_tag: &[String],
    remove_tag: &[String],
    model: Option<&str>,
    provider: Option<&str>,
    add_skill: &[String],
    remove_skill: &[String],
    max_iterations: Option<u32>,
    cycle_guard: Option<&str>,
    cycle_delay: Option<&str>,
    no_converge: bool,
    no_restart_on_failure: bool,
    max_failure_restarts: Option<u32>,
    visibility: Option<&str>,
    context_scope: Option<&str>,
    exec_mode: Option<&str>,
    delay: Option<&str>,
    not_before: Option<&str>,
    verify: Option<&str>,
    cron: Option<&str>,
    timeout: Option<&str>,
    verify_timeout: Option<&str>,
    allow_phantom: bool,
    allow_cycle: bool,
) -> Result<()> {
    run_with_reasoning(
        dir,
        task_id,
        title,
        description,
        add_after,
        remove_after,
        add_tag,
        remove_tag,
        model,
        None,
        provider,
        add_skill,
        remove_skill,
        max_iterations,
        cycle_guard,
        cycle_delay,
        no_converge,
        no_restart_on_failure,
        max_failure_restarts,
        visibility,
        context_scope,
        exec_mode,
        delay,
        not_before,
        verify,
        cron,
        timeout,
        verify_timeout,
        allow_phantom,
        allow_cycle,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_with_reasoning(
    dir: &Path,
    task_id: &str,
    title: Option<&str>,
    description: Option<&str>,
    add_after: &[String],
    remove_after: &[String],
    add_tag: &[String],
    remove_tag: &[String],
    model: Option<&str>,
    reasoning: Option<&str>,
    provider: Option<&str>,
    add_skill: &[String],
    remove_skill: &[String],
    max_iterations: Option<u32>,
    cycle_guard: Option<&str>,
    cycle_delay: Option<&str>,
    no_converge: bool,
    no_restart_on_failure: bool,
    max_failure_restarts: Option<u32>,
    visibility: Option<&str>,
    context_scope: Option<&str>,
    exec_mode: Option<&str>,
    delay: Option<&str>,
    not_before: Option<&str>,
    verify: Option<&str>,
    cron: Option<&str>,
    timeout: Option<&str>,
    verify_timeout: Option<&str>,
    allow_phantom: bool,
    allow_cycle: bool,
) -> Result<()> {
    run_with_reasoning_and_route_clear(
        dir,
        task_id,
        title,
        description,
        add_after,
        remove_after,
        add_tag,
        remove_tag,
        model,
        reasoning,
        provider,
        add_skill,
        remove_skill,
        max_iterations,
        cycle_guard,
        cycle_delay,
        no_converge,
        no_restart_on_failure,
        max_failure_restarts,
        visibility,
        context_scope,
        exec_mode,
        delay,
        not_before,
        verify,
        cron,
        timeout,
        verify_timeout,
        allow_phantom,
        allow_cycle,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_with_reasoning_and_route_clear(
    dir: &Path,
    task_id: &str,
    title: Option<&str>,
    description: Option<&str>,
    add_after: &[String],
    remove_after: &[String],
    add_tag: &[String],
    remove_tag: &[String],
    model: Option<&str>,
    reasoning: Option<&str>,
    provider: Option<&str>,
    add_skill: &[String],
    remove_skill: &[String],
    max_iterations: Option<u32>,
    cycle_guard: Option<&str>,
    cycle_delay: Option<&str>,
    no_converge: bool,
    no_restart_on_failure: bool,
    max_failure_restarts: Option<u32>,
    visibility: Option<&str>,
    context_scope: Option<&str>,
    exec_mode: Option<&str>,
    delay: Option<&str>,
    not_before: Option<&str>,
    verify: Option<&str>,
    cron: Option<&str>,
    timeout: Option<&str>,
    verify_timeout: Option<&str>,
    allow_phantom: bool,
    allow_cycle: bool,
    clear_route_pin: bool,
) -> Result<()> {
    let path = graph_path(dir);

    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // Validate self-blocking (can be done before loading graph)
    for dep in add_after {
        if dep == task_id {
            anyhow::bail!("Task '{}' cannot block itself", task_id);
        }
    }

    if provider.is_some() {
        anyhow::bail!(
            "WG-PI-ROUTE-REQUIRED: --provider is unsupported; use --model pi:<provider>:<model>"
        );
    }

    if let Some(model) = model {
        worksgood::config::parse_supported_execution_route(model).with_context(|| {
            format!(
                "WG-EXEC-ROUTE-REQUIRED: task model must be `pi:<provider>:<model>`, `claude:<native-model>`, or `codex:<native-model>`, got {model:?}"
            )
        })?;
    }
    let parsed_reasoning = reasoning
        .map(str::parse::<ReasoningLevel>)
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if clear_route_pin && (model.is_some() || reasoning.is_some() || provider.is_some()) {
        anyhow::bail!(
            "--clear-route-pin conflicts with --model, --reasoning, and --provider; clearing means dynamic profile inheritance, not writing a replacement pin"
        );
    }

    let inheritance_preview = clear_route_pin
        .then(|| current_route_inheritance(dir))
        .transpose()?;
    let active_attempt_route = if clear_route_pin {
        recorded_active_attempt(dir, task_id)?
    } else {
        None
    };
    let has_regular_edit = title.is_some()
        || description.is_some()
        || !add_after.is_empty()
        || !remove_after.is_empty()
        || !add_tag.is_empty()
        || !remove_tag.is_empty()
        || model.is_some()
        || reasoning.is_some()
        || provider.is_some()
        || !add_skill.is_empty()
        || !remove_skill.is_empty()
        || max_iterations.is_some()
        || cycle_guard.is_some()
        || cycle_delay.is_some()
        || no_converge
        || no_restart_on_failure
        || max_failure_restarts.is_some()
        || visibility.is_some()
        || context_scope.is_some()
        || exec_mode.is_some()
        || delay.is_some()
        || not_before.is_some()
        || verify.is_some()
        || cron.is_some()
        || timeout.is_some()
        || verify_timeout.is_some();

    let mut changed = false;
    let mut field_changes: Vec<serde_json::Value> = Vec::new();
    let mut cleared_route_fields: Vec<String> = Vec::new();
    let mut error: Option<anyhow::Error> = None;

    modify_graph(&path, |graph| {

    // Validate task exists
    if graph.get_task(task_id).is_none() {
        error = Some(anyhow::anyhow!("Task '{}' not found", task_id));
        return false;
    }

    // Validate add-after dependencies before taking mutable borrow (phantom edge prevention)
    if !allow_phantom {
        for dep in add_after {
            if worksgood::federation::parse_remote_ref(dep).is_some() {
                continue;
            }
            if graph.get_node(dep).is_none() {
                let mut msg = format!("Dependency '{}' does not exist.", dep);
                let all_ids: Vec<&str> = graph.tasks().map(|t| t.id.as_str()).collect();
                if let Some((suggestion, _)) =
                    worksgood::check::fuzzy_match_task_id(dep, all_ids.iter().copied(), 3)
                {
                    msg.push_str(&format!("\n  → Did you mean '{}'?", suggestion));
                }
                msg.push_str(
                    "\n  Hint: Use --allow-phantom to allow forward references.",
                );
                error = Some(anyhow::anyhow!("{}", msg));
                return false;
            }
        }
    }

    // Check for cycles before adding dependencies (unless allow_cycle is set)
    if !allow_cycle && !add_after.is_empty() {
        // Build adjacency list for cycle detection
        let task_ids: Vec<String> = graph.tasks().map(|t| t.id.clone()).collect();
        let mut task_id_to_index = std::collections::HashMap::new();
        for (i, id) in task_ids.iter().enumerate() {
            task_id_to_index.insert(id, i);
        }

        let mut adjacency_list = vec![Vec::new(); task_ids.len()];
        for task in graph.tasks() {
            if let Some(&task_idx) = task_id_to_index.get(&task.id) {
                for dep_id in &task.after {
                    if let Some(&dep_idx) = task_id_to_index.get(dep_id) {
                        adjacency_list[dep_idx].push(task_idx);
                    }
                }
            }
        }

        // Get the current task's dependencies to check what would actually be added
        let current_after = graph.get_task(task_id)
            .map(|t| &t.after)
            .unwrap_or(&vec![])
            .clone();

        // Check each new dependency for cycle creation
        let task_id_string = task_id.to_string();
        if let Some(&task_idx) = task_id_to_index.get(&task_id_string) {
            for dep in add_after {
                if !current_after.contains(dep)
                    && let Some(&dep_idx) = task_id_to_index.get(dep)
                {
                        match check_edge_addition(task_ids.len(), &adjacency_list, dep_idx, task_idx) {
                            EdgeAddResult::CreatesCycle { cycle_members } => {
                                // Check if the cycle would have CycleConfig
                                let has_cycle_config = cycle_members.iter()
                                    .filter_map(|&idx| task_ids.get(idx))
                                    .any(|cycle_task_id| {
                                        // Check if max_iterations will be set on this task
                                        if cycle_task_id == task_id && max_iterations.is_some() {
                                            return true;
                                        }
                                        graph.get_task(cycle_task_id)
                                            .map(|t| t.cycle_config.is_some())
                                            .unwrap_or(false)
                                    });

                                if !has_cycle_config && !allow_cycle {
                                    let cycle_task_names: Vec<String> = cycle_members.iter()
                                        .filter_map(|&idx| task_ids.get(idx))
                                        .cloned()
                                        .collect();
                                    error = Some(anyhow::anyhow!(
                                        "Adding dependency '{}' → '{}' would create a cycle without CycleConfig: [{}]. \
                                         Use --allow-cycle to override, or add --max-iterations to one of the cycle members.",
                                        dep, task_id, cycle_task_names.join(" → ")
                                    ));
                                    return false;
                                }
                            }
                            EdgeAddResult::NoCycle => {
                                // Safe to add - no action needed
                            }
                        }
                    }
            }
        }
    }

    // Modify the task in a block so the mutable borrow is released afterwards
    {
        let task = match graph.get_task_mut(task_id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id));
                return false;
            }
        };

        // Clearing a live task is safe only when its immutable actual route is
        // already represented by the assigned registry attempt. The process is
        // left untouched; only metadata consulted by a later spawn is cleared.
        if clear_route_pin && task.status == Status::InProgress {
            match active_attempt_route.as_ref() {
                Some(active) if task.assigned.as_deref() == Some(&active.agent_id) => {}
                Some(active) => {
                    error = Some(anyhow::anyhow!(
                        "Task '{}' changed active attempts while --clear-route-pin was running (expected '{}', found {:?}). Nothing was cleared; run the command again.",
                        task_id,
                        active.agent_id,
                        task.assigned
                    ));
                    return false;
                }
                None => {
                    error = Some(anyhow::anyhow!(
                        "Task '{}' became in-progress while --clear-route-pin was running, but its actual route was not recorded. Nothing was cleared; run the command again.",
                        task_id
                    ));
                    return false;
                }
            }
        }

        // Update title
        if let Some(new_title) = title {
            let old = task.title.clone();
            task.title = new_title.to_string();
            field_changes.push(serde_json::json!({"field": "title", "old": old, "new": new_title}));
            println!("Updated title: {}", new_title);
            changed = true;
        }

        // Update description
        if let Some(new_description) = description {
            let old = task.description.clone();
            task.description = Some(new_description.to_string());
            field_changes.push(
                serde_json::json!({"field": "description", "old": old, "new": new_description}),
            );
            println!("Updated description");
            changed = true;
        }

        // Add after dependencies (already validated above)
        for dep in add_after {
            if !task.after.contains(dep) {
                task.after.push(dep.clone());
                println!("Added after: {}", dep);
                changed = true;
            } else {
                println!("Already blocked by: {}", dep);
            }
        }

        // Remove after dependencies
        for dep in remove_after {
            if let Some(pos) = task.after.iter().position(|x| x == dep) {
                task.after.remove(pos);
                println!("Removed after: {}", dep);
                changed = true;
            } else {
                println!("Not blocked by: {}", dep);
            }
        }

        // Add tags
        for tag in add_tag {
            if !task.tags.contains(tag) {
                task.tags.push(tag.clone());
                println!("Added tag: {}", tag);
                changed = true;
            } else {
                println!("Already has tag: {}", tag);
            }
        }

        // Remove tags
        for tag in remove_tag {
            if let Some(pos) = task.tags.iter().position(|x| x == tag) {
                task.tags.remove(pos);
                println!("Removed tag: {}", tag);
                changed = true;
            } else {
                println!("Does not have tag: {}", tag);
            }
        }

        // Update model
        if let Some(new_model) = model {
            task.model = Some(new_model.to_string());
            println!("Updated model: {}", new_model);
            changed = true;
        }

        if let Some(parsed) = parsed_reasoning {
            task.reasoning = Some(parsed);
            println!("Updated reasoning: {}", parsed);
            changed = true;
        }

        // Update provider
        if let Some(new_provider) = provider {
            task.provider = Some(new_provider.to_string());
            println!("Updated provider: {}", new_provider);
            changed = true;
        }

        if clear_route_pin {
            let prior_session_id = task.session_id.clone();
            // These are every task-level selector consulted by worker route
            // planning. Clear them together under graph.lock. Historical
            // runtime/accounting fields live elsewhere and are not touched.
            macro_rules! clear_route_field {
                ($field:ident) => {
                    if task.$field.is_some() {
                        let old = serde_json::to_value(&task.$field).unwrap_or(serde_json::Value::Null);
                        task.$field = None;
                        cleared_route_fields.push(stringify!($field).to_string());
                        field_changes.push(serde_json::json!({
                            "field": stringify!($field),
                            "old": old,
                            "new": null,
                        }));
                    }
                };
            }
            clear_route_field!(model);
            clear_route_field!(reasoning);
            clear_route_field!(provider);
            clear_route_field!(endpoint);
            clear_route_field!(profile);
            clear_route_field!(tier);
            clear_route_field!(session_id);

            let preview = inheritance_preview
                .as_ref()
                .expect("clear-route-pin always resolves an inheritance preview");
            let generation = preview.profile_generation.as_deref().unwrap_or("none");
            let current_route = preview.route.as_deref().unwrap_or("unconfigured");
            let current_handler = preview.handler.as_deref().unwrap_or("none");
            let current_reasoning = preview.reasoning.as_deref().unwrap_or("unconfigured");
            let cleared = if cleared_route_fields.is_empty() {
                "none (already unpinned)".to_string()
            } else {
                cleared_route_fields.join(",")
            };
            let active = active_attempt_route
                .as_ref()
                .filter(|_| task.status == Status::InProgress)
                .map(|attempt| {
                    format!(
                        "; active_attempt_agent={} actual_executor={} actual_model={} actual_session={} preserved in attempt registry/audit only",
                        attempt.agent_id,
                        attempt.executor,
                        attempt.model,
                        prior_session_id
                            .as_deref()
                            .unwrap_or("pending-stream-capture")
                    )
                })
                .unwrap_or_default();
            task.log.push(LogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                actor: Some("clear-route-pin".to_string()),
                user: Some(worksgood::current_user()),
                message: format!(
                    "Future route pin cleared atomically: cleared_fields=[{}]; considered_fields=[model,reasoning,provider,endpoint,profile,tier,session_id]; inheritance=dynamic-at-dispatch; current_profile={} generation={} currently_resolves handler={} model={} reasoning={}{}. Unlike `wg retry --current-profile`, no route snapshot was written and task status/attempt history were unchanged.",
                    cleared,
                    preview.profile,
                    generation,
                    current_handler,
                    current_route,
                    current_reasoning,
                    active
                ),
            });
            println!("Cleared future route pin fields: {}", cleared);
            println!(
                "Dynamic inheritance (not pinned): profile={} generation={} currently resolves handler={} model={} reasoning={}",
                preview.profile,
                generation,
                current_handler,
                current_route,
                current_reasoning
            );
            changed = true;
        }

        if let Some(command) = verify {
            let command = command.trim();
            if command.len() > 16 * 1024 {
                error = Some(anyhow::anyhow!(
                    "--validation-command exceeds the 16384-byte bound"
                ));
                return false;
            }
            // Editing the public setting also consumes the singular legacy
            // `verify` spelling. Otherwise an old task could neither replace
            // nor clear that command through the supported CLI.
            let old = worksgood::completion_validation::configured_validation_commands(task);
            task.verify = None;
            task.validation_commands = if command.is_empty() {
                Vec::new()
            } else {
                eprintln!(
                    "Warning: --validation-command creates an authoritative hard gate. \
                     Prefer ## Validation for agent-selected checks; use this only when the operator \
                     explicitly requested the exact command or checked-in repository policy names it."
                );
                vec![command.to_string()]
            };
            field_changes.push(serde_json::json!({
                "field": "validation_commands",
                "old": old,
                "new": task.validation_commands,
            }));
            println!(
                "{} authoritative validation command",
                if command.is_empty() { "Cleared" } else { "Set" }
            );
            changed = true;
        }

        // Add skills
        for skill in add_skill {
            if !task.skills.contains(skill) {
                task.skills.push(skill.clone());
                println!("Added skill: {}", skill);
                changed = true;
            } else {
                println!("Already has skill: {}", skill);
            }
        }

        // Remove skills
        for skill in remove_skill {
            if let Some(pos) = task.skills.iter().position(|x| x == skill) {
                task.skills.remove(pos);
                println!("Removed skill: {}", skill);
                changed = true;
            } else {
                println!("Does not have skill: {}", skill);
            }
        }

        // Update cycle config
        if let Some(max_iter) = max_iterations {
            let guard = match cycle_guard {
                Some(expr) => match crate::commands::add::parse_guard_expr(expr) {
                    Ok(g) => Some(g),
                    Err(e) => {
                        error = Some(e);
                        return false;
                    }
                },
                None => task.cycle_config.as_ref().and_then(|c| c.guard.clone()),
            };
            let delay = match cycle_delay {
                Some(d) => {
                    if parse_delay(d).is_none() {
                        error = Some(anyhow::anyhow!(
                            "Invalid cycle delay '{}'. Use format: 30s, 5m, 1h, 24h, 7d",
                            d
                        ));
                        return false;
                    }
                    Some(d.to_string())
                }
                None => task.cycle_config.as_ref().and_then(|c| c.delay.clone()),
            };
            task.cycle_config = Some(CycleConfig {
                max_iterations: max_iter,
                guard,
                delay,
                no_converge,
                restart_on_failure: !no_restart_on_failure,
                max_failure_restarts,
            });
            println!(
                "Set cycle_config: max_iterations={}{}",
                max_iter,
                if no_converge { " (no-converge)" } else { "" }
            );
            changed = true;
        } else {
            // Allow updating guard/delay/no_converge on existing cycle config
            if let Some(expr) = cycle_guard {
                if let Some(ref mut config) = task.cycle_config {
                    config.guard = match crate::commands::add::parse_guard_expr(expr) {
                        Ok(g) => Some(g),
                        Err(e) => {
                            error = Some(e);
                            return false;
                        }
                    };
                    println!("Updated cycle guard");
                    changed = true;
                } else {
                    error = Some(anyhow::anyhow!(
                        "Cannot set --cycle-guard without --max-iterations: task has no cycle_config"
                    ));
                    return false;
                }
            }
            if let Some(d) = cycle_delay {
                if let Some(ref mut config) = task.cycle_config {
                    if parse_delay(d).is_none() {
                        error = Some(anyhow::anyhow!(
                            "Invalid cycle delay '{}'. Use format: 30s, 5m, 1h, 24h, 7d",
                            d
                        ));
                        return false;
                    }
                    config.delay = Some(d.to_string());
                    println!("Updated cycle delay: {}", d);
                    changed = true;
                } else {
                    error = Some(anyhow::anyhow!(
                        "Cannot set --cycle-delay without --max-iterations: task has no cycle_config"
                    ));
                    return false;
                }
            }
            if no_converge {
                if let Some(ref mut config) = task.cycle_config {
                    config.no_converge = true;
                    println!("Set no-converge on cycle");
                    changed = true;
                } else {
                    error = Some(anyhow::anyhow!(
                        "Cannot set --no-converge without --max-iterations: task has no cycle_config"
                    ));
                    return false;
                }
            }
            if no_restart_on_failure {
                if let Some(ref mut config) = task.cycle_config {
                    config.restart_on_failure = false;
                    println!("Disabled restart-on-failure for cycle");
                    changed = true;
                } else {
                    error = Some(anyhow::anyhow!(
                        "Cannot set --no-restart-on-failure without --max-iterations: task has no cycle_config"
                    ));
                    return false;
                }
            }
            if let Some(max) = max_failure_restarts {
                if let Some(ref mut config) = task.cycle_config {
                    config.max_failure_restarts = Some(max);
                    println!("Set max-failure-restarts: {}", max);
                    changed = true;
                } else {
                    error = Some(anyhow::anyhow!(
                        "Cannot set --max-failure-restarts without --max-iterations: task has no cycle_config"
                    ));
                    return false;
                }
            }
        }

        // Update visibility
        if let Some(vis) = visibility {
            match vis {
                "internal" | "public" | "peer" => {
                    let old = task.visibility.clone();
                    task.visibility = vis.to_string();
                    field_changes
                        .push(serde_json::json!({"field": "visibility", "old": old, "new": vis}));
                    println!("Updated visibility: {}", vis);
                    changed = true;
                }
                _ => {
                    error = Some(anyhow::anyhow!(
                        "Invalid visibility '{}'. Valid values: internal, public, peer",
                        vis
                    ));
                    return false;
                }
            }
        }

        // Update context scope
        if let Some(scope) = context_scope {
            // Validate
            if let Err(e) = scope.parse::<worksgood::context_scope::ContextScope>() {
                error = Some(anyhow::anyhow!("{}", e));
                return false;
            }
            let old = task.context_scope.clone();
            task.context_scope = Some(scope.to_string());
            field_changes
                .push(serde_json::json!({"field": "context_scope", "old": old, "new": scope}));
            println!("Updated context_scope: {}", scope);
            changed = true;
        }

        // Update exec mode
        if let Some(mode) = exec_mode {
            if let Err(e) = mode.parse::<worksgood::config::ExecMode>() {
                error = Some(anyhow::anyhow!("{}", e));
                return false;
            }
            let old = task.exec_mode.clone();
            task.exec_mode = Some(mode.to_string());
            field_changes.push(serde_json::json!({"field": "exec_mode", "old": old, "new": mode}));
            println!("Updated exec_mode: {}", mode);
            changed = true;
        }

        // Update not_before (from --delay or --not-before)
        if delay.is_some() && not_before.is_some() {
            error = Some(anyhow::anyhow!("Cannot specify both --delay and --not-before"));
            return false;
        }
        if let Some(d) = delay {
            let secs = match worksgood::graph::parse_delay(d) {
                Some(s) => s,
                None => {
                    error = Some(anyhow::anyhow!("Invalid delay '{}'. Use format: 30s, 5m, 1h, 24h, 7d", d));
                    return false;
                }
            };
            let new_ts = (chrono::Utc::now() + chrono::Duration::seconds(secs as i64)).to_rfc3339();
            let old = task.not_before.clone();
            task.not_before = Some(new_ts.clone());
            field_changes
                .push(serde_json::json!({"field": "not_before", "old": old, "new": new_ts}));
            println!("Set not_before: {} (delay {})", new_ts, d);
            changed = true;
        } else if let Some(ts) = not_before {
            if ts.parse::<chrono::DateTime<chrono::Utc>>().is_err()
                && chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S").is_err()
            {
                error = Some(anyhow::anyhow!("Invalid timestamp '{}'. Use ISO 8601 format", ts));
                return false;
            }
            let old = task.not_before.clone();
            task.not_before = Some(ts.to_string());
            field_changes.push(serde_json::json!({"field": "not_before", "old": old, "new": ts}));
            println!("Set not_before: {}", ts);
            changed = true;
        }
        // Update cron schedule
        if let Some(cron_expr) = cron {
            if cron_expr.is_empty() {
                // Clear cron scheduling
                task.cron_schedule = None;
                task.cron_enabled = false;
                task.next_cron_fire = None;
                task.last_cron_fire = None;
                println!("Cleared cron schedule");
                changed = true;
            } else {
                // Set or update cron schedule
                match worksgood::cron::parse_cron_expression(cron_expr) {
                    Ok(schedule) => {
                        task.cron_schedule = Some(cron_expr.to_string());
                        task.cron_enabled = true;
                        task.next_cron_fire = worksgood::cron::calculate_next_fire_with_jitter(
                            &task.id,
                            &schedule,
                            chrono::Utc::now(),
                        )
                        .map(|dt| dt.to_rfc3339());
                        println!(
                            "Set cron schedule: {} (next fire: {})",
                            cron_expr,
                            task.next_cron_fire.as_deref().unwrap_or("unknown")
                        );
                        changed = true;
                    }
                    Err(e) => {
                        error = Some(anyhow::anyhow!(
                            "Invalid cron expression '{}': {}",
                            cron_expr,
                            e
                        ));
                        return false;
                    }
                }
            }
        }

        // Set or clear the per-task worker timeout. An empty string clears
        // the field (recovering a task stuck on a stale/bad timeout value); a
        // non-empty value is validated with `parse_delay`, the same parser
        // `wg add --timeout` uses, so an accepted-at-add-time value is never
        // rejected here.
        if let Some(t) = timeout {
            if t.is_empty() {
                let old = task.timeout.clone();
                task.timeout = None;
                field_changes
                    .push(serde_json::json!({"field": "timeout", "old": old, "new": null}));
                println!("Cleared timeout (will fall back to executor/coordinator default)");
                changed = true;
            } else if parse_delay(t).is_none() {
                error = Some(anyhow::anyhow!(
                    "Invalid timeout '{}'. Use format: 30s, 5m, 1h, 4h, 1d (or empty string to clear)",
                    t
                ));
                return false;
            } else {
                let old = task.timeout.clone();
                task.timeout = Some(t.to_string());
                field_changes
                    .push(serde_json::json!({"field": "timeout", "old": old, "new": t}));
                println!("Set timeout: {}", t);
                changed = true;
            }
        }

        // Set or clear the per-task verify timeout override (same empty-clear
        // semantics as --timeout). Used by the `wg done` verify gate.
        if let Some(t) = verify_timeout {
            if t.is_empty() {
                let old = task.verify_timeout.clone();
                task.verify_timeout = None;
                field_changes.push(
                    serde_json::json!({"field": "verify_timeout", "old": old, "new": null}),
                );
                println!("Cleared verify_timeout (will fall back to coordinator default)");
                changed = true;
            } else if parse_delay(t).is_none() {
                error = Some(anyhow::anyhow!(
                    "Invalid verify_timeout '{}'. Use format: 30s, 5m, 1h, 4h, 1d (or empty string to clear)",
                    t
                ));
                return false;
            } else {
                let old = task.verify_timeout.clone();
                task.verify_timeout = Some(t.to_string());
                field_changes.push(
                    serde_json::json!({"field": "verify_timeout", "old": old, "new": t}),
                );
                println!("Set verify_timeout: {}", t);
                changed = true;
            }
        }

        // Reset spawn failure counter on any edit — the user may have fixed
        // the root cause (e.g., exec_mode mismatch), so the circuit breaker
        // should give the task a fresh set of attempts.
        if changed && task.spawn_failures > 0 && (!clear_route_pin || has_regular_edit) {
            task.spawn_failures = 0;
            println!("Reset spawn failure counter");
        }
    } // task borrow released here

    // When new dependencies are added, clear any selected agent metadata.
    // This prevents the race where a task gets assigned before its real
    // dependencies are wired (e.g., `wg add` then `wg edit --add-after`).
    if !add_after.is_empty() && changed {
        // Clear the agent field so the task gets re-assigned when actually ready
        let task = match graph.get_task_mut(task_id) {
            Some(t) => t,
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", task_id));
                return false;
            }
        };
        if task.agent.is_some() {
            task.agent = None;
            println!("Cleared agent assignment (dependencies changed, will re-assign when ready)");
        }
    }

    // Maintain bidirectional consistency: update `blocks` on referenced tasks
    let task_id_owned = task_id.to_string();
    for dep in add_after {
        if let Some(blocker) = graph.get_task_mut(dep)
            && !blocker.before.contains(&task_id_owned)
        {
            blocker.before.push(task_id_owned.clone());
        }
    }
    for dep in remove_after {
        if let Some(blocker) = graph.get_task_mut(dep) {
            blocker.before.retain(|b| b != &task_id_owned);
        }
    }

    // Return whether changes were made
    changed
    })
    .context("Failed to modify graph")?;
    if let Some(e) = error {
        return Err(e);
    }

    if changed {
        super::notify_graph_changed(dir);

        // Record operation
        let config = worksgood::config::Config::load_or_default(dir);
        let _ = worksgood::provenance::record(
            dir,
            "edit",
            Some(task_id),
            None,
            serde_json::json!({
                "fields": field_changes,
                "clear_route_pin": clear_route_pin.then(|| serde_json::json!({
                    "cleared_fields": cleared_route_fields,
                    "inheritance": inheritance_preview,
                    "dynamic_at_dispatch": true,
                    "snapshotted": false,
                })),
            }),
            config.log.rotation_threshold,
        );

        println!("\nTask '{}' updated successfully", task_id);
    } else {
        println!("No changes made to task '{}'", task_id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use worksgood::parser::{load_graph, save_graph};

    fn create_test_graph(dir: &Path) -> Result<()> {
        // Create the WG directory if it doesn't exist
        fs::create_dir_all(dir)?;

        // Create an empty graph.jsonl file
        let graph_path = graph_path(dir);
        fs::write(&graph_path, "")?;

        // Add a test task using the add command
        crate::commands::add::run(
            dir,
            "Test Task",
            Some("test-task"),
            Some("Original description"),
            &["dep1".to_string()],
            None,
            None,
            None,
            &["tag1".to_string()],
            &["skill1".to_string()],
            &[],
            &[],
            None,
            Some("claude:sonnet"),
            None,
            None, // verify
            None, // verify_timeout
            None, // validation
            None, // validator_agent
            None, // validator_model
            None,
            None,
            None,
            false,
            false,
            None,
            "internal",
            None,
            None,
            None,
            None,
            false,
            false,
            &[],
            &[],
            None,
            None,
            true,  // allow_phantom: test graph uses phantom deps
            false, // independent
            false, // no_tier_escalation
            None,  // iteration_config
            None,  // priority
            None,  // cron
            false, // subtask
        )?;

        Ok(())
    }

    fn create_test_graph_with_two_tasks(dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        let graph_path = graph_path(dir);
        fs::write(&graph_path, "")?;

        // Add two independent tasks (no initial dependency between them)
        crate::commands::add::run(
            dir,
            "Blocker Task",
            Some("blocker-task"),
            None,
            &[],
            None,
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            None, // verify
            None, // verify_timeout
            None, // validation
            None, // validator_agent
            None, // validator_model
            None,
            None,
            None,
            false,
            false,
            None,
            "internal",
            None,
            None,
            None,
            None,
            false,
            false,
            &[],
            &[],
            None,
            None,
            false,
            false,
            false, // no_tier_escalation
            None,  // iteration_config
            None,  // priority
            None,  // cron
            false, // subtask
        )?;

        crate::commands::add::run(
            dir,
            "Test Task",
            Some("test-task"),
            Some("Original description"),
            &[],
            None,
            None,
            None,
            &["tag1".to_string()],
            &["skill1".to_string()],
            &[],
            &[],
            None,
            Some("claude:sonnet"),
            None,
            None, // verify
            None, // verify_timeout
            None, // validation
            None, // validator_agent
            None, // validator_model
            None,
            None,
            None,
            false,
            false,
            None,
            "internal",
            None,
            None,
            None,
            None,
            false,
            false,
            &[],
            &[],
            None,
            None,
            false,
            false,
            false, // no_tier_escalation
            None,
            None,  // priority
            None,  // cron
            false, // subtask
        )?;

        Ok(())
    }

    #[test]
    fn test_edit_title() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            Some("New Title"),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert_eq!(task.title, "New Title");
    }

    #[test]
    fn test_edit_description() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            Some("New description"),
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert_eq!(task.description, Some("New description".to_string()));
    }

    #[test]
    fn test_add_after() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["dep2".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,  // cron
            None,  // timeout
            None,  // verify_timeout
            true,  // allow_phantom: dep2 doesn't exist in test graph
            false, // allow_cycle: tests should not allow cycles by default
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(task.after.contains(&"dep2".to_string()));
        assert!(task.after.contains(&"dep1".to_string()));
    }

    #[test]
    fn test_remove_after() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &["dep1".to_string()],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(!task.after.contains(&"dep1".to_string()));
    }

    #[test]
    fn test_add_tag() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &["tag2".to_string()],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(task.tags.contains(&"tag2".to_string()));
        assert!(task.tags.contains(&"tag1".to_string()));
    }

    #[test]
    fn test_remove_tag() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &[],
            &["tag1".to_string()],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(!task.tags.contains(&"tag1".to_string()));
    }

    #[test]
    fn test_edit_model() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            Some("claude:opus"),
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert_eq!(task.model, Some("claude:opus".to_string()));
    }

    #[test]
    fn test_add_skill() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &["skill2".to_string()],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(task.skills.contains(&"skill2".to_string()));
        assert!(task.skills.contains(&"skill1".to_string()));
    }

    #[test]
    fn test_remove_skill() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &["skill1".to_string()],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(!task.skills.contains(&"skill1".to_string()));
    }

    #[test]
    fn test_task_not_found() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "nonexistent-task",
            Some("New Title"),
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_no_changes() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_self_blocking_rejected() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph(temp_dir.path()).unwrap();

        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["test-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot block itself")
        );
    }

    #[test]
    fn test_add_after_updates_blocker_blocks() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph_with_two_tasks(temp_dir.path()).unwrap();

        // Add a new after edge
        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();

        // Verify bidirectional consistency
        let blocker = graph.get_task("blocker-task").unwrap();
        assert!(
            blocker.before.contains(&"test-task".to_string()),
            "blocker-task.before should contain test-task"
        );
    }

    #[test]
    fn test_remove_after_updates_blocker_blocks() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph_with_two_tasks(temp_dir.path()).unwrap();

        // First add the dependency, then remove it
        run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        )
        .unwrap();

        // Remove the after edge
        let result = run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &[],
            &["blocker-task".to_string()],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());

        let path = graph_path(temp_dir.path());
        let graph = load_graph(&path).unwrap();

        // Verify bidirectional consistency
        let blocker = graph.get_task("blocker-task").unwrap();
        assert!(
            !blocker.before.contains(&"test-task".to_string()),
            "blocker-task.before should NOT contain test-task after removal"
        );
    }

    #[test]
    fn test_add_after_clears_agent_assignment() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph_with_two_tasks(temp_dir.path()).unwrap();

        // Set an agent on test-task
        let path = graph_path(temp_dir.path());
        {
            let mut graph = load_graph(&path).unwrap();
            let task = graph.get_task_mut("test-task").unwrap();
            task.agent = Some("some-agent-hash".to_string());
            save_graph(&graph, &path).unwrap();
        }

        // Add a new dependency
        run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        )
        .unwrap();

        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(
            task.agent.is_none(),
            "agent should be cleared when new dependencies are added"
        );
        assert!(
            task.after.contains(&"blocker-task".to_string()),
            "dependency should be added"
        );
    }

    #[test]
    fn test_add_after_does_not_rewrite_legacy_assignment_row() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph_with_two_tasks(temp_dir.path()).unwrap();

        // Create an assign task for test-task
        let path = graph_path(temp_dir.path());
        {
            let mut graph = load_graph(&path).unwrap();
            let assign_task = worksgood::graph::Task {
                id: "assign-test-task".to_string(),
                title: "Assign agent for: Test Task".to_string(),
                status: worksgood::graph::Status::Open,
                tags: vec!["assignment".to_string()],
                before: vec!["test-task".to_string()],
                ..worksgood::graph::Task::default()
            };
            graph.add_node(worksgood::graph::Node::Task(assign_task));
            save_graph(&graph, &path).unwrap();
        }

        // Add a new dependency
        run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        )
        .unwrap();

        let graph = load_graph(&path).unwrap();
        let assign = graph.get_task("assign-test-task").unwrap();
        assert_eq!(assign.status, worksgood::graph::Status::Open);
        assert!(assign.lifecycle.audit.is_empty());
    }

    #[test]
    fn test_add_duplicate_dep_does_not_clear_agent() {
        let temp_dir = TempDir::new().unwrap();
        create_test_graph_with_two_tasks(temp_dir.path()).unwrap();

        // First add the dependency
        run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        )
        .unwrap();

        // Now set an agent
        let path = graph_path(temp_dir.path());
        {
            let mut graph = load_graph(&path).unwrap();
            let task = graph.get_task_mut("test-task").unwrap();
            task.agent = Some("some-agent-hash".to_string());
            save_graph(&graph, &path).unwrap();
        }

        // Try to add the same dep again (no actual new dep)
        run(
            temp_dir.path(),
            "test-task",
            None,
            None,
            &["blocker-task".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false,
        )
        .unwrap();

        // Agent should NOT be cleared since no new deps were actually added
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("test-task").unwrap();
        assert!(
            task.agent.is_some(),
            "agent should NOT be cleared when no new deps are actually added"
        );
    }

    #[test]
    fn test_cycle_detection_blocks_unconfigured_cycle() {
        use crate::commands::graph_path;
        use tempfile::TempDir;
        use worksgood::graph::{Node, Status, Task, WorkGraph};
        use worksgood::parser::save_graph;

        let temp_dir = TempDir::new().unwrap();
        let path = graph_path(temp_dir.path());

        // Create a simple graph with: task-a → task-b
        let mut graph = WorkGraph::new();

        let mut task_a = Task::default();
        task_a.id = "task-a".to_string();
        task_a.title = "Task A".to_string();
        task_a.status = Status::Open;

        let mut task_b = Task::default();
        task_b.id = "task-b".to_string();
        task_b.title = "Task B".to_string();
        task_b.status = Status::Open;
        task_b.after.push("task-a".to_string()); // task-b depends on task-a

        graph.add_node(Node::Task(task_a));
        graph.add_node(Node::Task(task_b));
        save_graph(&graph, &path).unwrap();

        // Try to add task-a -> task-b (would create cycle task-a -> task-b -> task-a)
        let result = run(
            temp_dir.path(),
            "task-a",
            None,
            None,
            &["task-b".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            false, // allow_cycle = false
        );

        // Should fail with cycle detection message
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("would create a cycle without CycleConfig"));
        assert!(error_msg.contains("--allow-cycle"));
    }

    #[test]
    fn test_cycle_detection_allows_with_flag() {
        use crate::commands::{graph_path, load_graph};
        use tempfile::TempDir;
        use worksgood::graph::{Node, Status, Task, WorkGraph};
        use worksgood::parser::save_graph;

        let temp_dir = TempDir::new().unwrap();
        let path = graph_path(temp_dir.path());

        // Create a simple graph with: task-a → task-b
        let mut graph = WorkGraph::new();

        let mut task_a = Task::default();
        task_a.id = "task-a".to_string();
        task_a.title = "Task A".to_string();
        task_a.status = Status::Open;

        let mut task_b = Task::default();
        task_b.id = "task-b".to_string();
        task_b.title = "Task B".to_string();
        task_b.status = Status::Open;
        task_b.after.push("task-a".to_string());

        graph.add_node(Node::Task(task_a));
        graph.add_node(Node::Task(task_b));
        save_graph(&graph, &path).unwrap();

        // Try to add task-a -> task-b with --allow-cycle
        let result = run(
            temp_dir.path(),
            "task-a",
            None,
            None,
            &["task-b".to_string()],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // cron
            None, // timeout
            None, // verify_timeout
            false,
            true, // allow_cycle = true
        );

        // Should succeed when allow_cycle is true
        assert!(result.is_ok());

        // Verify the cycle was actually created
        let graph = load_graph(&path).unwrap();
        let task_a = graph.get_task("task-a").unwrap();
        assert!(task_a.after.contains(&"task-b".to_string()));
    }

    /// Reproduction for bug-task-timeout-edit-publish-block: a task carrying
    /// a stale/hidden `timeout` field can be inspected, repaired, and cleared
    /// via `wg edit --timeout`/`--verify-timeout` instead of being abandoned.
    /// This exercises the exact recovery flow the user report said was missing.
    #[test]
    fn test_edit_timeout_set_clear_and_verify_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        fs::create_dir_all(dir).unwrap();
        let graph_path = graph_path(dir);
        fs::write(&graph_path, "").unwrap();

        // Create a task WITH a per-task timeout (the "hidden field" the user
        // could not previously repair).
        crate::commands::add::run(
            dir,
            "Stuck Task",
            Some("stuck-task"),
            None,
            &[],
            None,
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None, // max_retries
            None, // model
            None, // provider
            None, // verify
            None, // verify_timeout
            None,
            None,
            None, // validation, validator_agent, validator_model
            None,
            None,
            None, // max_iterations, cycle_guard, cycle_delay
            false,
            false,
            None,
            "internal",
            None,
            None,
            Some("4h"), // --timeout 4h
            None,       // exec_mode
            false,
            false, // paused, no_place
            &[],
            &[],
            None,
            None,
            false,
            false,
            false,
            None,  // iteration_config
            None,  // priority
            None,  // cron
            false, // subtask
        )
        .unwrap();

        // Confirm the hidden field landed and is visible via the graph.
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("stuck-task").unwrap();
        assert_eq!(task.timeout.as_deref(), Some("4h"));
        assert!(task.verify_timeout.is_none());

        // Edit the timeout to a new value.
        let result = run(
            dir,
            "stuck-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,        // verify
            None,        // cron
            Some("90m"), // --timeout 90m
            None,        // verify_timeout
            false,
            false,
        );
        assert!(result.is_ok());
        let graph = load_graph(&graph_path).unwrap();
        assert_eq!(
            graph.get_task("stuck-task").unwrap().timeout.as_deref(),
            Some("90m")
        );

        // Set verify_timeout too.
        let result = run(
            dir,
            "stuck-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,        // timeout unchanged
            Some("15m"), // --verify-timeout 15m
            false,
            false,
        );
        assert!(result.is_ok());
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("stuck-task").unwrap();
        assert_eq!(task.timeout.as_deref(), Some("90m"));
        assert_eq!(task.verify_timeout.as_deref(), Some("15m"));

        // RECOVERY: clear the stale timeout with an empty string. This is the
        // exact repair the user could not perform before — the task is recovered
        // in place, NOT abandoned/superseded.
        let result = run(
            dir,
            "stuck-task",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(""), // --timeout "" clears
            Some(""), // --verify-timeout "" clears
            false,
            false,
        );
        assert!(result.is_ok());
        let graph = load_graph(&graph_path).unwrap();
        let task = graph.get_task("stuck-task").unwrap();
        assert!(task.timeout.is_none(), "timeout should be cleared");
        assert!(
            task.verify_timeout.is_none(),
            "verify_timeout should be cleared"
        );
        // The task is still present and recoverable — not abandoned/superseded.
        assert_eq!(task.status, worksgood::graph::Status::Open);
        assert!(task.superseded_by.is_empty());
        assert!(task.supersedes.is_none());
    }

    fn clear_route_pin(dir: &Path, task_id: &str) -> Result<()> {
        run_with_reasoning_and_route_clear(
            dir,
            task_id,
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            true,
        )
    }

    #[test]
    fn test_clear_route_pin_is_atomic_and_preserves_history() {
        use worksgood::graph::{Node, Task, TokenUsage, WorkGraph};

        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        fs::create_dir_all(dir).unwrap();
        let path = graph_path(dir);
        let usage = TokenUsage {
            cost_usd: 1.25,
            input_tokens: 100,
            output_tokens: 20,
            cache_read_input_tokens: 5,
            cache_creation_input_tokens: 3,
        };
        let mut task = Task {
            id: "pinned".to_string(),
            title: "Pinned task".to_string(),
            status: Status::Failed,
            model: Some("pi:openrouter:old-model".to_string()),
            reasoning: Some(ReasoningLevel::Low),
            provider: Some("legacy-provider".to_string()),
            endpoint: Some("old-endpoint".to_string()),
            profile: Some("old-wcc-profile".to_string()),
            tier: Some("premium".to_string()),
            session_id: Some("route-specific-session".to_string()),
            token_usage: Some(usage.clone()),
            retry_count: 4,
            failure_reason: Some("historic failure".to_string()),
            ..Task::default()
        };
        task.log.push(LogEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            actor: Some("worker".to_string()),
            user: None,
            message: "historic attempt provenance".to_string(),
        });
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, &path).unwrap();

        clear_route_pin(dir, "pinned").unwrap();

        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("pinned").unwrap();
        assert!(task.model.is_none());
        assert!(task.reasoning.is_none());
        assert!(task.provider.is_none());
        assert!(task.endpoint.is_none());
        assert!(task.profile.is_none());
        assert!(task.tier.is_none());
        assert!(task.session_id.is_none());
        assert_eq!(task.status, Status::Failed);
        assert_eq!(task.retry_count, 4);
        assert_eq!(task.failure_reason.as_deref(), Some("historic failure"));
        assert_eq!(task.token_usage.as_ref(), Some(&usage));
        assert!(
            task.log
                .iter()
                .any(|entry| entry.message == "historic attempt provenance")
        );
        let audit = task
            .log
            .iter()
            .find(|entry| entry.actor.as_deref() == Some("clear-route-pin"))
            .expect("clear must be auditable");
        for field in [
            "model",
            "reasoning",
            "provider",
            "endpoint",
            "profile",
            "tier",
            "session_id",
        ] {
            assert!(audit.message.contains(field), "audit omitted {field}");
        }
        assert!(audit.message.contains("dynamic-at-dispatch"));
        assert!(audit.message.contains("no route snapshot was written"));
    }

    #[test]
    fn test_clear_route_pin_preserves_in_progress_actual_route_and_session() {
        use worksgood::graph::{Node, Task, WorkGraph};
        use worksgood::service::AgentRegistry;

        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        fs::create_dir_all(dir).unwrap();
        let path = graph_path(dir);
        let mut registry = AgentRegistry::new();
        let agent_id = registry.register_agent_with_model(
            std::process::id(),
            "live",
            "pi",
            "/tmp/live/output.log",
            Some("openrouter:actual-model"),
        );
        registry.save(dir).unwrap();
        let task = Task {
            id: "live".to_string(),
            title: "Live task".to_string(),
            status: Status::InProgress,
            assigned: Some(agent_id.clone()),
            model: Some("pi:openrouter:pinned-model".to_string()),
            reasoning: Some(ReasoningLevel::High),
            session_id: Some("active-session".to_string()),
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, &path).unwrap();

        clear_route_pin(dir, "live").unwrap();

        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("live").unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.assigned.as_deref(), Some(agent_id.as_str()));
        assert!(task.model.is_none());
        assert!(task.reasoning.is_none());
        assert!(task.session_id.is_none());
        let audit = task
            .log
            .iter()
            .find(|entry| entry.actor.as_deref() == Some("clear-route-pin"))
            .unwrap();
        assert!(
            audit
                .message
                .contains(&format!("active_attempt_agent={agent_id}"))
        );
        assert!(audit.message.contains("actual_executor=pi"));
        assert!(
            audit
                .message
                .contains("actual_model=openrouter:actual-model")
        );
        assert!(audit.message.contains("actual_session=active-session"));
        let entry = AgentRegistry::load(dir)
            .unwrap()
            .agents
            .remove(&agent_id)
            .unwrap();
        assert_eq!(entry.executor, "pi");
        assert_eq!(entry.model.as_deref(), Some("openrouter:actual-model"));
    }

    #[test]
    fn test_clear_route_pin_fails_closed_for_unrecorded_in_progress_attempt() {
        use worksgood::graph::{Node, Task, WorkGraph};

        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        fs::create_dir_all(dir).unwrap();
        let path = graph_path(dir);
        let task = Task {
            id: "unsafe-live".to_string(),
            title: "Unsafe live task".to_string(),
            status: Status::InProgress,
            assigned: Some("missing-agent".to_string()),
            model: Some("pi:openrouter:must-remain".to_string()),
            session_id: Some("must-remain".to_string()),
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(task));
        save_graph(&graph, &path).unwrap();

        let error = clear_route_pin(dir, "unsafe-live").unwrap_err().to_string();
        assert!(error.contains("refused"));
        let graph = load_graph(&path).unwrap();
        let task = graph.get_task("unsafe-live").unwrap();
        assert_eq!(task.model.as_deref(), Some("pi:openrouter:must-remain"));
        assert_eq!(task.session_id.as_deref(), Some("must-remain"));
    }

    /// An invalid timeout value must be rejected with a message naming the
    /// field and the accepted format — no silent corruption of the field.
    #[test]
    fn test_edit_timeout_rejects_invalid_value() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        fs::create_dir_all(dir).unwrap();
        let graph_path = graph_path(dir);
        fs::write(&graph_path, "").unwrap();

        crate::commands::add::run(
            dir,
            "Task",
            Some("task-x"),
            None,
            &[],
            None,
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None, // max_retries
            None, // model
            None, // provider
            None, // verify
            None, // verify_timeout
            None,
            None,
            None, // validation, validator_agent, validator_model
            None,
            None,
            None, // max_iterations, cycle_guard, cycle_delay
            false,
            false,
            None,
            "internal",
            None,
            None,
            None, // timeout
            None, // exec_mode
            false,
            false, // paused, no_place
            &[],
            &[],
            None,
            None,
            false,
            false,
            false,
            None,  // iteration_config
            None,  // priority
            None,  // cron
            false, // subtask
        )
        .unwrap();

        let result = run(
            dir,
            "task-x",
            None,
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            &[],
            &[],
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("not-a-duration"), // invalid
            None,
            false,
            false,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timeout"),
            "error must name the timeout field: {err}"
        );
        assert!(
            err.contains("empty string to clear"),
            "error must mention the clear escape hatch: {err}"
        );

        // The invalid edit must NOT have corrupted the field.
        let graph = load_graph(&graph_path).unwrap();
        assert!(graph.get_task("task-x").unwrap().timeout.is_none());
    }
}
