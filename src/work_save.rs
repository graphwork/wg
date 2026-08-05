//! Quiescent, immutable WorkSave capture.
//!
//! The adapter consumes an exact quiescence receipt and a final reconciled
//! [`WorktreeObserver`](crate::worktree_observer::WorktreeObserver) manifest. It
//! snapshots through a private Git index, publishes an immutable rescue ref,
//! fsyncs Git and evidence storage, then confirms that no late mutation raced
//! capture. It never changes the worker index, worktree, graph status, or a
//! dependency projection.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::completion_evidence::{
    AttemptSaveKey, EvidenceBinding, EvidenceHeader, WorkSaveReceipt, content_cid,
};
use crate::finalization::QuiescenceProof;
use crate::worktree_observer::{ObserverIdentity, WorkSaveManifest, WorktreeObserver};

pub const WORK_SAVE_CAPTURE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct WorkSaveCaptureRequest {
    pub source: AttemptSaveKey,
    pub worktree_root: PathBuf,
    pub project_root: PathBuf,
    pub observer_state_dir: PathBuf,
    pub completion_intent_cid: String,
    pub prepared_base_commit_oid: String,
    pub quiescence: QuiescenceProof,
    pub producer_build_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedWorkSave {
    pub receipt_cid: String,
    pub receipt: WorkSaveReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSaveTreeManifest {
    pub schema_version: u32,
    pub tree_oid: String,
    pub entries: Vec<WorkSaveTreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSaveTreeEntry {
    pub path: String,
    pub mode: String,
    pub kind: String,
    pub oid: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSaveDeltaManifest {
    pub schema_version: u32,
    pub base_commit_oid: String,
    pub saved_commit_oid: String,
    pub name_status_z_b64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CandidateBindingMaterial<'a> {
    version: u32,
    source: &'a AttemptSaveKey,
    base_commit_oid: &'a str,
    worker_head_oid: &'a str,
    saved_commit_oid: &'a str,
    saved_tree_oid: &'a str,
    full_manifest_cid: &'a str,
    delta_manifest_cid: &'a str,
    inclusion_policy_cid: &'a str,
    observer_manifest_digest: &'a str,
}

/// Content-addressed evidence store used by capture. The journal/head phase is
/// owned by the SaveTransaction adapter; this store guarantees that every
/// object it returns has reached the object file and parent directory first.
#[derive(Debug, Clone)]
pub struct WorkSaveStore {
    root: PathBuf,
}

impl WorkSaveStore {
    pub fn open(wg_dir: &Path) -> Result<Self> {
        let root = wg_dir.join("completion").join("v2");
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("tmp"))?;
        sync_dir(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn object_path(&self, cid: &str) -> PathBuf {
        self.root.join("objects").join(cid_file_name(cid))
    }

    pub fn put_object<T: Serialize>(&self, value: &T) -> Result<String> {
        let cid = content_cid(value).map_err(|error| anyhow::anyhow!(error))?;
        let path = self.object_path(&cid);
        if path.exists() {
            let existing: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
            let observed = content_cid(&existing).map_err(|error| anyhow::anyhow!(error))?;
            if observed != cid {
                bail!("work-save immutable object collision at {}", path.display());
            }
            return Ok(cid);
        }
        let bytes = serde_json::to_vec(value)?;
        let tmp = self.root.join("tmp").join(format!(
            "object-{}-{}",
            cid_file_name(&cid),
            uuid::Uuid::now_v7()
        ));
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        match fs::rename(&tmp, &path) {
            Ok(()) => {}
            Err(error) if path.exists() => {
                let _ = fs::remove_file(&tmp);
                let existing: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
                let observed = content_cid(&existing).map_err(|error| anyhow::anyhow!(error))?;
                if observed != cid {
                    return Err(error).context("immutable WorkSave object race disagreed");
                }
            }
            Err(error) => return Err(error).context("publish WorkSave object"),
        }
        sync_dir(path.parent().context("object path has no parent")?)?;
        Ok(cid)
    }
}

/// Capture one exact attempt. Repeating this call with unchanged bytes produces
/// the same tree, commit, candidate binding, immutable ref, and receipt CID.
pub fn capture_work_save(
    store: &WorkSaveStore,
    request: &WorkSaveCaptureRequest,
) -> Result<CapturedWorkSave> {
    validate_request(request)?;
    crate::control_plane::assert_live_identity(&request.project_root)?;
    crate::control_plane::assert_repository_has_no_tracked_control(&request.project_root)?;

    let canonical_root = request.worktree_root.canonicalize().with_context(|| {
        format!(
            "canonicalize WorkSave root {}",
            request.worktree_root.display()
        )
    })?;
    let mut observer = WorktreeObserver::open(&request.observer_state_dir)?;
    verify_observer_source(
        observer.projection().source.identity.clone(),
        &request.source,
    )?;
    if Path::new(&observer.projection().source.canonical_worktree_root) != canonical_root {
        bail!("work-save root does not match the observer's leased canonical root");
    }

    let now = chrono::Utc::now().timestamp();
    let observed_root_identity = content_cid(&observer.projection().source.root_identity)
        .map_err(|error| anyhow::anyhow!(error))?;
    if observed_root_identity != request.source.worktree_identity_digest {
        bail!("work-save root identity does not match the exact source tuple");
    }
    let final_manifest = observer
        .prepare_work_save_at(request.quiescence.observed_manifest_digest.as_deref(), now)?;
    let worker_head_oid = git_text(&canonical_root, &["rev-parse", "HEAD"])?;
    ensure_commit(
        &request.project_root,
        &request.prepared_base_commit_oid,
        "prepared base",
    )?;
    ensure_commit(&request.project_root, &worker_head_oid, "worker HEAD")?;

    let saved_tree_oid =
        snapshot_private_index(store, &canonical_root, &worker_head_oid, &final_manifest)?;
    crate::control_plane::assert_tree_has_no_control_plane(&request.project_root, &saved_tree_oid)?;
    let rescue_commit_oid = deterministic_commit(
        &canonical_root,
        &saved_tree_oid,
        &worker_head_oid,
        &request.source,
    )?;
    let full_manifest = tree_manifest(&request.project_root, &saved_tree_oid)?;
    let full_manifest_cid = store.put_object(&full_manifest)?;
    let delta_manifest = delta_manifest(
        &request.project_root,
        &request.prepared_base_commit_oid,
        &rescue_commit_oid,
    )?;
    let delta_manifest_cid = store.put_object(&delta_manifest)?;
    let candidate_id = content_cid(&CandidateBindingMaterial {
        version: WORK_SAVE_CAPTURE_VERSION,
        source: &request.source,
        base_commit_oid: &request.prepared_base_commit_oid,
        worker_head_oid: &worker_head_oid,
        saved_commit_oid: &rescue_commit_oid,
        saved_tree_oid: &saved_tree_oid,
        full_manifest_cid: &full_manifest_cid,
        delta_manifest_cid: &delta_manifest_cid,
        inclusion_policy_cid: &final_manifest.policy_digest,
        observer_manifest_digest: &final_manifest.manifest_digest,
    })
    .map_err(|error| anyhow::anyhow!(error))?;
    let immutable_ref = immutable_ref(&request.source, &candidate_id);
    publish_immutable_ref(&request.project_root, &immutable_ref, &rescue_commit_oid)?;
    sync_git_store(&request.project_root)?;

    // No receipt can be written if bytes moved while Git was reading them.
    observer.confirm_work_save_at(&final_manifest, chrono::Utc::now().timestamp())?;

    let worker_tree_oid = git_text(
        &request.project_root,
        &["rev-parse", &format!("{worker_head_oid}^{{tree}}")],
    )?;
    let branch = git_text(
        &canonical_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .ok();
    let root_identity = observed_root_identity;
    let late_mutation_quarantine_cid = if observer.projection().late_mutations.is_empty() {
        None
    } else {
        Some(store.put_object(&observer.projection().late_mutations)?)
    };
    let binding = EvidenceBinding {
        source: request.source.clone(),
        candidate_id,
        base_commit_oid: request.prepared_base_commit_oid.clone(),
    };
    let receipt = WorkSaveReceipt {
        header: EvidenceHeader::v2(request.producer_build_id.clone()),
        binding,
        completion_intent_cid: request.completion_intent_cid.clone(),
        quiescence_receipt_cid: request.quiescence.receipt_cid.clone(),
        worktree_root_identity: root_identity,
        branch,
        worker_head_oid,
        prepared_base_commit_oid: request.prepared_base_commit_oid.clone(),
        clean: saved_tree_oid == worker_tree_oid,
        rescue_commit_oid,
        saved_tree_oid,
        full_manifest_cid,
        delta_manifest_cid,
        immutable_ref,
        excluded_path_policy_cid: final_manifest.policy_digest,
        observer_manifest_digest: final_manifest.manifest_digest,
        observer_sequence: final_manifest.content_seq,
        late_mutation_quarantine_cid,
    };
    verify_receipt_shape(&receipt)?;
    let receipt_cid = store.put_object(&receipt)?;
    Ok(CapturedWorkSave {
        receipt_cid,
        receipt,
    })
}

fn validate_request(request: &WorkSaveCaptureRequest) -> Result<()> {
    for (name, value) in [
        (
            "completion intent CID",
            request.completion_intent_cid.as_str(),
        ),
        ("prepared base", request.prepared_base_commit_oid.as_str()),
        (
            "quiescence receipt",
            request.quiescence.receipt_cid.as_str(),
        ),
        (
            "process identity",
            request.quiescence.process_identity_digest.as_str(),
        ),
        ("producer build", request.producer_build_id.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("work-save capture requires {name}");
        }
    }
    if !request.quiescence.process_group_empty || !request.quiescence.nonce_pipe_eof {
        bail!("work-save capture requires exact process-group empty and nonce-pipe EOF proof");
    }
    if request.source.worktree_lease_epoch != request.source.attempt_fence {
        bail!("work-save source lease epoch/fence mismatch");
    }
    Ok(())
}

fn verify_observer_source(observed: ObserverIdentity, source: &AttemptSaveKey) -> Result<()> {
    if observed.task_id != source.task_id
        || observed.generation != source.generation
        || observed.attempt_id != source.attempt_id
        || observed.attempt_fence != source.attempt_fence
        || observed.worktree_lease_epoch != source.worktree_lease_epoch
        || observed.process_epoch != source.process_epoch
    {
        bail!("work-save observer is bound to a different source tuple");
    }
    Ok(())
}

fn snapshot_private_index(
    store: &WorkSaveStore,
    root: &Path,
    head: &str,
    manifest: &WorkSaveManifest,
) -> Result<String> {
    let index = store
        .root
        .join("tmp")
        .join(format!("index-{}", uuid::Uuid::now_v7()));
    let result = (|| {
        git_with_index(root, &index, &["read-tree", head])?;

        // Build the index from the observer's allowlisted manifest rather than
        // staging the ambient directory and attempting to subtract exclusions.
        // That makes ignored/generated/control bytes impossible to enter even
        // when an exclusion is represented by one pruned directory record.
        let candidate_paths = manifest
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let tracked = git_output_with_index(root, &index, &["ls-files", "-z"])?;
        if !tracked.status.success() {
            bail!("read private WorkSave index: {}", stderr(&tracked));
        }
        let mut deleted = Vec::new();
        for bytes in tracked
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8(bytes.to_vec())?;
            if !candidate_paths.contains(path.as_str())
                && !manifest.excluded_paths.iter().any(|excluded| {
                    path == *excluded
                        || path
                            .strip_prefix(excluded.trim_end_matches('/'))
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            {
                deleted.push(path);
            }
        }
        for chunk in deleted.chunks(128) {
            let mut args = vec![
                "update-index".to_string(),
                "--force-remove".into(),
                "--".into(),
            ];
            args.extend(chunk.iter().cloned());
            git_with_index_owned(root, &index, &args)?;
        }
        // Explicit deliverables may intentionally be ignored, hence `-f`.
        for chunk in manifest.entries.chunks(128) {
            let mut args = vec!["add".to_string(), "-f".into(), "--".into()];
            args.extend(chunk.iter().map(|entry| entry.path.clone()));
            git_with_index_owned(root, &index, &args)?;
        }
        git_text_with_index(root, &index, &["write-tree"])
    })();
    let _ = fs::remove_file(&index);
    let _ = fs::remove_file(index.with_extension("lock"));
    result
}

fn deterministic_commit(
    root: &Path,
    tree: &str,
    parent: &str,
    source: &AttemptSaveKey,
) -> Result<String> {
    let date = git_text(root, &["show", "-s", "--format=%cI", parent])?;
    let message = format!(
        "wg WorkSave {} generation {} attempt {} fence {}\n",
        source.task_id, source.generation, source.attempt_id, source.attempt_fence
    );
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(["commit-tree", tree, "-p", parent])
        .env("GIT_AUTHOR_NAME", "WorksGood WorkSave")
        .env("GIT_AUTHOR_EMAIL", "worksave@worksgood.invalid")
        .env("GIT_COMMITTER_NAME", "WorksGood WorkSave")
        .env("GIT_COMMITTER_EMAIL", "worksave@worksgood.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    child
        .stdin
        .as_mut()
        .context("commit-tree stdin")?
        .write_all(message.as_bytes())?;
    let output = child.wait_with_output()?;
    output_text(output, "create deterministic WorkSave commit")
}

fn tree_manifest(project: &Path, tree: &str) -> Result<WorkSaveTreeManifest> {
    let output = git_output(project, &["ls-tree", "-r", "-z", "--full-tree", "-l", tree])?;
    if !output.status.success() {
        bail!("read WorkSave tree manifest: {}", stderr(&output));
    }
    let mut entries = Vec::new();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|r| !r.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .context("invalid ls-tree record")?;
        let metadata = String::from_utf8_lossy(&record[..tab]);
        let mut fields = metadata.split_whitespace();
        entries.push(WorkSaveTreeEntry {
            mode: fields.next().unwrap_or_default().to_owned(),
            kind: fields.next().unwrap_or_default().to_owned(),
            oid: fields.next().unwrap_or_default().to_owned(),
            size: fields
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            path: String::from_utf8(record[tab + 1..].to_vec())?,
        });
    }
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Ok(WorkSaveTreeManifest {
        schema_version: WORK_SAVE_CAPTURE_VERSION,
        tree_oid: tree.to_owned(),
        entries,
    })
}

fn delta_manifest(project: &Path, base: &str, saved: &str) -> Result<WorkSaveDeltaManifest> {
    let output = git_output(
        project,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-z",
            base,
            saved,
        ],
    )?;
    if !output.status.success() {
        bail!("read WorkSave delta manifest: {}", stderr(&output));
    }
    Ok(WorkSaveDeltaManifest {
        schema_version: WORK_SAVE_CAPTURE_VERSION,
        base_commit_oid: base.to_owned(),
        saved_commit_oid: saved.to_owned(),
        name_status_z_b64: base64_encode(&output.stdout),
    })
}

fn immutable_ref(source: &AttemptSaveKey, candidate_id: &str) -> String {
    let task_hash = blake3::hash(source.task_id.as_bytes()).to_hex().to_string();
    format!(
        "refs/wg/work-saves/{}/{}/{}/{}",
        &task_hash[..16],
        source.generation,
        safe_ref_component(&source.attempt_id),
        candidate_id.rsplit(':').next().unwrap_or(candidate_id)
    )
}

fn publish_immutable_ref(project: &Path, reference: &str, oid: &str) -> Result<()> {
    let zero = "0".repeat(40);
    let output = git_output(project, &["update-ref", reference, oid, &zero])?;
    if output.status.success() {
        return Ok(());
    }
    let existing = git_text(project, &["rev-parse", "--verify", reference]).ok();
    if existing.as_deref() == Some(oid) {
        return Ok(());
    }
    bail!(
        "immutable WorkSave ref conflict at {reference}: {}",
        stderr(&output)
    )
}

fn verify_receipt_shape(receipt: &WorkSaveReceipt) -> Result<()> {
    for (name, value) in [
        ("candidate", receipt.binding.candidate_id.as_str()),
        ("base", receipt.binding.base_commit_oid.as_str()),
        ("intent", receipt.completion_intent_cid.as_str()),
        ("quiescence", receipt.quiescence_receipt_cid.as_str()),
        ("root identity", receipt.worktree_root_identity.as_str()),
        ("worker HEAD", receipt.worker_head_oid.as_str()),
        ("rescue commit", receipt.rescue_commit_oid.as_str()),
        ("saved tree", receipt.saved_tree_oid.as_str()),
        ("full manifest", receipt.full_manifest_cid.as_str()),
        ("delta manifest", receipt.delta_manifest_cid.as_str()),
        ("immutable ref", receipt.immutable_ref.as_str()),
        (
            "observer manifest",
            receipt.observer_manifest_digest.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            bail!("WorkSave receipt missing {name}");
        }
    }
    Ok(())
}

fn ensure_commit(root: &Path, oid: &str, label: &str) -> Result<()> {
    let output = git_output(root, &["cat-file", "-e", &format!("{oid}^{{commit}}")])?;
    if !output.status.success() {
        bail!("WorkSave {label} is not a commit: {}", stderr(&output));
    }
    Ok(())
}

fn sync_git_store(project: &Path) -> Result<()> {
    let raw = git_text(project, &["rev-parse", "--git-common-dir"])?;
    let common = if Path::new(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        project.join(raw)
    }
    .canonicalize()?;
    let objects = common.join("objects");
    if objects.is_dir() {
        for entry in fs::read_dir(&objects)? {
            let path = entry?.path();
            if path.is_dir() {
                sync_dir(&path)?;
            }
        }
        sync_dir(&objects)?;
    }
    let refs = common.join("refs");
    if refs.is_dir() {
        sync_dir(&refs)?;
    }
    sync_dir(&common)
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn git_with_index(root: &Path, index: &Path, args: &[&str]) -> Result<()> {
    let owned = args
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    git_with_index_owned(root, index, &owned)
}

fn git_with_index_owned(root: &Path, index: &Path, args: &[String]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), stderr(&output));
    }
    Ok(())
}

fn git_output_with_index(root: &Path, index: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new("git")
        .current_dir(root)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()?)
}

fn git_text_with_index(root: &Path, index: &Path, args: &[&str]) -> Result<String> {
    output_text(
        git_output_with_index(root, index, args)?,
        &format!("git {}", args.join(" ")),
    )
}

fn git_output(root: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new("git").current_dir(root).args(args).output()?)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    output_text(git_output(root, args)?, &format!("git {}", args.join(" ")))
}

fn output_text(output: Output, operation: &str) -> Result<String> {
    if !output.status.success() {
        bail!("{operation}: {}", stderr(&output));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn cid_file_name(cid: &str) -> String {
    cid.replace(':', "-")
}

fn safe_ref_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (a << 16) | (b << 8) | c;
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree_observer::{CandidatePathPolicy, ObserverConfig};
    use tempfile::TempDir;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn fixture() -> (TempDir, PathBuf, PathBuf, AttemptSaveKey) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        fs::write(root.join("tracked.txt"), "base\n").unwrap();
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-qm", "base"]);
        let wg = root.join(".wg");
        fs::create_dir(&wg).unwrap();
        let mut source = AttemptSaveKey {
            graph_id: "graph".into(),
            task_id: "task".into(),
            generation: 1,
            attempt_id: "attempt-1".into(),
            attempt_fence: 2,
            worktree_lease_epoch: 2,
            process_epoch: 3,
            wrapper_epoch: 1,
            route_snapshot_cid: "route".into(),
            session_proof_digest: "session".into(),
            worktree_identity_digest: "root".into(),
        };
        let observer_dir = wg.join("observer");
        let observer = WorktreeObserver::attach_at(
            &root,
            &observer_dir,
            ObserverIdentity {
                task_id: source.task_id.clone(),
                generation: source.generation,
                attempt_id: source.attempt_id.clone(),
                attempt_fence: source.attempt_fence,
                worktree_id: "agent".into(),
                worktree_lease_epoch: source.worktree_lease_epoch,
                process_epoch: source.process_epoch,
                observer_epoch: 1,
            },
            CandidatePathPolicy::new(Vec::new(), vec!["target/**".into()]).unwrap(),
            ObserverConfig::default(),
            1,
        )
        .unwrap();
        source.worktree_identity_digest =
            content_cid(&observer.projection().source.root_identity).unwrap();
        (temp, root, observer_dir, source)
    }

    #[test]
    fn work_save_captures_tracked_deleted_and_untracked_without_mutating_index() {
        let (_temp, root, observer_dir, source) = fixture();
        let head = git(&root, &["rev-parse", "HEAD"]);
        fs::remove_file(root.join("tracked.txt")).unwrap();
        fs::write(root.join("new.txt"), "new\n").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/generated.bin"), "excluded").unwrap();
        let index_path = root.join(git(&root, &["rev-parse", "--git-path", "index"]));
        let before_index = fs::read(&index_path).unwrap();
        let store = WorkSaveStore::open(&root.join(".wg")).unwrap();
        let captured = capture_work_save(
            &store,
            &WorkSaveCaptureRequest {
                source,
                worktree_root: root.clone(),
                project_root: root.clone(),
                observer_state_dir: observer_dir,
                completion_intent_cid: "intent".into(),
                prepared_base_commit_oid: head,
                producer_build_id: "test-build".into(),
                quiescence: QuiescenceProof {
                    receipt_cid: "quiescence".into(),
                    process_identity_digest: "pid-start-boot-nonce".into(),
                    process_group_empty: true,
                    nonce_pipe_eof: true,
                    observed_manifest_digest: None,
                },
            },
        )
        .unwrap();
        let names = git(
            &root,
            &[
                "ls-tree",
                "-r",
                "--name-only",
                &captured.receipt.rescue_commit_oid,
            ],
        );
        assert_eq!(names, "new.txt");
        assert!(!captured.receipt.clean);
        let after_index = fs::read(&index_path).unwrap();
        assert_eq!(before_index, after_index, "worker index must be untouched");
        assert_eq!(
            git(&root, &["rev-parse", &captured.receipt.immutable_ref]),
            captured.receipt.rescue_commit_oid
        );
    }

    #[test]
    fn work_save_clean_tree_is_real_and_idempotent() {
        let (_temp, root, observer_dir, source) = fixture();
        let head = git(&root, &["rev-parse", "HEAD"]);
        let store = WorkSaveStore::open(&root.join(".wg")).unwrap();
        let request = WorkSaveCaptureRequest {
            source,
            worktree_root: root.clone(),
            project_root: root,
            observer_state_dir: observer_dir,
            completion_intent_cid: "intent".into(),
            prepared_base_commit_oid: head,
            producer_build_id: "test-build".into(),
            quiescence: QuiescenceProof {
                receipt_cid: "q".into(),
                process_identity_digest: "p".into(),
                process_group_empty: true,
                nonce_pipe_eof: true,
                observed_manifest_digest: None,
            },
        };
        let first = capture_work_save(&store, &request).unwrap();
        let second = capture_work_save(&store, &request).unwrap();
        assert!(first.receipt.clean);
        assert_eq!(first, second);
    }

    #[test]
    fn work_save_refuses_unproven_quiescence() {
        let (_temp, root, observer_dir, source) = fixture();
        let head = git(&root, &["rev-parse", "HEAD"]);
        let store = WorkSaveStore::open(&root.join(".wg-state")).unwrap();
        let error = capture_work_save(
            &store,
            &WorkSaveCaptureRequest {
                source,
                worktree_root: root.clone(),
                project_root: root,
                observer_state_dir: observer_dir,
                completion_intent_cid: "intent".into(),
                prepared_base_commit_oid: head,
                producer_build_id: "test".into(),
                quiescence: QuiescenceProof {
                    receipt_cid: "q".into(),
                    process_identity_digest: "p".into(),
                    process_group_empty: false,
                    nonce_pipe_eof: true,
                    observed_manifest_digest: None,
                },
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("process-group empty"));
    }

    #[test]
    fn work_save_refuses_a_different_root_identity() {
        let (_temp, root, observer_dir, mut source) = fixture();
        source.worktree_identity_digest = "wrong-root".into();
        let head = git(&root, &["rev-parse", "HEAD"]);
        let store = WorkSaveStore::open(&root.join(".wg")).unwrap();
        let error = capture_work_save(
            &store,
            &WorkSaveCaptureRequest {
                source,
                worktree_root: root.clone(),
                project_root: root,
                observer_state_dir: observer_dir,
                completion_intent_cid: "intent".into(),
                prepared_base_commit_oid: head,
                producer_build_id: "test".into(),
                quiescence: QuiescenceProof {
                    receipt_cid: "q".into(),
                    process_identity_digest: "p".into(),
                    process_group_empty: true,
                    nonce_pipe_eof: true,
                    observed_manifest_digest: None,
                },
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("root identity"));
    }

    #[test]
    fn work_save_confirmation_quarantines_a_late_mutation() {
        let (_temp, root, observer_dir, _source) = fixture();
        let mut observer = WorktreeObserver::open(&observer_dir).unwrap();
        let manifest = observer.prepare_work_save_at(None, 10).unwrap();
        fs::write(root.join("late.txt"), "late writer\n").unwrap();
        let error = observer
            .confirm_work_save_at(&manifest, 11)
            .unwrap_err()
            .to_string();
        assert!(error.contains("late worktree mutation"));
        assert!(observer.projection().quarantine_required);
        assert!(!observer.projection().late_mutations.is_empty());
    }
}
