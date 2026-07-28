use std::path::{Path, PathBuf};

use anyhow::Result;
use worksgood::worktree_observer::{ReconcileSource, WorktreeObserver, run_watch_loop};

fn mutable_state_dir(state_dir: &Path) -> Result<PathBuf> {
    if state_dir
        .components()
        .any(|part| part.as_os_str() == "by-source-tuple")
    {
        return Ok(state_dir.to_path_buf());
    }
    let projection = worksgood::worktree_observer::read_projection(state_dir)?;
    let mut ancestor = Some(state_dir);
    let wg_dir = loop {
        let Some(path) = ancestor else {
            anyhow::bail!(
                "cannot locate graph authority for historical observer {}",
                state_dir.display()
            );
        };
        if path.join("graph.jsonl").is_file() {
            break path;
        }
        ancestor = path.parent();
    };
    let identity = projection.source.identity;
    let key = worksgood::attempt_runtime::AttemptRuntimeKey::new(
        identity.task_id,
        identity.generation,
        identity.attempt_id,
        identity.attempt_fence,
        identity.worktree_lease_epoch,
    );
    worksgood::attempt_runtime::component_for_update(wg_dir, &key, "worktree-observer")
}

pub fn run_watch(state_dir: &Path, parent_pid: Option<u32>) -> Result<()> {
    run_watch_loop(&mutable_state_dir(state_dir)?, parent_pid)
}

pub fn run_reconcile(
    state_dir: &Path,
    preservation: bool,
    after_reap: bool,
    overflow: bool,
    json: bool,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let state_dir = mutable_state_dir(state_dir)?;
    let mut observer = WorktreeObserver::open(&state_dir)?;
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
