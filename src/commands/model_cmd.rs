//! `wg model` — Model registry and routing management.
//!
//! Wraps existing config_cmd operations with a simplified, discoverable CLI.

use anyhow::Result;
use std::path::Path;

use super::config_cmd::{self, ConfigScope};

/// `wg model list` — show all models in the effective registry.
pub fn run_list(dir: &Path, tier: Option<&str>, json: bool) -> Result<()> {
    if let Some(tier_str) = tier {
        // Validate tier string early
        let _: worksgood::config::Tier = tier_str.parse()?;
    }

    let config = worksgood::config::Config::load_merged(dir)?;
    let entries = config.effective_registry();

    let filtered: Vec<_> = if let Some(tier_str) = tier {
        let tier: worksgood::config::Tier = tier_str.parse()?;
        entries.into_iter().filter(|e| e.tier == tier).collect()
    } else {
        entries
    };

    if json {
        let val: Vec<serde_json::Value> = filtered
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "provider": e.provider,
                    "model": e.model,
                    "tier": e.tier.to_string(),
                    "endpoint": e.endpoint,
                    "context_window": e.context_window,
                    "cost_per_input_mtok": e.cost_per_input_mtok,
                    "cost_per_output_mtok": e.cost_per_output_mtok,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No model registry entries.");
        return Ok(());
    }

    // Determine the default model
    let default_model = config
        .models
        .get_role(worksgood::config::DispatchRole::Default)
        .and_then(|r| r.model.as_deref());

    println!(
        "  {:<12} {:<12} {:<30} {:<10} COST (in/out per MTok)",
        "ID", "PROVIDER", "MODEL", "TIER"
    );
    println!("  {}", "-".repeat(85));

    for entry in &filtered {
        let is_default = default_model == Some(&entry.id);
        let marker = if is_default { "*" } else { " " };
        let cost = if entry.cost_per_input_mtok > 0.0 || entry.cost_per_output_mtok > 0.0 {
            format!(
                "${:.2}/${:.2}",
                entry.cost_per_input_mtok, entry.cost_per_output_mtok
            )
        } else {
            "-".to_string()
        };
        println!(
            "{} {:<12} {:<12} {:<30} {:<10} {}",
            marker, entry.id, entry.provider, entry.model, entry.tier, cost,
        );
    }

    if let Some(dm) = default_model {
        println!("\n  * = default model ({})", dm);
    }

    Ok(())
}

/// `wg model add <alias>` — add or update a registry entry.
#[allow(clippy::too_many_arguments)]
pub fn run_add(
    dir: &Path,
    alias: &str,
    provider: &str,
    model_id: Option<&str>,
    tier: &str,
    endpoint: Option<&str>,
    context_window: Option<u64>,
    cost_in: Option<f64>,
    cost_out: Option<f64>,
    global: bool,
) -> Result<()> {
    let scope = if global {
        ConfigScope::Global
    } else {
        ConfigScope::Local
    };
    // model_id defaults to the alias if not specified
    let model = model_id.unwrap_or(alias);
    config_cmd::add_registry_entry(
        dir,
        scope,
        alias,
        provider,
        model,
        tier,
        endpoint,
        context_window,
        cost_in,
        cost_out,
    )
}

/// `wg model remove <alias>` — remove a registry entry.
pub fn run_remove(dir: &Path, alias: &str, force: bool, global: bool, json: bool) -> Result<()> {
    let scope = if global {
        ConfigScope::Global
    } else {
        ConfigScope::Local
    };
    config_cmd::remove_registry_entry(dir, scope, alias, force, json)
}

/// `wg model set-default <alias>` — set the default model for agent dispatch.
pub fn run_set_default(dir: &Path, alias: &str, global: bool) -> Result<()> {
    use worksgood::config::{Config, DispatchRole, parse_model_spec};

    let scope = if global {
        ConfigScope::Global
    } else {
        ConfigScope::Local
    };
    config_cmd::reject_global_project_write(scope)?;

    // Parse provider:model format for registry lookup
    let spec = parse_model_spec(alias);
    let lookup_id = &spec.model_id;

    // Validate alias exists in the effective registry
    let merged = Config::load_merged(dir)?;
    if merged.registry_lookup(lookup_id).is_none() {
        anyhow::bail!(
            "Model '{}' not found in the registry.\n  \
             Try: `wg models search {}` to find valid alternatives\n  \
             Or:  `wg models list` to see the local registry\n  \
             Add: `wg model add {} --provider <provider>` to register it\n  \
             Tip: `openrouter/auto` is a safe default that auto-routes to the best model.",
            alias,
            alias.split('/').next_back().unwrap_or(alias),
            alias,
        );
    }

    let mut config = match scope {
        ConfigScope::Global => Config::load_global()?.unwrap_or_default(),
        ConfigScope::Local => Config::load(dir)?,
    };

    config.models.set_model(DispatchRole::Default, alias);

    match scope {
        ConfigScope::Global => config.save_global()?,
        ConfigScope::Local => config.save(dir)?,
    }

    println!("Set default model to: {}", alias);
    Ok(())
}

/// `wg model routing` — show per-role model routing.
pub fn run_routing(dir: &Path, json: bool) -> Result<()> {
    config_cmd::show_model_routing(dir, json)
}

/// `wg model set <role> <model>` — set model for a dispatch role.
pub fn run_set(
    dir: &Path,
    role: &str,
    model: &str,
    provider: Option<&str>,
    endpoint: Option<&str>,
    global: bool,
) -> Result<()> {
    let scope = if global {
        ConfigScope::Global
    } else {
        ConfigScope::Local
    };

    let set_model_args = vec![role.to_string(), model.to_string()];
    let set_provider_args = provider.map(|p| vec![role.to_string(), p.to_string()]);
    let set_endpoint_args = endpoint.map(|e| vec![role.to_string(), e.to_string()]);

    config_cmd::update_model_routing(
        dir,
        scope,
        Some(&set_model_args),
        set_provider_args.as_deref(),
        set_endpoint_args.as_deref(),
    )
}
