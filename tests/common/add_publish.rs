//! Test-only migration helper for fixtures that need runnable work.
//!
//! Production `wg add` is staging-only. Tests mark legacy runnable fixtures
//! with `__publish_after_add`; this helper executes the real two-command user
//! flow (`wg add`, then `wg publish <id> --only`) without exposing a CLI flag.

use std::path::Path;
use std::process::Output;

pub const PUBLISH_MARKER: &str = "__publish_after_add";

pub fn run(wg_dir: &Path, args: &[&str], runner: impl Fn(&Path, &[&str]) -> Output) -> Output {
    if !args.contains(&PUBLISH_MARKER) {
        return runner(wg_dir, args);
    }

    let filtered: Vec<&str> = args
        .iter()
        .copied()
        .filter(|arg| *arg != PUBLISH_MARKER)
        .collect();
    let added = runner(wg_dir, &filtered);
    if !added.status.success() {
        return added;
    }

    let stdout = String::from_utf8_lossy(&added.stdout);
    let task_id = filtered
        .iter()
        .position(|arg| *arg == "--id")
        .and_then(|index| filtered.get(index + 1))
        .copied()
        .or_else(|| {
            stdout
                .lines()
                .next()
                .and_then(|line| line.rsplit_once('('))
                .map(|(_, tail)| tail.trim_end_matches(')'))
        })
        .expect("staged add should identify its task id");

    let published = runner(wg_dir, &["publish", task_id, "--only"]);
    assert!(
        published.status.success(),
        "explicit publish of '{}' failed:\nstdout: {}\nstderr: {}",
        task_id,
        String::from_utf8_lossy(&published.stdout),
        String::from_utf8_lossy(&published.stderr)
    );
    added
}
