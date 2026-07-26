use std::path::Path;

use anyhow::Result;
use worksgood::worktree_observer::{ReconcileSource, WorktreeObserver, run_watch_loop};

pub fn run_watch(state_dir: &Path, parent_pid: Option<u32>) -> Result<()> {
    run_watch_loop(state_dir, parent_pid)
}

pub fn run_reconcile(
    state_dir: &Path,
    preservation: bool,
    after_reap: bool,
    overflow: bool,
    json: bool,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut observer = WorktreeObserver::open(state_dir)?;
    if preservation {
        observer.enter_preservation_at(after_reap, now)?;
    }
    if overflow {
        observer.mark_overflow_at("operator-test-overflow", now)?;
    }
    let outcome = observer.reconcile_at(ReconcileSource::Manual, now)?;
    if json {
        println!("{}", serde_json::to_string_pretty(observer.projection())?);
    } else {
        println!(
            "Worktree activity: observed/unproven seq={} outcome={outcome:?} manifest={}",
            observer.projection().content_seq,
            observer.projection().manifest_digest,
        );
    }
    Ok(())
}
