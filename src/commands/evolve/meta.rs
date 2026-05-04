use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::path::Path;

use workgraph::agency::{self, AccessControl, Lineage, PerformanceRecord, Role};
use workgraph::config::Config;

use super::deferred::{defer_operation, should_defer};
use super::operations::parse_category;
use super::strategy::EvolverOperation;

// ---------------------------------------------------------------------------
// Randomisation apply functions
// ---------------------------------------------------------------------------

pub(crate) fn apply_random_compose_role(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
) -> Result<serde_json::Value> {
    // Check deferred gate for outcome oversight
    if let Some(reason) = should_defer(op, agency_dir) {
        return defer_operation(op, reason, run_id, agency_dir);
    }

    let comp_ids = op
        .component_ids
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("random_compose_role requires component_ids"))?;
    let outcome_id = op
        .outcome_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("random_compose_role requires outcome_id"))?;

    // Verify all components exist
    let components_dir = agency_dir.join("primitives/components");
    for cid in comp_ids {
        if !components_dir.join(format!("{}.yaml", cid)).exists() {
            bail!(
                "random_compose_role: component '{}' not found in store",
                cid
            );
        }
    }
    // Verify outcome exists
    let outcomes_dir = agency_dir.join("primitives/outcomes");
    if !outcomes_dir.join(format!("{}.yaml", outcome_id)).exists() {
        bail!(
            "random_compose_role: outcome '{}' not found in store",
            outcome_id
        );
    }

    let mut sorted_ids = comp_ids.clone();
    sorted_ids.sort();
    let new_role_id = agency::content_hash_role(&sorted_ids, outcome_id);

    // Check if already exists
    let roles_dir = agency_dir.join("cache/roles");
    if roles_dir.join(format!("{}.yaml", new_role_id)).exists() {
        return Ok(serde_json::json!({
            "op": "random_compose_role",
            "status": "no_op",
            "reason": "This composition already exists",
            "existing_id": new_role_id,
        }));
    }

    let new_role = Role {
        id: new_role_id.clone(),
        name: op
            .new_name
            .clone()
            .unwrap_or_else(|| format!("random-role-{}", &new_role_id[..8.min(new_role_id.len())])),
        description: op
            .new_description
            .clone()
            .unwrap_or_else(|| "Randomly composed role".to_string()),
        component_ids: sorted_ids,
        outcome_id: outcome_id.to_string(),
        performance: PerformanceRecord::default(),
        lineage: Lineage {
            parent_ids: vec![],
            generation: 0,
            created_by: format!("evolver-randomise-{}", run_id),
            created_at: Utc::now(),
            reframing_potential: None,
        },
        default_context_scope: None,
        default_exec_mode: None,
    };

    let path = agency::save_role(&new_role, &roles_dir)?;
    Ok(serde_json::json!({
        "op": "random_compose_role",
        "new_id": new_role_id,
        "component_ids": new_role.component_ids,
        "outcome_id": outcome_id,
        "path": path.display().to_string(),
        "status": "applied",
    }))
}

pub(crate) fn apply_random_compose_agent(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
) -> Result<serde_json::Value> {
    let role_id = op
        .role_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("random_compose_agent requires role_id"))?;
    let tradeoff_id = op
        .tradeoff_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("random_compose_agent requires tradeoff_id"))?;

    // Verify role exists
    let roles_dir = agency_dir.join("cache/roles");
    if !roles_dir.join(format!("{}.yaml", role_id)).exists() {
        bail!(
            "random_compose_agent: role '{}' not found in store",
            role_id
        );
    }
    // Verify tradeoff exists
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
    if !tradeoffs_dir.join(format!("{}.yaml", tradeoff_id)).exists() {
        bail!(
            "random_compose_agent: tradeoff '{}' not found in store",
            tradeoff_id
        );
    }

    let new_agent_id = agency::content_hash_agent(role_id, tradeoff_id);
    let agents_dir = agency_dir.join("cache/agents");

    // Check if already exists
    if agents_dir.join(format!("{}.yaml", new_agent_id)).exists() {
        return Ok(serde_json::json!({
            "op": "random_compose_agent",
            "status": "no_op",
            "reason": "This agent composition already exists",
            "existing_id": new_agent_id,
        }));
    }

    let new_agent = agency::Agent {
        id: new_agent_id.clone(),
        role_id: role_id.to_string(),
        tradeoff_id: tradeoff_id.to_string(),
        name: op.new_name.clone().unwrap_or_else(|| {
            format!(
                "random-agent-{}",
                &new_agent_id[..8.min(new_agent_id.len())]
            )
        }),
        performance: PerformanceRecord::default(),
        lineage: Lineage {
            parent_ids: vec![],
            generation: 0,
            created_by: format!("evolver-randomise-{}", run_id),
            created_at: Utc::now(),
            reframing_potential: None,
        },
        capabilities: vec![],
        rate: None,
        capacity: None,
        trust_level: workgraph::graph::TrustLevel::Provisional,
        contact: None,
        executor: "claude".to_string(),
        preferred_model: None,
        preferred_provider: None,
        deployment_history: vec![],
        attractor_weight: 0.3,
        staleness_flags: vec![],
    };

    let path = agency::save_agent(&new_agent, &agents_dir)?;
    Ok(serde_json::json!({
        "op": "random_compose_agent",
        "new_id": new_agent_id,
        "role_id": role_id,
        "tradeoff_id": tradeoff_id,
        "path": path.display().to_string(),
        "status": "applied",
    }))
}

// ---------------------------------------------------------------------------
// Bizarre ideation apply function
// ---------------------------------------------------------------------------

pub(crate) fn apply_bizarre_ideation(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
) -> Result<serde_json::Value> {
    let entity_type = op
        .entity_type
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("bizarre_ideation requires entity_type"))?;

    // Check deferred gate (outcomes are always deferred)
    if let Some(reason) = should_defer(op, agency_dir) {
        return defer_operation(op, reason, run_id, agency_dir);
    }

    match entity_type {
        "component" => {
            let components_dir = agency_dir.join("primitives/components");
            let desc = op.new_description.as_deref().ok_or_else(|| {
                anyhow::anyhow!("bizarre_ideation component requires new_description")
            })?;
            let content = if let Some(ref c) = op.new_content {
                agency::ContentRef::Inline(c.clone())
            } else {
                agency::ContentRef::Inline(desc.to_string())
            };
            let category = parse_category(op.new_category.as_deref());

            let new_id = agency::content_hash_component(desc, &category, &content);
            let new_component = agency::RoleComponent {
                id: new_id.clone(),
                name: op
                    .new_name
                    .clone()
                    .unwrap_or_else(|| format!("bizarre-{}", &new_id[..8.min(new_id.len())])),
                description: desc.to_string(),
                quality: 100,
                domain_specificity: 0,
                domain: vec![],
                scope: None,
                origin_instance_id: None,
                parent_content_hash: None,
                category,
                content,
                performance: PerformanceRecord::default(),
                lineage: Lineage {
                    parent_ids: vec![],
                    generation: 0,
                    created_by: format!("evolver-bizarre-{}", run_id),
                    created_at: Utc::now(),
                    reframing_potential: None,
                },
                access_control: AccessControl::default(),
                domain_tags: vec![],
                metadata: std::collections::HashMap::new(),
                former_agents: vec![],
                former_deployments: vec![],
            };

            let path = agency::save_component(&new_component, &components_dir)?;
            Ok(serde_json::json!({
                "op": "bizarre_ideation",
                "entity_type": "component",
                "new_id": new_id,
                "name": new_component.name,
                "path": path.display().to_string(),
                "status": "applied",
            }))
        }
        "tradeoff" => {
            let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
            let desc = op.new_description.as_deref().ok_or_else(|| {
                anyhow::anyhow!("bizarre_ideation tradeoff requires new_description")
            })?;
            let acceptable = op.new_acceptable_tradeoffs.clone().unwrap_or_default();
            let unacceptable = op.new_unacceptable_tradeoffs.clone().unwrap_or_default();

            let new_id = agency::content_hash_tradeoff(&acceptable, &unacceptable, desc);
            let new_tradeoff = agency::TradeoffConfig {
                id: new_id.clone(),
                name: op
                    .new_name
                    .clone()
                    .unwrap_or_else(|| format!("bizarre-{}", &new_id[..8.min(new_id.len())])),
                description: desc.to_string(),
                quality: 100,
                domain_specificity: 0,
                domain: vec![],
                scope: None,
                origin_instance_id: None,
                parent_content_hash: None,
                acceptable_tradeoffs: acceptable,
                unacceptable_tradeoffs: unacceptable,
                performance: PerformanceRecord::default(),
                lineage: Lineage {
                    parent_ids: vec![],
                    generation: 0,
                    created_by: format!("evolver-bizarre-{}", run_id),
                    created_at: Utc::now(),
                    reframing_potential: None,
                },
                access_control: AccessControl::default(),
                domain_tags: vec![],
                metadata: std::collections::HashMap::new(),
                former_agents: vec![],
                former_deployments: vec![],
            };

            let path = agency::save_tradeoff(&new_tradeoff, &tradeoffs_dir)?;
            Ok(serde_json::json!({
                "op": "bizarre_ideation",
                "entity_type": "tradeoff",
                "new_id": new_id,
                "name": new_tradeoff.name,
                "path": path.display().to_string(),
                "status": "applied",
            }))
        }
        "outcome" => {
            // This should have been caught by should_defer, but handle gracefully
            bail!("bizarre_ideation on outcomes must go through the deferred queue");
        }
        other => bail!("bizarre_ideation: unsupported entity_type '{}'", other),
    }
}

// ---------------------------------------------------------------------------
// Meta-agent (AgentConfigurations level) apply functions
// ---------------------------------------------------------------------------

/// Resolve a meta_role string to the config field accessor names.
/// Returns (slot_label, current_agent_hash) or an error if the slot is invalid.
fn resolve_meta_slot<'a>(
    meta_role: &str,
    config: &'a Config,
) -> Result<(&'static str, Option<&'a String>)> {
    match meta_role {
        "assigner" => Ok(("assigner_agent", config.agency.assigner_agent.as_ref())),
        "evaluator" => Ok(("evaluator_agent", config.agency.evaluator_agent.as_ref())),
        "evolver" => Ok(("evolver_agent", config.agency.evolver_agent.as_ref())),
        other => bail!(
            "Unknown meta_role '{}'. Valid: assigner, evaluator, evolver",
            other
        ),
    }
}

/// Update the config's meta-agent slot with a new agent hash.
fn update_meta_slot(meta_role: &str, new_agent_hash: &str, config: &mut Config) {
    match meta_role {
        "assigner" => config.agency.assigner_agent = Some(new_agent_hash.to_string()),
        "evaluator" => config.agency.evaluator_agent = Some(new_agent_hash.to_string()),
        "evolver" => config.agency.evolver_agent = Some(new_agent_hash.to_string()),
        _ => {}
    }
}

/// Swap the role of a meta-agent (assigner/evaluator/evolver), keeping its tradeoff.
/// Creates a new agent with the new role and updates the config slot.
pub(crate) fn apply_meta_swap_role(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
    dir: &Path,
) -> Result<serde_json::Value> {
    let meta_role = op
        .meta_role
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_swap_role requires meta_role"))?;
    let new_role_id = op
        .role_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_swap_role requires role_id (the new role)"))?;

    let mut config = Config::load_or_default(dir);
    let (slot_label, current_hash) = resolve_meta_slot(meta_role, &config)?;
    let current_hash = current_hash
        .ok_or_else(|| anyhow::anyhow!("meta_swap_role: no {} currently configured", slot_label))?
        .clone();

    // Load current agent to get its tradeoff_id
    let agents_dir = agency_dir.join("cache/agents");
    let old_agent = agency::load_agent(&agents_dir.join(format!("{}.yaml", current_hash)))
        .context("Failed to load current meta-agent")?;

    // Verify new role exists
    let roles_dir = agency_dir.join("cache/roles");
    if !roles_dir.join(format!("{}.yaml", new_role_id)).exists() {
        bail!("meta_swap_role: role '{}' not found in store", new_role_id);
    }

    if old_agent.role_id == new_role_id {
        return Ok(serde_json::json!({
            "op": "meta_swap_role",
            "meta_role": meta_role,
            "status": "no_op",
            "reason": "Meta-agent already has this role",
        }));
    }

    let new_agent_id = agency::content_hash_agent(new_role_id, &old_agent.tradeoff_id);
    let new_agent = agency::Agent {
        id: new_agent_id.clone(),
        role_id: new_role_id.to_string(),
        tradeoff_id: old_agent.tradeoff_id.clone(),
        name: old_agent.name.clone(),
        performance: PerformanceRecord::default(),
        lineage: Lineage::mutation(&current_hash, old_agent.lineage.generation, run_id),
        capabilities: old_agent.capabilities.clone(),
        rate: old_agent.rate,
        capacity: old_agent.capacity,
        trust_level: old_agent.trust_level.clone(),
        contact: old_agent.contact.clone(),
        executor: old_agent.executor.clone(),
        preferred_model: old_agent.preferred_model.clone(),
        preferred_provider: old_agent.preferred_provider.clone(),
        deployment_history: vec![],
        attractor_weight: 0.3,
        staleness_flags: vec![],
    };

    let path = agency::save_agent(&new_agent, &agents_dir)?;
    update_meta_slot(meta_role, &new_agent_id, &mut config);
    config
        .save(dir)
        .context("Failed to save config after meta_swap_role")?;

    Ok(serde_json::json!({
        "op": "meta_swap_role",
        "meta_role": meta_role,
        "old_agent": current_hash,
        "new_agent": new_agent_id,
        "new_role_id": new_role_id,
        "path": path.display().to_string(),
        "status": "applied",
    }))
}

/// Swap the tradeoff of a meta-agent (assigner/evaluator/evolver), keeping its role.
/// Creates a new agent with the new tradeoff and updates the config slot.
pub(crate) fn apply_meta_swap_tradeoff(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
    dir: &Path,
) -> Result<serde_json::Value> {
    let meta_role = op
        .meta_role
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_swap_tradeoff requires meta_role"))?;
    let new_tradeoff_id = op
        .tradeoff_id
        .as_deref()
        .or(op.new_tradeoff_id.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!("meta_swap_tradeoff requires tradeoff_id or new_tradeoff_id")
        })?;

    let mut config = Config::load_or_default(dir);
    let (slot_label, current_hash) = resolve_meta_slot(meta_role, &config)?;
    let current_hash = current_hash
        .ok_or_else(|| {
            anyhow::anyhow!("meta_swap_tradeoff: no {} currently configured", slot_label)
        })?
        .clone();

    // Load current agent
    let agents_dir = agency_dir.join("cache/agents");
    let old_agent = agency::load_agent(&agents_dir.join(format!("{}.yaml", current_hash)))
        .context("Failed to load current meta-agent")?;

    // Verify new tradeoff exists
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
    if !tradeoffs_dir
        .join(format!("{}.yaml", new_tradeoff_id))
        .exists()
    {
        bail!(
            "meta_swap_tradeoff: tradeoff '{}' not found in store",
            new_tradeoff_id
        );
    }

    if old_agent.tradeoff_id == new_tradeoff_id {
        return Ok(serde_json::json!({
            "op": "meta_swap_tradeoff",
            "meta_role": meta_role,
            "status": "no_op",
            "reason": "Meta-agent already has this tradeoff",
        }));
    }

    let new_agent_id = agency::content_hash_agent(&old_agent.role_id, new_tradeoff_id);
    let new_agent = agency::Agent {
        id: new_agent_id.clone(),
        role_id: old_agent.role_id.clone(),
        tradeoff_id: new_tradeoff_id.to_string(),
        name: old_agent.name.clone(),
        performance: PerformanceRecord::default(),
        lineage: Lineage::mutation(&current_hash, old_agent.lineage.generation, run_id),
        capabilities: old_agent.capabilities.clone(),
        rate: old_agent.rate,
        capacity: old_agent.capacity,
        trust_level: old_agent.trust_level.clone(),
        contact: old_agent.contact.clone(),
        executor: old_agent.executor.clone(),
        preferred_model: old_agent.preferred_model.clone(),
        preferred_provider: old_agent.preferred_provider.clone(),
        deployment_history: vec![],
        attractor_weight: 0.3,
        staleness_flags: vec![],
    };

    let path = agency::save_agent(&new_agent, &agents_dir)?;
    update_meta_slot(meta_role, &new_agent_id, &mut config);
    config
        .save(dir)
        .context("Failed to save config after meta_swap_tradeoff")?;

    Ok(serde_json::json!({
        "op": "meta_swap_tradeoff",
        "meta_role": meta_role,
        "old_agent": current_hash,
        "new_agent": new_agent_id,
        "new_tradeoff_id": new_tradeoff_id,
        "path": path.display().to_string(),
        "status": "applied",
    }))
}

/// Compose a new agent for a meta-agent slot from a role_id + tradeoff_id.
/// Creates the agent if it doesn't exist, then updates the config slot.
pub(crate) fn apply_meta_compose_agent(
    op: &EvolverOperation,
    run_id: &str,
    agency_dir: &Path,
    dir: &Path,
) -> Result<serde_json::Value> {
    let meta_role = op
        .meta_role
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_compose_agent requires meta_role"))?;
    let role_id = op
        .role_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_compose_agent requires role_id"))?;
    let tradeoff_id = op
        .tradeoff_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("meta_compose_agent requires tradeoff_id"))?;

    // Verify role exists
    let roles_dir = agency_dir.join("cache/roles");
    if !roles_dir.join(format!("{}.yaml", role_id)).exists() {
        bail!("meta_compose_agent: role '{}' not found in store", role_id);
    }
    // Verify tradeoff exists
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
    if !tradeoffs_dir.join(format!("{}.yaml", tradeoff_id)).exists() {
        bail!(
            "meta_compose_agent: tradeoff '{}' not found in store",
            tradeoff_id
        );
    }

    let new_agent_id = agency::content_hash_agent(role_id, tradeoff_id);
    let agents_dir = agency_dir.join("cache/agents");

    // Create the agent if it doesn't already exist
    let agent_path = agents_dir.join(format!("{}.yaml", new_agent_id));
    let path = if agent_path.exists() {
        agent_path
    } else {
        let config_peek = Config::load_or_default(dir);
        let (_, current_hash) = resolve_meta_slot(meta_role, &config_peek)?;
        let parent_gen = current_hash
            .and_then(|h| agency::load_agent(&agents_dir.join(format!("{}.yaml", h))).ok())
            .map(|a| a.lineage.generation)
            .unwrap_or(0);
        let parent_ids: Vec<String> = current_hash.map(|h| vec![h.clone()]).unwrap_or_default();

        let new_agent = agency::Agent {
            id: new_agent_id.clone(),
            role_id: role_id.to_string(),
            tradeoff_id: tradeoff_id.to_string(),
            name: op.new_name.clone().unwrap_or_else(|| {
                format!(
                    "{}-agent-{}",
                    meta_role,
                    &new_agent_id[..8.min(new_agent_id.len())]
                )
            }),
            performance: PerformanceRecord::default(),
            lineage: Lineage {
                parent_ids,
                generation: parent_gen.saturating_add(1),
                created_by: format!("evolver-meta-{}", run_id),
                created_at: Utc::now(),
                reframing_potential: None,
            },
            capabilities: vec![],
            rate: None,
            capacity: None,
            trust_level: workgraph::graph::TrustLevel::Provisional,
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            deployment_history: vec![],
            attractor_weight: 0.3,
            staleness_flags: vec![],
        };
        agency::save_agent(&new_agent, &agents_dir)?
    };

    let mut config = Config::load_or_default(dir);
    let old_hash = resolve_meta_slot(meta_role, &config)?.1.cloned();
    update_meta_slot(meta_role, &new_agent_id, &mut config);
    config
        .save(dir)
        .context("Failed to save config after meta_compose_agent")?;

    Ok(serde_json::json!({
        "op": "meta_compose_agent",
        "meta_role": meta_role,
        "old_agent": old_hash,
        "new_agent": new_agent_id,
        "role_id": role_id,
        "tradeoff_id": tradeoff_id,
        "path": path.display().to_string(),
        "status": "applied",
    }))
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

pub(crate) fn print_operation_result(op: &EvolverOperation, result: &serde_json::Value) {
    let status = result["status"].as_str().unwrap_or("unknown");
    let symbol = if status == "applied" { "+" } else { "!" };

    match op.op.as_str() {
        "create_role" => {
            println!(
                "  [{}] Created role: {} ({})",
                symbol,
                op.name.as_deref().unwrap_or("?"),
                op.new_id.as_deref().unwrap_or("?"),
            );
        }
        "modify_role" => {
            println!(
                "  [{}] Modified role: {} -> {} (gen {})",
                symbol,
                op.target_id.as_deref().unwrap_or("?"),
                op.new_id.as_deref().unwrap_or("?"),
                result["generation"].as_u64().unwrap_or(0),
            );
        }
        "create_motivation" => {
            println!(
                "  [{}] Created motivation: {} ({})",
                symbol,
                op.name.as_deref().unwrap_or("?"),
                op.new_id.as_deref().unwrap_or("?"),
            );
        }
        "modify_motivation" => {
            println!(
                "  [{}] Modified motivation: {} -> {} (gen {})",
                symbol,
                op.target_id.as_deref().unwrap_or("?"),
                op.new_id.as_deref().unwrap_or("?"),
                result["generation"].as_u64().unwrap_or(0),
            );
        }
        "retire_role" => {
            println!(
                "  [{}] Retired role: {}",
                symbol,
                op.target_id.as_deref().unwrap_or("?"),
            );
        }
        "retire_motivation" => {
            println!(
                "  [{}] Retired motivation: {}",
                symbol,
                op.target_id.as_deref().unwrap_or("?"),
            );
        }
        "wording_mutation" => {
            println!(
                "  [{}] Wording mutation ({}) {} -> {}",
                symbol,
                op.entity_type.as_deref().unwrap_or("?"),
                op.target_id.as_deref().unwrap_or("?"),
                result["new_id"].as_str().unwrap_or("?"),
            );
        }
        "component_substitution" => {
            println!(
                "  [{}] Component substitution on {} (-{} +{})",
                symbol,
                op.target_id.as_deref().unwrap_or("?"),
                op.remove_component_id.as_deref().unwrap_or("?"),
                op.add_component_id.as_deref().unwrap_or("?"),
            );
        }
        "config_add_component" | "config_remove_component" => {
            println!(
                "  [{}] {} on {} -> {}",
                symbol,
                op.op,
                op.target_id.as_deref().unwrap_or("?"),
                result["new_id"].as_str().unwrap_or("?"),
            );
        }
        "config_swap_outcome" | "config_swap_tradeoff" => {
            println!(
                "  [{}] {} on {} -> {}",
                symbol,
                op.op,
                op.target_id.as_deref().unwrap_or("?"),
                result["new_id"].as_str().unwrap_or("?"),
            );
        }
        "random_compose_role" | "random_compose_agent" => {
            println!(
                "  [{}] {} -> {}",
                symbol,
                op.op,
                result["new_id"].as_str().unwrap_or("?"),
            );
        }
        "bizarre_ideation" => {
            println!(
                "  [{}] Bizarre ideation ({}) -> {}",
                symbol,
                op.entity_type.as_deref().unwrap_or("?"),
                result["new_id"]
                    .as_str()
                    .or(result["deferred_id"].as_str())
                    .unwrap_or("?"),
            );
        }
        other => {
            println!("  [{}] {}: {:?}", symbol, other, result);
        }
    }

    if let Some(rationale) = &op.rationale {
        println!("        Rationale: {}", rationale);
    }
}
