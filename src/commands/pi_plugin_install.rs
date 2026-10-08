//! `wg pi-plugin` — compatibility install/inspect surface for `@worksgood/pi`.
//!
//! The escape hatch that mirrors `wg skill install` (`src/commands/skills.rs`).
//! Nobody *needs* to run it — the three wiring points (`wg setup`,
//! `wg profile use pi`, and the JIT `wg pi-handler` pre-flight) call
//! `ensure-pi-plugin` automatically — but it exists as the manual repair/verify
//! handle. All operations delegate to [`worksgood::pi_plugin`].

use anyhow::Result;

use worksgood::pi_plugin::{self, CacheState, EnsureMode, Source};

use crate::cli::PiPluginCommands;

/// Dispatch `wg pi-plugin <sub>`.
pub fn run(cmd: PiPluginCommands) -> Result<()> {
    match cmd {
        PiPluginCommands::Install { dev } => run_install(dev),
        PiPluginCommands::Status => run_status(),
        PiPluginCommands::Path => run_path(),
        PiPluginCommands::CompatVersion => {
            // Console-critical: a human pi session shells this at load, so heal a
            // stale console cache here (worker spawns already self-heal via the
            // Hermetic ensure). Loud on stderr when a refresh happened.
            if let Some(warning) = pi_plugin::console_self_heal().unwrap_or(None) {
                eprintln!("WorksGood pi-plugin: {warning}");
            }
            println!("{}", pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION);
            Ok(())
        }
        PiPluginCommands::Digest => {
            println!("{}", pi_plugin::embedded_digest());
            Ok(())
        }
    }
}

/// The outcome of an install run: did anything actually change, or was the
/// plugin cache and settings already correct? A matching cache is an explicit
/// no-op — never a bare success report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOutcome {
    Changed,
    Unchanged,
}

fn install_outcome(plugin: &pi_plugin::ResolvedPlugin) -> InstallOutcome {
    if plugin.cache_refreshed || plugin.console_settings_changed || plugin.legacy_settings_migrated
    {
        InstallOutcome::Changed
    } else {
        InstallOutcome::Unchanged
    }
}

/// `wg pi-plugin install [--dev]` — the blessed Console install. Materializes
/// the (cache or repo-dist) build and wires `~/.pi/agent/settings.json` so a
/// human running `pi` gets the wg tools/commands auto-loaded, version-locked.
fn run_install(dev: bool) -> Result<()> {
    let plugin = if dev {
        pi_plugin::ensure_pi_plugin_dev(EnsureMode::Console)?
    } else {
        pi_plugin::ensure_pi_plugin(EnsureMode::Console)?
    };
    let source = match plugin.source {
        Source::Dev => "in-repo worksgood-pi/pi-worksgood (dev)",
        Source::Cache => "embedded → versioned cache",
        Source::EnvOverride => "WG_PI_PLUGIN_DIR override",
    };
    let status = pi_plugin::status();

    // A skipped / already-current install must SAY SO rather than reporting
    // success. "Nothing changed" is the honest outcome when the cache matched
    // this binary's embed digest and settings were already wired.
    let changed = install_outcome(&plugin) == InstallOutcome::Changed;
    if changed {
        println!(
            "Installed pi-worksgood (npm: @worksgood/pi, compat {}) from {}.",
            plugin.compat, source
        );
        if plugin.cache_refreshed {
            println!(
                "  cache:            refreshed (was missing or drifted from this binary's embed)"
            );
        } else if plugin.cache_validated {
            println!("  cache:            verified current (no change)");
        }
    } else {
        println!(
            "pi-worksgood is already up to date — nothing changed (compat {}).",
            plugin.compat
        );
        println!("  source:           {source}");
    }
    println!("  extension: {}", plugin.dist_entry.display());
    println!("  embed digest: {}", status.embed_digest);
    println!(
        "  cache digest: {}",
        status.cache_digest.as_deref().unwrap_or("<none>")
    );
    println!(
        "  wired into pi settings: {} ({})",
        status.settings_path.display(),
        if status.console_wired {
            "console wired"
        } else {
            "not wired"
        }
    );
    if plugin.source == Source::EnvOverride && !plugin.cache_validated {
        println!(
            "  note: WG_PI_PLUGIN_DIR names a non-cache directory ({}); content-digest \
             validation applies only to the canonical cache. Unset it to sync the embedded build.",
            plugin.root.display()
        );
    }
    println!(
        "A human `pi` session in this project will now auto-load the wg tools + /wg commands."
    );
    if plugin.legacy_package_accepted {
        println!(
            "  Compatibility: retained the legacy @worksgood/wg-pi-plugin package record with its extension disabled; pi-worksgood now loads once from the compatible embedded cache."
        );
        println!(
            "  After verifying your console, remove the unused legacy install with: pi remove npm:@worksgood/wg-pi-plugin"
        );
    } else if plugin.legacy_settings_migrated {
        println!("  Compatibility: migrated the legacy managed extension path to pi-worksgood.");
    }
    Ok(())
}

/// `wg pi-plugin status` — resolved source, cache path, compat, wired state, drift.
fn run_status() -> Result<()> {
    let s = pi_plugin::status();
    let source = match s.source {
        Source::Dev => "Dev (in-repo worksgood-pi/pi-worksgood)",
        Source::Cache => "Cache (embedded → versioned cache)",
        Source::EnvOverride => "EnvOverride (WG_PI_PLUGIN_DIR)",
    };
    let cache_state = match s.cache_state {
        CacheState::Current => "current (matches this binary's embedded build)",
        CacheState::Missing => "MISSING — run `wg pi-plugin install`",
        CacheState::Drift => {
            "DRIFTED — stale/incompatible cache vs this binary's embedded build; run `wg pi-plugin install`"
        }
    };
    println!("WorksGood Pi integration status (pi-worksgood / @worksgood/pi)");
    println!("  compat version:   {}", s.compat);
    println!("  source:           {}", source);
    println!("  resolved entry:   {}", s.dist_entry.display());
    println!("  cache dir:        {}", s.cache_version_dir.display());
    println!("  embed digest:     {}", s.embed_digest);
    println!(
        "  cache digest:     {}",
        s.cache_digest.as_deref().unwrap_or("<none>")
    );
    println!("  cache state:      {}", cache_state);
    println!(
        "  build ready:      {}",
        if s.ready {
            "yes".to_string()
        } else if s.source == Source::Cache || s.cache_state != CacheState::Current {
            "NO — run `wg pi-plugin install` to repair".to_string()
        } else {
            "no (resolved dev/override entry is missing)".to_string()
        }
    );
    println!("  pi settings:      {}", s.settings_path.display());
    println!(
        "  console wired:    {}",
        if s.console_wired {
            "yes"
        } else {
            "no (run `wg pi-plugin install`)"
        }
    );
    if s.cache_state != CacheState::Current {
        println!(
            "  WARNING: the embedded cache is {} — a live `pi` session may be running stale wg tools until you run `wg pi-plugin install`.",
            match s.cache_state {
                CacheState::Drift => "DRIFTED from this binary's embedded build",
                CacheState::Missing => "not populated",
                CacheState::Current => unreachable!(),
            }
        );
    }
    // A `wg` installed from a worktree is exactly the hazard that made this
    // command point ~/.pi/agent/settings.json at a prunable path, so flag it
    // here even when everything else resolves cleanly.
    if let Some(warning) = worksgood::build_provenance::worktree_build_warning() {
        println!("  WARNING: {warning}");
    }
    Ok(())
}

/// `wg pi-plugin path` — print the resolved `pi-worksgood` entry (scriptable).
fn run_path() -> Result<()> {
    println!("{}", pi_plugin::status().dist_entry.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin(cache_refreshed: bool, settings_changed: bool) -> pi_plugin::ResolvedPlugin {
        pi_plugin::ResolvedPlugin {
            root: PathBuf::from("/c/wg/worksgood-pi/0.3.0"),
            dist_entry: PathBuf::from("/c/wg/worksgood-pi/0.3.0/pi-worksgood/index.js"),
            host_script: PathBuf::from("/c/wg/worksgood-pi/0.3.0/host/wg-pi-host.mjs"),
            compat: pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION.to_string(),
            source: Source::Cache,
            has_node_modules: false,
            legacy_settings_migrated: false,
            legacy_package_accepted: false,
            console_settings_changed: settings_changed,
            cache_validated: true,
            cache_refreshed,
        }
    }

    #[test]
    fn matching_cache_is_an_explicit_no_op_outcome() {
        assert_eq!(
            install_outcome(&plugin(false, false)),
            InstallOutcome::Unchanged,
            "a matching cache + wired settings must be an explicit no-op"
        );
    }

    #[test]
    fn refreshed_cache_or_settings_change_is_a_changed_outcome() {
        assert_eq!(
            install_outcome(&plugin(true, false)),
            InstallOutcome::Changed
        );
        assert_eq!(
            install_outcome(&plugin(false, true)),
            InstallOutcome::Changed
        );
    }
}
