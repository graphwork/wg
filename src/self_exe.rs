//! Authoritative paths for recursively executing the running WG image.
//!
//! `std::env::current_exe()` is an identity/display path, not always an
//! executable path. On Linux, replacing an installed binary while a long-lived
//! TUI is open makes it return `.../wg (deleted)`. Re-executing that string
//! fails with ENOENT even though the current process still has the exact image
//! mapped. `/proc/self/exe` is the kernel-owned handle to that image and remains
//! executable until the process exits.

use std::io;
use std::path::PathBuf;

/// Path for an immediate `Command::new(...).spawn/output/exec` of this exact
/// running image.
///
/// On Linux/Android this deliberately returns `/proc/self/exe`, not its
/// symlink target. The child still has the parent's image before `execve`, so
/// `/proc/self/exe` resolves to the authoritative bytes even after an atomic
/// package-manager/cargo-install replacement. Other platforms use the absolute
/// path reported by the runtime.
pub fn direct_reexec_path() -> io::Result<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let proc_self_exe = PathBuf::from("/proc/self/exe");
        if std::fs::metadata(&proc_self_exe).is_ok() {
            return Ok(proc_self_exe);
        }
    }

    std::env::current_exe()
}

/// Absolute path that another process may execute while this process is alive.
///
/// Unlike `/proc/self/exe`, a command handed to tmux/shell must pin the owner
/// PID: once tmux resolves `/proc/self/exe`, `self` would mean tmux. The
/// `/proc/<pid>/exe` form continues to name this WG image across an installed
/// pathname replacement and is valid for the short handoff window.
pub fn handoff_reexec_path() -> io::Result<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let proc_pid_exe = PathBuf::from(format!("/proc/{}/exe", std::process::id()));
        if std::fs::metadata(&proc_pid_exe).is_ok() {
            return Ok(proc_pid_exe);
        }
    }

    std::env::current_exe()
}

/// Human-readable identity path for diagnostics. This may intentionally show
/// ` (deleted)` on Linux; callers should execute [`direct_reexec_path`] rather
/// than this value.
pub fn display_identity() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<current executable unavailable: {error}>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_reexec_is_absolute_and_exists() {
        let path = direct_reexec_path().expect("running executable should resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(std::fs::metadata(&path).is_ok(), "{}", path.display());
    }

    #[test]
    fn handoff_reexec_is_absolute_and_exists() {
        let path = handoff_reexec_path().expect("handoff executable should resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(std::fs::metadata(&path).is_ok(), "{}", path.display());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn linux_paths_use_kernel_owned_running_image_links() {
        assert_eq!(
            direct_reexec_path().unwrap(),
            PathBuf::from("/proc/self/exe")
        );
        assert_eq!(
            handoff_reexec_path().unwrap(),
            PathBuf::from(format!("/proc/{}/exe", std::process::id()))
        );
    }
}
