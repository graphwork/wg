//! Collision-free storage for attempt-scoped runtime evidence.
//!
//! Lifecycle attempt IDs (`attempt-G-N`) are local to a task.  They must not be
//! used as a graph-global directory key.  New state is stored under a digest of
//! the complete authoritative source tuple.  Flat `attempts/attempt-G-N`
//! directories remain immutable compatibility evidence and are selected only
//! after their embedded source tuple matches the caller's expected tuple.

use crate::atomic_file::write_atomic_create_new;
use crate::graph::Task;
use crate::lifecycle::AttemptRef;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const TUPLE_FILE: &str = "source-tuple.json";
const NAMESPACE_DIR: &str = "by-source-tuple";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRuntimeKey {
    pub schema_version: u32,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
}

impl AttemptRuntimeKey {
    pub fn new(
        task_id: impl Into<String>,
        generation: u64,
        attempt_id: impl Into<String>,
        attempt_fence: u64,
        worktree_lease_epoch: u64,
    ) -> Self {
        Self {
            schema_version: 1,
            task_id: task_id.into(),
            generation,
            attempt_id: attempt_id.into(),
            attempt_fence,
            worktree_lease_epoch,
        }
    }

    pub fn for_attempt(task: &Task, attempt: &AttemptRef) -> Self {
        Self::new(
            task.id.clone(),
            attempt.generation,
            attempt.id.clone(),
            attempt.fence,
            attempt.fence,
        )
    }

    pub fn current(task: &Task) -> Result<Self> {
        let attempt = task
            .lifecycle
            .current_attempt
            .as_ref()
            .context("task has no current attempt")?;
        Ok(Self::for_attempt(task, attempt))
    }

    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("attempt runtime key serializes");
        blake3::hash(&bytes).to_hex().to_string()
    }
}

pub fn canonical_dir(wg_dir: &Path, key: &AttemptRuntimeKey) -> PathBuf {
    wg_dir
        .join("attempts")
        .join(NAMESPACE_DIR)
        .join(key.digest())
}

pub fn legacy_dir(wg_dir: &Path, key: &AttemptRuntimeKey) -> PathBuf {
    wg_dir.join("attempts").join(&key.attempt_id)
}

/// Read-only validation of a prospective namespace. This is run before spawn
/// workspace allocation so occupied/corrupt state cannot cause destructive
/// preparation followed by reservation retries.
pub fn preflight_namespace(wg_dir: &Path, key: &AttemptRuntimeKey) -> Result<()> {
    let root = canonical_dir(wg_dir, key);
    if !root.exists() {
        return Ok(());
    }
    let tuple_path = root.join(TUPLE_FILE);
    let found: AttemptRuntimeKey = serde_json::from_slice(&fs::read(&tuple_path).with_context(|| {
        format!(
            "occupied attempt runtime namespace lacks its tuple manifest: {} (evidence preserved; inspect once before retry)",
            tuple_path.display()
        )
    })?)?;
    if &found != key {
        bail!(
            "occupied attempt runtime namespace tuple mismatch at {}; evidence preserved — stop and inspect before retry",
            tuple_path.display()
        );
    }
    Ok(())
}

/// Bind a new runtime namespace to exactly one authoritative tuple. Replays are
/// idempotent. A digest collision or foreign pre-existing manifest fails once
/// with an actionable diagnostic and is never repaired by deleting evidence.
pub fn ensure_namespace(wg_dir: &Path, key: &AttemptRuntimeKey) -> Result<PathBuf> {
    let root = canonical_dir(wg_dir, key);
    fs::create_dir_all(&root).with_context(|| {
        format!(
            "failed to create attempt runtime namespace {}",
            root.display()
        )
    })?;
    let tuple_path = root.join(TUPLE_FILE);
    let bytes = serde_json::to_vec_pretty(key)?;
    match write_atomic_create_new(&tuple_path, &bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let found: AttemptRuntimeKey = serde_json::from_slice(&fs::read(&tuple_path)?)
                .with_context(|| {
                    format!(
                        "occupied attempt runtime namespace has an unreadable tuple manifest: {}",
                        tuple_path.display()
                    )
                })?;
            if &found != key {
                bail!(
                    "occupied attempt runtime namespace {} belongs to task '{}' generation {} attempt {} fence {} lease {}, not task '{}' generation {} attempt {} fence {} lease {}; evidence was preserved — inspect {} and stop/retry only after repairing the authoritative lifecycle tuple",
                    root.display(),
                    found.task_id,
                    found.generation,
                    found.attempt_id,
                    found.attempt_fence,
                    found.worktree_lease_epoch,
                    key.task_id,
                    key.generation,
                    key.attempt_id,
                    key.attempt_fence,
                    key.worktree_lease_epoch,
                    tuple_path.display()
                );
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(root)
}

/// Detect whether the historical flat slot is occupied by another tuple. This
/// is deliberately read-only and is suitable for spawn preflight before any
/// output/worktree allocation. Foreign evidence is not an error because new
/// state has a collision-free namespace; malformed/unattributable evidence is
/// reported as a single actionable error rather than probing later attempt IDs.
pub fn preflight_legacy_slot(
    wg_dir: &Path,
    key: &AttemptRuntimeKey,
) -> Result<Option<AttemptRuntimeKey>> {
    let legacy = legacy_dir(wg_dir, key);
    if !legacy.exists() {
        return Ok(None);
    }
    match embedded_key(&legacy)? {
        Some(found) if found == *key => Ok(None),
        Some(found) => Ok(Some(found)),
        None => bail!(
            "historical flat attempt state at {} has no readable authoritative source tuple; evidence was preserved. New runtime state would use {}, but launch is stopped so an operator can inspect the unattributed evidence once (no attempt-ID probing will occur)",
            legacy.display(),
            canonical_dir(wg_dir, key).display()
        ),
    }
}

/// Resolve a component for an exact tuple. New namespaced evidence wins. Flat
/// evidence is returned only after its embedded task/generation/attempt/fence/
/// lease tuple matches. This function never mutates historical bytes.
pub fn resolve_component(
    wg_dir: &Path,
    key: &AttemptRuntimeKey,
    component: &str,
) -> Result<Option<PathBuf>> {
    let canonical = canonical_dir(wg_dir, key);
    let manifest = canonical.join(TUPLE_FILE);
    if manifest.exists() {
        let found: AttemptRuntimeKey = serde_json::from_slice(&fs::read(&manifest)?)?;
        if found != *key {
            bail!(
                "attempt runtime namespace tuple mismatch at {}",
                manifest.display()
            );
        }
        let path = canonical.join(component);
        return Ok(path.exists().then_some(path));
    }

    let legacy = legacy_dir(wg_dir, key);
    if !legacy.exists() {
        return Ok(None);
    }
    let Some(found) = embedded_key(&legacy)? else {
        return Ok(None);
    };
    if found != *key {
        return Ok(None);
    }
    let path = legacy.join(component);
    Ok(path.exists().then_some(path))
}

pub fn component_for_write(
    wg_dir: &Path,
    key: &AttemptRuntimeKey,
    component: &str,
) -> Result<PathBuf> {
    Ok(ensure_namespace(wg_dir, key)?.join(component))
}

/// Lazily materialize a historical component into the exact namespace before
/// a mutating watchdog/observer opens it. The flat source is copied, never
/// renamed or edited. A same-directory temporary tree plus rename makes crash
/// replay idempotent; concurrent winners are accepted only at the exact target.
pub fn component_for_update(
    wg_dir: &Path,
    key: &AttemptRuntimeKey,
    component: &str,
) -> Result<PathBuf> {
    if component.is_empty() || component.contains('/') || component.contains('\\') {
        bail!("attempt runtime update component must be one path segment");
    }
    let canonical_root = canonical_dir(wg_dir, key);
    let canonical = canonical_root.join(component);
    if canonical.exists() {
        preflight_namespace(wg_dir, key)?;
        return Ok(canonical);
    }

    // Inspect compatibility evidence directly even if a prior crash already
    // created only the namespace manifest. Canonical components, once present,
    // still shadow the flat source for every reader.
    let legacy_root = legacy_dir(wg_dir, key);
    let legacy = if legacy_root.exists() && embedded_key(&legacy_root)?.as_ref() == Some(key) {
        let path = legacy_root.join(component);
        path.exists().then_some(path)
    } else {
        None
    };
    ensure_namespace(wg_dir, key)?;
    if canonical.exists() || legacy.is_none() {
        return Ok(canonical);
    }
    let legacy = legacy.expect("checked above");
    let temporary = canonical_root.join(format!(".migrate-{}-{}", component, uuid::Uuid::now_v7()));
    copy_tree(&legacy, &temporary).with_context(|| {
        format!(
            "failed to copy historical attempt evidence {} into exact namespace",
            legacy.display()
        )
    })?;
    if canonical.exists() {
        let _ = fs::remove_dir_all(&temporary);
        return Ok(canonical);
    }
    if let Err(error) = fs::rename(&temporary, &canonical) {
        let _ = fs::remove_dir_all(&temporary);
        if !canonical.exists() {
            return Err(error).with_context(|| {
                format!(
                    "failed to publish lazily indexed attempt evidence {}",
                    canonical.display()
                )
            });
        }
    }
    Ok(canonical)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(windows)]
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
    } else {
        fs::copy(source, destination)?;
    }
    Ok(())
}

/// Enumerate both new and historical component directories without modifying
/// either. Callers still validate each component's own embedded source tuple.
pub fn list_component_dirs(wg_dir: &Path, component: &str, limit: usize) -> Vec<PathBuf> {
    let attempts = wg_dir.join("attempts");
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(attempts.join(NAMESPACE_DIR)) {
        for entry in entries.flatten().take(limit) {
            let root = entry.path();
            let manifest = root.join(TUPLE_FILE);
            let valid = fs::read(&manifest)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<AttemptRuntimeKey>(&bytes).ok())
                .is_some_and(|key| entry.file_name().to_string_lossy() == key.digest());
            if !valid {
                continue;
            }
            let path = root.join(component);
            if path.exists() {
                out.push(path);
            }
        }
    }
    if out.len() < limit
        && let Ok(entries) = fs::read_dir(&attempts)
    {
        for entry in entries.flatten() {
            if out.len() >= limit {
                break;
            }
            if entry.file_name() == NAMESPACE_DIR {
                continue;
            }
            let path = entry.path().join(component);
            if path.exists() {
                out.push(path);
            }
        }
    }
    out
}

fn embedded_key(root: &Path) -> Result<Option<AttemptRuntimeKey>> {
    let observer = root.join("worktree-observer/state.json");
    let pi = root.join("pi/state.json");
    let mut keys = Vec::new();
    if observer.is_file() {
        let value: Value = serde_json::from_slice(&fs::read(&observer)?)?;
        if let Some(key) = key_from_value(value.pointer("/projection/source")) {
            keys.push(key);
        }
    }
    if pi.is_file() {
        let value: Value = serde_json::from_slice(&fs::read(&pi)?)?;
        if let Some(key) = key_from_value(value.pointer("/state/source")) {
            keys.push(key);
        }
    }
    if keys.windows(2).any(|pair| pair[0] != pair[1]) {
        bail!(
            "historical attempt evidence at {} contains conflicting observer/Pi source tuples; preserving it read-only",
            root.display()
        );
    }
    Ok(keys.into_iter().next())
}

fn key_from_value(value: Option<&Value>) -> Option<AttemptRuntimeKey> {
    let value = value?;
    Some(AttemptRuntimeKey::new(
        value.get("task_id")?.as_str()?,
        value.get("generation")?.as_u64()?,
        value.get("attempt_id")?.as_str()?,
        value.get("attempt_fence")?.as_u64()?,
        value.get("worktree_lease_epoch")?.as_u64()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(task: &str) -> AttemptRuntimeKey {
        AttemptRuntimeKey::new(task, 0, "attempt-0-1", 1, 1)
    }

    #[test]
    fn same_bare_attempt_for_two_tasks_has_distinct_namespaces() {
        let tmp = tempfile::tempdir().unwrap();
        let a = component_for_write(tmp.path(), &key("a"), "pi").unwrap();
        let b = component_for_write(tmp.path(), &key("b"), "pi").unwrap();
        assert_ne!(a, b);
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("state.json"), b"a").unwrap();
        fs::write(b.join("state.json"), b"b").unwrap();
        assert_eq!(fs::read(a.join("state.json")).unwrap(), b"a");
        assert_eq!(fs::read(b.join("state.json")).unwrap(), b"b");
    }

    #[test]
    fn foreign_flat_evidence_is_preserved_and_never_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let foreign = key("old-task");
        let legacy = legacy_dir(tmp.path(), &foreign);
        fs::create_dir_all(legacy.join("pi")).unwrap();
        let state = serde_json::json!({"state":{"source":{
            "task_id": foreign.task_id,
            "generation": foreign.generation,
            "attempt_id": foreign.attempt_id,
            "attempt_fence": foreign.attempt_fence,
            "worktree_lease_epoch": foreign.worktree_lease_epoch
        }}});
        let original = serde_json::to_vec(&state).unwrap();
        fs::write(legacy.join("pi/state.json"), &original).unwrap();

        let current = key("new-task");
        assert_eq!(
            preflight_legacy_slot(tmp.path(), &current).unwrap(),
            Some(foreign)
        );
        assert!(
            resolve_component(tmp.path(), &current, "pi/state.json")
                .unwrap()
                .is_none()
        );
        component_for_write(tmp.path(), &current, "pi").unwrap();
        assert_eq!(fs::read(legacy.join("pi/state.json")).unwrap(), original);
    }

    #[test]
    fn namespace_preparation_restart_is_idempotent_and_preserves_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let expected = key("restart-task");
        let root = ensure_namespace(tmp.path(), &expected).unwrap();
        let manifest = fs::read(root.join(TUPLE_FILE)).unwrap();
        fs::create_dir_all(root.join("worktree-observer")).unwrap();
        fs::write(root.join("worktree-observer/prepared"), b"partial").unwrap();
        fs::remove_dir_all(root.join("worktree-observer")).unwrap();

        let replayed = ensure_namespace(tmp.path(), &expected).unwrap();
        assert_eq!(replayed, root);
        assert_eq!(fs::read(root.join(TUPLE_FILE)).unwrap(), manifest);
        assert!(!root.join("worktree-observer").exists());
    }

    #[test]
    fn matching_flat_evidence_remains_readable_without_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let expected = key("old-task");
        let legacy = legacy_dir(tmp.path(), &expected);
        fs::create_dir_all(legacy.join("worktree-observer")).unwrap();
        fs::write(
            legacy.join("worktree-observer/state.json"),
            serde_json::to_vec(&serde_json::json!({"projection":{"source":{
                "task_id": expected.task_id,
                "generation": expected.generation,
                "attempt_id": expected.attempt_id,
                "attempt_fence": expected.attempt_fence,
                "worktree_lease_epoch": expected.worktree_lease_epoch
            }}}))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            resolve_component(tmp.path(), &expected, "worktree-observer").unwrap(),
            Some(legacy.join("worktree-observer"))
        );
        let before = fs::read(legacy.join("worktree-observer/state.json")).unwrap();
        let indexed = component_for_update(tmp.path(), &expected, "worktree-observer").unwrap();
        assert_ne!(indexed, legacy.join("worktree-observer"));
        assert_eq!(fs::read(indexed.join("state.json")).unwrap(), before);
        fs::write(indexed.join("state.json"), b"updated canonical state").unwrap();
        assert_eq!(
            fs::read(legacy.join("worktree-observer/state.json")).unwrap(),
            before,
            "lazy indexing must never mutate historical evidence"
        );
    }
}
