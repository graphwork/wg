//! Profile management commands: set, show, list provider profiles.

use anyhow::{Context, Result};
use std::path::Path;
use workgraph::config::Config;
use workgraph::model_benchmarks::{self, BenchmarkRegistry, RankedTiers};
use workgraph::profile;

/// File name for the cached ranked tiers (inside .workgraph/).
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

        println!("Profile set: {} (dynamic — auto-configured from registry)", name);
        println!();
        print_tier_selection("fast", &ranked.fast, config.tiers.fast.as_deref());
        print_tier_selection("standard", &ranked.standard, config.tiers.standard.as_deref());
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
    println!(
        "  {:<10} → {}",
        tier_name,
        selected_id.unwrap_or(&top.id)
    );
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

/// Save ranked tiers to `.workgraph/profile_ranked_tiers.json`.
fn save_ranked_tiers(dir: &Path, ranked: &RankedTiers) -> Result<()> {
    let path = dir.join(RANKED_TIERS_FILE);
    let content = serde_json::to_string_pretty(ranked)
        .context("Failed to serialize ranked tiers")?;
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
        println!(
            "  fast:     {} candidates",
            ranked.fast.len()
        );
        println!(
            "  standard: {} candidates",
            ranked.standard.len()
        );
        println!(
            "  premium:  {} candidates",
            ranked.premium.len()
        );
    } else {
        println!();
        println!("Registry updated. Set a dynamic profile to auto-rank:");
        println!("  wg profile set openrouter");
    }

    Ok(())
}

/// Show current profile and resolved model mappings.
pub fn show(dir: &Path, json: bool, verbose: bool) -> Result<()> {
    let config = Config::load_merged(dir)?;

    let effective_tiers = config.effective_tiers_public();

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
            "profile": config.profile,
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
    match config.profile.as_deref() {
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
            println!("  Using default Anthropic tier mappings.");
            println!("  Set a profile with: wg profile set <name>");
        }
    }

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
                    if stale { " (stale — run `wg profile refresh`)" } else { "" },
                );
            }

            println!();
            println!("  Ranked Alternatives (by popularity-weighted score):");
            print_ranked_tier("fast", &ranked.fast, verbose);
            print_ranked_tier("standard", &ranked.standard, verbose);
            print_ranked_tier("premium", &ranked.premium, verbose);
        } else {
            println!();
            println!("  No ranked data available. Run `wg profile set openrouter` to auto-configure.");
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
        tier_name, ranked.len(), curated_count, proxy_count
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
            let tools = if model.supports_tools { "tools" } else { "no-tools" };
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

/// List available profiles.
pub fn list(dir: &Path, json: bool) -> Result<()> {
    let config = Config::load_merged(dir)?;
    let active_profile = config.profile.as_deref();

    let profiles = profile::builtin_profiles();

    if json {
        let val: Vec<serde_json::Value> = profiles
            .iter()
            .map(|p| {
                let tiers = p.resolve_tiers();
                serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "strategy": p.strategy_label(),
                    "active": active_profile == Some(p.name),
                    "tiers": tiers.as_ref().map(|t| serde_json::json!({
                        "fast": t.fast,
                        "standard": t.standard,
                        "premium": t.premium,
                    })),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    println!("Available profiles:");
    println!();

    for p in &profiles {
        let active_marker = if active_profile == Some(p.name) {
            " *"
        } else {
            ""
        };
        println!(
            "  {:<12} {} ({}){}", p.name, p.description, p.strategy_label(), active_marker
        );

        if let Some(tiers) = p.resolve_tiers() {
            println!(
                "               fast: {}  standard: {}  premium: {}",
                tiers.fast.as_deref().unwrap_or("?"),
                tiers.standard.as_deref().unwrap_or("?"),
                tiers.premium.as_deref().unwrap_or("?"),
            );
        } else {
            println!("               (resolved dynamically from benchmark registry)");
        }
        println!();
    }

    match active_profile {
        Some(name) => println!("  Active: {}", name),
        None => println!("  Active: (none — using default Anthropic tiers)"),
    }

    Ok(())
}

