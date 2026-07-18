//! Cross-platform replacement for `timeout(1)`.
//!
//! Unix *usually* has `timeout(1)`, which wraps a command and kills it (exit
//! code 124) if it runs past a deadline. But it is not guaranteed: macOS ships
//! NO `timeout` binary by default — coreutils provides it, and then usually
//! under the name `gtimeout`. Windows has no equivalent at all — `TIMEOUT.EXE`
//! is an interactive "press any key to continue, else wait N seconds" utility
//! and fails with "Default option is not allowed more than '1' time(s)" when
//! invoked with `timeout(1)`-style arguments.
//!
//! [`spawn_with_timeout`] hides the platform difference behind one API:
//!
//! - On Unix, when a GNU `timeout` binary is available: prefixes the command
//!   with `<bin> <secs>s`, preserving the exit-code-124-on-timeout semantics
//!   callers may be relying on. The binary is resolved once (cached) by
//!   probing PATH for `gtimeout` then `timeout`, mirroring the shell wrapper's
//!   `command -v gtimeout || command -v timeout` probe in
//!   `src/commands/spawn/execution.rs`.
//! - On Unix with NO `timeout` binary (default macOS-without-coreutils), and
//!   always on Windows: spawns the program directly and arms a background
//!   watchdog thread that kills the child at the deadline. The thread is
//!   disarmed (via an atomic flag) when the returned [`TimeoutGuard`] drops,
//!   so children that exit in time don't risk a PID-reuse race.
//!
//! In the watchdog fallback the child is killed with `SIGKILL` (Unix) /
//! `taskkill /F /T` (Windows) rather than exiting 124 — callers that
//! specifically branch on exit code 124 should not rely on it when no
//! `timeout` binary is present.
//!
//! Callers bind the guard to a `_killer` local that lives as long as the
//! child-wait call:
//!
//! ```ignore
//! let (mut child, _killer) = platform_timeout::spawn_with_timeout(
//!     "claude",
//!     |cmd| cmd.arg("--print").stdin(Stdio::piped()),
//!     30,
//! )?;
//! let output = child.wait_with_output()?;
//! // _killer drops here; disarms the watchdog kill-thread if still pending.
//! ```

use std::ffi::OsStr;
use std::io;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::OnceLock;

/// RAII guard that disarms the watchdog kill-thread when dropped.
///
/// `done` is `Some` only when a watchdog thread was armed (always on Windows;
/// on Unix only in the no-`timeout`-binary fallback). When an external
/// `timeout(1)`/`gtimeout` enforces the deadline there is nothing to disarm and
/// `done` is `None`.
pub struct TimeoutGuard {
    done: Option<Arc<AtomicBool>>,
}

impl Drop for TimeoutGuard {
    fn drop(&mut self) {
        if let Some(done) = &self.done {
            done.store(true, Ordering::Release);
        }
    }
}

/// Spawn `program`, with the `Command` further configured by `configure`,
/// subject to a time budget of `timeout_secs`.
///
/// See the module-level docs for the semantics on each platform. The
/// returned guard must outlive the child-wait call; bind it to a `_killer`
/// local so it drops at the right point.
pub fn spawn_with_timeout<P, F>(
    program: P,
    configure: F,
    timeout_secs: u64,
) -> io::Result<(Child, TimeoutGuard)>
where
    P: AsRef<OsStr>,
    F: FnOnce(&mut Command) -> &mut Command,
{
    #[cfg(unix)]
    {
        if let Some(bin) = resolve_timeout_bin() {
            let mut cmd = Command::new(bin);
            cmd.arg(format!("{}s", timeout_secs)).arg(program);
            configure(&mut cmd);
            let child = cmd.spawn()?;
            return Ok((child, TimeoutGuard { done: None }));
        }
        // No `timeout`/`gtimeout` on PATH (e.g. macOS without coreutils):
        // fall back to the same watchdog-thread strategy used on Windows.
        spawn_with_watchdog(program, configure, timeout_secs)
    }
    #[cfg(windows)]
    {
        spawn_with_watchdog(program, configure, timeout_secs)
    }
}

/// Spawn `program` directly and arm a background thread that kills the child
/// (and its process tree, best effort) once `timeout_secs` elapses, unless the
/// returned guard is dropped first.
///
/// This is the Windows path, and the Unix fallback when no `timeout` binary is
/// available. The two platforms differ only in how the kill is delivered — see
/// [`kill_process_tree`].
fn spawn_with_watchdog<P, F>(
    program: P,
    configure: F,
    timeout_secs: u64,
) -> io::Result<(Child, TimeoutGuard)>
where
    P: AsRef<OsStr>,
    F: FnOnce(&mut Command) -> &mut Command,
{
    use std::thread;
    use std::time::Duration;

    let mut cmd = Command::new(program);
    configure(&mut cmd);
    // Put the child in its OWN process group so the watchdog can SIGKILL the
    // WHOLE tree (leader + descendants), not just the immediate child — a lone
    // `kill(pid)` leaves a pipeline/background grandchild alive holding inherited
    // pipes open. See memory bg-job-pid-macos-setsid: macOS ships no `setsid`
    // binary, so we call `libc::setsid()` in `pre_exec` directly.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid`/`setpgid` only rearrange process-group membership in
        // the forked child before `execvp`; they allocate nothing, touch no
        // shared state, and report errors via errno.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    let _ = libc::setpgid(0, 0);
                }
                Ok(())
            });
        }
    }
    let child = cmd.spawn()?;
    let pid = child.id();

    let done = Arc::new(AtomicBool::new(false));
    let done_clone = Arc::clone(&done);
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout_secs));
        if !done_clone.load(Ordering::Acquire) {
            kill_process_tree(pid);
        }
    });

    Ok((child, TimeoutGuard { done: Some(done) }))
}

/// Best-effort kill of the process (tree) identified by `pid`. Errors are
/// ignored: the child may have just finished on its own, which is fine.
#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    // /T walks children, /F forces.
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

/// Best-effort kill of the process (group) rooted at `pid`. The child was
/// spawned as its own session/group leader (`pid == pgid`), so signalling the
/// negative pid reaps the WHOLE tree — leader plus any pipeline/background
/// descendants that would otherwise survive a lone `kill(pid)` and keep
/// inherited pipes open. We also signal `pid` directly as a fallback in case
/// the `setsid` in `spawn_with_watchdog` did not take. Errors are ignored: the
/// child may have just finished on its own.
#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    // SAFETY: `kill(2)` is always safe to call — it validates the pid and
    // signal itself and reports failure via errno rather than UB. We ignore
    // the return value because this is best-effort at a deadline.
    unsafe {
        // Negative pid ⇒ signal the entire process group led by `pid`.
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Resolve the GNU `timeout` binary once, caching the result for the process
/// lifetime. Probes PATH for `gtimeout` then `timeout`, mirroring the shell
/// wrapper's `command -v gtimeout || command -v timeout` order in
/// `src/commands/spawn/execution.rs`. Returns `None` when neither is present
/// (default macOS-without-coreutils), which drives the watchdog fallback.
#[cfg(unix)]
fn resolve_timeout_bin() -> Option<&'static Path> {
    static TIMEOUT_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    TIMEOUT_BIN
        .get_or_init(|| which_first(&["gtimeout", "timeout"]))
        .as_deref()
}

/// Minimal `which(1)`: return the first `candidate` (in order) found as an
/// executable file on any `$PATH` entry. A candidate earlier in the list wins
/// over one later, even if the later one appears earlier on PATH — matching
/// `command -v a || command -v b`.
#[cfg(unix)]
fn which_first(candidates: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    which_first_in(&dirs, candidates)
}

/// `which_first` factored to take explicit search dirs, so probe order is
/// unit-testable without mutating the process environment.
#[cfg(unix)]
fn which_first_in(dirs: &[PathBuf], candidates: &[&str]) -> Option<PathBuf> {
    for name in candidates {
        for dir in dirs {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// The watchdog fallback (the path taken on macOS without coreutils)
    /// enforces the deadline: a `sleep 30` armed with a 1s budget must be
    /// killed well before it would finish on its own.
    #[test]
    fn watchdog_fallback_enforces_deadline() {
        let start = Instant::now();
        let (mut child, _killer) =
            spawn_with_watchdog("sleep", |c| c.arg("30"), 1).expect("spawn sleep");
        let status = child.wait().expect("wait");
        let elapsed = start.elapsed();

        assert!(
            !status.success(),
            "child should have been killed, not exited cleanly"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "watchdog did not enforce the 1s deadline: slept {elapsed:?}"
        );
    }

    /// A child that finishes before the deadline is left alone: the guard
    /// disarms the watchdog on drop and the process exits successfully.
    #[test]
    fn watchdog_does_not_kill_fast_child() {
        let (mut child, killer) = spawn_with_watchdog("true", |c| c, 30).expect("spawn true");
        let status = child.wait().expect("wait");
        drop(killer);
        assert!(status.success(), "fast child should exit 0, got {status:?}");
    }

    /// The public API enforces the deadline regardless of which path it takes
    /// (external `timeout` binary on most Linux, watchdog fallback on macOS).
    /// This guards the existing call sites: `spawn_with_timeout` still kills a
    /// runaway child.
    #[test]
    fn spawn_with_timeout_enforces_deadline() {
        let start = Instant::now();
        let (mut child, _killer) =
            spawn_with_timeout("sleep", |c| c.arg("30"), 1).expect("spawn sleep");
        let status = child.wait().expect("wait");
        assert!(!status.success(), "runaway child should have been killed");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "deadline not enforced"
        );
    }

    /// Probe order: `gtimeout` wins over `timeout` when both are on PATH, even
    /// if `timeout` sits in an earlier directory — matching the shell wrapper's
    /// `command -v gtimeout || command -v timeout`.
    #[cfg(unix)]
    #[test]
    fn probe_prefers_gtimeout_over_timeout() {
        use std::os::unix::fs::PermissionsExt;

        let base =
            std::env::temp_dir().join(format!("wg-probe-{}-{}", std::process::id(), "prefer"));
        let dir_a = base.join("a"); // earlier on PATH, only `timeout`
        let dir_b = base.join("b"); // later on PATH, has `gtimeout`
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let write_exe = |p: &Path| {
            std::fs::write(p, b"#!/bin/sh\nexit 0\n").unwrap();
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(p, perm).unwrap();
        };
        write_exe(&dir_a.join("timeout"));
        write_exe(&dir_b.join("gtimeout"));
        write_exe(&dir_b.join("timeout"));

        let dirs = vec![dir_a.clone(), dir_b.clone()];
        let resolved = which_first_in(&dirs, &["gtimeout", "timeout"]).unwrap();
        assert_eq!(
            resolved.file_name().unwrap(),
            "gtimeout",
            "gtimeout must win over timeout regardless of PATH order"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    /// When only `timeout` is present it is selected; when neither is present
    /// the probe returns `None`, which drives the watchdog fallback.
    #[cfg(unix)]
    #[test]
    fn probe_falls_back_to_timeout_then_none() {
        use std::os::unix::fs::PermissionsExt;

        let base =
            std::env::temp_dir().join(format!("wg-probe-{}-{}", std::process::id(), "fallback"));
        let with_timeout = base.join("with");
        let empty = base.join("empty");
        std::fs::create_dir_all(&with_timeout).unwrap();
        std::fs::create_dir_all(&empty).unwrap();

        let exe = with_timeout.join("timeout");
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perm = std::fs::metadata(&exe).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&exe, perm).unwrap();

        let only_timeout =
            which_first_in(&[with_timeout.clone()], &["gtimeout", "timeout"]).unwrap();
        assert_eq!(only_timeout.file_name().unwrap(), "timeout");

        let none = which_first_in(&[empty.clone()], &["gtimeout", "timeout"]);
        assert!(none.is_none(), "no binary present should probe to None");

        std::fs::remove_dir_all(&base).ok();
    }
}
