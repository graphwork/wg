//! Single-writer fences for the service daemon.
//!
//! Two independent, kernel-backed advisory locks make "at most one daemon per
//! socket" a structural invariant instead of a best-effort `state.json` check:
//!
//! * `service/daemon.lock` — held by the **daemon** for its whole lifetime.
//!   The lock owner record carries `pid` + OS process-birth identity, so a
//!   second daemon (or a start whose state file was lost) cannot bind the same
//!   socket and start a second ticking coordinator loop. `flock(2)` releases
//!   automatically when the process dies (even on `SIGKILL`), so a crashed
//!   daemon never wedges the graph.
//! * `service/start.lock` — held by `wg service start` between its
//!   already-running check and the readiness confirmation. Two concurrent
//!   starts serialize here, so the loser observes the winner's daemon lock and
//!   refuses loudly instead of stacking a supervisor+daemon pair.
//!
//! The lock **file** is only a carrier for the owner metadata; the authority
//! is the kernel `flock`, never the file's presence.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::is_process_alive;

/// Owner metadata written into `daemon.lock`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonLockOwner {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start_identity: Option<String>,
    pub socket_path: String,
    pub started_at: String,
}

impl DaemonLockOwner {
    /// True only when the recorded PID is alive *and* still the same process
    /// birth. A reused numeric PID is never owner authority.
    pub fn is_live(&self) -> bool {
        is_process_alive(self.pid) && self.birth_matches()
    }

    fn birth_matches(&self) -> bool {
        match self.pid_start_identity.as_deref() {
            Some(birth) => {
                worksgood::service_identity::pid_start_identity(self.pid).as_deref() == Some(birth)
            }
            // Platforms without a birth identity fall back to liveness only.
            None => true,
        }
    }
}

pub fn daemon_lock_path(dir: &Path) -> PathBuf {
    dir.join("service").join("daemon.lock")
}

fn start_lock_path(dir: &Path) -> PathBuf {
    dir.join("service").join("start.lock")
}

/// Read the daemon lock owner record without taking the lock (read-only).
pub fn read_daemon_owner(dir: &Path) -> Option<DaemonLockOwner> {
    read_owner_at(&daemon_lock_path(dir))
}

fn read_owner_at(path: &Path) -> Option<DaemonLockOwner> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A held advisory lock. Dropping it releases the kernel lock; the
/// `remove_on_drop` flag controls whether the carrier file is also removed.
pub struct FileLock {
    file: File,
    path: PathBuf,
    remove_on_drop: bool,
    #[allow(dead_code)]
    owner: Option<DaemonLockOwner>,
}

impl FileLock {
    fn open(path: &Path) -> Result<File> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create lock dir {}", parent.display()))?;
        }
        OpenOptions::new()
            .create(true)
            // The owner record is rewritten in place (seek + set_len below);
            // we manage truncation explicitly rather than on open.
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open lock file {}", path.display()))
    }

    /// Try to take the lock without blocking. Returns `Err(contention)` when
    /// another process already holds it.
    fn try_lock(path: &Path, remove_on_drop: bool) -> Result<Self> {
        let file = Self::open(path)?;
        if !try_flock_exclusive(&file)? {
            anyhow::bail!("lock {} is held by another process", path.display());
        }
        Ok(FileLock {
            file,
            path: path.to_path_buf(),
            remove_on_drop,
            owner: None,
        })
    }

    fn write_owner(&mut self, owner: &DaemonLockOwner) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(owner)?;
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&bytes)?;
        self.file.flush()?;
        self.file.sync_all()?;
        self.owner = Some(owner.clone());
        Ok(())
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            use std::os::fd::AsRawFd;
            let _ = libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The daemon's lifetime lock.
pub struct DaemonLock {
    #[allow(dead_code)]
    inner: FileLock,
}

impl DaemonLock {
    /// Acquire the daemon lock for `dir`, writing our pid + birth identity.
    ///
    /// Fails loudly when another live daemon already holds it. A stale carrier
    /// file (dead owner) is harmless: `flock` succeeds and we overwrite it.
    pub fn acquire(dir: &Path, socket_path: &str) -> Result<Self> {
        let path = daemon_lock_path(dir);
        let mut inner = FileLock::try_lock(&path, true).with_context(|| {
            let owner = read_owner_at(&path);
            match owner {
                Some(owner) => format!(
                    "another service daemon (PID {}) already holds the single-writer lock for {}",
                    owner.pid,
                    dir.display()
                ),
                None => format!(
                    "another service daemon already holds the single-writer lock for {}",
                    dir.display()
                ),
            }
        })?;
        let owner = DaemonLockOwner {
            pid: std::process::id(),
            pid_start_identity: worksgood::service_identity::pid_start_identity(std::process::id()),
            socket_path: socket_path.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        inner.write_owner(&owner)?;
        Ok(DaemonLock { inner })
    }
}

/// Remove a stale `daemon.lock` carrier file whose owner is provably dead.
///
/// Returns `true` when a stale file was removed. Never touches a live lock
/// (the kernel lock would also still be held).
pub fn reap_stale_daemon_lock(dir: &Path) -> bool {
    let path = daemon_lock_path(dir);
    let Some(owner) = read_owner_at(&path) else {
        return false;
    };
    if owner.is_live() {
        return false;
    }
    // Confirm the kernel lock is actually free before touching the file, so a
    // just-restarted daemon cannot be raced.
    match FileLock::try_lock(&path, false) {
        Ok(_probe) => {
            drop(_probe);
            let _ = fs::remove_file(&path);
            true
        }
        Err(_) => false,
    }
}

/// True when the daemon lock is currently free (no live holder).
pub fn is_daemon_lock_free(dir: &Path) -> bool {
    let path = daemon_lock_path(dir);
    if !path.exists() {
        return true;
    }
    match FileLock::try_lock(&path, false) {
        Ok(probe) => {
            drop(probe);
            true
        }
        Err(_) => false,
    }
}

/// Serializes concurrent `wg service start` invocations for one graph.
pub struct StartLock {
    #[allow(dead_code)]
    inner: FileLock,
}

impl StartLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        let path = start_lock_path(dir);
        let inner = FileLock::try_lock(&path, true).with_context(|| {
            format!(
                "another `wg service start` is in progress for {}",
                dir.display()
            )
        })?;
        Ok(StartLock { inner })
    }
}

#[cfg(unix)]
fn try_flock_exclusive(file: &File) -> Result<bool> {
    use std::os::fd::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EINTR => Ok(false),
        _ => Err(err).context("flock failed"),
    }
}

#[cfg(not(unix))]
fn try_flock_exclusive(_file: &File) -> Result<bool> {
    // No portable advisory lock here; single-writer is still guarded by the
    // live-owner process-identity pre-check in `run_start`.
    Ok(true)
}

/// Read the raw lock bytes for diagnostics (used by tests).
#[allow(dead_code)]
pub fn read_raw(dir: &Path) -> Option<String> {
    let mut buf = String::new();
    File::open(daemon_lock_path(dir))
        .ok()?
        .read_to_string(&mut buf)
        .ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn socket_stub(dir: &Path) -> String {
        dir.join("service/daemon.sock").display().to_string()
    }

    #[test]
    fn daemon_lock_records_live_owner_and_blocks_a_second_holder() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let lock = DaemonLock::acquire(dir, &socket_stub(dir)).unwrap();
        let owner = read_daemon_owner(dir).expect("owner record must be written");
        assert_eq!(owner.pid, std::process::id());
        assert!(owner.is_live());

        // A second lock in the same process: flock is per-open-file-description,
        // so this detects the held lock and refuses loudly.
        let second = DaemonLock::acquire(dir, &socket_stub(dir));
        assert!(second.is_err(), "second daemon lock must be refused");
        drop(lock);
        let third = DaemonLock::acquire(dir, &socket_stub(dir));
        assert!(third.is_ok(), "lock must be re-acquirable after release");
    }

    #[test]
    fn stale_owner_is_reaped_and_reacquired() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        fs::create_dir_all(dir.join("service")).unwrap();
        // A PID that cannot exist: an obviously dead owner with a matching
        // (absent) birth identity.
        let dead = DaemonLockOwner {
            pid: 0x7fff_fffe,
            pid_start_identity: None,
            socket_path: socket_stub(dir),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        fs::write(daemon_lock_path(dir), serde_json::to_vec(&dead).unwrap()).unwrap();
        assert!(!dead.is_live());
        assert!(reap_stale_daemon_lock(dir));
        assert!(!daemon_lock_path(dir).exists());
        assert!(DaemonLock::acquire(dir, &socket_stub(dir)).is_ok());
    }

    #[test]
    fn start_lock_serializes_and_removes_its_carrier() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let held = StartLock::acquire(dir).unwrap();
        assert!(StartLock::acquire(dir).is_err());
        let path = start_lock_path(dir);
        assert!(path.exists());
        drop(held);
        assert!(!path.exists(), "start lock carrier must be removed on drop");
        assert!(StartLock::acquire(dir).is_ok());
    }
}
