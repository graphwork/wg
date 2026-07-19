pub mod async_fs;
pub mod auxiliary;
pub mod bootstrap;
pub mod chat_palette;
pub mod chat_startup;
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

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::state::VizApp;
use crate::commands::viz::VizOptions;

/// Returns true when running inside an asciinema recording session.
fn detect_asciinema() -> bool {
    std::env::var_os("ASCIINEMA_REC").is_some()
}

/// Policy for requesting enhanced keyboard input from the *outer* terminal.
///
/// Mosh transports screen state rather than a byte-transparent terminal
/// stream. Its Kitty/CSI-u handling is not reliable enough to distinguish a
/// physical Enter from Shift+Enter, including when mosh launches or attaches
/// tmux. Environment markers survive that tmux hop, so decide once at startup
/// and never infer the transport from mutable chat/runtime metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OuterKeyboardEnhancementPolicy {
    EnabledForReliableTransport,
    DisabledForRecording,
    DisabledForMosh,
}

impl OuterKeyboardEnhancementPolicy {
    fn detect_with(recording: bool, mut env_var: impl FnMut(&str) -> Option<OsString>) -> Self {
        if recording {
            return Self::DisabledForRecording;
        }
        // MOSH_SERVER_PID is the canonical marker used elsewhere in the TUI.
        // MOSH_IP covers mosh installations/wrappers that expose the peer but
        // not the server pid. Do not use MOSH_PREDICTION_DISPLAY: users often
        // export it globally even for ordinary SSH/local terminals.
        if env_var("MOSH_SERVER_PID").is_some() || env_var("MOSH_IP").is_some() {
            return Self::DisabledForMosh;
        }
        Self::EnabledForReliableTransport
    }

    fn detect(recording: bool) -> Self {
        Self::detect_with(recording, |name| std::env::var_os(name))
    }

    fn should_enable(self) -> bool {
        matches!(self, Self::EnabledForReliableTransport)
    }
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
    let outer_keyboard_policy = OuterKeyboardEnhancementPolicy::detect(recording);
    let keyboard_enhancement_pushed = Arc::new(AtomicBool::new(false));

    let original_hook = std::panic::take_hook();
    let panic_keyboard_enhancement_pushed = keyboard_enhancement_pushed.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal(panic_keyboard_enhancement_pushed.load(Ordering::Relaxed));
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

    // Enable Kitty keyboard disambiguation only across a byte-reliable outer
    // transport. Do not synchronously query support here: crossterm's query
    // waits up to two seconds when a terminal (notably a detached tmux pane)
    // does not answer, which prevents both the neutral first frame and input
    // handling from starting. The protocol's push sequence is itself a safe
    // capability request — supporting terminals enable it and other ANSI
    // terminals ignore it. In particular, emit nothing through mosh: support
    // beyond mosh does not make mosh a reliable CSI-u carrier. Shift+Enter is
    // therefore unavailable there and Ctrl+J remains the reliable fallback.
    let has_keyboard_enhancement = outer_keyboard_policy.should_enable()
        && execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
    keyboard_enhancement_pushed.store(has_keyboard_enhancement, Ordering::Relaxed);

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

    let _ = restore_terminal(keyboard_enhancement_pushed.load(Ordering::Relaxed));

    result
}

fn restore_terminal(keyboard_enhancement_pushed: bool) -> Result<()> {
    use io::Write;
    // Best-effort cleanup: don't short-circuit on individual failures
    // so that later steps still run even if an earlier one fails.
    let r1 = disable_raw_mode();
    // Disable mouse modes with raw escape sequences (matching event.rs set_mouse_capture)
    let r2 = io::stdout().write_all(b"\x1b[?1003l\x1b[?1006l\x1b[?1002l");
    // Pop only WG's own successful push. Emitting a pop after a policy-skipped
    // negotiation could otherwise consume an ancestor terminal/tmux flag.
    let r3 = if keyboard_enhancement_pushed {
        execute!(io::stdout(), PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
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

#[cfg(test)]
mod outer_keyboard_policy_tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(recording: bool, vars: &[(&str, &str)]) -> OuterKeyboardEnhancementPolicy {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        OuterKeyboardEnhancementPolicy::detect_with(recording, |name| {
            vars.get(name).map(OsString::from)
        })
    }

    #[test]
    fn mosh_markers_disable_keyboard_enhancement() {
        for marker in ["MOSH_SERVER_PID", "MOSH_IP"] {
            let policy = detect(false, &[(marker, "present")]);
            assert_eq!(policy, OuterKeyboardEnhancementPolicy::DisabledForMosh);

            assert!(!policy.should_enable());
        }
    }

    #[test]
    fn tmux_alone_enables_but_tmux_over_mosh_disables_it() {
        assert_eq!(
            detect(false, &[("TMUX", "/tmp/tmux-1000/default,1,0")]),
            OuterKeyboardEnhancementPolicy::EnabledForReliableTransport,
            "tmux is capable of forwarding extended keys"
        );
        assert_eq!(
            detect(
                false,
                &[
                    ("TMUX", "/tmp/tmux-1000/default,1,0"),
                    ("MOSH_SERVER_PID", "4242"),
                ],
            ),
            OuterKeyboardEnhancementPolicy::DisabledForMosh,
            "the outer mosh transport remains authoritative through tmux"
        );
    }

    #[test]
    fn non_mosh_terminal_enables_and_recording_never_does() {
        let policy = detect(false, &[("TERM", "xterm-kitty")]);
        assert_eq!(
            policy,
            OuterKeyboardEnhancementPolicy::EnabledForReliableTransport
        );
        assert!(policy.should_enable());

        let recording = detect(true, &[("TERM", "xterm-kitty")]);
        assert_eq!(
            recording,
            OuterKeyboardEnhancementPolicy::DisabledForRecording
        );
        assert!(!recording.should_enable());
    }

    #[test]
    fn storage_independent_app_starts_with_a_neutral_visible_shell() {
        let app = VizApp::new(
            PathBuf::from("storage-must-not-be-read"),
            VizOptions::default(),
            Some(false),
            None,
            true,
        );
        assert_eq!(app.lines, ["0 tasks"]);
        assert_eq!(app.task_counts.total, 0);
        assert!(!app.bootstrap_complete);
    }
}
