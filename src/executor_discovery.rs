//! Discover which executor backends are available on this system.
//!
//! The TUI's coordinator-creation dialog and the config panel use
//! this to populate executor choice menus with things that actually
//! work — not a static list baked into the binary. When `codex` is
//! missing from PATH, it shouldn't be offered as an option.
//!
//! `native` is always available (it's this binary itself). The CLI
//! adapters (`claude`, `codex`, `gemini`, stable worker CLIs, and
//! experimental worker CLIs) are probed by looking for the binary on PATH.

use std::path::PathBuf;

pub const CORE_EXECUTORS: &[&str] = &["native", "claude", "codex", "shell"];
pub const STABLE_EXTERNAL_EXECUTORS: &[&str] = &["opencode", "aider", "goose", "qwen", "cline"];
pub const PROVIDER_SPECIFIC_EXECUTORS: &[&str] = &["gemini"];
pub const EXPERIMENTAL_EXTERNAL_EXECUTORS: &[&str] =
    &["octomind", "dexto", "crush", "amplifier", "pi"];

/// One executor, whether it's usable here, and where the backing
/// binary lives (if applicable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorInfo {
    /// Short name matching `coordinator.executor` config values:
    /// "native", "claude", "codex", "gemini", "opencode", etc.
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
        name: "shell",
        description: "Shell command executor (`bash -c`; no LLM)",
        binary_candidates: &["bash"],
    },
    ExecutorSpec {
        name: "gemini",
        description: "Google Gemini CLI",
        binary_candidates: &["gemini"],
    },
    ExecutorSpec {
        name: "opencode",
        description: "Stable OpenCode CLI worker (`opencode run --format json`)",
        binary_candidates: &["opencode"],
    },
    ExecutorSpec {
        name: "aider",
        description: "Stable Aider CLI worker (`aider --message-file ... --yes-always`)",
        binary_candidates: &["aider"],
    },
    ExecutorSpec {
        name: "goose",
        description: "Stable Goose CLI worker (`goose run --no-session -i ...`)",
        binary_candidates: &["goose"],
    },
    ExecutorSpec {
        name: "qwen",
        description: "Stable Qwen Code CLI worker (`qwen --output-format json --yolo`)",
        binary_candidates: &["qwen", "qwen-code", "qwen_code"],
    },
    ExecutorSpec {
        name: "cline",
        description: "Stable Cline CLI worker (`cline --json --auto-approve true`)",
        binary_candidates: &["cline"],
    },
    ExecutorSpec {
        name: "octomind",
        description: "Octomind CLI live chat (`octomind run -m openrouter:<vendor>/<model>`; line-oriented REPL, tmux-safe)",
        binary_candidates: &["octomind"],
    },
    ExecutorSpec {
        name: "dexto",
        description: "Dexto CLI live chat (`dexto --agent <yml>`; OpenRouter via generated agent config)",
        binary_candidates: &["dexto"],
    },
    ExecutorSpec {
        name: "crush",
        description: "Experimental Crush CLI worker (`crush run`; verify flags against your installed version)",
        binary_candidates: &["crush"],
    },
    ExecutorSpec {
        name: "amplifier",
        description: "Experimental Amplifier CLI worker (`amplifier run --mode single --output-format json --bundle wg`)",
        binary_candidates: &["amplifier"],
    },
    ExecutorSpec {
        name: "pi",
        description: "Experimental Pi CLI (pi.dev): plain chat panes launch interactive `pi`; `wg pi-handler` remains the RPC/worker bridge",
        binary_candidates: &["pi"],
    },
];

// --- pi: route satisfiability (Topology A `pi` binary OR Topology B Node host) -

/// The Node-host backing for a `pi:` route (Topology B): WG spawns
/// `node <host_script>`, which loads the built plugin bundle in-process.
/// All three pieces must be present for the route to be satisfiable this way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiNodeHost {
    /// Absolute path to the `node` binary.
    pub node: PathBuf,
    /// Absolute path to `worksgood-pi/host/wg-pi-host.mjs`.
    pub host_script: PathBuf,
    /// Absolute path to `worksgood-pi/pi-worksgood/index.js`.
    pub plugin_bundle: PathBuf,
}

/// Which transports can satisfy a `pi:` route on this system.
///
/// A `pi:` route is satisfiable by **either** a `pi` binary on PATH
/// (Topology A — `pi --mode rpc`) **or** Node + the `wg-pi-host.mjs` SDK
/// host + the built plugin bundle (Topology B — `node wg-pi-host.mjs`).
/// `wg config lint` rejects a configured `pi:` route when neither is present
/// (`integration-plan-v2.md` §4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PiRouteAvailability {
    /// Absolute path to the `pi` binary, if found (Topology A).
    pub pi_binary: Option<PathBuf>,
    /// The Node-host triple, if all three pieces are present (Topology B).
    pub node_host: Option<PiNodeHost>,
}

impl PiRouteAvailability {
    /// True when at least one transport (A or B) can run a `pi:` route.
    pub fn satisfiable(&self) -> bool {
        self.pi_binary.is_some() || self.node_host.is_some()
    }
}

/// Probe a fixed set of PATH dirs for the first matching candidate binary.
/// Split out from [`which_on_path`] so tests can inject a synthetic PATH.
fn which_in_dirs(dirs: &[PathBuf], candidates: &[&str]) -> Option<PathBuf> {
    for dir in dirs {
        for name in candidates {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Locate a plugin root that holds BOTH the SDK host script and the built
/// bundle. Returns `(host_script, plugin_bundle)` for the first candidate dir
/// where `host/wg-pi-host.mjs` and `pi-worksgood/index.js` both exist.
fn locate_pi_node_host(plugin_dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    for dir in plugin_dirs {
        let host_script = dir.join("host").join("wg-pi-host.mjs");
        let plugin_bundle = dir.join("pi-worksgood").join("index.js");
        if host_script.is_file() && plugin_bundle.is_file() {
            return Some((host_script, plugin_bundle));
        }
    }
    None
}

/// Pure satisfiability check over injected inputs — the testable core of
/// [`pi_route_availability`]. `path_dirs` are PATH entries to probe for the
/// `pi` and `node` binaries; `plugin_dirs` are candidate plugin roots probed
/// for the host script + built bundle.
pub fn pi_route_availability_in(
    path_dirs: &[PathBuf],
    plugin_dirs: &[PathBuf],
) -> PiRouteAvailability {
    let pi_binary = which_in_dirs(path_dirs, &["pi"]);
    let node_host = match (
        which_in_dirs(path_dirs, &["node"]),
        locate_pi_node_host(plugin_dirs),
    ) {
        (Some(node), Some((host_script, plugin_bundle))) => Some(PiNodeHost {
            node,
            host_script,
            plugin_bundle,
        }),
        _ => None,
    };
    PiRouteAvailability {
        pi_binary,
        node_host,
    }
}

/// Candidate `worksgood-pi/` roots to probe for the Node-host bundle, in
/// precedence order: explicit `WG_PI_PLUGIN_DIR`, the in-repo source tree
/// (dev builds), next to the running binary, then the global pi-extension
/// install (`~/.pi/agent/extensions/pi-worksgood`).
pub fn pi_plugin_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(explicit) = std::env::var_os("WG_PI_PLUGIN_DIR") {
        dirs.push(PathBuf::from(explicit));
    }
    // In-repo source tree (dev): <crate>/worksgood-pi.
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("worksgood-pi"));
    // Alongside the installed binary: <exe_dir>/worksgood-pi.
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("worksgood-pi"));
    }
    // Global pi-extension install.
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(
            PathBuf::from(home)
                .join(".pi")
                .join("agent")
                .join("extensions")
                .join("pi-worksgood"),
        );
    }
    dirs
}

/// Determine, from the live environment, whether a `pi:` route is satisfiable
/// (Topology A `pi` binary OR Topology B Node host + bundle).
pub fn pi_route_availability() -> PiRouteAvailability {
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    pi_route_availability_in(&path_dirs, &pi_plugin_candidate_dirs())
}

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
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"gemini"));
        assert!(names.contains(&"opencode"));
        assert!(names.contains(&"aider"));
        assert!(names.contains(&"goose"));
        assert!(names.contains(&"qwen"));
        assert!(names.contains(&"cline"));
        assert!(names.contains(&"crush"));
        assert!(names.contains(&"amplifier"));
        assert!(names.contains(&"octomind"));
        assert!(names.contains(&"dexto"));
        assert!(names.contains(&"pi"));
    }

    #[test]
    fn executor_choice_groups_match_discovery_names() {
        let names: Vec<_> = discover().into_iter().map(|e| e.name).collect();
        for group in [
            CORE_EXECUTORS,
            STABLE_EXTERNAL_EXECUTORS,
            PROVIDER_SPECIFIC_EXECUTORS,
            EXPERIMENTAL_EXTERNAL_EXECUTORS,
        ] {
            for name in group {
                assert!(
                    names.contains(name),
                    "choice group includes {name}, but discovery omitted it"
                );
            }
        }
    }

    #[test]
    fn available_filters_to_usable_only() {
        let usable = available();
        assert!(usable.iter().all(|e| e.available));
        // native should always be in the list.
        assert!(usable.iter().any(|e| e.name == "native"));
    }

    /// Create a fake executable file at `dir/name` (0o755 on unix).
    fn touch_exe(dir: &std::path::Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    #[test]
    fn pi_route_satisfiable_via_fake_pi_binary() {
        let bin = tempfile::TempDir::new().unwrap();
        touch_exe(bin.path(), "pi");
        // No plugin dirs at all → only the pi binary can satisfy the route.
        let avail =
            pi_route_availability_in(&[bin.path().to_path_buf()], &[] as &[std::path::PathBuf]);
        assert!(avail.satisfiable(), "a `pi` binary alone must satisfy pi:");
        assert!(avail.pi_binary.is_some());
        assert!(avail.node_host.is_none());
    }

    #[test]
    fn pi_route_satisfiable_via_fake_node_host_bundle() {
        let bin = tempfile::TempDir::new().unwrap();
        touch_exe(bin.path(), "node"); // node present, but NO pi binary
        let plugin = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(plugin.path().join("host")).unwrap();
        std::fs::create_dir_all(plugin.path().join("pi-worksgood")).unwrap();
        std::fs::write(plugin.path().join("host").join("wg-pi-host.mjs"), b"//host").unwrap();
        std::fs::write(
            plugin.path().join("pi-worksgood").join("index.js"),
            b"//bundle",
        )
        .unwrap();

        let avail =
            pi_route_availability_in(&[bin.path().to_path_buf()], &[plugin.path().to_path_buf()]);
        assert!(
            avail.satisfiable(),
            "node + wg-pi-host.mjs + pi-worksgood/index.js must satisfy pi:"
        );
        assert!(avail.pi_binary.is_none(), "no pi binary in this scenario");
        let host = avail.node_host.expect("node host triple present");
        assert!(host.host_script.ends_with("wg-pi-host.mjs"));
        assert!(host.plugin_bundle.ends_with("index.js"));
    }

    #[test]
    fn pi_route_rejected_when_neither_present() {
        // node exists but the bundle does NOT, and there is no pi binary.
        let bin = tempfile::TempDir::new().unwrap();
        touch_exe(bin.path(), "node");
        let empty_plugin = tempfile::TempDir::new().unwrap(); // no host/ or pi-worksgood/
        let avail = pi_route_availability_in(
            &[bin.path().to_path_buf()],
            &[empty_plugin.path().to_path_buf()],
        );
        assert!(
            !avail.satisfiable(),
            "node alone (no host script / bundle, no pi binary) must NOT satisfy pi:"
        );

        // And the fully-empty case (no binaries, no plugin dirs).
        let nothing =
            pi_route_availability_in(&[] as &[std::path::PathBuf], &[] as &[std::path::PathBuf]);
        assert!(!nothing.satisfiable());
    }

    #[test]
    fn pi_route_node_host_needs_node_binary_too() {
        // A complete bundle but NO node binary on PATH → Topology B unusable.
        let bin = tempfile::TempDir::new().unwrap(); // empty PATH dir
        let plugin = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(plugin.path().join("host")).unwrap();
        std::fs::create_dir_all(plugin.path().join("pi-worksgood")).unwrap();
        std::fs::write(plugin.path().join("host").join("wg-pi-host.mjs"), b"//host").unwrap();
        std::fs::write(
            plugin.path().join("pi-worksgood").join("index.js"),
            b"//bundle",
        )
        .unwrap();
        let avail =
            pi_route_availability_in(&[bin.path().to_path_buf()], &[plugin.path().to_path_buf()]);
        assert!(
            !avail.satisfiable(),
            "bundle without a node binary cannot run Topology B"
        );
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
