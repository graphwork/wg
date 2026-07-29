use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use worksgood::concierge::{self, LifecycleOptions};
use worksgood::config::ReasoningLevel;

#[derive(Debug, Parser)]
#[command(
    name = "worksgood",
    version,
    about = "Attended WorksGood lifecycle concierge (the complete expert CLI remains `wg`)",
    after_help = "PRODUCT BOUNDARY:\n  This concierge does not rename or replace the complete `wg` expert CLI.\n  Internal lifecycle operations use one authenticated absolute sibling `wg` executable.\n\nBare attended use runs setup/reconcile and opens the TUI. Bare non-TTY use refuses without mutation."
)]
struct Cli {
    /// Repository/worktree target. Never falls back to ~/.wg.
    #[arg(long, global = true)]
    project: Option<PathBuf>,

    /// Print one immutable redacted plan; write absolutely nothing.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Copy one exact Pi route to every LLM role (simple path).
    /// Worker/chat reasoning defaults high; Eval/assign/FLIP/weak defaults low.
    #[arg(
        long,
        global = true,
        value_name = "pi:<provider>:<model>",
        conflicts_with_all = ["without_ai", "profile", "strong_model", "weak_model"]
    )]
    model: Option<String>,

    /// Select an existing reusable base profile (advanced customization path).
    #[arg(long, global = true)]
    profile: Option<String>,

    /// Explicitly open a setup-neutral graph/TUI with no LLM service.
    #[arg(long, global = true, conflicts_with_all = ["profile", "model"])]
    without_ai: bool,

    /// Exact Pi Worker/chat handler-first route for an existing --profile.
    #[arg(long, global = true, requires = "profile", conflicts_with = "model")]
    strong_model: Option<String>,

    /// Exact Pi Agency/FLIP/evaluation route. Use the same exact value as
    /// --strong-model to explicitly choose “Same as worker”.
    #[arg(long, global = true, requires = "strong_model")]
    weak_model: Option<String>,

    /// Worker/chat effort. With --model, defaults to the explicit shorthand policy: high.
    #[arg(long, global = true)]
    strong_reasoning: Option<ReasoningLevel>,

    /// Eval/assign/FLIP/weak-role effort. With --model, defaults to low.
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
    /// Re-enter profile selection and commit setup; does not open the TUI.
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.model.is_some()
        && matches!(
            cli.command,
            Some(Commands::Status | Commands::Stop | Commands::Restart | Commands::Tui)
        )
    {
        anyhow::bail!(
            "--model is a setup/reconcile option; use it with bare `worksgood` or `worksgood setup`"
        );
    }
    match cli.command {
        None => concierge::run_lifecycle(&lifecycle_options(&cli, true), false),
        Some(Commands::Setup { rollback }) => {
            if rollback {
                concierge::run_rollback(cli.project.as_deref())
            } else {
                concierge::run_lifecycle(&lifecycle_options(&cli, false), true)
            }
        }
        Some(Commands::Status) => concierge::run_status(cli.project.as_deref()),
        Some(Commands::Stop) => concierge::run_stop(cli.project.as_deref()),
        Some(Commands::Restart) => concierge::run_restart(cli.project.as_deref(), cli.yes),
        Some(Commands::Tui) => concierge::run_tui(cli.project.as_deref()),
    }
}
