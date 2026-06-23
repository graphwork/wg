//! `wg pi-plugin` — explicit install / inspect surface for the wg-pi-plugin.
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
        Source::Dev => "in-repo pi-plugin/dist (dev)",
        Source::Cache => "embedded → versioned cache",
        Source::EnvOverride => "WG_PI_PLUGIN_DIR override",
    };
    println!(
        "Installed wg-pi-plugin (compat {}) from {}.",
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
    Ok(())
}

/// `wg pi-plugin status` — resolved source, cache path, compat, wired state, drift.
fn run_status() -> Result<()> {
    let s = pi_plugin::status();
    let source = match s.source {
        Source::Dev => "Dev (in-repo pi-plugin/dist)",
        Source::Cache => "Cache (embedded → versioned cache)",
        Source::EnvOverride => "EnvOverride (WG_PI_PLUGIN_DIR)",
    };
    println!("wg-pi-plugin status");
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

/// `wg pi-plugin path` — print the resolved dist entry (scriptable).
fn run_path() -> Result<()> {
    println!("{}", pi_plugin::status().dist_entry.display());
    Ok(())
}
