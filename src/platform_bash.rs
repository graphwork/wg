//! Resolve the bash executable to use for wg wrapper-script invocations.
//!
//! Git for Windows' default installer adds `C:\Program Files\Git\cmd` to PATH
//! but not `C:\Program Files\Git\bin`. As a result, `Command::new("bash")` on
//! a stock Windows install usually resolves to `C:\Windows\System32\bash.exe`
//! — the WSL shim — which can't handle native Windows paths and silently
//! breaks every wg wrapper script.
//!
//! [`bash_exe_path`] returns a single canonical bash path with this precedence
//! (highest wins):
//!
//! 1. `[bash] path = "..."` from `.workgraph/config.toml`, if the caller passes
//!    one in.
//! 2. `WG_BASH_PATH` env var.
//! 3. Known Git-for-Windows install locations (Windows only):
//!    - `C:\Program Files\Git\bin\bash.exe`
//!    - `C:\Program Files (x86)\Git\bin\bash.exe`
//!    - `%PROGRAMFILES%\Git\bin\bash.exe` (deduped against #1)
//!    - `%LOCALAPPDATA%\Programs\Git\bin\bash.exe`
//! 4. `bash.exe` on PATH, skipping `C:\Windows\System32\bash.exe` (WSL shim).
//!
//! On non-Windows, we just return `"bash"` and let the OS loader do its thing
//! — there is no WSL-shim trap on Unix.
//!
//! [`resolve_bash`] is the diagnostic form used by `wg doctor`: it returns
//! the path *and* a tag describing which rule matched, plus an optional
//! warning string.

use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

/// Which resolution rule produced the final bash path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashSource {
    /// From `[bash] path` in config.toml.
    Config,
    /// From the `WG_BASH_PATH` env var.
    Env,
    /// From a well-known Git-for-Windows install location.
    KnownLocation,
    /// From a PATH scan (WSL shim filtered out).
    Path,
    /// Fallback: bare `"bash"` (non-Windows, or no candidate found on Windows
    /// but we want callers to see something actionable).
    Fallback,
}

/// Full resolution result for `wg doctor`-style diagnostics.
#[derive(Debug, Clone)]
pub struct BashResolution {
    pub path: PathBuf,
    pub source: BashSource,
    /// Non-fatal note, e.g. "C:\\Windows\\System32\\bash.exe on PATH is the
    /// WSL shim — skipped."
    pub warning: Option<String>,
}

/// Resolve the bash executable. Simple form — most callers want this.
///
/// `config_override` is the `[bash] path` value from the nearest loaded
/// `Config`, or `None` if the caller has no Config handy.
pub fn bash_exe_path(config_override: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_bash(config_override)?.path)
}

/// Resolve the bash executable and report which rule matched.
///
/// Never fails on non-Windows — always returns `PathBuf::from("bash")` with
/// `source = Fallback`. Errors on Windows only when every rule misses, which
/// should be extraordinarily rare (no Git for Windows, no WG_BASH_PATH, no
/// non-WSL bash on PATH).
pub fn resolve_bash(config_override: Option<&Path>) -> Result<BashResolution> {
    // Rule 1: explicit config override.
    if let Some(path) = config_override {
        return Ok(BashResolution {
            path: path.to_path_buf(),
            source: BashSource::Config,
            warning: None,
        });
    }

    // Rule 2: environment override.
    if let Ok(raw) = std::env::var("WG_BASH_PATH") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(BashResolution {
                path: PathBuf::from(trimmed),
                source: BashSource::Env,
                warning: None,
            });
        }
    }

    #[cfg(windows)]
    {
        // Rule 3: known install locations.
        if let Some(path) = find_known_windows_bash() {
            return Ok(BashResolution {
                path,
                source: BashSource::KnownLocation,
                warning: None,
            });
        }

        // Rule 4: PATH scan, filtering the WSL shim.
        let path_var = std::env::var_os("PATH").unwrap_or_default();
        let candidates = std::env::split_paths(&path_var)
            .map(|dir| dir.join("bash.exe"))
            .collect::<Vec<_>>();
        let (picked, warning) = scan_path_candidates(&candidates);
        if let Some(path) = picked {
            return Ok(BashResolution {
                path,
                source: BashSource::Path,
                warning,
            });
        }

        Err(anyhow!(
            "Could not locate a usable bash executable. Git for Windows' \
             `bash.exe` was not found in the well-known install locations, \
             and no non-WSL `bash.exe` is on PATH. Install Git for Windows \
             (https://git-scm.com/download/win), or set `WG_BASH_PATH` / \
             `[bash] path` in `.workgraph/config.toml`. Run `wg doctor` for \
             a full diagnosis."
        ))
    }

    #[cfg(not(windows))]
    {
        Ok(BashResolution {
            path: PathBuf::from("bash"),
            source: BashSource::Fallback,
            warning: None,
        })
    }
}

/// Windows-only: probe the handful of install paths Git for Windows uses.
#[cfg(windows)]
fn find_known_windows_bash() -> Option<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();

    let push = |candidates: &mut Vec<PathBuf>, seen: &mut Vec<PathBuf>, p: PathBuf| {
        if !seen.iter().any(|s| paths_eq_ci(s, &p)) {
            seen.push(p.clone());
            candidates.push(p);
        }
    };

    push(
        &mut candidates,
        &mut seen,
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
    );
    push(
        &mut candidates,
        &mut seen,
        PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
    );
    if let Ok(pf) = std::env::var("ProgramFiles") {
        push(
            &mut candidates,
            &mut seen,
            PathBuf::from(pf).join("Git").join("bin").join("bash.exe"),
        );
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        push(
            &mut candidates,
            &mut seen,
            PathBuf::from(local)
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }

    candidates.into_iter().find(|p| p.is_file())
}

/// Scan a list of `bash.exe` candidates and pick the first that exists and
/// isn't the WSL shim (`C:\Windows\System32\bash.exe`).
///
/// Factored out from the real PATH scan so tests can feed in a synthetic list
/// without touching real env vars.
///
/// Returns `(picked, warning)`. `warning` is populated when we skipped the
/// WSL shim — that's useful for `wg doctor` to tell the user what happened.
pub(crate) fn scan_path_candidates(candidates: &[PathBuf]) -> (Option<PathBuf>, Option<String>) {
    let mut warning: Option<String> = None;
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        #[cfg(windows)]
        {
            if is_wsl_shim(candidate) {
                if warning.is_none() {
                    warning = Some(format!(
                        "{} is the WSL shim — skipped",
                        candidate.display()
                    ));
                }
                continue;
            }
        }
        return (Some(candidate.clone()), warning);
    }
    (None, warning)
}

/// Case-insensitive path equality, suitable for Windows filesystem compares.
#[cfg(windows)]
fn paths_eq_ci(a: &Path, b: &Path) -> bool {
    let a_s = a.to_string_lossy().to_ascii_lowercase();
    let b_s = b.to_string_lossy().to_ascii_lowercase();
    // Normalize forward/backslash to backslash so
    // "C:/Windows/System32/bash.exe" and "C:\\Windows\\System32\\bash.exe"
    // compare equal.
    let a_n = a_s.replace('/', "\\");
    let b_n = b_s.replace('/', "\\");
    a_n == b_n
}

/// Returns true if `candidate` refers to `C:\Windows\System32\bash.exe` (the
/// WSL shim).
#[cfg(windows)]
fn is_wsl_shim(candidate: &Path) -> bool {
    // Check against the literal System32 path first (fast path).
    let shim_literal = PathBuf::from(r"C:\Windows\System32\bash.exe");
    if paths_eq_ci(candidate, &shim_literal) {
        return true;
    }
    // Also check %SystemRoot%\System32\bash.exe in case Windows is installed
    // on a non-C: drive.
    if let Ok(sysroot) = std::env::var("SystemRoot") {
        let shim = PathBuf::from(sysroot)
            .join("System32")
            .join("bash.exe");
        if paths_eq_ci(candidate, &shim) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize env-mutating tests so they don't race each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests hold ENV_LOCK, so no concurrent env access.
            unsafe {
                std::env::set_var(key, val);
            }
            EnvGuard { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            EnvGuard { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn config_override_wins() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("WG_BASH_PATH", "/should/be/ignored");
        let override_path = PathBuf::from("/custom/bash");
        let r = resolve_bash(Some(&override_path)).unwrap();
        assert_eq!(r.source, BashSource::Config);
        assert_eq!(r.path, override_path);
    }

    #[test]
    fn env_wins_over_known_and_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("WG_BASH_PATH", "/from/env/bash");
        let r = resolve_bash(None).unwrap();
        assert_eq!(r.source, BashSource::Env);
        assert_eq!(r.path, PathBuf::from("/from/env/bash"));
    }

    #[test]
    fn env_empty_is_ignored() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("WG_BASH_PATH", "   ");
        // Should not return Env when the value is whitespace-only.
        let r = resolve_bash(None);
        // On non-Windows, falls through to Fallback.
        // On Windows, would try known locations / PATH; either way Env must
        // not be the source.
        if let Ok(res) = r {
            assert_ne!(res.source, BashSource::Env);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_returns_plain_bash() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("WG_BASH_PATH");
        let r = resolve_bash(None).unwrap();
        assert_eq!(r.source, BashSource::Fallback);
        assert_eq!(r.path, PathBuf::from("bash"));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_shim_on_path_is_skipped() {
        // Synthetic candidate list: first is the WSL shim, second is a real
        // bash we write into a tempdir.
        let tmp = tempfile::tempdir().unwrap();
        let real_bash = tmp.path().join("bash.exe");
        std::fs::write(&real_bash, b"\x4D\x5A fake exe").unwrap();

        let shim = PathBuf::from(r"C:\Windows\System32\bash.exe");
        let candidates = vec![shim, real_bash.clone()];

        let (picked, warning) = scan_path_candidates(&candidates);
        // If the system doesn't actually have C:\Windows\System32\bash.exe
        // (e.g. WSL not installed), the first candidate is filtered by the
        // `is_file()` check, not the shim check — no warning emitted, but
        // the real bash is still picked.
        assert_eq!(picked, Some(real_bash));
        // Warning is only asserted if the shim actually exists on this box.
        let shim_exists =
            PathBuf::from(r"C:\Windows\System32\bash.exe").is_file();
        if shim_exists {
            assert!(warning.is_some(), "expected WSL-shim warning");
        }
    }

    #[cfg(windows)]
    #[test]
    fn paths_eq_ci_handles_case_and_separators() {
        assert!(paths_eq_ci(
            Path::new(r"C:\Windows\System32\bash.exe"),
            Path::new(r"c:\windows\system32\BASH.EXE"),
        ));
        assert!(paths_eq_ci(
            Path::new(r"C:\Windows\System32\bash.exe"),
            Path::new("C:/Windows/System32/bash.exe"),
        ));
        assert!(!paths_eq_ci(
            Path::new(r"C:\Windows\System32\bash.exe"),
            Path::new(r"C:\Program Files\Git\bin\bash.exe"),
        ));
    }

    #[cfg(windows)]
    #[test]
    fn scan_empty_list_returns_none() {
        let (picked, warning) = scan_path_candidates(&[]);
        assert!(picked.is_none());
        assert!(warning.is_none());
    }
}
