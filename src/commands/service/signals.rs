//! Daemon signal handling — make the daemon's death **loud**.
//!
//! ## Why this exists (`fix-coordinator-daemon`)
//!
//! Before this module the daemon installed a panic hook but **no signal
//! handlers**. A stray `SIGTERM` / `SIGHUP` / `SIGINT` / `SIGQUIT` terminated
//! the process with the default disposition: the panic hook does not run for
//! signals, `run_daemon` never returns, and nothing reached `daemon.log`. The
//! PID simply vanished mid coordinator tick — the recurring "silent death".
//!
//! `setsid()` (in `run_start`'s `pre_exec`) already detaches the daemon from
//! the launching terminal so a close-PTY `SIGHUP` no longer reaches it, but it
//! does not install handlers, so any other signal source still killed it
//! silently.
//!
//! ## Design
//!
//! Async-signal-safe handlers write the caught signal number (one byte) to the
//! write end of a self-pipe. The daemon's main loop already polls a self-pipe
//! for the graph watcher; it polls this signal pipe alongside it and, when it
//! fires, logs a clear `WARN`/`FATAL` line naming the signal. This is the
//! self-pipe trick: the handler does only `libc::write` (async-signal-safe);
//! all logging / mutex / allocation happens in the main loop.
//!
//! The handlers do **not** themselves terminate the process — they only notify
//! the main loop, which decides what to do per [`SignalPolicy`]. That keeps the
//! daemon alive across stray terminal/session signals (the primary fix) while
//! still surfacing every signal in the log.

#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(unix)]
static SIGNAL_PIPE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// Policy the main loop applies when it observes a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDisposition {
    /// Log and keep ticking. Used for terminal/session stray signals that a
    /// detached daemon should shrug off — this is what stops the silent death.
    Survive,
    /// Log and shut down gracefully (without the clean-sentinel, so the
    /// supervisor restarts). Used for `SIGTERM`.
    Restart,
}

#[cfg(unix)]
extern "C" fn handle_signal(sig: libc::c_int) {
    // Async-signal-safe: a single `write(2)` of one byte. No locks, no alloc,
    // no std. If the pipe is full or not installed, the byte is dropped — the
    // main loop's own poll cadence still makes progress.
    let fd = SIGNAL_PIPE_WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let buf: [u8; 1] = [sig as u8];
        unsafe {
            let _ = libc::write(fd, buf.as_ptr() as *const libc::c_void, 1);
        }
    }
}

/// Human-readable name for a signal number (best-effort, for log lines).
pub fn signal_name(sig: i32) -> &'static str {
    #[cfg(unix)]
    {
        match sig {
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            libc::SIGTERM => "SIGTERM",
            libc::SIGPIPE => "SIGPIPE",
            _ => "UNKNOWN",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = sig;
        "SIGNAL"
    }
}

/// Decide what the main loop should do for a given signal.
pub fn disposition_for(sig: i32) -> SignalDisposition {
    #[cfg(unix)]
    {
        match sig {
            // A detached daemon survives terminal/session signals. Reaching one
            // is suspicious (the launching terminal is gone) but not fatal.
            libc::SIGHUP | libc::SIGINT | libc::SIGQUIT | libc::SIGPIPE => {
                SignalDisposition::Survive
            }
            // SIGTERM conventionally means "stop". We honor it with a graceful
            // shutdown, but without the clean-sentinel so the supervisor
            // restarts — `wg service stop` (IPC) is the clean-stop path.
            libc::SIGTERM => SignalDisposition::Restart,
            _ => SignalDisposition::Survive,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = sig;
        SignalDisposition::Survive
    }
}

/// Create the self-pipe with both ends `FD_CLOEXEC` and `O_NONBLOCK`.
///
/// `pipe2` does both atomically but is a Linux/BSD extension — macOS has no such
/// symbol, so calling it directly under a bare `cfg(unix)` breaks the build on
/// Darwin entirely. Returns 0 on success, -1 on failure, matching `pipe2`.
#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn make_signal_pipe(fds: &mut [std::os::unix::io::RawFd; 2]) -> i32 {
    unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }
}

/// Darwin fallback: `pipe()` then set the flags on each end.
///
/// NOT atomic — a `fork` racing between `pipe` and the `fcntl` calls could
/// inherit an un-CLOEXEC'd descriptor. There is no `pipe2` on this platform to
/// close that window, and the handlers are installed during daemon startup
/// before any agent is spawned, so the race is not reachable here.
#[cfg(target_os = "macos")]
unsafe fn make_signal_pipe(fds: &mut [std::os::unix::io::RawFd; 2]) -> i32 {
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return -1;
        }
        for &fd in fds.iter() {
            // Preserve the existing status flags; F_SETFL replaces the whole set.
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags == -1
                || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1
                || libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1
            {
                libc::close(fds[0]);
                libc::close(fds[1]);
                return -1;
            }
        }
        0
    }
}

/// Install the daemon signal handlers and return the **read** end of the
/// self-pipe (or `-1` on non-Unix / failure). The write end is stored in a
/// static for the handler. Both ends are `FD_CLOEXEC` so a forked agent does
/// not inherit them.
///
/// `SIGPIPE` is forced to `SIG_IGN` defensively (Rust's runtime does this, but
/// a C library can reset it); a handler is still installed so a fire is logged.
#[cfg(unix)]
pub fn install_daemon_signal_handlers() -> i32 {
    use std::os::unix::io::RawFd;

    let mut fds: [RawFd; 2] = [-1, -1];
    // CLOEXEC + NONBLOCK so agent children don't inherit the pipe and the handler
    // never blocks. `pipe2` sets both atomically but does not exist on macOS —
    // this function is `cfg(unix)`, and macOS is unix, so the direct call made the
    // whole binary fail to compile there (`cannot find function pipe2 in libc`).
    let rc = unsafe { make_signal_pipe(&mut fds) };
    if rc != 0 {
        return -1;
    }
    let read_fd = fds[0];
    let write_fd = fds[1];
    SIGNAL_PIPE_WRITE_FD.store(write_fd, Ordering::SeqCst);

    // SIGPIPE: ignore defensively. (Handler below still logs a fire via the
    // pipe, but the disposition table says Survive, so the daemon never dies
    // from it.)
    unsafe {
        let mut ign: libc::sigaction = std::mem::zeroed();
        ign.sa_sigaction = libc::SIG_IGN;
        libc::sigaction(libc::SIGPIPE, &ign, std::ptr::null_mut());
    }

    let install = |sig: libc::c_int| {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            // Cast through a typed fn-pointer to avoid the
            // `function item as integer` warning and stay portable.
            let handler: extern "C" fn(libc::c_int) = handle_signal;
            sa.sa_sigaction = handler as usize;
            // No SA_RESTART: we want blocking syscalls (poll/read) to EINTR so
            // the main loop wakes promptly to log the signal. The main loop
            // already handles EINTR / non-blocking retries.
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    };
    install(libc::SIGHUP);
    install(libc::SIGINT);
    install(libc::SIGQUIT);
    install(libc::SIGTERM);
    install(libc::SIGPIPE);

    read_fd
}

#[cfg(not(unix))]
pub fn install_daemon_signal_handlers() -> i32 {
    -1
}

/// Drain pending signal bytes from the pipe. Returns the signal numbers seen.
/// Non-blocking: returns an empty vec when nothing is pending.
#[cfg(unix)]
pub fn drain_signal_pipe(read_fd: i32) -> Vec<i32> {
    if read_fd < 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 16];
    loop {
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        for &b in &buf[..n as usize] {
            out.push(b as i32);
        }
    }
    out
}

#[cfg(not(unix))]
pub fn drain_signal_pipe(_read_fd: i32) -> Vec<i32> {
    Vec::new()
}
