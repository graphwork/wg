pub mod async_fs;
pub mod bootstrap;
pub mod chat_palette;
pub mod chat_tab_state;
pub mod event;
pub mod file_browser;
#[allow(dead_code)]
pub mod file_browser_render;
pub mod log_render;
pub mod render;
pub mod screen_dump;
pub mod snapshot_engine;
pub mod state;
pub mod trace;

#[cfg(test)]
mod editor_tests;

#[cfg(test)]
mod scroll_mode_tests;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::state::VizApp;
use crate::commands::viz::VizOptions;

/// Returns true when running inside an asciinema recording session.
fn detect_asciinema() -> bool {
    std::env::var_os("ASCIINEMA_REC").is_some()
}

/// Run the viz viewer TUI.
///
/// `mouse_override`: `Some(false)` to force mouse off (--no-mouse),
/// `None` for default (enabled).
///
/// `recording`: when true (or auto-detected via `ASCIINEMA_REC`), disables
/// mouse capture and keyboard enhancement queries that produce escape
/// sequences incompatible with asciinema recording/playback.
///
/// `trace_path`: when `Some`, record all input events to the given JSONL file.
#[allow(clippy::too_many_arguments)]
pub fn run(
    workgraph_dir: PathBuf,
    viz_options: VizOptions,
    mouse_override: Option<bool>,
    recording: bool,
    trace_path: Option<PathBuf>,
    show_keys: bool,
    history_depth: Option<usize>,
    no_history: bool,
) -> Result<()> {
    // Check if stdout is a terminal before any terminal operations to avoid "open terminal failed" errors
    if !crossterm::tty::IsTty::is_tty(&io::stdout()) {
        return Err(anyhow::anyhow!(
            "Cannot create TUI: stdout is not a terminal (this is normal in test/CI environments)"
        ));
    }

    let recording = recording || detect_asciinema();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    enable_raw_mode().context(
        "failed to enable raw mode — is this an interactive terminal?\n\
         Hint: `wg tui` requires a real terminal (not a pipe or agent context)",
    )?;
    // EnableFocusChange (DECSET 1004) makes tmux forward the outer terminal's
    // focus-in/out reports into wg's pane (when `focus-events on`). That focus-in
    // signal is what the event loop needs to re-assert its input grab and close
    // the post-focus-in keystroke-leak window (the user's first key being parsed
    // by tmux instead of wg). Harmless on terminals that don't report focus.
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange
    )?;

    // Enable kitty keyboard protocol if supported — this lets us distinguish
    // Shift+Enter from Enter (and other modified special keys).
    // Skip in recording mode: the query/response escape sequences pollute
    // the .cast file and can confuse asciinema-player.
    let has_keyboard_enhancement = if recording {
        false
    } else {
        supports_keyboard_enhancement().unwrap_or(false)
    };
    if has_keyboard_enhancement {
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to create terminal for TUI")?;

    // In recording mode, force mouse off — mouse escape sequences are not
    // useful in recordings and some asciinema-player versions render them
    // as visible artifacts.
    let effective_mouse = if recording {
        Some(false)
    } else {
        mouse_override
    };
    let mut app = VizApp::new(
        workgraph_dir.clone(),
        viz_options,
        effective_mouse,
        history_depth,
        no_history,
    );
    app.has_keyboard_enhancement = has_keyboard_enhancement;
    app.key_feedback_enabled = show_keys;

    // Paint the storage-independent shell before starting any project I/O,
    // watcher/poller, dump socket, daemon probe, or trace file.  In
    // particular, `.wg` may be on an NFS mount whose first metadata call takes
    // seconds; terminal input must already be live by then.
    terminal.draw(|frame| render::draw(frame, &mut app))?;

    app.start_bootstrap(trace_path, show_keys);

    // The dump socket is diagnostic only.  Its setup touches `.wg/service`,
    // so detach it from the input/render thread after the first frame.
    let shared_screen = screen_dump::new_shared_screen();
    let dump_shutdown = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    let dump_server_started = {
        let workgraph_dir = workgraph_dir.clone();
        let shared_screen = shared_screen.clone();
        let dump_shutdown = dump_shutdown.clone();
        std::thread::Builder::new()
            .name("wg-tui-dump-start".into())
            .spawn(move || {
                let _ = screen_dump::start_server(&workgraph_dir, shared_screen, dump_shutdown);
            })
            .is_ok()
    };
    #[cfg(not(unix))]
    let dump_server_started = false;
    let _ = dump_server_started;

    let result = event::run_event_loop(&mut terminal, &mut app, &shared_screen);

    // Signal the dump server to shut down and clean up the socket.
    dump_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);

    let _ = restore_terminal();

    result
}

fn restore_terminal() -> Result<()> {
    use io::Write;
    // Best-effort cleanup: don't short-circuit on individual failures
    // so that later steps still run even if an earlier one fails.
    let r1 = disable_raw_mode();
    // Disable mouse modes with raw escape sequences (matching event.rs set_mouse_capture)
    let r2 = io::stdout().write_all(b"\x1b[?1003l\x1b[?1006l\x1b[?1002l");
    // Pop kitty keyboard enhancement (no-op if it wasn't pushed).
    let r3 = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    let r4 = execute!(
        io::stdout(),
        DisableFocusChange,
        LeaveAlternateScreen,
        DisableBracketedPaste
    );
    r1?;
    r2?;
    let _ = r3; // Ignore error — may not have been pushed.
    r4?;
    Ok(())
}
