//! Discover which executor backends are available on this system.
//!
//! The TUI's coordinator-creation dialog and the config panel use
//! this to populate executor choice menus with things that actually
//! work — not a static list baked into the binary. When `codex` is
//! missing from PATH, it shouldn't be offered as an option.
//!
//! `native` is always available (it's this binary itself). The CLI
//! adapters (`claude`, `codex`, `gemini`) are probed by looking for
//! the binary on PATH.

use std::path::PathBuf;

/// One executor, whether it's usable here, and where the backing
/// binary lives (if applicable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorInfo {
    /// Short name matching `coordinator.executor` config values:
    /// "native", "claude", "codex", "gemini".
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Absolute path to the backing binary, if any. `None` for
    /// `native` (it's the running `wg` process itself).
    pub binary_path: Option<PathBuf>,
    /// True when the executor is usable right now. `native` is
    /// always true; others are true iff their binary was found.
    pub available: bool,
}

/// Probe PATH (and a handful of common absolute paths) for each
/// known executor. Order in the returned list matches `EXECUTORS`
/// declaration order so TUI menus have a stable presentation.
pub fn discover() -> Vec<ExecutorInfo> {
    EXECUTORS
        .iter()
        .map(|spec| {
            let (path, available) = match spec.name {
                "native" => (None, true),
                _ => {
                    let found = which_probes(spec.binary_candidates);
                    let avail = found.is_some();
                    (found, avail)
                }
            };
            ExecutorInfo {
                name: spec.name,
                description: spec.description,
                binary_path: path,
                available,
            }
        })
        .collect()
}

/// List only the executors usable right now. Convenient for TUI
/// pickers that shouldn't offer unusable options.
pub fn available() -> Vec<ExecutorInfo> {
    discover().into_iter().filter(|e| e.available).collect()
}

struct ExecutorSpec {
    name: &'static str,
    description: &'static str,
    /// Binaries to probe, in order. First hit wins.
    binary_candidates: &'static [&'static str],
}

const EXECUTORS: &[ExecutorSpec] = &[
    ExecutorSpec {
        name: "native",
        description: "nex — WG's built-in LLM agent loop (use endpoint URL for non-default servers)",
        binary_candidates: &[],
    },
    ExecutorSpec {
        name: "claude",
        description: "Claude CLI (Anthropic) via stream-json",
        binary_candidates: &["claude"],
    },
    ExecutorSpec {
        name: "codex",
        description: "OpenAI Codex CLI (`codex exec --json`)",
        binary_candidates: &["codex"],
    },
    ExecutorSpec {
        name: "gemini",
        description: "Google Gemini CLI",
        binary_candidates: &["gemini"],
    },
];

/// Look up each candidate on PATH via `which`-style lookup. Returns
/// the absolute path of the first hit, or `None`.
fn which_probes(candidates: &[&str]) -> Option<PathBuf> {
    for name in candidates {
        if let Some(path) = which_on_path(name) {
            return Some(path);
        }
    }
    None
}

/// Minimal which(1): split $PATH on `:` and return the first
/// executable file named `cmd`. Skips empty PATH entries.
fn which_on_path(cmd: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(cmd);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_is_always_available() {
        let info = discover()
            .into_iter()
            .find(|e| e.name == "native")
            .expect("native entry");
        assert!(info.available);
        assert!(info.binary_path.is_none());
    }

    #[test]
    fn discover_returns_all_executors() {
        let all = discover();
        let names: Vec<_> = all.iter().map(|e| e.name).collect();
        assert!(names.contains(&"native"));
        assert!(names.contains(&"claude"));
        assert!(names.contains(&"codex"));
        assert!(names.contains(&"gemini"));
        assert!(!names.contains(&"amplifier"));
    }

    #[test]
    fn available_filters_to_usable_only() {
        let usable = available();
        assert!(usable.iter().all(|e| e.available));
        // native should always be in the list.
        assert!(usable.iter().any(|e| e.name == "native"));
    }

    #[test]
    fn which_on_path_finds_common_shell_builtin_location() {
        // /bin/sh is on PATH basically everywhere; this is a sanity
        // check that the PATH walk + executable check work.
        // Skip on non-unix.
        #[cfg(unix)]
        {
            if std::path::Path::new("/bin/sh").exists() {
                let r = which_on_path("sh");
                assert!(r.is_some(), "expected to find `sh` on PATH");
            }
        }
    }
}
