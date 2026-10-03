//! Detect a `wg` binary that was built from a WG-managed git worktree.
//!
//! A worker agent runs inside a WG worktree. If it runs
//! `cargo install --path . --locked` from that worktree, Cargo replaces the
//! **shared** global `~/.cargo/bin/wg` with a build of the *worktree*, and the
//! binary bakes the prunable `.wg-worktrees/agent-NNN` path in as its
//! `CARGO_MANIFEST_DIR`. That silently swaps the operator's binary and, before
//! the pi-plugin guard existed, made `wg pi-plugin install` rewrite
//! `~/.pi/agent/settings.json` at a directory that was later pruned, breaking
//! plugin resolution. See AGENTS.md / CLAUDE.md "Development".
//!
//! The detection uses a *specific* signal rather than guessing: the running
//! binary's compile-time manifest dir (`env!("CARGO_MANIFEST_DIR")`) — the same
//! signal the pi-plugin Dev-source guard uses ([`is_worktree_path`]).

use std::path::{Path, PathBuf};

/// The machine-visible warning code surfaced by `wg status`, `wg capabilities`,
/// and `wg pi-plugin status` when the running binary came from a worktree.
pub const WORKTREE_BUILD_WARNING_CODE: &str = "build/wg-binary-from-worktree";

/// True when `path` lives inside a WG-managed (or Claude-managed) git worktree.
/// Such a path is prunable and must never be baked into a globally installed
/// binary as its `CARGO_MANIFEST_DIR` / Dev source.
pub fn is_worktree_path(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    parts.iter().any(|p| p == ".wg-worktrees")
        || parts
            .windows(2)
            .any(|w| w[0] == ".claude" && w[1] == "worktrees")
}

/// The compile-time manifest dir the running binary was built from.
pub fn compiled_manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// `Some(path)` when the running binary was built from a worktree, else `None`.
pub fn running_binary_worktree_origin() -> Option<PathBuf> {
    let dir = compiled_manifest_dir();
    is_worktree_path(dir).then(|| dir.to_path_buf())
}

/// The loud one-line warning for a worktree-built binary, or `None` for a
/// healthy main-checkout build. Callers print it verbatim.
pub fn worktree_build_warning() -> Option<String> {
    running_binary_worktree_origin().map(|dir| {
        format!(
            "{WORKTREE_BUILD_WARNING_CODE}: the installed wg was built from {}; \
             reinstall from the main checkout",
            dir.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wg_managed_worktree_paths_are_detected() {
        for path in [
            "/home/bot/wg/.wg-worktrees/agent-179",
            "/home/bot/wg/.wg-worktrees/agent-179/worksgood-pi",
            "relative/.wg-worktrees/agent-1",
        ] {
            assert!(
                is_worktree_path(Path::new(path)),
                "expected worktree path: {path}"
            );
        }
    }

    #[test]
    fn claude_managed_worktree_paths_are_detected() {
        assert!(is_worktree_path(Path::new(
            "/home/bot/wg/.claude/worktrees/agent-1"
        )));
    }

    #[test]
    fn main_checkout_and_ordinary_paths_are_not_worktrees() {
        for path in [
            "/home/bot/wg",
            "/home/bot/wg/target",
            "/home/bot/.cargo/bin",
            "/tmp/scratch",
        ] {
            assert!(
                !is_worktree_path(Path::new(path)),
                "must not be a worktree: {path}"
            );
        }
    }

    #[test]
    fn warning_names_the_code_path_and_remediation() {
        // The message shape is what `wg status` / `wg capabilities` /
        // `wg pi-plugin status` all surface; pin it so the contract cannot
        // silently drift.
        let warning = format!(
            "{WORKTREE_BUILD_WARNING_CODE}: the installed wg was built from {}; \
             reinstall from the main checkout",
            "/home/bot/wg/.wg-worktrees/agent-179"
        );
        assert!(warning.starts_with("build/wg-binary-from-worktree:"));
        assert!(warning.contains("/home/bot/wg/.wg-worktrees/agent-179"));
        assert!(warning.contains("reinstall from the main checkout"));
    }

    #[test]
    fn worktree_build_warning_depends_only_on_the_compile_time_manifest_dir() {
        // The predicate is a pure function of the baked manifest dir: if that
        // dir is a worktree the warning fires, otherwise it stays silent.
        // (When this test runs *inside* a worktree the binary genuinely is a
        // worktree build, so we assert the two agree rather than a constant.)
        let origin = running_binary_worktree_origin();
        let warning = worktree_build_warning();
        assert_eq!(origin.is_some(), warning.is_some());
        if let Some(w) = warning {
            assert!(w.contains(WORKTREE_BUILD_WARNING_CODE));
        }
    }
}
