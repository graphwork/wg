//! Profile management commands: set, show, list provider profiles.

use anyhow::{Context, Result};
use std::path::Path;
use workgraph::config::Config;
use workgraph::model_benchmarks::{self, BenchmarkRegistry, RankedTiers};
use workgraph::profile;
use workgraph::profile::named as named_profile;

/// Extract the top-level `description` key from a profile TOML string.
/// Returns None if the file has no description or fails to parse.
fn parse_top_level_description(content: &str) -> Option<String> {
    let val: toml::Value = content.parse().ok()?;
    val.get("description")?.as_str().map(|s| s.to_string())
}

/// File name for the cached ranked tiers (inside .wg/).
/// Note: `profile::load_ranked_tiers()` provides the public read path;
/// this constant is kept here only for `save_ranked_tiers`.
const RANKED_TIERS_FILE: &str = "profile_ranked_tiers.json";

/// Set the active provider profile.
///
/// If `fast`, `standard`, or `premium` are provided, those tiers are pinned
/// to the specified model IDs in the `[tiers]` config section. This lets
/// users override the dynamic or static defaults without editing config.toml.
pub fn set(
    dir: &Path,
    name: &str,
    fast: Option<&str>,
    standard: Option<&str>,
    premium: Option<&str>,
) -> Result<()> {
    // Validate the profile name
    let prof = profile::get_profile(name).ok_or_else(|| {
        let available: Vec<&str> = profile::builtin_profiles().iter().map(|p| p.name).collect();
        anyhow::anyhow!(
            "Unknown profile '{}'. Available profiles: {}",
            name,
            available.join(", ")
        )
    })?;

    let has_tier_pins = fast.is_some() || standard.is_some() || premium.is_some();

    let mut config = Config::load_merged(dir)?;
    config.profile = Some(name.to_string());

    if prof.is_dynamic() && !has_tier_pins {
        // Dynamic profile without manual pins: load registry, rank models, auto-configure.
        let ranked = auto_configure_dynamic(dir, &mut config)?;

        // Apply any explicit tier pins on top of auto-configured tiers
        apply_tier_pins(&mut config, fast, standard, premium);
        config.save(dir)?;

        println!(
            "Profile set: {} (dynamic — auto-configured from registry)",
            name
        );
        println!();
        print_tier_selection("fast", &ranked.fast, config.tiers.fast.as_deref());
        print_tier_selection(
            "standard",
            &ranked.standard,
            config.tiers.standard.as_deref(),
        );
        print_tier_selection("premium", &ranked.premium, config.tiers.premium.as_deref());
    } else {
        // Static profile, or dynamic with explicit tier pins — apply pins directly.
        apply_tier_pins(&mut config, fast, standard, premium);
        config.save(dir)?;

        println!("Profile set: {}", name);
        println!("  Tier mappings:");
        let effective = config.effective_tiers_public();
        println!(
            "    fast     → {}",
            effective.fast.as_deref().unwrap_or("(unset)")
        );
        println!(
            "    standard → {}",
            effective.standard.as_deref().unwrap_or("(unset)")
        );
        println!(
            "    premium  → {}",
            effective.premium.as_deref().unwrap_or("(unset)")
        );

        if has_tier_pins {
            println!();
            println!("  Pinned tiers:");
            if let Some(f) = fast {
                println!("    fast     = {}", f);
            }
            if let Some(s) = standard {
                println!("    standard = {}", s);
            }
            if let Some(p) = premium {
                println!("    premium  = {}", p);
            }
        }
    }

    println!();
    println!("  Note: Per-role overrides in [models] still take precedence.");
    println!("  Run `wg profile show` for full details.");

    Ok(())
}

/// Apply explicit tier pins to config.tiers.
fn apply_tier_pins(
    config: &mut Config,
    fast: Option<&str>,
    standard: Option<&str>,
    premium: Option<&str>,
) {
    if let Some(f) = fast {
        config.tiers.fast = Some(f.to_string());
    }
    if let Some(s) = standard {
        config.tiers.standard = Some(s.to_string());
    }
    if let Some(p) = premium {
        config.tiers.premium = Some(p.to_string());
    }
}

/// Auto-configure a dynamic profile from the benchmark registry.
///
/// Loads the registry, runs the popularity-weighted ranking, writes the top picks
/// into config tiers, and saves the full ranked lists to a sidecar JSON file.
fn auto_configure_dynamic(dir: &Path, config: &mut Config) -> Result<RankedTiers> {
    let registry = BenchmarkRegistry::load(dir)?
        .context("No benchmark registry found. Run `wg models fetch` first to populate it.")?;

    let ranked = model_benchmarks::rank_models_for_profile(&registry);

    // Write the top pick from each tier into config.tiers (using openrouter: prefix).
    if let Some(top) = ranked.fast.first() {
        config.tiers.fast = Some(format!("openrouter:{}", top.id));
    }
    if let Some(top) = ranked.standard.first() {
        config.tiers.standard = Some(format!("openrouter:{}", top.id));
    }
    if let Some(top) = ranked.premium.first() {
        config.tiers.premium = Some(format!("openrouter:{}", top.id));
    }

    // Save the full ranked lists for `wg profile show` and fallback support.
    save_ranked_tiers(dir, &ranked)?;

    Ok(ranked)
}

/// Print the tier selection with score breakdown.
fn print_tier_selection(
    tier_name: &str,
    ranked: &[model_benchmarks::RankedModel],
    selected_id: Option<&str>,
) {
    if ranked.is_empty() {
        println!("  {:<10} (no candidates)", tier_name);
        return;
    }

    let top = &ranked[0];
    println!("  {:<10} → {}", tier_name, selected_id.unwrap_or(&top.id));
    println!(
        "             {} | popularity: {:.1} | benchmarks: {:.1} | composite: {:.1}",
        top.name, top.popularity_score, top.benchmark_score, top.composite_score
    );
    if ranked.len() > 1 {
        println!(
            "             ({} alternative{} available)",
            ranked.len() - 1,
            if ranked.len() == 2 { "" } else { "s" }
        );
    }
}

/// Save ranked tiers to `.wg/profile_ranked_tiers.json`.
fn save_ranked_tiers(dir: &Path, ranked: &RankedTiers) -> Result<()> {
    let path = dir.join(RANKED_TIERS_FILE);
    let content =
        serde_json::to_string_pretty(ranked).context("Failed to serialize ranked tiers")?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Load ranked tiers — delegates to the shared implementation in profile module.
fn load_ranked_tiers(dir: &Path) -> Result<Option<RankedTiers>> {
    profile::load_ranked_tiers(dir)
}

/// Refresh model data from OpenRouter and recompute rankings.
pub fn refresh(dir: &Path) -> Result<()> {
    use crate::commands::models::run_fetch;

    eprintln!("Refreshing model data from OpenRouter...");
    run_fetch(dir, true)?;

    // Re-rank if dynamic profile is active.
    let mut config = Config::load_merged(dir)?;
    let is_dynamic = config
        .profile
        .as_deref()
        .and_then(profile::get_profile)
        .map(|p| p.is_dynamic())
        .unwrap_or(false);

    if is_dynamic {
        let ranked = auto_configure_dynamic(dir, &mut config)?;
        config.save(dir)?;
        println!();
        println!("Rankings updated:");
        println!("  fast:     {} candidates", ranked.fast.len());
        println!("  standard: {} candidates", ranked.standard.len());
        println!("  premium:  {} candidates", ranked.premium.len());
    } else {
        println!();
        println!("Registry updated. Set a dynamic profile to auto-rank:");
        println!("  wg profile set openrouter");
    }

    Ok(())
}

/// Show current profile and resolved model mappings.
pub fn show(
    dir: &Path,
    json: bool,
    verbose: bool,
    profile_name: Option<&str>,
    _diff_base: bool,
) -> Result<()> {
    // If a specific named profile is requested, show its snapshot contents.
    // Profiles are complete Config snapshots now (post-2026-05 pivot), so we
    // surface the same keys a `~/.wg/config.toml` would carry.
    if let Some(name) = profile_name {
        let prof = named_profile::load(name)?;
        let path = named_profile::profile_path(name)?;
        if json {
            let val = serde_json::json!({
                "name": name,
                "description": prof.description,
                "agent_model": prof.config.agent.model,
                "dispatcher_model": prof.config.coordinator.model,
                "tiers": {
                    "fast": prof.config.tiers.fast,
                    "standard": prof.config.tiers.standard,
                    "premium": prof.config.tiers.premium,
                },
                "file": path.display().to_string(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        } else {
            println!("Profile: {}", name);
            if let Some(ref desc) = prof.description {
                println!("  {}", desc);
            }
            println!();
            println!("  agent.model      = \"{}\"", prof.config.agent.model);
            if let Some(ref m) = prof.config.coordinator.model {
                println!("  dispatcher.model = \"{}\"", m);
            }
            if let Some(ref f) = prof.config.tiers.fast {
                println!("  tiers.fast       = \"{}\"", f);
            }
            if let Some(ref s) = prof.config.tiers.standard {
                println!("  tiers.standard   = \"{}\"", s);
            }
            if let Some(ref p) = prof.config.tiers.premium {
                println!("  tiers.premium    = \"{}\"", p);
            }
            if let Some(ref m) = prof.config.models.evaluator {
                if let Some(ref ms) = m.model {
                    println!("  models.evaluator.model       = \"{}\"", ms);
                }
            }
            if let Some(ref m) = prof.config.models.assigner {
                if let Some(ref ms) = m.model {
                    println!("  models.assigner.model        = \"{}\"", ms);
                }
            }
            if let Some(ref m) = prof.config.models.flip_inference {
                if let Some(ref ms) = m.model {
                    println!("  models.flip_inference.model  = \"{}\"", ms);
                }
            }
            if let Some(ref m) = prof.config.models.flip_comparison {
                if let Some(ref ms) = m.model {
                    println!("  models.flip_comparison.model = \"{}\"", ms);
                }
            }
            for endpoint in &prof.config.llm_endpoints.endpoints {
                println!(
                    "  endpoint: {} ({}) url={}",
                    endpoint.name,
                    endpoint.provider,
                    endpoint.url.as_deref().unwrap_or("(none)")
                );
            }
            println!();
            println!("  File: {}", path.display());
        }
        return Ok(());
    }

    // Default: show current merged config (with active profile applied).
    let config = Config::load_merged(dir)?;
    let effective_tiers = config.effective_tiers_public();
    let active = named_profile::active().unwrap_or(None);

    // Load ranked alternatives if available.
    let ranked_tiers = load_ranked_tiers(dir)?;
    let is_dynamic = config
        .profile
        .as_deref()
        .and_then(profile::get_profile)
        .map(|p| p.is_dynamic())
        .unwrap_or(false);

    if json {
        let mut val = serde_json::json!({
            "active_named_profile": active,
            "profile": config.profile,
            "agent_model": config.agent.model,
            "dispatcher_model": config.coordinator.model,
            "effective_tiers": {
                "fast": effective_tiers.fast,
                "standard": effective_tiers.standard,
                "premium": effective_tiers.premium,
            },
        });
        if let Some(ref ranked) = ranked_tiers {
            val["ranked_alternatives"] = serde_json::to_value(ranked)?;
        }
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    // Header
    match active.as_deref() {
        Some(name) => {
            println!("Active named profile: {} *", name);
            if let Ok(prof) = named_profile::load(name) {
                if let Some(ref desc) = prof.description {
                    println!("  {}", desc);
                }
            }
        }
        None => match config.profile.as_deref() {
            Some(name) => {
                if let Some(prof) = profile::get_profile(name) {
                    println!("Profile: {} ({})", name, prof.strategy_label());
                    println!("  {}", prof.description);
                } else {
                    println!("Profile: {} (unknown — not a built-in profile)", name);
                }
            }
            None => {
                println!("Profile: (none)");
                println!(
                    "  Using default config. Run `wg profile init-starters` and `wg profile use <name>`."
                );
            }
        },
    }

    println!();
    println!("  Active config (named profile is authoritative for routing):");
    println!("    agent.model      = {}", config.agent.model);
    println!(
        "    dispatcher.model = {}",
        config.coordinator.model.as_deref().unwrap_or("(unset)")
    );
    println!();
    println!("  Tier Mappings:");
    println!(
        "    fast     → {}",
        effective_tiers.fast.as_deref().unwrap_or("(unset)")
    );
    println!(
        "    standard → {}",
        effective_tiers.standard.as_deref().unwrap_or("(unset)")
    );
    println!(
        "    premium  → {}",
        effective_tiers.premium.as_deref().unwrap_or("(unset)")
    );

    // Show if any explicit tier overrides are active
    let has_overrides = config.tiers.fast.is_some()
        || config.tiers.standard.is_some()
        || config.tiers.premium.is_some();
    if has_overrides && !is_dynamic {
        println!();
        println!("  Tier overrides (from [tiers] config):");
        if let Some(ref f) = config.tiers.fast {
            println!("    fast     = {}", f);
        }
        if let Some(ref s) = config.tiers.standard {
            println!("    standard = {}", s);
        }
        if let Some(ref p) = config.tiers.premium {
            println!("    premium  = {}", p);
        }
    }

    // Show ranked alternatives for dynamic profiles.
    if is_dynamic {
        if let Some(ref ranked) = ranked_tiers {
            // Show data freshness.
            if let Some(ref registry) = BenchmarkRegistry::load(dir)? {
                let stale = registry.is_stale(24);
                let scored = registry
                    .models
                    .values()
                    .filter(|m| m.fitness.score.is_some())
                    .count();
                let total = registry.models.len();
                println!();
                println!(
                    "  Registry: {} models ({} scored), fetched {}{}",
                    total,
                    scored,
                    &registry.fetched_at[..10],
                    if stale {
                        " (stale — run `wg profile refresh`)"
                    } else {
                        ""
                    },
                );
            }

            println!();
            println!("  Ranked Alternatives (by benchmark-weighted score):");
            print_ranked_tier("fast", &ranked.fast, verbose);
            print_ranked_tier("standard", &ranked.standard, verbose);
            print_ranked_tier("premium", &ranked.premium, verbose);
        } else {
            println!();
            println!(
                "  No ranked data available. Run `wg profile set openrouter` to auto-configure."
            );
        }
    }

    Ok(())
}

/// Print a ranked tier's alternatives.
fn print_ranked_tier(tier_name: &str, ranked: &[model_benchmarks::RankedModel], verbose: bool) {
    if ranked.is_empty() {
        return;
    }

    let curated_count = ranked.iter().filter(|m| m.is_curated).count();
    let proxy_count = ranked.len() - curated_count;

    println!();
    println!(
        "    {} tier ({} candidates, {} curated, {} proxy)",
        tier_name,
        ranked.len(),
        curated_count,
        proxy_count
    );

    let display_count = if verbose { 20 } else { 10 };
    for (i, model) in ranked.iter().take(display_count).enumerate() {
        let marker = if i == 0 { " ← selected" } else { "" };
        let source = if model.is_curated { "" } else { " ~" };
        println!(
            "      {:>2}. {:<40} pop:{:>5.1}  bench:{:>5.1}  score:{:>5.1}{}{}",
            i + 1,
            model.id,
            model.popularity_score,
            model.benchmark_score,
            model.composite_score,
            source,
            marker,
        );

        if verbose {
            let in_price = model.input_per_mtok.unwrap_or(0.0);
            let out_price = model.output_per_mtok.unwrap_or(0.0);
            let ctx = model
                .context_window
                .map(|c| format!("{}k", c / 1000))
                .unwrap_or_else(|| "?".to_string());
            let tools = if model.supports_tools {
                "tools"
            } else {
                "no-tools"
            };
            println!(
                "          in:${:.2}/MTok  out:${:.2}/MTok  ctx:{}  {}",
                in_price, out_price, ctx, tools,
            );
        }
    }
    if ranked.len() > display_count {
        println!("      ... and {} more", ranked.len() - display_count);
    }
}

/// List available profiles (installed user profiles + built-in starters).
pub fn list(dir: &Path, json: bool, installed_only: bool) -> Result<()> {
    let active = named_profile::active().unwrap_or(None);
    let installed = named_profile::list_installed().unwrap_or_default();
    let builtin_names = named_profile::STARTER_NAMES;

    if json {
        let mut items: Vec<serde_json::Value> = vec![];

        // Installed user profiles
        for name in &installed {
            let is_active = active.as_deref() == Some(name.as_str());
            let desc = named_profile::load(name).ok().and_then(|p| p.description);
            items.push(serde_json::json!({
                "name": name,
                "kind": "user",
                "active": is_active,
                "description": desc,
            }));
        }

        // Built-in starters (not shown if installed_only)
        if !installed_only {
            for name in builtin_names {
                if !installed.iter().any(|i| i == name) {
                    items.push(serde_json::json!({
                        "name": name,
                        "kind": "builtin",
                        "active": false,
                        "description": named_profile::starter_template(name)
                            .and_then(parse_top_level_description),
                    }));
                }
            }
        }

        // Legacy built-in profiles (for backward compat display)
        if !installed_only {
            for p in profile::builtin_profiles() {
                items.push(serde_json::json!({
                    "name": p.name,
                    "kind": "legacy-builtin",
                    "active": active.is_none() && false,
                    "description": p.description,
                }));
            }
        }

        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    println!("Named profiles:");
    println!();

    if installed.is_empty() && !installed_only {
        println!("  (no profiles installed — run `wg profile init-starters`)");
    } else {
        for name in &installed {
            let is_active = active.as_deref() == Some(name.as_str());
            let desc = named_profile::load(name)
                .ok()
                .and_then(|p| p.description)
                .unwrap_or_default();
            let marker = if is_active { " *" } else { "" };
            println!("  [user]    {:<14} {}{}", name, desc, marker);
        }
    }

    if !installed_only {
        println!();
        println!("Starter templates (not yet installed — run `wg profile init-starters`):");
        println!();
        for name in builtin_names {
            if !installed.iter().any(|i| i == name) {
                let desc = named_profile::starter_template(name)
                    .and_then(parse_top_level_description)
                    .unwrap_or_default();
                println!("  [builtin] {:<14} {}", name, desc);
            }
        }
    }

    println!();
    match active.as_deref() {
        Some(name) => println!("Active: {} *", name),
        None => println!("Active: (none — run `wg profile use <name>` to activate)"),
    }

    // Also show legacy built-in profiles
    if !installed_only {
        let legacy = profile::builtin_profiles();
        if !legacy.is_empty() {
            println!();
            println!("Legacy tier presets (wg profile set <name>):");
            for p in &legacy {
                let config = Config::load_merged(dir)?;
                let active_legacy = config.profile.as_deref();
                let marker = if active_legacy == Some(p.name) {
                    " *"
                } else {
                    ""
                };
                println!("  {:<12} {}{}", p.name, p.description, marker);
            }
        }
    }

    Ok(())
}

// ── Named profile commands ────────────────────────────────────────────────────

/// Activate a named profile: copy `~/.wg/profiles/<name>.toml` over
/// `~/.wg/config.toml` (the global config), remove project-local routing keys
/// that would shadow the profile, update the active-pointer file, and
/// hot-reload the daemon.
///
/// **Profile-as-swap, not overlay** (2026-05 pivot): the profile file IS the
/// global config. No merge logic, no resolution chain. What's in the profile
/// file is exactly what runs. Local non-routing settings are preserved.
pub fn use_profile(dir: &Path, name: Option<&str>, no_reload: bool, clear: bool) -> Result<()> {
    if clear || name.is_none() {
        let prev = named_profile::active().unwrap_or(None);
        named_profile::set_active(None)?;
        match prev.as_deref() {
            Some(p) => println!(
                "Active profile cleared (was: {}). ~/.wg/config.toml left as-is — edit or `wg config init` to change.",
                p
            ),
            None => println!("No active profile was set. Nothing changed."),
        }
        if !no_reload {
            trigger_daemon_reload(dir, None);
        }
        return Ok(());
    }

    let profile_name = name.unwrap();
    let prof = named_profile::load(profile_name)?;

    // Pre-flight: check that any api_key_ref in the profile's endpoints are reachable.
    let secrets_cfg = workgraph::secret::SecretsConfig::load_global();
    for ep in &prof.config.llm_endpoints.endpoints {
        if let Some(ref r) = ep.api_key_ref {
            match workgraph::secret::check_ref_reachable(r, &secrets_cfg) {
                Ok(true) => {}
                Ok(false) => {
                    let hint = if let Some(n) = r.strip_prefix("keyring:") {
                        format!("Run: wg secret set {}", n)
                    } else if let Some(n) = r.strip_prefix("plain:") {
                        format!("Run: wg secret set {} --backend plaintext", n)
                    } else {
                        String::new()
                    };
                    eprintln!(
                        "Warning: profile '{}' references secret '{}' but no entry found.\n  {}",
                        profile_name, r, hint
                    );
                }
                Err(e) => {
                    eprintln!(
                        "Warning: profile '{}' secret check failed for '{}': {}",
                        profile_name, r, e
                    );
                }
            }
        }
    }

    let prev = named_profile::active().unwrap_or(None);
    let written = named_profile::apply_profile_as_global_config(profile_name)?;
    let local_cleanup = named_profile::clear_local_profile_routing_overrides(dir)?;
    named_profile::set_active(Some(profile_name))?;

    match prev.as_deref() {
        Some(p) if p != profile_name => println!(
            "Active profile: {} (was: {}). Wrote {}. Next worker will use {} models.",
            profile_name,
            p,
            written.display(),
            profile_name
        ),
        Some(_) => println!(
            "Active profile: {} (re-applied). Wrote {}.",
            profile_name,
            written.display()
        ),
        None => println!(
            "Active profile: {}. Wrote {}. Next worker will use {} models.",
            profile_name,
            written.display(),
            profile_name
        ),
    }

    if let Some(cleanup) = local_cleanup {
        println!(
            "  Cleared local routing overrides from {}: {}",
            cleanup.path.display(),
            cleanup.removed_keys.join(", ")
        );
        println!("  Local config backup: {}", cleanup.backup_path.display());
    } else {
        println!("  No local routing overrides needed clearing.");
    }

    if !no_reload {
        trigger_daemon_reload(dir, Some(profile_name));
    }

    Ok(())
}

/// Send a Reconfigure IPC to the running daemon (if any), or silently continue.
fn trigger_daemon_reload(dir: &Path, profile_name: Option<&str>) {
    use crate::commands::service::ipc::IpcRequest;
    use crate::commands::service::{self, ServiceState};
    use workgraph::service::is_process_alive;

    let running = match ServiceState::load(dir) {
        Ok(Some(state)) => is_process_alive(state.pid),
        _ => false,
    };

    if !running {
        println!("  (Daemon not running — profile applies on next wg service start)");
        return;
    }

    let req = IpcRequest::Reconfigure {
        max_agents: None,
        executor: None,
        poll_interval: None,
        model: None,
        profile: profile_name.map(str::to_string),
    };

    match service::send_request(dir, &req) {
        Ok(resp) if resp.ok => {
            println!("  Daemon reloaded — next worker will use the new profile.");
        }
        Ok(resp) => {
            eprintln!(
                "  Warning: daemon reconfigure returned error: {}",
                resp.error.unwrap_or_default()
            );
        }
        Err(e) => {
            eprintln!(
                "  Warning: could not reach daemon: {}. Profile will apply on next start.",
                e
            );
        }
    }
}

/// Create a new named profile file.
///
/// A profile is a complete config snapshot (post-2026-05 pivot). When `from`
/// is supplied, the new profile starts as a byte-for-byte copy of the source
/// file/template; we then patch the `description`, `[agent].model`,
/// `[dispatcher].model`, and `[[llm_endpoints.endpoints]]` keys as requested
/// by overlaying surgical line-level edits onto the source TOML.
pub fn create_profile(
    name: &str,
    model: Option<&str>,
    endpoint: Option<&str>,
    from: Option<&str>,
    description: Option<&str>,
    force: bool,
) -> Result<()> {
    let path = named_profile::profile_path(name)?;
    if path.exists() && !force {
        anyhow::bail!(
            "Profile '{}' already exists at {}.\nUse --force to overwrite.",
            name,
            path.display()
        );
    }

    // Start from `from` (existing profile or starter template), or empty.
    let base_content = if let Some(from_name) = from {
        let from_path = named_profile::profile_path(from_name)?;
        if from_path.exists() {
            std::fs::read_to_string(&from_path)
                .with_context(|| format!("Failed to read source profile {}", from_path.display()))?
        } else if let Some(tmpl) = named_profile::starter_template(from_name) {
            tmpl.to_string()
        } else {
            anyhow::bail!("Profile or starter '{}' not found", from_name);
        }
    } else {
        String::new()
    };

    // Parse, patch, and serialize via toml::Value to keep the result valid TOML.
    let mut val: toml::Value = if base_content.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        base_content
            .parse()
            .with_context(|| format!("Failed to parse source profile content for '{}'", name))?
    };
    if let Some(desc) = description {
        if let Some(table) = val.as_table_mut() {
            table.insert(
                "description".to_string(),
                toml::Value::String(desc.to_string()),
            );
        }
    }
    if let Some(m) = model {
        let m = toml::Value::String(m.to_string());
        if let Some(table) = val.as_table_mut() {
            let agent = table
                .entry("agent".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let Some(t) = agent.as_table_mut() {
                t.insert("model".to_string(), m.clone());
            }
            let dispatcher = table
                .entry("dispatcher".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let Some(t) = dispatcher.as_table_mut() {
                t.insert("model".to_string(), m);
            }
        }
    }
    if let Some(url) = endpoint {
        let mut ep = toml::map::Map::new();
        ep.insert("name".into(), toml::Value::String("default".into()));
        ep.insert("provider".into(), toml::Value::String("oai-compat".into()));
        ep.insert("url".into(), toml::Value::String(url.to_string()));
        ep.insert("is_default".into(), toml::Value::Boolean(true));
        if let Some(table) = val.as_table_mut() {
            let llm = table
                .entry("llm_endpoints".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let Some(t) = llm.as_table_mut() {
                t.insert(
                    "endpoints".to_string(),
                    toml::Value::Array(vec![toml::Value::Table(ep)]),
                );
            }
        }
    }
    let content = toml::to_string_pretty(&val).context("Failed to serialize new profile")?;
    named_profile::save_raw(name, &content)?;
    println!("Profile '{}' created at {}", name, path.display());
    println!("  Use it with: wg profile use {}", name);
    Ok(())
}

/// Open a profile file in $EDITOR, then validate and optionally hot-reload.
pub fn edit_profile(dir: &Path, name: &str, no_reload: bool) -> Result<()> {
    let path = named_profile::profile_path(name)?;
    if !path.exists() {
        anyhow::bail!(
            "Profile '{}' not found at {}.\nCreate it first with: wg profile create {}",
            name,
            path.display(),
            name,
        );
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("Failed to launch editor '{}'", editor))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    named_profile::validate_file(&path).with_context(|| {
        format!(
            "Profile '{}' has invalid content after editing. File at {}",
            name,
            path.display()
        )
    })?;
    println!("Profile '{}' saved and validated.", name);

    let is_active = named_profile::active().unwrap_or(None).as_deref() == Some(name);

    if is_active && !no_reload {
        trigger_daemon_reload(dir, Some(name));
    }

    Ok(())
}

/// Delete a named profile file.
pub fn delete_profile(name: &str, force: bool) -> Result<()> {
    let path = named_profile::profile_path(name)?;
    if !path.exists() {
        anyhow::bail!("Profile '{}' not found at {}", name, path.display());
    }

    let is_active = named_profile::active().unwrap_or(None).as_deref() == Some(name);

    if is_active && !force {
        anyhow::bail!(
            "Profile '{}' is currently active. Use --force to delete it.",
            name,
        );
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("Failed to delete profile file {}", path.display()))?;

    if is_active {
        named_profile::set_active(None)?;
        println!("Profile '{}' deleted. Active profile cleared.", name);
    } else {
        println!("Profile '{}' deleted.", name);
    }

    Ok(())
}

/// Show a diff between two profiles (or empty vs a profile).
///
/// Profiles are byte-exact files on disk now (post-2026-05 pivot), so we diff
/// the raw file contents rather than reconstructing TOML from a structured
/// view — keeps comments, ordering, and per-line nuance.
pub fn diff_profiles(a: &str, b: Option<&str>) -> Result<()> {
    let path_a = named_profile::profile_path(a)?;
    let toml_a = std::fs::read_to_string(&path_a)
        .with_context(|| format!("Failed to read profile '{}' at {}", a, path_a.display()))?;

    let (label_b, toml_b) = if let Some(b_name) = b {
        let path_b = named_profile::profile_path(b_name)?;
        let content = std::fs::read_to_string(&path_b).with_context(|| {
            format!(
                "Failed to read profile '{}' at {}",
                b_name,
                path_b.display()
            )
        })?;
        (b_name.to_string(), content)
    } else {
        ("(base)".to_string(), String::new())
    };

    println!("--- {}", if b.is_some() { a } else { "(base)" });
    println!("+++ {}", label_b);
    println!();

    let lines_a: Vec<&str> = if b.is_some() {
        toml_a.lines().collect()
    } else {
        vec![]
    };
    let lines_b: Vec<&str> = if b.is_some() {
        toml_b.lines().collect()
    } else {
        toml_a.lines().collect()
    };
    print_simple_diff(&lines_a, &lines_b);

    Ok(())
}

fn print_simple_diff(a: &[&str], b: &[&str]) {
    // Build sets for quick lookup
    let a_set: std::collections::HashSet<&str> = a.iter().copied().collect();
    let b_set: std::collections::HashSet<&str> = b.iter().copied().collect();

    for line in a {
        if !b_set.contains(line) {
            println!("- {}", line);
        } else {
            println!("  {}", line);
        }
    }
    for line in b {
        if !a_set.contains(line) {
            println!("+ {}", line);
        }
    }
}

/// Write the three starter profiles to ~/.wg/profiles/ if missing.
pub fn init_starters(force: bool) -> Result<()> {
    let dir = named_profile::profiles_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create profiles directory {}", dir.display()))?;

    // Auto-migrate the legacy `wgnext.toml` starter to the canonical `nex.toml`
    // name (matches the `wg nex` subcommand). Only renames when the canonical
    // file is absent — never clobbers a user's existing nex.toml.
    let legacy_path = dir.join(format!("{}.toml", named_profile::LEGACY_NEX_NAME));
    let canonical_path = dir.join("nex.toml");
    let mut migrated = 0;
    if legacy_path.exists() {
        if !canonical_path.exists() {
            std::fs::rename(&legacy_path, &canonical_path).with_context(|| {
                format!(
                    "Failed to migrate {} -> {}",
                    legacy_path.display(),
                    canonical_path.display()
                )
            })?;
            migrated += 1;
            println!(
                "  migrated {} -> {} (canonical name now matches `wg nex`)",
                legacy_path.display(),
                canonical_path.display()
            );
        } else {
            // Both files exist — never clobber. Surface a one-line note so the
            // user knows the legacy file is being preserved alongside the
            // canonical one and can resolve manually if intentional.
            println!(
                "  note: both {} and {} exist; preserving both. Run `wg profile delete wgnext` to drop the legacy file once you've migrated any custom edits.",
                legacy_path.display(),
                canonical_path.display()
            );
        }
    }

    // Refresh stale `wg-next:` descriptions in an on-disk `nex.toml` left over
    // from before the rename. This is content (not file) migration: a user who
    // ran an older `init-starters` got a `nex.toml` (or a freshly renamed
    // `wgnext.toml -> nex.toml`) that still says `wg-next:` in its description.
    // The previous rename only updated the in-binary template; existing files
    // were untouched. Conservative: only the description line is rewritten.
    if canonical_path.exists() && named_profile::migrate_stale_description(&canonical_path)? {
        migrated += 1;
        println!(
            "  refreshed description in {} (was 'wg-next:', now 'wg nex:')",
            canonical_path.display()
        );
    }

    let mut written = 0;
    let mut skipped = 0;

    for &name in named_profile::STARTER_NAMES {
        let path = dir.join(format!("{}.toml", name));
        if path.exists() && !force {
            skipped += 1;
            println!(
                "  skip  {} (already exists; use --force to overwrite)",
                name
            );
            continue;
        }
        let tmpl = named_profile::starter_template(name).expect("starter template must exist");
        std::fs::write(&path, tmpl)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        written += 1;
        println!("  wrote {}", path.display());
    }

    println!();
    println!(
        "Starter profiles: {} written, {} skipped{}.",
        written,
        skipped,
        if migrated > 0 {
            format!(", {} migrated", migrated)
        } else {
            String::new()
        }
    );
    if written > 0 {
        println!("Activate one with: wg profile use claude|codex|nex");
    }

    Ok(())
}
