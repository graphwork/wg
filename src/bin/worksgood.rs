use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use worksgood::concierge::{self, LifecycleOptions};
use worksgood::config::ReasoningLevel;

#[derive(Debug, Parser)]
#[command(
    name = "worksgood",
    version,
    about = "Thin existing-graph TUI launcher (the complete expert CLI remains `wg`)",
    after_help = "PRODUCT BOUNDARY:\n  In a repository that already has `.wg`, bare `worksgood` directly opens the exact same setup-neutral TUI as `wg tui`, using the authenticated absolute sibling `wg` executable and exact project graph. It does not inspect Pi, plugins, profiles, concierge state, config, or services.\n  In a new repository, bare `worksgood` performs one minimal route-free graph bootstrap and then opens that TUI. The TUI owns chat creation and reports executor availability only when a user chooses an executor.\n  `worksgood setup` explicitly enables/configures unattended workers and evaluation (advanced).\n\nNew-repository bootstrap and setup require an attended TTY."
)]
struct Cli {
    /// Repository/worktree target. Never falls back to ~/.wg.
    #[arg(long, global = true)]
    project: Option<PathBuf>,

    /// Advanced concierge: print one immutable redacted plan; write absolutely nothing.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Advanced concierge: configure one exact Pi route for every unattended LLM role.
    /// Worker reasoning defaults high; Eval/assign/FLIP reasoning defaults low.
    /// This never selects the model in an attended Pi chat.
    #[arg(
        long,
        global = true,
        value_name = "pi:<provider>:<model>",
        conflicts_with_all = ["without_ai", "profile", "strong_model", "weak_model"]
    )]
    model: Option<String>,

    /// Advanced concierge: select an existing reusable base profile for automation.
    #[arg(long, global = true)]
    profile: Option<String>,

    /// Advanced concierge: configure a graph without an LLM service.
    #[arg(long, global = true, conflicts_with_all = ["profile", "model"])]
    without_ai: bool,

    /// Advanced concierge: exact unattended worker/chat route for an existing --profile.
    #[arg(long, global = true, requires = "profile", conflicts_with = "model")]
    strong_model: Option<String>,

    /// Advanced concierge: exact unattended Agency/FLIP/evaluation route. Use the same exact value as
    /// --strong-model to explicitly choose “Same as worker”.
    #[arg(long, global = true, requires = "strong_model")]
    weak_model: Option<String>,

    /// Advanced concierge: unattended worker/chat effort. With --model, defaults to high.
    #[arg(long, global = true)]
    strong_reasoning: Option<ReasoningLevel>,

    /// Advanced concierge: unattended Eval/assign/FLIP/weak-role effort. With --model, defaults to low.
    #[arg(long, global = true)]
    weak_reasoning: Option<ReasoningLevel>,

    /// Accept the displayed immutable plan. Still requires an attended TTY.
    #[arg(long, global = true)]
    yes: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Enable/configure unattended workers and evaluation (advanced); no TUI.
    Setup {
        /// Roll back an uncommitted failed setup's exact project selection and
        /// service start. Graph init and handler-owned auth/plugin stay intact.
        #[arg(long)]
        rollback: bool,
    },
    /// Read-only repository/profile/service identity summary.
    Status,
    /// Gracefully stop only this graph's authenticated daemon.
    Stop,
    /// Warn, confirm, and replace only this graph's daemon.
    Restart,
    /// Open the existing setup-neutral TUI; no setup or reconcile.
    Tui,
}

fn lifecycle_options(cli: &Cli, open_tui: bool) -> LifecycleOptions {
    LifecycleOptions {
        project: cli.project.clone(),
        dry_run: cli.dry_run,
        requested_model: cli.model.clone(),
        requested_profile: cli.profile.clone(),
        continue_without_ai: cli.without_ai,
        strong_model: cli.strong_model.clone(),
        weak_model: cli.weak_model.clone(),
        strong_reasoning: cli.strong_reasoning,
        weak_reasoning: cli.weak_reasoning,
        yes: cli.yes,
        open_tui,
    }
}

fn advanced_concierge_option(cli: &Cli) -> Option<&'static str> {
    if cli.dry_run {
        Some("--dry-run")
    } else if cli.model.is_some() {
        Some("--model")
    } else if cli.profile.is_some() {
        Some("--profile")
    } else if cli.without_ai {
        Some("--without-ai")
    } else if cli.strong_model.is_some() {
        Some("--strong-model")
    } else if cli.weak_model.is_some() {
        Some("--weak-model")
    } else if cli.strong_reasoning.is_some() {
        Some("--strong-reasoning")
    } else if cli.weak_reasoning.is_some() {
        Some("--weak-reasoning")
    } else {
        None
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(
        cli.command,
        Some(Commands::Status | Commands::Stop | Commands::Restart | Commands::Tui)
    ) && let Some(option) = advanced_concierge_option(&cli)
    {
        anyhow::bail!("{option} is an advanced concierge option; use it with `worksgood setup`");
    }
    match cli.command {
        None => {
            // No setup input means exactly the thin launcher. Retain the
            // historical explicit-option shorthand as an advanced concierge
            // request; `worksgood setup` is the discoverable/canonical form.
            if advanced_concierge_option(&cli).is_some() || cli.yes {
                concierge::run_lifecycle(&lifecycle_options(&cli, true))
            } else {
                concierge::run_bare(cli.project.as_deref())
            }
        }
        Some(Commands::Setup { rollback }) => {
            if rollback {
                concierge::run_rollback(cli.project.as_deref())
            } else {
                concierge::run_lifecycle(&lifecycle_options(&cli, false))
            }
        }
        Some(Commands::Status) => concierge::run_status(cli.project.as_deref()),
        Some(Commands::Stop) => concierge::run_stop(cli.project.as_deref()),
        Some(Commands::Restart) => concierge::run_restart(cli.project.as_deref(), cli.yes),
        Some(Commands::Tui) => concierge::run_tui(cli.project.as_deref()),
    }
}
