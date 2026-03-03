pub mod event;
pub mod render;
pub mod state;

use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::state::VizApp;
use crate::commands::viz::VizOptions;

/// Run the viz viewer TUI.
///
/// `mouse_override`: `Some(false)` to force mouse off (--no-mouse),
/// `None` for auto-detection (disabled in tmux splits).
pub fn run(
    workgraph_dir: PathBuf,
    viz_options: VizOptions,
    mouse_override: Option<bool>,
) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    enable_raw_mode().context(
        "failed to enable raw mode — is this an interactive terminal?\n\
         Hint: `wg tui` requires a real terminal (not a pipe or agent context)",
    )?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to create terminal for TUI")?;
    let mut app = VizApp::new(workgraph_dir, viz_options, mouse_override);
    let result = event::run_event_loop(&mut terminal, &mut app);

    let _ = restore_terminal();

    result
}

fn restore_terminal() -> Result<()> {
    // Best-effort cleanup: don't short-circuit on individual failures
    // so that later steps still run even if an earlier one fails.
    let r1 = disable_raw_mode();
    let r2 = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        DisableBracketedPaste
    );
    r1?;
    r2?;
    Ok(())
}
