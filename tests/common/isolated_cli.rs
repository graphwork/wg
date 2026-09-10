//! Hermetic command boundary for disposable CLI integration fixtures.
//!
//! A test runner may itself be a WG worker.  Passing that worker's graph,
//! attempt, capability, or worktree environment to a `wg --dir <temp>` child
//! makes the child look like a cross-graph mutation attempt.  That failure is
//! correct in production, but it means the fixture never exercises its own
//! graph.  Build disposable commands from an allowlist instead of trying to
//! maintain a partial denylist of worker variables.

use std::path::Path;
use std::process::Command;

/// Construct a command with only the host values a disposable CLI fixture
/// needs.  This changes the child command only; it never mutates the test
/// runner's environment or the authority of the live worker running the test.
pub fn command(binary: &Path, fixture_root: &Path) -> Command {
    let home = fixture_root.join(".wg-test-home");
    std::fs::create_dir_all(home.join(".config")).expect("create fixture config home");
    std::fs::create_dir_all(home.join(".cache")).expect("create fixture cache home");

    let mut command = Command::new(binary);
    command.env_clear();
    command.env("HOME", &home);
    command.env("XDG_CONFIG_HOME", home.join(".config"));
    command.env("XDG_CACHE_HOME", home.join(".cache"));
    command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command.env("USER", "wg-integration-test");
    command.env("LANG", "C.UTF-8");
    command.env("LC_ALL", "C.UTF-8");
    command
}

/// Assert the allowlisted command boundary stays free of every worker/control
/// variable.  `Command::get_envs` exposes only explicit overrides after
/// `env_clear`, so this catches a future accidental authority re-introduction.
pub fn assert_worker_authority_is_absent(command: &Command) {
    let leaked: Vec<String> = command
        .get_envs()
        .filter_map(|(key, value)| {
            let key = key.to_string_lossy();
            (key.starts_with("WG_") && value.is_some()).then(|| key.into_owned())
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "disposable CLI command leaked WG worker authority: {leaked:?}"
    );
}
