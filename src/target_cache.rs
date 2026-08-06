//! Immutable Cargo-target baselines with private hard-link copy-on-write layers.
//!
//! Cargo must never have two divergent worktrees writing one target directory.
//! This module instead keeps a content-keyed, read-only baseline and gives each
//! attempt its own directory tree. On ext4 (where native reflinks are absent),
//! unchanged regular files are hard-linked into the attempt layer. Baseline
//! files are read-only; Cargo's normal temp-file + rename publication breaks the
//! link on write. A direct in-place write fails closed rather than mutating the
//! shared artifact. Incremental compilation is disabled by the spawn path.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const CACHE_SCHEMA: u32 = 1;
const LAYER_MANIFEST: &str = ".wg-target-layer.json";
const BASELINE_MANIFEST: &str = ".wg-target-baseline.json";
const READY: &str = "READY";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCacheKey {
    pub schema: u32,
    pub source_baseline: String,
    pub cargo_lock: String,
    pub rustc: String,
    pub target_triple: String,
    pub features: String,
    pub profile: String,
    pub flags: String,
}

impl TargetCacheKey {
    pub fn digest(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("target cache key serializes");
        blake3::hash(&encoded).to_hex().to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LayerManifest {
    schema: u32,
    key: TargetCacheKey,
    source_root: String,
    baseline_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetLayer {
    pub path: PathBuf,
    pub key: TargetCacheKey,
    pub baseline_path: Option<PathBuf>,
}

struct KeyLock {
    _file: File,
}

impl KeyLock {
    fn acquire(root: &Path, key: &str) -> Result<Self> {
        let locks = root.join("locks");
        fs::create_dir_all(&locks)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(locks.join(format!("{key}.lock")))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error()).context("lock target cache key");
            }
        }
        Ok(Self { _file: file })
    }
}

fn command_stdout(root: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn hash_file(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .unwrap_or_else(|_| "missing".to_string())
}

/// Compute the immutable warm-layer namespace. Cargo itself still validates
/// fine-grained fingerprints; this outer key prevents reuse across all inputs
/// which can change artifact semantics.
pub fn compute_key(source_root: &Path) -> TargetCacheKey {
    let rustc = command_stdout(source_root, "rustc", &["--version", "--verbose"])
        .unwrap_or_else(|| "rustc-unavailable".to_string());
    let target_triple = std::env::var("CARGO_BUILD_TARGET")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            rustc
                .lines()
                .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "host-unknown".to_string());
    let source_baseline = command_stdout(source_root, "git", &["rev-parse", "HEAD^{tree}"])
        .unwrap_or_else(|| hash_file(&source_root.join("Cargo.toml")));
    let features = std::env::var("WG_CARGO_FEATURES").unwrap_or_else(|_| "default".to_string());
    let profile = std::env::var("WG_CARGO_PROFILE").unwrap_or_else(|_| "test".to_string());
    let flags = [
        ("RUSTFLAGS", ""),
        ("RUSTDOCFLAGS", ""),
        ("CARGO_ENCODED_RUSTFLAGS", ""),
        ("CARGO_BUILD_RUSTFLAGS", ""),
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only"),
        ("CARGO_PROFILE_TEST_DEBUG", "line-tables-only"),
    ]
    .into_iter()
    .map(|(name, default)| {
        format!(
            "{name}={}",
            std::env::var(name).unwrap_or_else(|_| default.to_string())
        )
    })
    .collect::<Vec<_>>()
    .join("\n");
    TargetCacheKey {
        schema: CACHE_SCHEMA,
        source_baseline,
        cargo_lock: hash_file(&source_root.join("Cargo.lock")),
        rustc,
        target_triple,
        features,
        profile,
        flags,
    }
}

fn baseline_dir(root: &Path, digest: &str) -> PathBuf {
    root.join("baselines").join(digest)
}

fn baseline_is_ready(path: &Path, key: &TargetCacheKey) -> bool {
    if !path.join(READY).is_file() {
        return false;
    }
    fs::read(path.join(BASELINE_MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TargetCacheKey>(&bytes).ok())
        .is_some_and(|found| &found == key)
}

/// Prepare one private writable target. No mutable directory is shared.
pub fn prepare_layer(cache_root: &Path, source_root: &Path, agent_id: &str) -> Result<TargetLayer> {
    let key = compute_key(source_root);
    prepare_layer_with_key(cache_root, source_root, agent_id, key)
}

fn prepare_layer_with_key(
    cache_root: &Path,
    source_root: &Path,
    agent_id: &str,
    key: TargetCacheKey,
) -> Result<TargetLayer> {
    let digest = key.digest();
    let _lock = KeyLock::acquire(cache_root, &digest)?;
    let layer_parent = cache_root.join("layers").join(&digest).join(agent_id);
    let target = layer_parent.join("target");
    if layer_parent.exists() {
        bail!(
            "target cache layer already exists: {}",
            layer_parent.display()
        );
    }
    fs::create_dir_all(&target)?;
    let baseline = baseline_dir(cache_root, &digest);
    let baseline_target = baseline.join("target");
    let baseline_path = if baseline_is_ready(&baseline, &key) {
        hardlink_tree(&baseline_target, &target)?;
        Some(baseline_target)
    } else {
        None
    };
    let manifest = LayerManifest {
        schema: CACHE_SCHEMA,
        key: key.clone(),
        source_root: source_root.to_string_lossy().to_string(),
        baseline_path: baseline_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    };
    write_new(
        &target.join(LAYER_MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(TargetLayer {
        path: target,
        key,
        baseline_path,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn hardlink_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if relative == Path::new(LAYER_MANIFEST)
            || relative == Path::new(BASELINE_MANIFEST)
            || relative == Path::new(READY)
            || entry.file_name() == ".cargo-lock"
            || entry.file_name() == ".rustc_info.json"
        {
            continue;
        }
        let output = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&output)?;
        } else if entry.file_type().is_symlink() {
            let link = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, &output)?;
            #[cfg(windows)]
            {
                if entry.path().is_dir() {
                    std::os::windows::fs::symlink_dir(link, &output)?;
                } else {
                    std::os::windows::fs::symlink_file(link, &output)?;
                }
            }
        } else if entry.file_type().is_file() {
            fs::hard_link(entry.path(), &output).with_context(|| {
                format!(
                    "hard-link immutable target artifact {} -> {}",
                    entry.path().display(),
                    output.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_baseline_read_only(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut entries = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.depth()));
    for entry in entries {
        if entry.file_type().is_symlink() {
            continue;
        }
        let metadata = entry.metadata()?;
        let mode = metadata.permissions().mode();
        let readonly = if entry.file_type().is_dir() {
            0o555
        } else if mode & 0o111 != 0 {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(entry.path(), fs::Permissions::from_mode(readonly))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_baseline_read_only(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_symlink() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

/// Promote a clean completed attempt to the immutable baseline. A layer built
/// from changed/uncommitted source is never promoted into its old namespace.
pub fn promote_layer(target: &Path) -> Result<bool> {
    let bytes = fs::read(target.join(LAYER_MANIFEST))
        .with_context(|| format!("read target layer manifest in {}", target.display()))?;
    let manifest: LayerManifest = serde_json::from_slice(&bytes)?;
    let source_root = PathBuf::from(&manifest.source_root);
    if source_is_dirty(&source_root) {
        return Ok(false);
    }
    // A programmer commonly commits after its last successful build. The
    // clean committed tree is a stronger key than the launch-time baseline;
    // publish under that current tree rather than poisoning the old key.
    let current_key = compute_key(&source_root);
    promote_layer_validated(target, &current_key)
}

fn source_is_dirty(source_root: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(source_root)
        .output()
        .map(|output| {
            !output.status.success()
                || String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line != "?? .wg-cleanup-pending")
        })
        .unwrap_or(true)
}

fn promote_layer_validated(target: &Path, key: &TargetCacheKey) -> Result<bool> {
    let digest = key.digest();
    let cache_root = target
        .ancestors()
        .nth(4)
        .ok_or_else(|| anyhow!("invalid target layer layout: {}", target.display()))?;
    let _lock = KeyLock::acquire(cache_root, &digest)?;
    let baseline = baseline_dir(cache_root, &digest);
    if baseline_is_ready(&baseline, key) {
        return Ok(false);
    }
    fs::create_dir_all(baseline.parent().expect("baseline parent"))?;
    if baseline.exists() {
        // A missing READY marker is a crashed, never-published promotion. The
        // per-key lock proves no live promoter owns it; repair in place.
        make_tree_writable(&baseline)?;
        fs::remove_dir_all(&baseline)?;
    }
    let baseline_target = baseline.join("target");
    fs::create_dir_all(&baseline_target)?;
    hardlink_tree(target, &baseline_target)?;
    write_new(
        &baseline.join(BASELINE_MANIFEST),
        &serde_json::to_vec_pretty(key)?,
    )?;
    // READY is the publication boundary. Readers hold the same key lock while
    // cloning and never observe a partially materialized baseline.
    write_new(&baseline.join(READY), b"ready\n")?;
    make_baseline_read_only(&baseline)?;
    Ok(true)
}

#[cfg(unix)]
fn make_tree_writable(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_symlink() {
            let mode = if entry.file_type().is_dir() {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_tree_writable(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_symlink() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

/// Extract the key digest from a canonical layer path.
pub fn layer_key_from_path(target: &Path) -> Option<String> {
    let manifest = fs::read(target.join(LAYER_MANIFEST)).ok()?;
    let manifest: LayerManifest = serde_json::from_slice(&manifest).ok()?;
    Some(manifest.key.digest())
}

/// Logical bytes in `path` and physical bytes uniquely charged to it. Files
/// with multiple hard links count logically but not as private physical delta.
/// Remove superseded immutable baselines while protecting every key referenced
/// by a live layer. One newest inactive baseline is retained as a warm fallback.
/// Incomplete crash remnants are always eligible once their key is inactive.
pub fn gc_superseded_baselines(
    cache_root: &Path,
    active_keys: &HashSet<String>,
) -> Result<Vec<(PathBuf, u64)>> {
    let root = cache_root.join("baselines");
    let mut candidates = fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| !active_keys.contains(&entry.file_name().to_string_lossy().to_string()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        )
    });
    let mut removed = Vec::new();
    for entry in candidates.into_iter().skip(1) {
        let digest = entry.file_name().to_string_lossy().to_string();
        let _lock = KeyLock::acquire(cache_root, &digest)?;
        if active_keys.contains(&digest) {
            continue;
        }
        let path = entry.path();
        let bytes = layer_bytes(&path).1;
        make_tree_writable(&path)?;
        fs::remove_dir_all(&path)?;
        removed.push((path, bytes));
    }
    Ok(removed)
}

pub fn layer_bytes(path: &Path) -> (u64, u64) {
    let mut logical = 0u64;
    let mut private = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        logical = logical.saturating_add(metadata.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() == 1 {
                private = private.saturating_add(metadata.blocks().saturating_mul(512));
            }
        }
        #[cfg(not(unix))]
        {
            private = private.saturating_add(metadata.len());
        }
    }
    (logical, private)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn key(source: &str) -> TargetCacheKey {
        TargetCacheKey {
            schema: CACHE_SCHEMA,
            source_baseline: source.into(),
            cargo_lock: "lock".into(),
            rustc: "rustc 1.96 host:x86_64-unknown-linux-gnu".into(),
            target_triple: "x86_64-unknown-linux-gnu".into(),
            features: "default".into(),
            profile: "test".into(),
            flags: "incremental=0".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn same_baseline_layers_share_physical_artifacts_without_writable_target() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first = prepare_layer_with_key(temp.path(), &source, "agent-1", key("a")).unwrap();
        fs::create_dir_all(first.path.join("debug/deps")).unwrap();
        fs::write(first.path.join("debug/deps/libsame.rlib"), vec![7; 4096]).unwrap();
        assert!(promote_layer_validated(&first.path, &first.key).unwrap());

        let second = prepare_layer_with_key(temp.path(), &source, "agent-2", key("a")).unwrap();
        let base = second
            .baseline_path
            .unwrap()
            .join("debug/deps/libsame.rlib");
        let upper = second.path.join("debug/deps/libsame.rlib");
        assert_eq!(
            fs::metadata(base).unwrap().ino(),
            fs::metadata(&upper).unwrap().ino()
        );
        assert_eq!(
            layer_bytes(&second.path).1,
            fs::metadata(second.path.join(LAYER_MANIFEST))
                .unwrap()
                .blocks()
                * 512
        );
        assert!(
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&upper)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn two_concurrent_same_baseline_worktrees_share_lower_and_keep_private_writes() {
        use std::os::unix::fs::MetadataExt;
        use std::sync::{Arc, Barrier};
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let warm = prepare_layer_with_key(temp.path(), &source, "warm", key("same")).unwrap();
        fs::write(warm.path.join("artifact"), "baseline").unwrap();
        promote_layer_validated(&warm.path, &warm.key).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let handles = ["agent-a", "agent-b"].map(|agent| {
            let root = temp.path().to_path_buf();
            let source = source.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let layer = prepare_layer_with_key(&root, &source, agent, key("same")).unwrap();
                let inode = fs::metadata(layer.path.join("artifact")).unwrap().ino();
                let replacement = layer.path.join("replacement");
                fs::write(&replacement, agent).unwrap();
                fs::rename(&replacement, layer.path.join("artifact")).unwrap();
                (layer, inode)
            })
        });
        let [(left, left_inode), (right, right_inode)] =
            handles.map(|handle| handle.join().unwrap());
        assert_eq!(
            left_inode, right_inode,
            "unchanged bytes must be one physical inode"
        );
        assert_eq!(
            fs::read_to_string(left.path.join("artifact")).unwrap(),
            "agent-a"
        );
        assert_eq!(
            fs::read_to_string(right.path.join("artifact")).unwrap(),
            "agent-b"
        );
        assert_eq!(
            fs::read_to_string(left.baseline_path.unwrap().join("artifact")).unwrap(),
            "baseline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn divergent_layer_replacement_does_not_clobber_immutable_baseline() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first = prepare_layer_with_key(temp.path(), &source, "agent-1", key("a")).unwrap();
        fs::write(first.path.join("artifact"), "baseline").unwrap();
        promote_layer_validated(&first.path, &first.key).unwrap();
        let second = prepare_layer_with_key(temp.path(), &source, "agent-2", key("a")).unwrap();
        let replacement = second.path.join("replacement");
        fs::write(&replacement, "diverged").unwrap();
        fs::rename(&replacement, second.path.join("artifact")).unwrap();
        assert_eq!(
            fs::read_to_string(second.path.join("artifact")).unwrap(),
            "diverged"
        );
        assert_eq!(
            fs::read_to_string(second.baseline_path.unwrap().join("artifact")).unwrap(),
            "baseline"
        );
    }

    #[test]
    fn crash_remnant_is_repaired_and_superseded_baseline_gc_protects_active_key() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let first_key = key("active");
        let first =
            prepare_layer_with_key(temp.path(), &source, "agent-1", first_key.clone()).unwrap();
        fs::write(first.path.join("artifact"), "active").unwrap();
        // Simulate a crash between baseline directory creation and READY.
        let incomplete = baseline_dir(temp.path(), &first_key.digest());
        fs::create_dir_all(incomplete.join("target")).unwrap();
        fs::write(incomplete.join("target/partial"), "partial").unwrap();
        assert!(promote_layer_validated(&first.path, &first_key).unwrap());
        assert!(!incomplete.join("target/partial").exists());

        for (idx, source_id) in ["old-a", "old-b"].into_iter().enumerate() {
            let layer =
                prepare_layer_with_key(temp.path(), &source, &format!("old-{idx}"), key(source_id))
                    .unwrap();
            fs::write(layer.path.join("artifact"), source_id).unwrap();
            promote_layer_validated(&layer.path, &layer.key).unwrap();
        }
        let active = HashSet::from([first_key.digest()]);
        let removed = gc_superseded_baselines(temp.path(), &active).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(baseline_is_ready(&incomplete, &first_key));
    }

    #[test]
    fn key_invalidates_every_semantic_dimension() {
        let original = key("a");
        let mut variants = Vec::new();
        macro_rules! changed {
            ($field:ident, $value:expr) => {{
                let mut k = original.clone();
                k.$field = $value.into();
                variants.push(k);
            }};
        }
        changed!(source_baseline, "b");
        changed!(cargo_lock, "other-lock");
        changed!(rustc, "other-rustc");
        changed!(target_triple, "aarch64-unknown-linux-gnu");
        changed!(features, "telegram");
        changed!(profile, "test-full-debug");
        changed!(flags, "RUSTFLAGS=-Ctarget-cpu=native");
        assert!(
            variants
                .iter()
                .all(|variant| variant.digest() != original.digest())
        );
    }
}
