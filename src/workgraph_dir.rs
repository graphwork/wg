use std::path::PathBuf;

/// Dir-name candidates we accept, in priority order.
///
/// `.wg` is the modern name (written by `wg init`); `.workgraph` is the
/// legacy name that pre-existing projects still use.
pub const WORKGRAPH_DIR_NAMES: &[&str] = &[".wg", ".workgraph"];

/// Resolve the WG directory for this invocation.
///
/// Precedence, highest first:
///
/// 1. Explicit `--dir <path>` CLI flag.
/// 2. `WG_DIR` environment variable.
/// 3. Project discovery, walking up from `cwd`.
/// 4. Global fallback `~/.wg`, then `~/.workgraph`.
/// 5. Default `./.wg` in the current directory.
///
/// The resolver does not create directories. Callers decide which commands
/// should auto-create a global fallback.
pub fn resolve_workgraph_dir(
    cli_dir: Option<PathBuf>,
    env_dir: Option<PathBuf>,
    cwd: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(p) = cli_dir {
        return descend_into_wg_subdir_if_project_root(p);
    }

    if let Some(p) = env_dir.filter(|p| !p.as_os_str().is_empty()) {
        return descend_into_wg_subdir_if_project_root(p);
    }

    if let Some(start) = cwd.as_ref() {
        let mut cur = start.as_path();
        loop {
            for name in WORKGRAPH_DIR_NAMES {
                let candidate = cur.join(name);
                if candidate.is_dir() {
                    return candidate;
                }
            }
            match cur.parent() {
                Some(parent) => cur = parent,
                None => break,
            }
        }
    }

    if let Some(home) = home_dir.as_ref() {
        for name in WORKGRAPH_DIR_NAMES {
            let global = home.join(name);
            if global.is_dir() {
                return global;
            }
        }
    }

    cwd.map(|c| c.join(".wg"))
        .unwrap_or_else(|| PathBuf::from(".wg"))
}

/// If the given path looks like a project root containing a `.wg` or
/// `.workgraph` subdir, descend into that subdir. Otherwise return the
/// path unchanged.
pub fn descend_into_wg_subdir_if_project_root(p: PathBuf) -> PathBuf {
    let basename = p.file_name().and_then(|n| n.to_str());
    if matches!(basename, Some(".wg") | Some(".workgraph")) {
        return p;
    }
    if p.join("graph.jsonl").is_file() {
        return p;
    }
    for name in WORKGRAPH_DIR_NAMES {
        let candidate = p.join(name);
        if candidate.is_dir() {
            return candidate;
        }
    }
    p
}
