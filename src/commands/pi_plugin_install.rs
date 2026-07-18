//! `wg pi-plugin` — compatibility install/inspect surface for `@worksgood/pi`.
//!
//! The escape hatch that mirrors `wg skill install` (`src/commands/skills.rs`).
//! Nobody *needs* to run it — the three wiring points (`wg setup`,
//! `wg profile use pi`, and the JIT `wg pi-handler` pre-flight) call
//! `ensure-pi-plugin` automatically — but it exists as the manual repair/verify
//! handle. All operations delegate to [`worksgood::pi_plugin`].

use anyhow::Result;

use worksgood::pi_plugin::{self, EnsureMode, Source};

use crate::cli::PiPluginCommands;

/// Dispatch `wg pi-plugin <sub>`.
pub fn run(cmd: PiPluginCommands) -> Result<()> {
    match cmd {
        PiPluginCommands::Install { dev } => run_install(dev),
        PiPluginCommands::Status => run_status(),
        PiPluginCommands::Path => run_path(),
        PiPluginCommands::CompatVersion => {
            println!("{}", pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION);
            Ok(())
        }
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
    println!(
        "Installed pi-worksgood (npm: @worksgood/pi, compat {}) from {}.",
        plugin.compat, source
    );
    println!("  extension: {}", plugin.dist_entry.display());
    println!(
        "  wired into pi settings: {}",
        pi_plugin::status().settings_path.display()
    );
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
    println!("WorksGood Pi integration status (pi-worksgood / @worksgood/pi)");
    println!("  compat version:   {}", s.compat);
    println!("  source:           {}", source);
    println!("  resolved entry:   {}", s.dist_entry.display());
    println!("  cache dir:        {}", s.cache_version_dir.display());
    println!(
        "  build ready:      {}",
        if s.ready {
            "yes"
        } else {
            "NO — run `wg pi-plugin install` to repair"
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
    Ok(())
}

/// `wg pi-plugin path` — print the resolved `pi-worksgood` entry (scriptable).
fn run_path() -> Result<()> {
    println!("{}", pi_plugin::status().dist_entry.display());
    Ok(())
}
