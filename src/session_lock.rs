//! Per-session handler lock — enforces at-most-one live handler per
//! chat session at a time.
//!
//! See `docs/design/sessions-as-identity.md` for the full model. This
//! module implements the lock contract:
//!
//!   * `acquire(dir, kind)`: O_EXCL create on `<dir>/.handler.pid`.
//!     If the file exists with a LIVE PID, refuses. If the file
//!     exists with a DEAD PID, recovers (removes + retakes).
//!   * `SessionLock::drop`: removes the file on clean exit.
//!   * `read_holder(dir)`: non-destructive read of who currently
//!     holds the lock (for takeover signalling).
//!   * `request_release(dir)`: writes `<dir>/.handler.release-requested`
//!     as a cooperative signal — the live handler should notice this
//!     at its next turn boundary and exit cleanly.
//!
//! The lock file format is deliberately small and human-readable so a
//! user running `cat .handler.pid` can understand what's going on:
//!
//! ```text
//! <pid>\n
//! <iso-8601-start-time>\n
//! <kind-label>\n
//! ```
//!
//! Stale detection uses `kill(pid, 0)` on Unix — returns 0 if the
//! process exists, error otherwise. On Windows we fall back to
//! treating the lock as always-fresh (Windows handler is a follow-up).

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

const LOCK_FILENAME: &str = ".handler.pid";
const RELEASE_MARKER: &str = ".handler.release-requested";
const TUI_DRIVER_SENTINEL: &str = ".tui-driven";

/// What kind of handler owns the lock. Used for diagnostics — when
/// you see the lock file, you know what kind of thing is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlerKind {
    /// Interactive `wg nex` at a terminal (rustyline-backed).
    InteractiveNex,
    /// Autonomous `wg nex` — task agent or background coordinator.
    AutonomousNex,
    /// Chat-tethered `wg nex` driving a chat session from inbox.
    ChatNex,
    /// TUI-owned PTY-backed handler.
    TuiPty,
    /// Other / adapter-dispatched handler (claude, codex, etc.).
    Adapter,
}

impl HandlerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::InteractiveNex => "interactive-nex",
            Self::AutonomousNex => "autonomous-nex",
            Self::ChatNex => "chat-nex",
            Self::TuiPty => "tui-pty",
            Self::Adapter => "adapter",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "interactive-nex" => Some(Self::InteractiveNex),
            "autonomous-nex" => Some(Self::AutonomousNex),
            "chat-nex" => Some(Self::ChatNex),
            "tui-pty" => Some(Self::TuiPty),
            "adapter" => Some(Self::Adapter),
            _ => None,
        }
    }
}

/// Snapshot of who currently holds the lock. Returned by
/// `read_holder`. Useful for takeover decisions and diagnostics.
#[derive(Clone, Debug)]
pub struct LockInfo {
    pub pid: u32,
    pub started_at: String,
    pub kind: Option<HandlerKind>,
    /// Whether the holder process is still alive (via `kill(pid, 0)`
    /// on Unix). If `false`, the lock is stale and safe to take.
    pub alive: bool,
}

/// Snapshot of the TUI process that has claimed this chat session.
#[derive(Clone, Debug)]
pub struct TuiDriverInfo {
    pub pid: u32,
    pub written_at: String,
    /// Whether the TUI process is still alive. Stale sentinels are
    /// ignored by readers, matching stale lock recovery semantics.
    pub alive: bool,
}

/// RAII lock handle. Drop removes the file (idempotent — safe even
/// if already removed). Call `release()` for an explicit early
/// release; otherwise the lock lives for the `SessionLock`'s scope.
pub struct SessionLock {
    path: PathBuf,
    /// Set to true once we've removed the file so Drop doesn't try
    /// a second time. Not a correctness issue (remove is idempotent)
    /// but saves a syscall and avoids log noise.
    released: bool,
}

impl SessionLock {
    /// Path where the lock file lives for a given chat session dir.
    pub fn lock_path(chat_dir: &Path) -> PathBuf {
        chat_dir.join(LOCK_FILENAME)
    }

    /// Path of the cooperative release marker.
    pub fn release_marker_path(chat_dir: &Path) -> PathBuf {
        chat_dir.join(RELEASE_MARKER)
    }

    /// Try to acquire the lock. Creates the chat dir if it doesn't
    /// exist (we own the lock so we own the dir initialisation).
    ///
    /// Returns `Err` if the lock is currently held by a live process.
    /// The error includes the holder's PID and kind so callers can
    /// decide whether to takeover.
    pub fn acquire(chat_dir: &Path, kind: HandlerKind) -> Result<Self> {
        std::fs::create_dir_all(chat_dir)
            .with_context(|| format!("create chat dir {:?}", chat_dir))?;
        let path = Self::lock_path(chat_dir);

        // O_EXCL create. Two racing processes: one wins, the other
        // gets EEXIST and falls into the stale-check branch.
        let create_result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&path);

        match create_result {
            Ok(mut f) => {
                // We own the lock. Write our identity.
                let contents = format!(
                    "{}\n{}\n{}\n",
                    std::process::id(),
                    chrono::Utc::now().to_rfc3339(),
                    kind.label(),
                );
                f.write_all(contents.as_bytes())
                    .with_context(|| format!("write lock file {:?}", path))?;
                f.sync_all().context("fsync lock file")?;
                Ok(Self {
                    path,
                    released: false,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Someone else has it. Decide: recover (stale) or fail.
                match read_holder_at(&path)? {
                    Some(holder) if holder.alive => Err(anyhow!(
                        "session lock held by live handler pid={} kind={} started={}",
                        holder.pid,
                        holder.kind.map(|k| k.label()).unwrap_or("unknown"),
                        holder.started_at
                    )),
                    Some(holder) => {
                        // Stale — clean up and retake.
                        eprintln!(
                            "[session-lock] recovering stale lock (dead pid={}, kind={}) at {:?}",
                            holder.pid,
                            holder.kind.map(|k| k.label()).unwrap_or("unknown"),
                            path
                        );
                        std::fs::remove_file(&path)
                            .with_context(|| format!("remove stale lock {:?}", path))?;
                        // Recurse once; if someone raced us here,
                        // this will either succeed or hit the live
                        // branch above.
                        Self::acquire(chat_dir, kind)
                    }
                    None => {
                        // File existed but couldn't parse — treat as
                        // stale. Same recovery as above.
                        eprintln!("[session-lock] recovering unparseable lock at {:?}", path);
                        std::fs::remove_file(&path)
                            .with_context(|| format!("remove corrupt lock {:?}", path))?;
                        Self::acquire(chat_dir, kind)
                    }
                }
            }
            Err(e) => Err(anyhow!("open lock file {:?}: {}", path, e)),
        }
    }

    /// Explicitly release the lock. Idempotent. The Drop impl also
    /// calls this, so manual release is only needed when you want
    /// release to happen before scope end (e.g., before spawning a
    /// successor handler).
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        if self.path.exists()
            && let Err(e) = std::fs::remove_file(&self.path)
        {
            eprintln!(
                "[session-lock] warning: failed to remove lock {:?}: {}",
                self.path, e
            );
        }
        self.released = true;
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Read the current lock holder, if any. Non-destructive — does not
/// touch the file, just reports what's there.
pub fn read_holder(chat_dir: &Path) -> Result<Option<LockInfo>> {
    read_holder_at(&SessionLock::lock_path(chat_dir))
}

fn read_holder_at(path: &Path) -> Result<Option<LockInfo>> {
    let mut f = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("open lock file {:?}: {}", path, e)),
    };
    let mut buf = String::new();
    f.read_to_string(&mut buf)
        .with_context(|| format!("read lock file {:?}", path))?;

    let mut lines = buf.lines();
    let pid_line = match lines.next() {
        Some(s) => s,
        None => return Ok(None),
    };
    let pid: u32 = match pid_line.trim().parse() {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let started_at = lines.next().unwrap_or("").to_string();
    let kind = lines.next().and_then(HandlerKind::parse);

    let alive = pid_is_alive(pid);
    Ok(Some(LockInfo {
        pid,
        started_at,
        kind,
        alive,
    }))
}

/// Ask the current lock holder (if any) to release the lock
/// cooperatively. Writes a marker file at
/// `<chat_dir>/.handler.release-requested`. The holder's conversation
/// loop checks for this marker at each turn boundary and exits
/// cleanly when it sees it.
///
/// Does nothing if there's no holder. Idempotent — writing the
/// marker twice is fine.
pub fn request_release(chat_dir: &Path) -> Result<()> {
    let marker = SessionLock::release_marker_path(chat_dir);
    std::fs::write(&marker, format!("{}\n", chrono::Utc::now().to_rfc3339()))
        .with_context(|| format!("write release marker {:?}", marker))?;
    Ok(())
}

/// True if a release has been requested for this session. The
/// running handler polls this at turn boundaries.
pub fn release_requested(chat_dir: &Path) -> bool {
    SessionLock::release_marker_path(chat_dir).exists()
}

/// Clear any pending release marker. Called by a handler after it
/// observes the marker and acts on it (so a successor handler doesn't
/// see a stale marker and immediately exit).
pub fn clear_release_marker(chat_dir: &Path) {
    let marker = SessionLock::release_marker_path(chat_dir);
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
    }
}

/// Path of the TUI ownership sentinel for a chat session.
pub fn tui_driver_sentinel_path(chat_dir: &Path) -> PathBuf {
    chat_dir.join(TUI_DRIVER_SENTINEL)
}

/// Mark a chat session as currently driven by a live `wg tui` process.
pub fn write_tui_driver_sentinel(chat_dir: &Path, pid: u32) -> Result<()> {
    std::fs::create_dir_all(chat_dir).with_context(|| format!("create chat dir {:?}", chat_dir))?;
    let path = tui_driver_sentinel_path(chat_dir);
    let contents = format!("{}\n{}\n", pid, chrono::Utc::now().to_rfc3339());
    std::fs::write(&path, contents).with_context(|| format!("write TUI sentinel {:?}", path))?;
    Ok(())
}

/// Read the TUI ownership sentinel, if present.
pub fn read_tui_driver_sentinel(chat_dir: &Path) -> Result<Option<TuiDriverInfo>> {
    let path = tui_driver_sentinel_path(chat_dir);
    let mut f = match OpenOptions::new().read(true).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("open TUI sentinel {:?}: {}", path, e)),
    };
    let mut buf = String::new();
    f.read_to_string(&mut buf)
        .with_context(|| format!("read TUI sentinel {:?}", path))?;

    let mut lines = buf.lines();
    let pid_line = match lines.next() {
        Some(s) => s,
        None => return Ok(None),
    };
    let pid: u32 = match pid_line.trim().parse() {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let written_at = lines.next().unwrap_or("").to_string();
    let alive = pid_is_alive(pid);
    Ok(Some(TuiDriverInfo {
        pid,
        written_at,
        alive,
    }))
}

/// Clear the TUI ownership sentinel. Idempotent.
pub fn clear_tui_driver_sentinel(chat_dir: &Path) {
    let path = tui_driver_sentinel_path(chat_dir);
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

/// True when a live `wg tui` process currently owns the chat surface.
pub fn tui_driver_sentinel_alive(chat_dir: &Path) -> bool {
    read_tui_driver_sentinel(chat_dir)
        .ok()
        .flatten()
        .is_some_and(|info| info.alive)
}

/// Wait for the lock at `chat_dir` to become free. Polls every
/// `poll_interval` up to `timeout`. Returns `Ok(())` once the lock
/// is gone, `Err` on timeout.
pub fn wait_for_release(chat_dir: &Path, timeout: Duration) -> Result<()> {
    let poll = Duration::from_millis(100);
    let start = std::time::Instant::now();
    loop {
        match read_holder(chat_dir)? {
            None => return Ok(()),
            Some(info) if !info.alive => return Ok(()),
            Some(_) => {
                if start.elapsed() >= timeout {
                    return Err(anyhow!(
                        "timed out waiting for lock release after {:?}",
                        timeout
                    ));
                }
                std::thread::sleep(poll);
            }
        }
    }
}

/// PID-is-alive check using `kill(pid, 0)`. A signal of 0 is the
/// "does this process exist and am I allowed to signal it?" query —
/// it does NOT actually deliver a signal.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) is safe for any pid. It probes existence
    // and permission; it does not deliver a signal.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r == 0 {
        true
    } else {
        // ESRCH = no such process. EPERM = process exists but we
        // can't signal it — still counts as alive.
        let errno = std::io::Error::last_os_error().raw_os_error();
        errno == Some(libc::EPERM)
    }
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    // Conservative: assume alive on non-Unix. This makes stale
    // recovery a no-op on Windows, which is acceptable until we add
    // proper Windows support.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn acquire_creates_lock_file_with_pid() {
        let dir = tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path(), HandlerKind::InteractiveNex).unwrap();
        let p = SessionLock::lock_path(dir.path());
        assert!(p.exists());
        let contents = std::fs::read_to_string(&p).unwrap();
        assert!(contents.contains(&format!("{}", std::process::id())));
        assert!(contents.contains("interactive-nex"));
        drop(lock);
        assert!(!p.exists(), "Drop should remove the lock file");
    }

    #[test]
    fn second_acquire_fails_while_first_held() {
        let dir = tempdir().unwrap();
        let _first = SessionLock::acquire(dir.path(), HandlerKind::InteractiveNex).unwrap();
        let second = SessionLock::acquire(dir.path(), HandlerKind::InteractiveNex);
        assert!(second.is_err(), "second acquire must fail while first held");
        let err = second.err().unwrap().to_string();
        assert!(
            err.contains("pid="),
            "error must name the holder pid: {}",
            err
        );
    }

    #[test]
    fn stale_lock_recovers() {
        let dir = tempdir().unwrap();
        // Write a lock file pointing at a dead PID.
        let p = SessionLock::lock_path(dir.path());
        std::fs::write(&p, "999999\n2020-01-01T00:00:00Z\nchat-nex\n").unwrap();
        // New acquire should detect dead PID and recover.
        let lock = SessionLock::acquire(dir.path(), HandlerKind::ChatNex).unwrap();
        let contents = std::fs::read_to_string(&p).unwrap();
        // Now owned by our process.
        assert!(contents.contains(&format!("{}", std::process::id())));
        drop(lock);
    }

    #[test]
    fn read_holder_returns_live_flag() {
        let dir = tempdir().unwrap();
        let _lock = SessionLock::acquire(dir.path(), HandlerKind::TuiPty).unwrap();
        let info = read_holder(dir.path()).unwrap().unwrap();
        assert_eq!(info.pid, std::process::id());
        assert_eq!(info.kind, Some(HandlerKind::TuiPty));
        assert!(info.alive);
    }

    #[test]
    fn read_holder_none_when_no_lock() {
        let dir = tempdir().unwrap();
        assert!(read_holder(dir.path()).unwrap().is_none());
    }

    #[test]
    fn release_marker_round_trip() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        assert!(!release_requested(dir.path()));
        request_release(dir.path()).unwrap();
        assert!(release_requested(dir.path()));
        clear_release_marker(dir.path());
        assert!(!release_requested(dir.path()));
    }

    #[test]
    fn explicit_release_allows_reacquire() {
        let dir = tempdir().unwrap();
        let mut lock = SessionLock::acquire(dir.path(), HandlerKind::ChatNex).unwrap();
        lock.release();
        // New lock can be taken.
        let _new = SessionLock::acquire(dir.path(), HandlerKind::ChatNex).unwrap();
    }

    #[test]
    fn corrupt_lock_recovers() {
        let dir = tempdir().unwrap();
        let p = SessionLock::lock_path(dir.path());
        std::fs::write(&p, "not a pid\ngarbage\n").unwrap();
        let lock = SessionLock::acquire(dir.path(), HandlerKind::ChatNex).unwrap();
        drop(lock);
    }

    #[test]
    fn wait_for_release_succeeds_when_free() {
        let dir = tempdir().unwrap();
        // No lock held — should return immediately.
        wait_for_release(dir.path(), Duration::from_millis(50)).unwrap();
    }

    #[test]
    fn wait_for_release_times_out_while_held() {
        let dir = tempdir().unwrap();
        let _lock = SessionLock::acquire(dir.path(), HandlerKind::ChatNex).unwrap();
        let r = wait_for_release(dir.path(), Duration::from_millis(100));
        assert!(r.is_err());
    }

    #[test]
    fn tui_driver_sentinel_round_trip() {
        let dir = tempdir().unwrap();
        write_tui_driver_sentinel(dir.path(), std::process::id()).unwrap();

        let info = read_tui_driver_sentinel(dir.path()).unwrap().unwrap();
        assert_eq!(info.pid, std::process::id());
        assert!(info.alive);
        assert!(tui_driver_sentinel_alive(dir.path()));

        clear_tui_driver_sentinel(dir.path());
        assert!(read_tui_driver_sentinel(dir.path()).unwrap().is_none());
        assert!(!tui_driver_sentinel_alive(dir.path()));
    }

    #[test]
    fn stale_tui_driver_sentinel_is_not_alive() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            tui_driver_sentinel_path(dir.path()),
            "999999\n2020-01-01T00:00:00Z\n",
        )
        .unwrap();

        let info = read_tui_driver_sentinel(dir.path()).unwrap().unwrap();
        assert_eq!(info.pid, 999999);
        assert!(!info.alive);
        assert!(!tui_driver_sentinel_alive(dir.path()));
    }
}
