//! Profile management commands: set, show, list provider profiles.

use anyhow::{Context, Result};
use std::path::Path;
use workgraph::config::Config;
use workgraph::model_benchmarks::{self, BenchmarkRegistry, RankedTiers};
use workgraph::profile;

/// File name for the cached ranked tiers (inside .workgraph/).
const RANKED_TIERS_FILE: &str = "profile_ranked_tiers.json";

/// Set the active provider profile.
pub fn set(dir: &Path, name: &str) -> Result<()> {
    // Validate the profile name
    let prof = profile::get_profile(name).ok_or_else(|| {
        let available: Vec<&str> = profile::builtin_profiles().iter().map(|p| p.name).collect();
        anyhow::anyhow!(
            "Unknown profile '{}'. Available profiles: {}",
            name,
            available.join(", ")
        )
    })?;

    let mut config = Config::load_merged(dir)?;
    config.profile = Some(name.to_string());

    if prof.is_dynamic() {
        // Dynamic profile: load registry, rank models, auto-configure tiers.
        let ranked = auto_configure_dynamic(dir, &mut config)?;
        config.save(dir)?;

        println!("Profile set: {} (dynamic — auto-configured from registry)", name);
        println!();
        print_tier_selection("fast", &ranked.fast, config.tiers.fast.as_deref());
        print_tier_selection("standard", &ranked.standard, config.tiers.standard.as_deref());
        print_tier_selection("premium", &ranked.premium, config.tiers.premium.as_deref());
    } else {
        config.save(dir)?;

        println!("Profile set: {}", name);
        if let Some(tiers) = prof.resolve_tiers() {
            println!("  Resolved tier mappings:");
            println!(
                "    fast     → {}",
                tiers.fast.as_deref().unwrap_or("(unset)")
            );
            println!(
                "    standard → {}",
                tiers.standard.as_deref().unwrap_or("(unset)")
            );
            println!(
                "    premium  → {}",
                tiers.premium.as_deref().unwrap_or("(unset)")
            );
        }
    }

    println!();
    println!("  Note: Per-role overrides in [models] still take precedence.");
    println!("  Run `wg profile show` for full details.");

    Ok(())
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

/// Load ranked tiers from `.workgraph/profile_ranked_tiers.json`.
fn load_ranked_tiers(dir: &Path) -> Result<Option<RankedTiers>> {
    let path = dir.join(RANKED_TIERS_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let ranked: RankedTiers = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(Some(ranked))
}

/// Show current profile and resolved model mappings.
pub fn show(dir: &Path, json: bool) -> Result<()> {
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
            println!();
            println!("  Ranked Alternatives (by popularity-weighted score):");
            print_ranked_tier("fast", &ranked.fast);
            print_ranked_tier("standard", &ranked.standard);
            print_ranked_tier("premium", &ranked.premium);
        } else {
            println!();
            println!("  No ranked data available. Run `wg profile set openrouter` to auto-configure.");
        }
    }

    Ok(())
}

/// Print a ranked tier's alternatives.
fn print_ranked_tier(tier_name: &str, ranked: &[model_benchmarks::RankedModel]) {
    if ranked.is_empty() {
        return;
    }
    println!();
    println!("    {} tier ({} candidates):", tier_name, ranked.len());
    for (i, model) in ranked.iter().take(10).enumerate() {
        let marker = if i == 0 { " ← selected" } else { "" };
        println!(
            "      {:>2}. {:<40} pop:{:>5.1}  bench:{:>5.1}  score:{:>5.1}{}",
            i + 1,
            model.id,
            model.popularity_score,
            model.benchmark_score,
            model.composite_score,
            marker,
        );
    }
    if ranked.len() > 10 {
        println!("      ... and {} more", ranked.len() - 10);
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
