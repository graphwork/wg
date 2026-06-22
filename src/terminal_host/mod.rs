//! Generic terminal-host trait — the WG-owned layer for hosting interactive,
//! terminal-grabbing child tools (pi, the `claude`/`codex` CLIs, `aider`,
//! `opencode`, arbitrary REPLs) behind a single interface.
//!
//! This module is the **Axis-2 fallback for pi** and the **primary path for
//! every non-plugin tool** (`docs/pi-integration/integration-plan-v2.md` §3.1,
//! `docs/pi-integration/terminal-host-research.md` §4/§5).
//!
//! ## What this module is (and is not)
//!
//! This is the **trait foundation only** (the `terminal-host-trait` task from
//! `terminal-host-research.md` §6.1). It introduces three pieces:
//!
//! 1. [`HostedChild`] — *what to run* (command/args/env/cwd/session id),
//!    independent of *how* it is hosted.
//! 2. [`TerminalProfile`] — declarative, **pure data** describing *what kind of
//!    terminal citizen* a tool is. This is the single source of truth that
//!    today is scattered across `executor_uses_child_scroll_keys`,
//!    `build_*_chat_pty_args`, and the per-tool handler files. It contains **no
//!    control flow** — the host reads it to pick a strategy.
//! 3. [`TerminalHost`] — a trait with **one method per hosting mode** (a–e from
//!    the research doc).
//!
//! The default implementation, [`PtyTerminalHost`], **delegates to today's
//! [`PtyPane`]** (`src/tui/pty_pane.rs`) for the embed mode — there is **no
//! behavior change and no rewrite of any existing path** in this foundation.
//!
//! The full generic port-out (port-embed, handoff, port-headless, the
//! standalone full-screen host) is owned by `terminal-host-research.md` §6 as
//! its **own** track and is intentionally **not** implemented here — those
//! modes currently return [`HostError::Unsupported`]. The auxiliary handle
//! types ([`DetachedHandle`], [`RpcChannel`], [`OuterTerminal`],
//! [`CaptureSpec`]) are `#[non_exhaustive]` foundation placeholders the §6
//! tasks flesh out.

use std::path::{Path, PathBuf};

use portable_pty::PtySize;

use crate::tui::pty_pane::{self, PtyPane};

/// What to run — independent of *how* it is hosted.
///
/// Credentials travel via [`env`](HostedChild::env), never argv
/// (`executor-research.md` B5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostedChild {
    /// Program to execute (binary name or absolute path).
    pub command: String,
    /// Arguments passed to the program, in order.
    pub args: Vec<String>,
    /// Environment overrides applied on top of the inherited environment.
    /// Credentials belong here (by env), never in [`args`](HostedChild::args).
    pub env: Vec<(String, String)>,
    /// Working directory to pin the child to. `None` ⇒ inherit WG's cwd
    /// (matches `PtyPane::spawn`).
    pub cwd: Option<PathBuf>,
    /// Optional session id. When set (and tmux is available), the embed
    /// backend uses a detached tmux session named this so the child survives
    /// WG restarts and a human can `tmux attach`; otherwise embed uses a
    /// direct `portable-pty` child.
    pub session_id: Option<String>,
}

impl HostedChild {
    /// Start a spec for `command` with no args/env/cwd/session.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            session_id: None,
        }
    }

    /// Builder: set the argument list.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: set the environment overrides.
    pub fn env<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env = env.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        self
    }

    /// Builder: pin the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Builder: set the tmux session id (enables the detached-tmux embed
    /// backend when tmux is present).
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Borrow this spec in the **exact shape** [`PtyPane::spawn_in`] /
    /// [`PtyPane::spawn_via_tmux`] accept: `(command, args, env, cwd)`.
    ///
    /// This is the single mapping point [`PtyTerminalHost::embed`] uses,
    /// factored out so a unit test can assert it is byte-identical to a
    /// hand-written `PtyPane::spawn_in` call — the "constructs the same
    /// portable-pty child PtyPane builds today" guarantee.
    fn spawn_args(&self) -> (&str, Vec<&str>, &[(String, String)], Option<&Path>) {
        (
            self.command.as_str(),
            self.args.iter().map(String::as_str).collect(),
            &self.env,
            self.cwd.as_deref(),
        )
    }
}

/// What kind of terminal citizen a tool is. **Pure data** — the single source
/// of truth that today is spread across `executor_uses_child_scroll_keys`
/// (`state.rs:1391`), `build_*_chat_pty_args`, and the handler files. Contains
/// no control flow; the host reads these fields to choose a launch context and
/// PTY backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalProfile {
    /// Tool takes the alternate screen (`opencode`) vs renders inline/line
    /// (`claude`/`codex`/`nex`/pi). Drives the scroll strategy: child scroll
    /// keys vs tmux copy-mode (replaces `executor_uses_child_scroll_keys`).
    pub alt_screen: bool,
    /// Tool emits DA/XTVERSION/DECRQM and blocks until answered (`claude`).
    /// When `true`, the host's capability-query responder is mandatory
    /// (`pty_pane.rs:243-249`).
    pub needs_capability_replies: bool,
    /// How to force headless: a flag (e.g. `["-p"]`, or `["--mode","rpc"]`).
    /// `None` ⇒ withholding a TTY is sufficient on its own.
    pub headless_flag: Option<Vec<String>>,
    /// Tool speaks a line-delimited JSONL/RPC protocol when headless
    /// (`pi --mode rpc`, `opencode`) ⇒ eligible for mode d (`open_protocol`).
    pub rpc_capable: bool,
    /// Tool exits non-zero on error when headless, so a supervisor timeout +
    /// exit code is a reliable failure signal. Interactive mode does not.
    pub exits_on_error_headless: bool,
    /// Animated full-screen repaints bracketed by DEC-2026 synchronized output
    /// (`codex`) ⇒ enable the sync-mode scrollback trim
    /// (`pty_pane.rs:262-270`).
    pub sync_mode_repaints: bool,
}

impl TerminalProfile {
    /// pi's terminal profile (`integration-plan-v2.md` §3.1,
    /// `terminal-host-research.md` §5.3): inline repaint (no alt-screen),
    /// capability replies required when embedded, RPC-capable headless via
    /// `--mode rpc`, exits non-zero on error when headless.
    pub fn pi() -> Self {
        Self {
            alt_screen: false,
            needs_capability_replies: true,
            headless_flag: Some(vec!["--mode".to_string(), "rpc".to_string()]),
            rpc_capable: true,
            exits_on_error_headless: true,
            sync_mode_repaints: false,
        }
    }
}

/// Errors a [`TerminalHost`] can return.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The child could not be spawned (PTY allocation or `spawn` failed).
    #[error("failed to spawn hosted child: {0}")]
    Spawn(anyhow::Error),
    /// The child did not complete within its supervisor budget.
    #[error("hosted child timed out")]
    Timeout,
    /// The child exited with a non-zero status.
    #[error("hosted child exited non-zero: {0}")]
    NonZeroExit(i32),
    /// A tmux-backed mode was requested but tmux is not on `PATH`.
    #[error("tmux is not available on PATH")]
    TmuxUnavailable,
    /// The requested hosting mode is not available in the trait foundation.
    /// The named mode is owned by the `terminal-host-research.md` §6 port-out
    /// track (`terminal-host-port-embed` / `-handoff` / `-port-headless`).
    #[error(
        "terminal-host mode not yet available in the trait foundation: {0} \
         (owned by terminal-host-research §6 port-out track)"
    )]
    Unsupported(&'static str),
}

/// Capture configuration for [`TerminalHost::run_headless`] (mode b).
///
/// **Foundation placeholder.** The field set (run dir, stdout/stderr tee
/// targets, `wg done` bookends) is owned by `terminal-host-port-headless`
/// (`terminal-host-research.md` §6.4).
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct CaptureSpec {}

/// Handle to a detached, headless child from [`TerminalHost::run_headless`]
/// (mode b).
///
/// **Foundation placeholder.** Mirrors the worker-spawn result (pid, run dir,
/// exit polling); fleshed out by `terminal-host-port-headless`.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct DetachedHandle {}

/// A framed JSONL/RPC channel to a piped child from
/// [`TerminalHost::open_protocol`] (mode d).
///
/// **Foundation placeholder.** The framing (LF-delimited `read_until`, reply
/// extraction) is owned by `terminal-host-port-headless`; today's behavior
/// lives in `opencode_handler.rs`.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct RpcChannel {}

/// WG's outer terminal state, saved/restored around a [`TerminalHost::handoff`]
/// (mode c).
///
/// **Foundation placeholder.** `suspend`/`resume` (extracted from
/// `viz_viewer/mod.rs:162-167`) are owned by `terminal-host-handoff`
/// (`terminal-host-research.md` §6.3).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct OuterTerminal {}

/// A host for interactive, terminal-grabbing child tools, with **one method
/// per hosting mode** (a–e from `terminal-host-research.md` §3/§4).
///
/// Executors pick a method; the host applies the right launch context and PTY
/// backend. The per-tool [`TerminalProfile`] carries the quirks so no
/// tool-specific control flow leaks into the host.
pub trait TerminalHost {
    /// **(a) Embed** an interactive child in a TUI pane: the child gets its own
    /// *private* PTY (raw-mode grab contained), rendered into `size`. The
    /// backend is tmux-wrapped when [`HostedChild::session_id`] is set and tmux
    /// is available, else a direct `portable-pty` child.
    fn embed(
        &mut self,
        child: HostedChild,
        profile: &TerminalProfile,
        size: PtySize,
    ) -> Result<PtyPane, HostError>;

    /// **(b) Headless / detached** long-run: null/file stdio, `setsid`, wrapper
    /// capture. No TTY ⇒ a well-behaved tool self-selects its headless mode.
    fn run_headless(
        &mut self,
        child: HostedChild,
        profile: &TerminalProfile,
        capture: CaptureSpec,
    ) -> Result<DetachedHandle, HostError>;

    /// **(c) Handoff**: lend the real terminal to the child, run to completion,
    /// then restore WG's TUI (`term` saves/restores outer terminal state).
    fn handoff(
        &mut self,
        child: HostedChild,
        profile: &TerminalProfile,
        term: &mut OuterTerminal,
    ) -> Result<std::process::ExitStatus, HostError>;

    /// **(d) Protocol**: piped stdio, never a PTY; returns a framed JSONL/RPC
    /// channel.
    fn open_protocol(
        &mut self,
        child: HostedChild,
        profile: &TerminalProfile,
    ) -> Result<RpcChannel, HostError>;

    /// **(e) Standalone PTY host**: one child filling the window (mode-a with
    /// `area` = whole screen + an own ratatui shell).
    fn host_fullscreen(
        &mut self,
        child: HostedChild,
        profile: &TerminalProfile,
    ) -> Result<std::process::ExitStatus, HostError>;
}

/// The default [`TerminalHost`], backed by WG's existing [`PtyPane`]
/// (`portable-pty` + `vt100` + optional tmux-wrap).
///
/// In the trait foundation only [`embed`](PtyTerminalHost::embed) is wired to
/// real behavior — it **delegates to [`PtyPane`]** with no behavior change. The
/// other modes are owned by the `terminal-host-research.md` §6 port-out track
/// and currently return [`HostError::Unsupported`].
#[derive(Debug, Default, Clone, Copy)]
pub struct PtyTerminalHost;

impl PtyTerminalHost {
    /// Create the default PTY-backed terminal host.
    pub fn new() -> Self {
        Self
    }
}

impl TerminalHost for PtyTerminalHost {
    fn embed(
        &mut self,
        child: HostedChild,
        // The profile is threaded through for the §6 `terminal-host-port-embed`
        // task (alt-screen → child-scroll-keys, capability replies, sync-mode
        // trim). The foundation deliberately applies *no* profile-driven
        // behavior — embed spawns exactly as the current code does.
        _profile: &TerminalProfile,
        size: PtySize,
    ) -> Result<PtyPane, HostError> {
        let (command, args, env, cwd) = child.spawn_args();
        let use_tmux = child.session_id.is_some() && pty_pane::tmux_available();
        let pane = if use_tmux {
            // `is_some()` checked above.
            let session = child.session_id.as_deref().unwrap_or_default();
            PtyPane::spawn_via_tmux(session, command, &args, env, cwd, size.rows, size.cols)
        } else {
            PtyPane::spawn_in(command, &args, env, cwd, size.rows, size.cols)
        };
        pane.map_err(HostError::Spawn)
    }

    fn run_headless(
        &mut self,
        _child: HostedChild,
        _profile: &TerminalProfile,
        _capture: CaptureSpec,
    ) -> Result<DetachedHandle, HostError> {
        Err(HostError::Unsupported("run_headless (mode b)"))
    }

    fn handoff(
        &mut self,
        _child: HostedChild,
        _profile: &TerminalProfile,
        _term: &mut OuterTerminal,
    ) -> Result<std::process::ExitStatus, HostError> {
        Err(HostError::Unsupported("handoff (mode c)"))
    }

    fn open_protocol(
        &mut self,
        _child: HostedChild,
        _profile: &TerminalProfile,
    ) -> Result<RpcChannel, HostError> {
        Err(HostError::Unsupported("open_protocol (mode d)"))
    }

    fn host_fullscreen(
        &mut self,
        _child: HostedChild,
        _profile: &TerminalProfile,
    ) -> Result<std::process::ExitStatus, HostError> {
        Err(HostError::Unsupported("host_fullscreen (mode e)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pty_size(rows: u16, cols: u16) -> PtySize {
        PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Render a pane to a `TestBackend` repeatedly until `needle` appears or we
    /// time out. The child's output arrives asynchronously on the reader
    /// thread, so we poll — same approach as `pty_pane.rs`'s spawn tests.
    #[cfg(unix)]
    fn render_until_contains(pane: &PtyPane, w: u16, h: u16, needle: &str) -> bool {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    pane.render(frame, area);
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let mut rendered = String::new();
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    rendered.push_str(buf[(x, y)].symbol());
                }
                rendered.push('\n');
            }
            if rendered.contains(needle) {
                return true;
            }
        }
        false
    }

    /// Validation #1/#2: `embed()` of a known tool constructs the same
    /// portable-pty child `PtyPane` builds today.
    ///
    /// Two assertions:
    /// 1. The `HostedChild` → spawn-args mapping is byte-identical to a
    ///    hand-written `PtyPane::spawn_in` call (deterministic, CI-safe).
    /// 2. `embed()` actually produces a live, rendering `PtyPane` whose output
    ///    matches the direct `PtyPane::spawn_in` path — i.e. it *delegates*
    ///    rather than re-implementing.
    #[cfg(unix)]
    #[test]
    fn test_terminal_host_embed_delegates_to_ptypane() {
        // (1) Exact spawn-arg mapping.
        let child = HostedChild::new("/bin/echo")
            .args(["hello from host"])
            .env([("WG_TH_TEST", "1")]);
        let (command, args, env, cwd) = child.spawn_args();
        assert_eq!(command, "/bin/echo");
        assert_eq!(args, vec!["hello from host"]);
        assert_eq!(env, &[("WG_TH_TEST".to_string(), "1".to_string())][..]);
        assert_eq!(cwd, None);

        // (2) embed() delegates to a real PtyPane that renders the child's
        //     output. No session_id ⇒ direct portable-pty backend, the same
        //     path PtyPane::spawn_in drives today.
        let mut host = PtyTerminalHost::new();
        let profile = TerminalProfile::pi();
        let via_host = host
            .embed(child, &profile, pty_size(5, 40))
            .expect("embed should delegate to PtyPane::spawn_in");
        assert!(
            render_until_contains(&via_host, 40, 5, "hello from host"),
            "pane spawned via TerminalHost::embed did not render child output"
        );

        // The direct PtyPane path renders the identical marker — confirming
        // embed constructs the same child, not a divergent one.
        let direct = PtyPane::spawn_in("/bin/echo", &["hello from host"], &[], None, 5, 40)
            .expect("direct PtyPane::spawn_in");
        assert!(
            render_until_contains(&direct, 40, 5, "hello from host"),
            "direct PtyPane::spawn_in did not render child output"
        );
    }

    /// Validation #3: `TerminalProfile` is pure data; the pi profile literal
    /// from the task compiles and matches [`TerminalProfile::pi`].
    #[test]
    fn test_terminal_profile_pi_literal_is_pure_data() {
        let pi = TerminalProfile {
            alt_screen: false,
            needs_capability_replies: true,
            headless_flag: Some(vec!["--mode".to_string(), "rpc".to_string()]),
            rpc_capable: true,
            exits_on_error_headless: true,
            sync_mode_repaints: false,
        };
        assert_eq!(pi, TerminalProfile::pi());
        assert!(!pi.alt_screen);
        assert!(pi.needs_capability_replies);
        assert_eq!(
            pi.headless_flag.as_deref(),
            Some(&["--mode".to_string(), "rpc".to_string()][..])
        );
        assert!(pi.rpc_capable);
        assert!(pi.exits_on_error_headless);
        assert!(!pi.sync_mode_repaints);
    }

    /// The trait carries one method per mode; in the foundation only `embed`
    /// is wired. The other four are explicit, well-typed `Unsupported` stubs
    /// (owned by the §6 port-out track) — never silently no-op.
    #[test]
    fn test_nonembed_modes_are_unsupported_in_foundation() {
        let mut host = PtyTerminalHost::new();
        let profile = TerminalProfile::pi();
        let spec = || HostedChild::new("/bin/true");

        assert!(matches!(
            host.run_headless(spec(), &profile, CaptureSpec::default()),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            host.open_protocol(spec(), &profile),
            Err(HostError::Unsupported(_))
        ));
        let mut term = OuterTerminal::default();
        assert!(matches!(
            host.handoff(spec(), &profile, &mut term),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            host.host_fullscreen(spec(), &profile),
            Err(HostError::Unsupported(_))
        ));
    }

    /// `HostedChild` builders compose to the expected spec.
    #[test]
    fn test_hosted_child_builders() {
        let child = HostedChild::new("pi")
            .args(["--mode", "rpc"])
            .env([("WG_TASK_ID", "t-1")])
            .cwd("/tmp/work")
            .session_id("wg-chat-1");
        assert_eq!(child.command, "pi");
        assert_eq!(child.args, vec!["--mode".to_string(), "rpc".to_string()]);
        assert_eq!(
            child.env,
            vec![("WG_TASK_ID".to_string(), "t-1".to_string())]
        );
        assert_eq!(child.cwd.as_deref(), Some(Path::new("/tmp/work")));
        assert_eq!(child.session_id.as_deref(), Some("wg-chat-1"));
    }
}
