//! Fail-closed boundary between Git source state and the live `.wg` control plane.
//!
//! Git candidates may move source refs and project exact source paths, but they
//! never own `.wg`.  This module centralizes path normalization, tree/index
//! inspection, live-control identity receipts, out-of-band snapshots, exact
//! path projection, and the operator recovery used when an old repository
//! accidentally tracked control-plane bytes.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

const IDENTITY_SCHEMA: u32 = 1;
const SNAPSHOT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ControlIdentity {
    schema: u32,
    project_root: String,
    control_path: String,
    canonical_control_path: String,
    #[serde(default)]
    device: Option<u64>,
    #[serde(default)]
    inode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlSnapshotReceipt {
    pub schema: u32,
    pub receipt_id: String,
    pub reason: String,
    pub project_root: String,
    pub control_path: String,
    pub identity_digest: String,
    pub entries: Vec<SnapshotEntry>,
    pub snapshot_path: PathBuf,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRecoveryReceipt {
    pub old_commit: String,
    pub new_commit: String,
    pub target_ref: String,
    pub removed_paths: Vec<String>,
    pub snapshot_receipt: String,
}

/// Whether a repository-relative byte path names `.wg` or anything beneath it.
///
/// Git paths are slash-separated.  On UTF-8 paths we NFKC-normalize and perform
/// Unicode lowercase before comparison, so compatibility/case variants such as
/// `.WG`, `．ｗｇ`, and decomposed forms cannot evade the boundary. Backslash is
/// treated as a separator as well so a Windows spelling cannot be sealed on a
/// Unix coordinator and later become dangerous when promoted elsewhere.
pub fn is_protected_repo_path(raw: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(raw) else {
        // `.wg` itself is ASCII. Preserve non-UTF-8 source generally, but still
        // catch an ASCII component embedded in it without lossy conversion.
        return raw
            .split(|byte| matches!(byte, b'/' | b'\\'))
            .any(|part| part.eq_ignore_ascii_case(b".wg"));
    };
    text.split(['/', '\\']).any(|component| {
        let normalized: String = component.nfkc().flat_map(char::to_lowercase).collect();
        normalized == ".wg"
    })
}

fn protected_label(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    command
}

fn git_output(root: &Path, args: &[&str]) -> Result<Output> {
    git_command(root, args)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), root.display()))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = git_output(root, args)?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), stderr(&output));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn os_from_git_path(raw: &[u8]) -> Result<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(raw.to_vec()))
    }
    #[cfg(not(unix))]
    {
        Ok(OsString::from(
            std::str::from_utf8(raw).context("non-UTF-8 Git path cannot be projected here")?,
        ))
    }
}

fn tree_records(project: &Path, treeish: &str) -> Result<Vec<(String, String, String, Vec<u8>)>> {
    let output = git_output(project, &["ls-tree", "-r", "-z", "--full-tree", treeish])?;
    if !output.status.success() {
        bail!("control-plane.tree_unavailable: {}", stderr(&output));
    }
    let mut records = Vec::new();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|row| !row.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .context("control-plane.invalid_ls_tree_record")?;
        let meta = String::from_utf8_lossy(&record[..tab]);
        let mut fields = meta.split_whitespace();
        records.push((
            fields.next().unwrap_or("").to_string(),
            fields.next().unwrap_or("").to_string(),
            fields.next().unwrap_or("").to_string(),
            record[tab + 1..].to_vec(),
        ));
    }
    Ok(records)
}

pub fn protected_tree_paths(project: &Path, treeish: &str) -> Result<Vec<String>> {
    Ok(tree_records(project, treeish)?
        .into_iter()
        .filter(|(_, _, _, path)| is_protected_repo_path(path))
        .map(|(_, _, _, path)| protected_label(&path))
        .collect())
}

pub fn protected_index_paths(worktree: &Path) -> Result<Vec<String>> {
    let output = git_output(worktree, &["ls-files", "-z", "--stage"])?;
    if !output.status.success() {
        bail!("control-plane.index_unavailable: {}", stderr(&output));
    }
    let mut paths = Vec::new();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|row| !row.is_empty())
    {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let path = &record[tab + 1..];
        if is_protected_repo_path(path) {
            paths.push(protected_label(path));
        }
    }
    Ok(paths)
}

pub fn assert_tree_has_no_control_plane(project: &Path, treeish: &str) -> Result<()> {
    let paths = protected_tree_paths(project, treeish)?;
    if !paths.is_empty() {
        bail!(
            "control-plane.tracked_tree_refused: Git tree {} contains protected path(s): {}; live .wg is never candidate source. Recover without touching live data using `wg candidate recover-control-plane --yes`",
            treeish,
            paths.join(", ")
        );
    }
    Ok(())
}

pub fn assert_index_has_no_control_plane(worktree: &Path) -> Result<()> {
    let paths = protected_index_paths(worktree)?;
    if !paths.is_empty() {
        bail!(
            "control-plane.tracked_index_refused: Git index contains protected path(s): {}; unstage/remove them without touching live data using `wg candidate recover-control-plane --yes` from the project root",
            paths.join(", ")
        );
    }
    Ok(())
}

pub fn assert_repository_has_no_tracked_control(project: &Path) -> Result<()> {
    assert_tree_has_no_control_plane(project, "HEAD")?;
    assert_index_has_no_control_plane(project)
}

/// Reject protected changes before candidate/rescue sealing. The exact legacy
/// managed `.wg` symlink is tolerated only while it is absent from both HEAD
/// and the index and resolves to the expected live directory. New worktrees do
/// not create that link; workers use the inherited absolute `WG_DIR` instead.
pub fn assert_worker_boundary(
    project: &Path,
    worktree: &Path,
    base: &str,
    head: &str,
) -> Result<()> {
    assert_tree_has_no_control_plane(project, base)?;
    assert_tree_has_no_control_plane(project, head)?;
    assert_index_has_no_control_plane(worktree)?;

    let changed = git_output(
        project,
        &["diff", "--name-only", "-z", "--no-renames", base, head],
    )?;
    if !changed.status.success() {
        bail!("control-plane.diff_unavailable: {}", stderr(&changed));
    }
    let changed_protected: Vec<_> = changed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty() && is_protected_repo_path(path))
        .map(protected_label)
        .collect();
    if !changed_protected.is_empty() {
        bail!(
            "control-plane.candidate_change_refused: candidate changes protected path(s): {}",
            changed_protected.join(", ")
        );
    }

    let legacy_helper = fs::symlink_metadata(worktree.join(".wg"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && worktree.join(".wg").canonicalize().ok() == project.join(".wg").canonicalize().ok();

    // Catch untracked case/normalization variants. Exact `.wg` is inspected
    // separately because common Git excludes intentionally hide the old helper.
    let status = git_output(
        worktree,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !status.status.success() {
        bail!("control-plane.status_unavailable: {}", stderr(&status));
    }
    let mut unsafe_status = Vec::new();
    for row in status
        .stdout
        .split(|byte| *byte == 0)
        .filter(|row| !row.is_empty())
    {
        let path = if row.len() > 3 && row[2] == b' ' {
            &row[3..]
        } else {
            row
        };
        if is_protected_repo_path(path) {
            let untracked_legacy = row.starts_with(b"?? ") && path == b".wg" && legacy_helper;
            if !untracked_legacy {
                unsafe_status.push(protected_label(path));
            }
        }
    }
    if !unsafe_status.is_empty() {
        bail!(
            "control-plane.worktree_change_refused: worker worktree contains protected source change(s): {}",
            unsafe_status.join(", ")
        );
    }

    for entry in fs::read_dir(worktree)
        .with_context(|| format!("inspect worktree boundary {}", worktree.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if !is_protected_repo_path(name.to_string_lossy().as_bytes()) {
            continue;
        }
        if name == OsStr::new(".wg") {
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() && legacy_helper {
                continue;
            }
        }
        bail!(
            "control-plane.worktree_path_refused: protected path {} is not the verified legacy runtime link",
            entry.path().display()
        );
    }
    Ok(())
}

fn git_common_dir(project: &Path) -> Result<PathBuf> {
    let raw = PathBuf::from(git_text(project, &["rev-parse", "--git-common-dir"])?);
    let path = if raw.is_absolute() {
        raw
    } else {
        project.join(raw)
    };
    path.canonicalize()
        .with_context(|| format!("canonicalize Git common directory {}", path.display()))
}

fn identity_for(project: &Path) -> Result<(ControlIdentity, PathBuf, PathBuf)> {
    let canonical_project = project
        .canonicalize()
        .with_context(|| format!("canonicalize project root {}", project.display()))?;
    let control = project.join(".wg");
    let metadata = fs::symlink_metadata(&control).with_context(|| {
        format!(
            "control-plane.identity_missing: expected live directory path={}",
            control.display()
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "control-plane.identity_type_changed: live path must be a real directory, never file/symlink/gitlink path={}",
            control.display()
        );
    }
    let canonical_control = control.canonicalize().with_context(|| {
        format!(
            "control-plane.identity_unresolvable: self-referential or missing path={}",
            control.display()
        )
    })?;
    if canonical_control != canonical_project.join(".wg") {
        bail!(
            "control-plane.identity_changed: expected={} observed={}",
            canonical_project.join(".wg").display(),
            canonical_control.display()
        );
    }
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (Some(metadata.dev()), Some(metadata.ino()))
    };
    #[cfg(not(unix))]
    let (device, inode) = (None, None);
    Ok((
        ControlIdentity {
            schema: IDENTITY_SCHEMA,
            project_root: canonical_project.to_string_lossy().into_owned(),
            control_path: control.to_string_lossy().into_owned(),
            canonical_control_path: canonical_control.to_string_lossy().into_owned(),
            device,
            inode,
        },
        control,
        git_common_dir(project)?,
    ))
}

fn durable_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        directory.sync_all()?;
    }
    Ok(())
}

fn assert_or_record_identity(project: &Path) -> Result<(ControlIdentity, PathBuf, PathBuf)> {
    let (identity, control, common) = identity_for(project)?;
    if common.starts_with(&control) {
        bail!(
            "control-plane.external_receipt_unavailable: Git common dir {} is inside live control path {}",
            common.display(),
            control.display()
        );
    }
    let external = common.join("wg-control-plane");
    fs::create_dir_all(&external)?;
    let path = external.join("identity.json");
    if path.exists() {
        let expected: ControlIdentity = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("parse durable control identity {}", path.display()))?;
        if expected != identity {
            bail!(
                "control-plane.identity_changed: durable={} current={} path={}",
                serde_json::to_string(&expected)?,
                serde_json::to_string(&identity)?,
                control.display()
            );
        }
    } else {
        durable_write(&path, &serde_json::to_vec_pretty(&identity)?)?;
    }
    Ok((identity, control, external))
}

pub fn assert_live_identity(project: &Path) -> Result<()> {
    assert_or_record_identity(project).map(|_| ())
}

fn copy_snapshot_source(
    control: &Path,
    source: &Path,
    destination_root: &Path,
    object_root: &Path,
    entries: &mut Vec<SnapshotEntry>,
) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for item in WalkDir::new(source).follow_links(false) {
        let item = item?;
        let path = item.path();
        let relative = path.strip_prefix(control)?;
        let destination = destination_root.join(relative);
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_dir() {
            fs::create_dir_all(&destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(path)?;
            let bytes = target.to_string_lossy().as_bytes().to_vec();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &destination)?;
            #[cfg(not(unix))]
            fs::write(&destination, &bytes)?;
            entries.push(SnapshotEntry {
                path: relative.to_string_lossy().into_owned(),
                kind: "symlink".into(),
                size: bytes.len() as u64,
                blake3: blake3::hash(&bytes).to_hex().to_string(),
            });
        } else if metadata.file_type().is_file() {
            let bytes = fs::read(path)?;
            let digest = blake3::hash(&bytes).to_hex().to_string();
            let object = object_root.join(&digest);
            if object.exists() {
                let existing = fs::read(&object)?;
                if blake3::hash(&existing).to_hex().as_str() != digest {
                    bail!(
                        "control-plane.snapshot_object_corrupt: path={} expected={digest}",
                        object.display()
                    );
                }
            } else {
                durable_write(&object, &bytes)?;
            }
            // Receipts remain independently browsable/restorable, but equal
            // historical chat/session bytes occupy storage only once. Hard
            // links are safe because objects are immutable; cross-device or
            // platform limitations fall back to an ordinary durable copy.
            if fs::hard_link(&object, &destination).is_err() {
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&destination)?;
                output.write_all(&bytes)?;
                output.sync_all()?;
            }
            entries.push(SnapshotEntry {
                path: relative.to_string_lossy().into_owned(),
                kind: "file".into(),
                size: bytes.len() as u64,
                blake3: digest,
            });
        } else {
            bail!(
                "control-plane.snapshot_special_file_refused: {}",
                path.display()
            );
        }
    }
    Ok(())
}

/// Preserve graph/config/chat/message bytes outside `.wg` before a root ref or
/// worktree mutation. Repeated calls intentionally create independent receipts;
/// each is durable before the caller is allowed to mutate Git state.
pub fn snapshot_live_control(project: &Path, reason: &str) -> Result<ControlSnapshotReceipt> {
    let (identity, control, external) = assert_or_record_identity(project)?;
    let snapshots = external.join("snapshots");
    let objects = external.join("objects");
    fs::create_dir_all(&snapshots)?;
    fs::create_dir_all(&objects)?;
    let nonce = uuid::Uuid::now_v7().to_string();
    let temporary = snapshots.join(format!(".tmp-{nonce}"));
    fs::create_dir(&temporary)?;
    let data = temporary.join("data");
    fs::create_dir(&data)?;
    let mut entries = Vec::new();
    for relative in ["graph.jsonl", "config.toml", "chat", "messages"] {
        copy_snapshot_source(
            &control,
            &control.join(relative),
            &data,
            &objects,
            &mut entries,
        )?;
    }
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let created_at = chrono::Utc::now().to_rfc3339();
    let identity_bytes = serde_json::to_vec(&identity)?;
    let receipt_body = serde_json::json!({
        "schema": SNAPSHOT_SCHEMA,
        "reason": reason,
        "project_root": &identity.project_root,
        "control_path": &identity.control_path,
        "identity_digest": blake3::hash(&identity_bytes).to_hex().to_string(),
        "entries": &entries,
        "nonce": nonce,
        "created_at": created_at,
    });
    let receipt_id = format!(
        "wg-control-snapshot:v1:blake3:{}",
        blake3::hash(&serde_json::to_vec(&receipt_body)?).to_hex()
    );
    let final_path = snapshots.join(receipt_id.replace(':', "_"));
    let receipt = ControlSnapshotReceipt {
        schema: SNAPSHOT_SCHEMA,
        receipt_id: receipt_id.clone(),
        reason: reason.into(),
        project_root: identity.project_root,
        control_path: identity.control_path,
        identity_digest: receipt_body["identity_digest"]
            .as_str()
            .unwrap_or_default()
            .into(),
        entries,
        snapshot_path: final_path.clone(),
        created_at,
    };
    durable_write(
        &temporary.join("receipt.json"),
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    fs::rename(&temporary, &final_path)?;
    fs::File::open(&snapshots)?.sync_all()?;
    assert_live_identity(project)?;
    Ok(receipt)
}

fn changed_paths(project: &Path, old: &str, new: &str) -> Result<Vec<Vec<u8>>> {
    assert_tree_has_no_control_plane(project, old)?;
    assert_tree_has_no_control_plane(project, new)?;
    let output = git_output(
        project,
        &["diff", "--name-only", "-z", "--no-renames", old, new],
    )?;
    if !output.status.success() {
        bail!("control-plane.projection_diff_failed: {}", stderr(&output));
    }
    let mut paths = Vec::new();
    for path in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if is_protected_repo_path(path) {
            bail!(
                "control-plane.projection_refused: protected path {}",
                protected_label(path)
            );
        }
        paths.push(path.to_vec());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn tree_entry(
    project: &Path,
    treeish: &str,
    path: &[u8],
) -> Result<Option<(String, String, String)>> {
    let mut command = git_command(project, &["ls-tree", "-z", treeish, "--"]);
    command.arg(os_from_git_path(path)?);
    let output = command.output()?;
    if !output.status.success() {
        bail!(
            "control-plane.projection_tree_read_failed: {}",
            stderr(&output)
        );
    }
    let Some(record) = output
        .stdout
        .split(|byte| *byte == 0)
        .find(|row| !row.is_empty())
    else {
        return Ok(None);
    };
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .context("control-plane.invalid_projection_tree_record")?;
    if &record[tab + 1..] != path {
        bail!("control-plane.projection_path_mismatch");
    }
    let meta = String::from_utf8_lossy(&record[..tab]);
    let mut fields = meta.split_whitespace();
    Ok(Some((
        fields.next().unwrap_or("").into(),
        fields.next().unwrap_or("").into(),
        fields.next().unwrap_or("").into(),
    )))
}

fn safe_destination(project: &Path, raw: &[u8]) -> Result<PathBuf> {
    if is_protected_repo_path(raw) {
        bail!("control-plane.projection_refused: {}", protected_label(raw));
    }
    let relative = PathBuf::from(os_from_git_path(raw)?);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!(
            "control-plane.projection_invalid_path: {}",
            protected_label(raw)
        );
    }
    let destination = project.join(&relative);
    let mut cursor = project.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            cursor.push(component.as_os_str());
            match fs::symlink_metadata(&cursor) {
                Ok(metadata) if metadata.file_type().is_symlink() => bail!(
                    "control-plane.projection_parent_symlink_refused: {}",
                    cursor.display()
                ),
                Ok(metadata) if !metadata.is_dir() => {
                    fs::remove_file(&cursor)?;
                    fs::create_dir(&cursor)?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&cursor)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(destination)
}

fn remove_exact_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir(path).with_context(|| {
                format!(
                    "control-plane.projection_directory_not_empty: exact tracked path {} contains untracked data",
                    path.display()
                )
            })?;
        }
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn update_exact_index(
    project: &Path,
    path: &[u8],
    entry: Option<&(String, String, String)>,
) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("-C").arg(project).arg("update-index");
    match entry {
        Some((mode, _, oid)) => {
            command.args(["--add", "--cacheinfo", mode, oid]);
            command.arg(os_from_git_path(path)?);
        }
        None => {
            command.args(["--force-remove", "--"]);
            command.arg(os_from_git_path(path)?);
        }
    }
    let output = command.output()?;
    if !output.status.success() {
        bail!("control-plane.projection_index_failed: {}", stderr(&output));
    }
    Ok(())
}

/// Synchronize only paths named by the clean old→new diff. This deliberately
/// does not invoke checkout/reset/read-tree/checkout-index against the root
/// worktree, so `.wg` cannot be replaced even when hostile history contains it.
pub fn project_exact_paths(project: &Path, old: &str, new: &str) -> Result<Vec<String>> {
    assert_live_identity(project)?;
    let paths = changed_paths(project, old, new)?;
    let mut changes = Vec::with_capacity(paths.len());
    for raw in paths {
        changes.push((raw.clone(), tree_entry(project, new, &raw)?));
    }

    // Remove old leaves first (deepest first), then establish final tree
    // directories, then write final leaves. This handles file↔directory
    // transitions without whole-tree checkout semantics.
    let mut removals: Vec<_> = changes
        .iter()
        .filter(|(_, entry)| entry.is_none())
        .collect();
    removals
        .sort_by_key(|(path, _)| std::cmp::Reverse(path.iter().filter(|b| **b == b'/').count()));
    for (raw, _) in removals {
        assert_live_identity(project)?;
        let destination = safe_destination(project, raw)?;
        remove_exact_path(&destination)?;
        update_exact_index(project, raw, None)?;
    }

    let mut directories: Vec<_> = changes
        .iter()
        .filter(|(_, entry)| entry.as_ref().is_some_and(|(_, kind, _)| kind == "tree"))
        .collect();
    directories.sort_by_key(|(path, _)| path.iter().filter(|b| **b == b'/').count());
    for (raw, _) in directories {
        assert_live_identity(project)?;
        let destination = safe_destination(project, raw)?;
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                remove_exact_path(&destination)?;
                fs::create_dir(&destination)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&destination)?;
            }
            Err(error) => return Err(error.into()),
        }
        // Trees are implicit in a Git index; never feed a tree object to
        // update-index --cacheinfo.
    }

    let mut leaves: Vec<_> = changes
        .iter()
        .filter(|(_, entry)| entry.as_ref().is_some_and(|(_, kind, _)| kind != "tree"))
        .collect();
    leaves.sort_by_key(|(path, _)| path.iter().filter(|b| **b == b'/').count());
    for (raw, entry) in leaves {
        assert_live_identity(project)?;
        let destination = safe_destination(project, raw)?;
        match entry.as_ref() {
            Some((_mode, kind, _oid)) if kind == "commit" => {
                // A gitlink's working directory is managed by submodule
                // plumbing. Updating the exact index entry is sufficient and
                // avoids recursively deleting untracked submodule data.
            }
            Some((mode, kind, oid)) if kind == "blob" => {
                let output = git_output(project, &["cat-file", "blob", oid])?;
                if !output.status.success() {
                    bail!("control-plane.projection_blob_missing: {oid}");
                }
                if mode == "120000" {
                    remove_exact_path(&destination)?;
                    let target = os_from_git_path(&output.stdout)?;
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(PathBuf::from(target), &destination)?;
                    #[cfg(not(unix))]
                    fs::write(&destination, &output.stdout)?;
                } else {
                    if fs::symlink_metadata(&destination)
                        .is_ok_and(|metadata| metadata.file_type().is_dir())
                    {
                        fs::remove_dir(&destination).with_context(|| {
                            format!(
                                "refuse to replace non-empty directory {}",
                                destination.display()
                            )
                        })?;
                    }
                    let temporary =
                        destination.with_extension(format!("wg-project-{}", uuid::Uuid::now_v7()));
                    let mut file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&temporary)?;
                    file.write_all(&output.stdout)?;
                    file.sync_all()?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(
                            &temporary,
                            fs::Permissions::from_mode(if mode == "100755" {
                                0o755
                            } else {
                                0o644
                            }),
                        )?;
                    }
                    fs::rename(&temporary, &destination)?;
                }
            }
            Some((_mode, kind, _oid)) => {
                bail!("control-plane.projection_kind_unsupported: {kind}")
            }
            None => unreachable!("removals were handled in the first phase"),
        }
        update_exact_index(project, raw, entry.as_ref())?;
    }
    assert_index_has_no_control_plane(project)?;
    assert_live_identity(project)?;
    Ok(changes
        .iter()
        .map(|(path, _)| protected_label(path))
        .collect())
}

/// Remove protected entries from the current index and create a clean commit
/// without reading, deleting, renaming, or chmod'ing the live `.wg` directory.
pub fn recover_tracked_control_plane(
    project: &Path,
    execute: bool,
) -> Result<Option<ControlRecoveryReceipt>> {
    let old_commit = git_text(project, &["rev-parse", "HEAD"])?;
    let tree_paths = protected_tree_paths(project, &old_commit)?;
    let index_paths = protected_index_paths(project)?;
    let mut paths = tree_paths;
    paths.extend(index_paths);
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Ok(None);
    }
    if !execute {
        bail!(
            "control-plane.recovery_confirmation_required: would remove protected Git entries [{}] without touching {}; rerun with --yes",
            paths.join(", "),
            project.join(".wg").display()
        );
    }
    let staged = git_output(project, &["diff", "--cached", "--name-only", "-z"])?;
    let unrelated: Vec<_> = staged
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty() && !is_protected_repo_path(path))
        .map(protected_label)
        .collect();
    if !unrelated.is_empty() {
        bail!(
            "control-plane.recovery_staged_changes_refused: unrelated staged paths: {}",
            unrelated.join(", ")
        );
    }
    let snapshot = snapshot_live_control(project, "recover-tracked-control-plane")?;

    // Derive the recovery tree from the exact current commit in an isolated
    // index. Using the live index would silently drop unrelated source that
    // arrived in the same historical commit as the bad `.wg` entry whenever
    // the working tree/index was partially restored after an incident.
    let recovery_index = git_common_dir(project)?
        .join("wg-control-plane")
        .join(format!("recovery-index-{}", uuid::Uuid::now_v7()));
    let mut read_tree = git_command(project, &["read-tree", &old_commit]);
    read_tree.env("GIT_INDEX_FILE", &recovery_index);
    let output = read_tree.output()?;
    if !output.status.success() {
        bail!(
            "control-plane.recovery_read_tree_failed: {}",
            stderr(&output)
        );
    }
    for path in &paths {
        let mut command = git_command(project, &["update-index", "--force-remove", "--"]);
        command.env("GIT_INDEX_FILE", &recovery_index).arg(path);
        let output = command.output()?;
        if !output.status.success() {
            let _ = fs::remove_file(&recovery_index);
            bail!("control-plane.recovery_index_failed: {}", stderr(&output));
        }
    }
    let mut write_tree = git_command(project, &["write-tree"]);
    write_tree.env("GIT_INDEX_FILE", &recovery_index);
    let output = write_tree.output()?;
    let _ = fs::remove_file(&recovery_index);
    if !output.status.success() {
        bail!(
            "control-plane.recovery_write_tree_failed: {}",
            stderr(&output)
        );
    }
    let tree = String::from_utf8(output.stdout)?.trim().to_string();
    assert_tree_has_no_control_plane(project, &tree)?;
    let mut commit = git_command(
        project,
        &[
            "commit-tree",
            &tree,
            "-p",
            &old_commit,
            "-m",
            "wg recovery: untrack protected .wg control plane",
        ],
    );
    commit
        .env("GIT_AUTHOR_NAME", "WG Recovery")
        .env("GIT_AUTHOR_EMAIL", "recovery@worksgood.local")
        .env("GIT_COMMITTER_NAME", "WG Recovery")
        .env("GIT_COMMITTER_EMAIL", "recovery@worksgood.local");
    let output = commit.output()?;
    if !output.status.success() {
        bail!("control-plane.recovery_commit_failed: {}", stderr(&output));
    }
    let new_commit = String::from_utf8(output.stdout)?.trim().to_string();
    let target_ref = git_text(project, &["symbolic-ref", "-q", "HEAD"])
        .context("control-plane.recovery_detached_head_refused")?;
    let update = git_output(
        project,
        &["update-ref", &target_ref, &new_commit, &old_commit],
    )?;
    if !update.status.success() {
        bail!("control-plane.recovery_target_moved: {}", stderr(&update));
    }
    // Clean only protected entries from the live index after the ref CAS. The
    // working `.wg` directory is never named to a checkout/rm command and its
    // bytes remain untouched.
    for path in protected_index_paths(project)? {
        let mut command = git_command(project, &["update-index", "--force-remove", "--"]);
        command.arg(path);
        let output = command.output()?;
        if !output.status.success() {
            bail!(
                "control-plane.recovery_live_index_failed: {}",
                stderr(&output)
            );
        }
    }
    assert_live_identity(project)?;
    assert_repository_has_no_tracked_control(project)?;
    Ok(Some(ControlRecoveryReceipt {
        old_commit,
        new_commit,
        target_ref,
        removed_paths: paths,
        snapshot_receipt: snapshot.receipt_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.name", "Control Test"]);
        git(&root, &["config", "user.email", "control@test.invalid"]);
        fs::write(root.join("source.txt"), "old\n").unwrap();
        git(&root, &["add", "source.txt"]);
        git(&root, &["commit", "-m", "base"]);
        fs::create_dir(root.join(".wg")).unwrap();
        fs::write(root.join(".wg/graph.jsonl"), "graph\n").unwrap();
        fs::write(root.join(".wg/config.toml"), "config\n").unwrap();
        fs::create_dir_all(root.join(".wg/chat/session-a")).unwrap();
        fs::write(root.join(".wg/chat/sessions.json"), "registry\n").unwrap();
        fs::write(root.join(".wg/chat/session-a/conversation.jsonl"), "chat\n").unwrap();
        (temporary, root)
    }

    fn inject_index_entry(root: &Path, mode: &str, oid: &str, path: &str) -> String {
        git(
            root,
            &["update-index", "--add", "--cacheinfo", mode, oid, path],
        );
        git(root, &["write-tree"])
    }

    #[test]
    fn protected_path_normalizes_case_prefix_and_compatibility_forms() {
        for path in [
            ".wg",
            ".wg/graph.jsonl",
            ".WG/chat/sessions.json",
            "．ｗｇ/config.toml",
            "src/.Wg/secret",
            ".wg\\chat\\x",
        ] {
            assert!(is_protected_repo_path(path.as_bytes()), "{path}");
        }
        for path in [".wgi", ".wg-old", "src/wg/file", ".git/config"] {
            assert!(!is_protected_repo_path(path.as_bytes()), "{path}");
        }
    }

    #[test]
    fn tree_and_index_reject_file_directory_symlink_gitlink_and_variants() {
        for (mode, path) in [
            ("100644", ".wg"),
            ("100644", ".wg/nested/file"),
            ("120000", ".WG"),
            ("100644", "．ｗｇ/config.toml"),
            ("100644", "src/.Wg/secret"),
            ("160000", ".wg"),
        ] {
            let (_temporary, root) = repo();
            let oid = if mode == "160000" {
                git(&root, &["rev-parse", "HEAD"])
            } else {
                let mut child = Command::new("git")
                    .args(["hash-object", "-w", "--stdin"])
                    .current_dir(&root)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"protected\n")
                    .unwrap();
                String::from_utf8(child.wait_with_output().unwrap().stdout)
                    .unwrap()
                    .trim()
                    .to_string()
            };
            let tree = inject_index_entry(&root, mode, &oid, path);
            let tree_error = assert_tree_has_no_control_plane(&root, &tree)
                .unwrap_err()
                .to_string();
            assert!(
                tree_error.contains("tracked_tree_refused"),
                "{mode} {path}: {tree_error}"
            );
            let index_error = assert_index_has_no_control_plane(&root)
                .unwrap_err()
                .to_string();
            assert!(
                index_error.contains("tracked_index_refused"),
                "{mode} {path}: {index_error}"
            );
        }
    }

    #[test]
    fn candidate_deleting_control_from_an_already_poisoned_base_is_still_refused() {
        let (_temporary, root) = repo();
        let parent = git(&root, &["rev-parse", "HEAD"]);
        let clean_tree = git(&root, &["rev-parse", "HEAD^{tree}"]);
        let blob = {
            let mut child = Command::new("git")
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(&root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(b"tracked\n").unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        };
        let poisoned_tree = inject_index_entry(&root, "100644", &blob, ".wg/nested/file");
        let poisoned = git(
            &root,
            &[
                "commit-tree",
                &poisoned_tree,
                "-p",
                &parent,
                "-m",
                "poisoned base",
            ],
        );
        let deleting_candidate = git(
            &root,
            &[
                "commit-tree",
                &clean_tree,
                "-p",
                &poisoned,
                "-m",
                "delete protected tree",
            ],
        );
        let error = assert_worker_boundary(&root, &root, &poisoned, &deleting_candidate)
            .unwrap_err()
            .to_string();
        assert!(error.contains("tracked_tree_refused"), "{error}");
    }

    #[test]
    fn recovery_untracks_existing_control_history_without_touching_live_bytes() {
        let (_temporary, root) = repo();
        let live = [
            root.join(".wg/graph.jsonl"),
            root.join(".wg/config.toml"),
            root.join(".wg/chat/sessions.json"),
            root.join(".wg/chat/session-a/conversation.jsonl"),
        ];
        let before: Vec<_> = live.iter().map(|path| fs::read(path).unwrap()).collect();
        let blob = {
            let mut child = Command::new("git")
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(&root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(b"poison\n").unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        };
        let source_blob = {
            let mut child = Command::new("git")
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(&root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"source from poisoned commit\n")
                .unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        };
        inject_index_entry(&root, "100644", &source_blob, "source.txt");
        let poisoned_tree = inject_index_entry(&root, "100644", &blob, ".wg/poison");
        let parent = git(&root, &["rev-parse", "HEAD"]);
        let poisoned_commit = git(
            &root,
            &[
                "commit-tree",
                &poisoned_tree,
                "-p",
                &parent,
                "-m",
                "bad tracked control",
            ],
        );
        git(
            &root,
            &["update-ref", "refs/heads/main", &poisoned_commit, &parent],
        );
        assert!(assert_repository_has_no_tracked_control(&root).is_err());

        let dry = recover_tracked_control_plane(&root, false)
            .unwrap_err()
            .to_string();
        assert!(dry.contains("confirmation_required"), "{dry}");
        let receipt = recover_tracked_control_plane(&root, true).unwrap().unwrap();
        assert_eq!(receipt.old_commit, poisoned_commit);
        assert_eq!(receipt.removed_paths, vec![".wg/poison"]);
        assert_repository_has_no_tracked_control(&root).unwrap();
        assert_eq!(
            git(&root, &["show", "HEAD:source.txt"]),
            "source from poisoned commit",
            "recovery must remove only protected entries from history"
        );
        for (path, expected) in live.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), expected, "{}", path.display());
        }
        assert!(
            receipt
                .snapshot_receipt
                .starts_with("wg-control-snapshot:v1:blake3:")
        );
    }

    #[test]
    fn exact_projection_lands_ordinary_source_and_preserves_control_bytes() {
        let (_temporary, root) = repo();
        let old = git(&root, &["rev-parse", "HEAD"]);
        let control_before = fs::read(root.join(".wg/graph.jsonl")).unwrap();
        let blob = {
            let mut child = Command::new("git")
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(&root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(b"new\n").unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        };
        let tree = inject_index_entry(&root, "100644", &blob, "source.txt");
        let new = git(
            &root,
            &["commit-tree", &tree, "-p", &old, "-m", "candidate"],
        );
        let snapshot = snapshot_live_control(&root, "unit-projection").unwrap();
        git(&root, &["update-ref", "refs/heads/main", &new, &old]);
        let paths = project_exact_paths(&root, &old, &new).unwrap();
        assert_eq!(paths, vec!["source.txt"]);
        assert_eq!(
            fs::read_to_string(root.join("source.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(
            fs::read(root.join(".wg/graph.jsonl")).unwrap(),
            control_before
        );
        assert!(snapshot.snapshot_path.join("receipt.json").is_file());
        assert!(
            snapshot
                .snapshot_path
                .join("data/chat/sessions.json")
                .is_file()
        );
    }

    #[cfg(unix)]
    #[test]
    fn durable_identity_fails_closed_after_control_path_replacement() {
        let (_temporary, root) = repo();
        snapshot_live_control(&root, "identity-baseline").unwrap();
        let original = root.join(".wg-original");
        fs::rename(root.join(".wg"), &original).unwrap();
        std::os::unix::fs::symlink(".wg", root.join(".wg")).unwrap();
        let error = assert_live_identity(&root).unwrap_err().to_string();
        assert!(
            error.contains("identity_type_changed") || error.contains("identity_unresolvable"),
            "{error}"
        );
    }
}
