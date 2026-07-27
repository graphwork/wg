//! Crash-safe, content-bound candidate finalization transaction.
//!
//! This domain never classifies Pi progress, signals a process, launches a
//! session, or writes a task status. It consumes a typed quiescence receipt,
//! snapshots source through Git plumbing, and emits immutable objects/refs and
//! replayable validation/merge receipts. Canonical lifecycle changes remain
//! requests to `LifecycleKernel` by command adapters.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinalizationPhase {
    NeedsFinalization,
    RescueCheckpointed,
    CandidateCheckpointed,
    Validating,
    Evaluating,
    MergePending,
    Merged,
    RepairNeeded,
    FailedPreserved,
    OperatorHold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuiescenceProof {
    pub receipt_cid: String,
    pub process_identity_digest: String,
    pub process_group_empty: bool,
    pub nonce_pipe_eof: bool,
    pub observed_manifest_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationContext {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub process_epoch: u32,
    pub worktree_id: String,
    pub worktree_lease_epoch: u64,
    pub worktree_path: PathBuf,
    pub project_root: PathBuf,
    pub terminal_reservation_id: String,
    pub evaluation_policy: String,
    pub route_snapshot_cid: String,
    pub quiescence: QuiescenceProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub git_mode: String,
    pub kind: String,
    pub git_object_oid: String,
    pub blake3_content_digest: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentManifest {
    pub schema_version: u32,
    pub tree_oid: String,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescueDescriptor {
    pub schema_version: u32,
    pub rescue_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub process_epoch: u32,
    pub terminal_reservation_id: String,
    pub quiescence_receipt_cid: String,
    pub process_identity_digest: String,
    pub worktree_id: String,
    pub worktree_lease_epoch: u64,
    pub worker_head_oid: String,
    pub rescue_commit_oid: String,
    pub rescue_tree_oid: String,
    pub manifest_cid: String,
    pub delta_manifest_cid: String,
    pub immutable_ref: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateBinding {
    pub candidate_id: String,
    pub commit_oid: String,
    pub tree_oid: String,
    pub manifest_cid: String,
    pub delta_manifest_cid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDescriptor {
    pub schema_version: u32,
    pub candidate_id: String,
    pub candidate_version: u64,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub process_epoch: u32,
    pub terminal_reservation_id: String,
    pub quiescence_receipt_cid: String,
    pub rescue_id: String,
    pub worktree_id: String,
    pub worktree_lease_epoch: u64,
    pub base_commit_oid: String,
    pub base_tree_oid: String,
    pub worker_head_oid: String,
    pub candidate_commit_oid: String,
    pub candidate_tree_oid: String,
    pub content_manifest_cid: String,
    pub delta_manifest_cid: String,
    pub validation_policy_cid: String,
    pub evaluation_policy: String,
    pub merge_policy_cid: String,
    pub route_snapshot_cid: String,
    pub immutable_ref: String,
    pub created_at: String,
    pub binding: CandidateBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub result_id: String,
    pub request_id: String,
    pub binding: CandidateBinding,
    pub policy_cid: String,
    pub materialized_tree_oid: String,
    pub materialized_manifest_cid: String,
    pub passed: bool,
    pub validator_identity: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationRequest {
    pub request_id: String,
    pub binding: CandidateBinding,
    pub validation_result_id: String,
    pub policy_identity: String,
    pub route_snapshot_cid: String,
    pub read_only_materialization: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeReceipt {
    pub receipt_id: String,
    pub action_id: String,
    pub binding: CandidateBinding,
    pub base_commit_oid: String,
    pub expected_target_commit_oid: String,
    pub expected_target_tree_oid: String,
    pub integration_commit_oid: String,
    pub result_tree_oid: String,
    pub result_manifest_cid: String,
    pub candidate_projection_digest: String,
    pub target_ref: String,
    pub ref_cas: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    pub conflict_id: String,
    pub binding: CandidateBinding,
    pub reason_code: String,
    pub expected_target_commit_oid: String,
    pub observed_target_commit_oid: String,
    pub retained_ref: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizationTransaction {
    pub schema_version: u32,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
    pub worktree_path: PathBuf,
    pub project_root: PathBuf,
    pub phase: FinalizationPhase,
    pub terminal_reservation_id: String,
    pub quiescence: QuiescenceProof,
    pub rescue: Option<RescueDescriptor>,
    pub candidate: Option<CandidateDescriptor>,
    pub validation: Option<ValidationResult>,
    #[serde(default)]
    pub evaluation_request: Option<EvaluationRequest>,
    pub merge_receipt: Option<MergeReceipt>,
    pub merge_conflict: Option<MergeConflict>,
    pub retained_reason: Option<String>,
    pub replay_action: Option<String>,
    pub safe_next_command: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct FinalizationStore {
    root: PathBuf,
}

impl FinalizationStore {
    pub fn open(wg_dir: &Path) -> Result<Self> {
        let root = wg_dir.join("finalization");
        for child in ["objects", "transactions", "journal", "tmp"] {
            fs::create_dir_all(root.join(child))?;
        }
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn load_task(&self, task: &str) -> Result<Option<FinalizationTransaction>> {
        let path = self.transaction_path(task);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
    }
    pub fn list(&self) -> Result<Vec<FinalizationTransaction>> {
        let mut values: Vec<FinalizationTransaction> = Vec::new();
        for entry in fs::read_dir(self.root.join("transactions"))? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                values.push(serde_json::from_slice(&fs::read(path)?)?);
            }
        }
        values.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(values)
    }
    pub fn verify_candidate(&self, candidate: &CandidateDescriptor) -> Result<()> {
        verify_derived_id(&candidate.candidate_id, candidate_body(candidate)?)?;
        let manifest: ContentManifest = self.read_object(&candidate.content_manifest_cid)?;
        if manifest.tree_oid != candidate.candidate_tree_oid {
            bail!("candidate.binding_mismatch: manifest tree differs");
        }
        let actual = git_text(
            &project_from_git_path(&candidate.immutable_ref, &self.root)?,
            &[
                "rev-parse",
                &format!("{}^{{tree}}", candidate.candidate_commit_oid),
            ],
        )?;
        if actual != candidate.candidate_tree_oid {
            bail!("candidate.binding_mismatch: commit tree differs");
        }
        Ok(())
    }
    pub fn read_candidate(&self, cid: &str) -> Result<CandidateDescriptor> {
        let value: CandidateDescriptor = serde_json::from_slice(&fs::read(self.object_path(cid))?)?;
        if value.candidate_id != cid || cid_bytes(&candidate_body(&value)?) != cid {
            bail!("candidate descriptor CID mismatch");
        }
        Ok(value)
    }
    pub fn read_rescue(&self, cid: &str) -> Result<RescueDescriptor> {
        let value: RescueDescriptor = serde_json::from_slice(&fs::read(self.object_path(cid))?)?;
        if value.rescue_id != cid {
            bail!("rescue descriptor CID mismatch");
        }
        Ok(value)
    }
    pub fn materialize_commit(&self, project_root: &Path, commit: &str, to: &Path) -> Result<()> {
        if to.exists() && fs::read_dir(to)?.next().is_some() {
            bail!("materialize target is not empty");
        }
        fs::create_dir_all(to)?;
        let archive = git_output(project_root, &["archive", "--format=tar", commit])?;
        if !archive.status.success() {
            bail!("git archive failed: {}", stderr(&archive));
        }
        let mut child = Command::new("tar")
            .args(["-xf", "-", "-C"])
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(&archive.stdout)?;
        if !child.wait()?.success() {
            bail!("tar extraction failed");
        }
        Ok(())
    }
    fn transaction_path(&self, task: &str) -> PathBuf {
        self.root
            .join("transactions")
            .join(format!("{}.json", safe_name(task)))
    }
    fn save(&self, tx: &FinalizationTransaction) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(tx)?;
        atomic_write(&self.transaction_path(&tx.task_id), &bytes)?;
        let journal = self
            .root
            .join("journal")
            .join(format!("{}.jsonl", safe_name(&tx.task_id)));
        let frame = serde_json::json!({"phase":tx.phase,"at":tx.updated_at,"digest":cid_bytes(&bytes),"replay":tx.replay_action});
        append_sync(&journal, &serde_json::to_vec(&frame)?)
    }
    fn put_object<T: Serialize>(&self, value: &T) -> Result<String> {
        let bytes = canonical(value)?;
        let cid = cid_bytes(&bytes);
        self.put_named_bytes(&cid, &bytes)?;
        Ok(cid)
    }
    fn put_named_object<T: Serialize>(&self, cid: &str, value: &T) -> Result<()> {
        self.put_named_bytes(cid, &canonical(value)?)
    }
    fn put_named_bytes(&self, cid: &str, bytes: &[u8]) -> Result<()> {
        let path = self.object_path(cid);
        if !path.exists() {
            atomic_write(&path, bytes)?;
        } else if fs::read(&path)? != bytes {
            bail!("content address slot mismatch");
        }
        Ok(())
    }
    fn read_object<T: for<'de> Deserialize<'de>>(&self, cid: &str) -> Result<T> {
        let bytes =
            fs::read(self.object_path(cid)).with_context(|| format!("object {cid} missing"))?;
        if cid_bytes(&bytes) != cid {
            bail!("object CID mismatch for {cid}");
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn object_path(&self, cid: &str) -> PathBuf {
        self.root.join("objects").join(cid.replace(':', "_"))
    }
}

pub fn checkpoint_candidate(
    store: &FinalizationStore,
    ctx: &FinalizationContext,
) -> Result<FinalizationTransaction> {
    checkpoint_rescue(store, ctx, true)
}

pub fn checkpoint_rescue(
    store: &FinalizationStore,
    ctx: &FinalizationContext,
    promote: bool,
) -> Result<FinalizationTransaction> {
    validate_context(ctx)?;
    let mut next_candidate_version = 1u64;
    if let Some(existing) = store.load_task(&ctx.task_id)? {
        if existing.generation == ctx.generation
            && existing.attempt_id == ctx.attempt_id
            && existing.attempt_fence == ctx.attempt_fence
            && existing.worktree_lease_epoch == ctx.worktree_lease_epoch
            && existing.terminal_reservation_id == ctx.terminal_reservation_id
            && existing.rescue.is_some()
            && (!promote || existing.candidate.is_some())
        {
            return Ok(existing);
        }
        if !matches!(
            existing.phase,
            FinalizationPhase::Merged
                | FinalizationPhase::RepairNeeded
                | FinalizationPhase::FailedPreserved
        ) {
            bail!(
                "candidate.version_exists: current transaction belongs to different source authority"
            );
        }
        next_candidate_version = existing
            .candidate
            .as_ref()
            .map(|c| c.candidate_version + 1)
            .unwrap_or(1);
    }
    let head = git_text(&ctx.worktree_path, &["rev-parse", "HEAD"])?;
    let base = canonical_main_base(&ctx.project_root, &head)?;
    let (tree, commit) = snapshot_tree(store, ctx, &head)?;
    let manifest = manifest_for_tree(&ctx.project_root, &tree)?;
    let manifest_cid = store.put_object(&manifest)?;
    let delta = delta_manifest(&ctx.project_root, &base, &commit)?;
    let delta_manifest_cid = store.put_object(&delta)?;
    let task_hash = &blake3::hash(ctx.task_id.as_bytes()).to_hex().to_string()[..16];
    let rescue_body = serde_json::json!({
        "schema_version":SCHEMA_VERSION,"task_id":ctx.task_id,"generation":ctx.generation,"attempt_id":ctx.attempt_id,
        "attempt_fence":ctx.attempt_fence,"process_epoch":ctx.process_epoch,"terminal_reservation_id":ctx.terminal_reservation_id,
        "quiescence_receipt_cid":ctx.quiescence.receipt_cid,"process_identity_digest":ctx.quiescence.process_identity_digest,
        "worktree_id":ctx.worktree_id,"worktree_lease_epoch":ctx.worktree_lease_epoch,"worker_head_oid":head,
        "rescue_commit_oid":commit,"rescue_tree_oid":tree,"manifest_cid":manifest_cid,"delta_manifest_cid":delta_manifest_cid,
        "created_at":Utc::now().to_rfc3339()
    });
    let rescue_id = cid_value(&rescue_body)?;
    let rescue_ref = format!(
        "refs/wg/rescues/{}/{}/{}/{}",
        task_hash,
        ctx.generation,
        safe_name(&ctx.attempt_id),
        cid_suffix(&rescue_id)
    );
    publish_ref(&ctx.project_root, &rescue_ref, &commit)?;
    let rescue = RescueDescriptor {
        schema_version: SCHEMA_VERSION,
        rescue_id: rescue_id.clone(),
        task_id: ctx.task_id.clone(),
        generation: ctx.generation,
        attempt_id: ctx.attempt_id.clone(),
        attempt_fence: ctx.attempt_fence,
        process_epoch: ctx.process_epoch,
        terminal_reservation_id: ctx.terminal_reservation_id.clone(),
        quiescence_receipt_cid: ctx.quiescence.receipt_cid.clone(),
        process_identity_digest: ctx.quiescence.process_identity_digest.clone(),
        worktree_id: ctx.worktree_id.clone(),
        worktree_lease_epoch: ctx.worktree_lease_epoch,
        worker_head_oid: head.clone(),
        rescue_commit_oid: commit.clone(),
        rescue_tree_oid: tree.clone(),
        manifest_cid: manifest_cid.clone(),
        delta_manifest_cid: delta_manifest_cid.clone(),
        immutable_ref: rescue_ref,
        created_at: rescue_body["created_at"].as_str().unwrap().into(),
    };
    store.put_named_object(&rescue_id, &rescue)?;
    let mut tx = FinalizationTransaction {
        schema_version: SCHEMA_VERSION,
        task_id: ctx.task_id.clone(),
        generation: ctx.generation,
        attempt_id: ctx.attempt_id.clone(),
        attempt_fence: ctx.attempt_fence,
        worktree_lease_epoch: ctx.worktree_lease_epoch,
        worktree_path: ctx.worktree_path.clone(),
        project_root: ctx.project_root.clone(),
        phase: FinalizationPhase::RescueCheckpointed,
        terminal_reservation_id: ctx.terminal_reservation_id.clone(),
        quiescence: ctx.quiescence.clone(),
        rescue: Some(rescue.clone()),
        candidate: None,
        validation: None,
        evaluation_request: None,
        merge_receipt: None,
        merge_conflict: None,
        retained_reason: None,
        replay_action: Some(format!("candidate:{}:v1", rescue_id)),
        safe_next_command: format!("wg finalize reconcile {}", ctx.task_id),
        updated_at: Utc::now().to_rfc3339(),
    };
    store.save(&tx)?;
    if !promote {
        tx.phase = FinalizationPhase::FailedPreserved;
        tx.retained_reason = Some("source-failure-rescue".into());
        tx.replay_action = None;
        tx.safe_next_command = format!("wg finalize status {}", ctx.task_id);
        tx.updated_at = Utc::now().to_rfc3339();
        store.save(&tx)?;
        return Ok(tx);
    }
    let base_tree = git_text(
        &ctx.project_root,
        &["rev-parse", &format!("{}^{{tree}}", base)],
    )?;
    let version = next_candidate_version;
    let validation_policy_cid = cid_bytes(b"wg-deterministic-binding-v1");
    let merge_policy_cid = cid_bytes(b"wg-merge-tree-cas-v1");
    let candidate_body_value = serde_json::json!({"schema_version":SCHEMA_VERSION,"candidate_version":version,"task_id":ctx.task_id,
        "generation":ctx.generation,"attempt_id":ctx.attempt_id,"attempt_fence":ctx.attempt_fence,"process_epoch":ctx.process_epoch,
        "terminal_reservation_id":ctx.terminal_reservation_id,"quiescence_receipt_cid":ctx.quiescence.receipt_cid,"rescue_id":rescue_id,
        "worktree_id":ctx.worktree_id,"worktree_lease_epoch":ctx.worktree_lease_epoch,"base_commit_oid":base,"base_tree_oid":base_tree,
        "worker_head_oid":head,"candidate_commit_oid":commit,"candidate_tree_oid":tree,"content_manifest_cid":manifest_cid,
        "delta_manifest_cid":delta_manifest_cid,"validation_policy_cid":validation_policy_cid,"evaluation_policy":ctx.evaluation_policy,
        "merge_policy_cid":merge_policy_cid,"route_snapshot_cid":ctx.route_snapshot_cid,"created_at":Utc::now().to_rfc3339()});
    let candidate_id = cid_value(&candidate_body_value)?;
    let candidate_ref = format!(
        "refs/wg/candidates/{}/{}/{}/v{}",
        task_hash,
        ctx.generation,
        safe_name(&ctx.attempt_id),
        version
    );
    publish_ref(&ctx.project_root, &candidate_ref, &commit)?;
    let binding = CandidateBinding {
        candidate_id: candidate_id.clone(),
        commit_oid: commit.clone(),
        tree_oid: tree.clone(),
        manifest_cid: manifest_cid.clone(),
        delta_manifest_cid: delta_manifest_cid.clone(),
    };
    let candidate = CandidateDescriptor {
        schema_version: SCHEMA_VERSION,
        candidate_id: candidate_id.clone(),
        candidate_version: version,
        task_id: ctx.task_id.clone(),
        generation: ctx.generation,
        attempt_id: ctx.attempt_id.clone(),
        attempt_fence: ctx.attempt_fence,
        process_epoch: ctx.process_epoch,
        terminal_reservation_id: ctx.terminal_reservation_id.clone(),
        quiescence_receipt_cid: ctx.quiescence.receipt_cid.clone(),
        rescue_id: rescue.rescue_id.clone(),
        worktree_id: ctx.worktree_id.clone(),
        worktree_lease_epoch: ctx.worktree_lease_epoch,
        base_commit_oid: base,
        base_tree_oid: base_tree,
        worker_head_oid: head,
        candidate_commit_oid: commit,
        candidate_tree_oid: tree,
        content_manifest_cid: manifest_cid,
        delta_manifest_cid,
        validation_policy_cid: validation_policy_cid.clone(),
        evaluation_policy: ctx.evaluation_policy.clone(),
        merge_policy_cid,
        route_snapshot_cid: ctx.route_snapshot_cid.clone(),
        immutable_ref: candidate_ref,
        created_at: candidate_body_value["created_at"].as_str().unwrap().into(),
        binding: binding.clone(),
    };
    store.put_named_object(&candidate_id, &candidate)?;
    tx.phase = FinalizationPhase::Validating;
    tx.candidate = Some(candidate.clone());
    tx.replay_action = Some(format!(
        "validate:{}:{}",
        candidate_id, validation_policy_cid
    ));
    tx.updated_at = Utc::now().to_rfc3339();
    store.save(&tx)?;
    let validation = validate_candidate(store, &ctx.project_root, &candidate)?;
    let evaluation_request = (ctx.evaluation_policy != "none").then(|| EvaluationRequest {
        request_id: cid_bytes(
            format!(
                "evaluate:{}:{}:{}",
                candidate_id, ctx.evaluation_policy, ctx.route_snapshot_cid
            )
            .as_bytes(),
        ),
        binding: binding.clone(),
        validation_result_id: validation.result_id.clone(),
        policy_identity: ctx.evaluation_policy.clone(),
        route_snapshot_cid: ctx.route_snapshot_cid.clone(),
        read_only_materialization: true,
        created_at: Utc::now().to_rfc3339(),
    });
    tx.phase = FinalizationPhase::CandidateCheckpointed;
    tx.validation = Some(validation);
    tx.evaluation_request = evaluation_request;
    tx.replay_action = Some(format!(
        "merge:{}:refs/heads/main:{}",
        candidate_id, candidate.base_commit_oid
    ));
    tx.safe_next_command = format!("wg finalize reconcile {}", ctx.task_id);
    tx.updated_at = Utc::now().to_rfc3339();
    store.save(&tx)?;
    Ok(tx)
}

pub fn merge_candidate(
    store: &FinalizationStore,
    candidate: &CandidateDescriptor,
) -> Result<FinalizationTransaction> {
    let mut tx = store
        .load_task(&candidate.task_id)?
        .context("finalization transaction missing")?;
    if tx.candidate.as_ref().map(|c| &c.binding) != Some(&candidate.binding) {
        bail!("candidate.binding_mismatch: transaction does not name descriptor");
    }
    if let Some(receipt) = &tx.merge_receipt {
        if receipt.binding == candidate.binding {
            return Ok(tx);
        }
    }
    let project = tx.project_root.clone();
    verify_candidate_at(store, &project, candidate)?;
    let target_ref = "refs/heads/main";
    let action_id = format!(
        "merge:{}:{}:{}:{}",
        candidate.candidate_id, target_ref, candidate.base_commit_oid, candidate.merge_policy_cid
    );
    let result_ref = format!(
        "refs/wg/merge-results/{}",
        cid_suffix(&cid_bytes(action_id.as_bytes()))
    );
    let observed = git_text(&project, &["rev-parse", target_ref])?;
    if observed != candidate.base_commit_oid {
        // Crash replay after target CAS but before receipt publication: the
        // immutable result ref proves the exact authorized effect. Rebuild the
        // same content-bound receipt; never merge or charge again.
        if let Ok(prepared) = git_text(&project, &["rev-parse", "--verify", &result_ref])
            && prepared == observed
        {
            let result_tree =
                git_text(&project, &["rev-parse", &format!("{}^{{tree}}", prepared)])?;
            let expected_tree = git_text(
                &project,
                &[
                    "rev-parse",
                    &format!("{}^{{tree}}", candidate.base_commit_oid),
                ],
            )?;
            let result_manifest = manifest_for_tree(&project, &result_tree)?;
            let result_manifest_cid = store.put_object(&result_manifest)?;
            let projection = candidate_projection_digest(
                &project,
                &candidate.base_commit_oid,
                &candidate.candidate_commit_oid,
                &result_tree,
            )?;
            let body = serde_json::json!({"action_id":action_id,"binding":candidate.binding,"base":candidate.base_commit_oid,"expected":candidate.base_commit_oid,"integration":prepared,"tree":result_tree,"manifest":result_manifest_cid,"projection":projection,"target":target_ref});
            let receipt = MergeReceipt {
                receipt_id: cid_value(&body)?,
                action_id,
                binding: candidate.binding.clone(),
                base_commit_oid: candidate.base_commit_oid.clone(),
                expected_target_commit_oid: candidate.base_commit_oid.clone(),
                expected_target_tree_oid: expected_tree,
                integration_commit_oid: prepared,
                result_tree_oid: result_tree,
                result_manifest_cid,
                candidate_projection_digest: projection,
                target_ref: target_ref.into(),
                ref_cas: true,
                created_at: Utc::now().to_rfc3339(),
            };
            store.put_object(&receipt)?;
            tx.phase = FinalizationPhase::Merged;
            tx.merge_receipt = Some(receipt);
            tx.replay_action = None;
            tx.safe_next_command = format!("wg finalize status {}", candidate.task_id);
            tx.updated_at = Utc::now().to_rfc3339();
            store.save(&tx)?;
            return Ok(tx);
        }

        tx.phase = FinalizationPhase::RepairNeeded;
        let conflict = MergeConflict {
            conflict_id: cid_bytes(format!("{}:{}", action_id, observed).as_bytes()),
            binding: candidate.binding.clone(),
            reason_code: "merge.target_moved".into(),
            expected_target_commit_oid: candidate.base_commit_oid.clone(),
            observed_target_commit_oid: observed,
            retained_ref: candidate.immutable_ref.clone(),
            created_at: Utc::now().to_rfc3339(),
        };
        tx.merge_conflict = Some(conflict);
        tx.retained_reason = Some("merge.target_moved".into());
        tx.replay_action = None;
        tx.safe_next_command = format!("wg candidate repair {}", candidate.candidate_id);
        tx.updated_at = Utc::now().to_rfc3339();
        store.save(&tx)?;
        return Ok(tx);
    }
    tx.phase = FinalizationPhase::MergePending;
    tx.replay_action = Some(action_id.clone());
    tx.updated_at = Utc::now().to_rfc3339();
    store.save(&tx)?;
    let target_tree = git_text(&project, &["rev-parse", &format!("{}^{{tree}}", observed)])?;
    let result_tree = match merge_tree(
        &project,
        &candidate.base_commit_oid,
        &observed,
        &candidate.candidate_commit_oid,
        store,
    ) {
        Ok(tree) => tree,
        Err(error) => {
            tx.phase = FinalizationPhase::RepairNeeded;
            tx.merge_conflict = Some(MergeConflict {
                conflict_id: cid_bytes(format!("{}:{}", action_id, error).as_bytes()),
                binding: candidate.binding.clone(),
                reason_code: "merge.conflict".into(),
                expected_target_commit_oid: candidate.base_commit_oid.clone(),
                observed_target_commit_oid: observed.clone(),
                retained_ref: candidate.immutable_ref.clone(),
                created_at: Utc::now().to_rfc3339(),
            });
            tx.retained_reason = Some("merge.conflict".into());
            tx.replay_action = None;
            tx.safe_next_command = format!("wg candidate repair {}", candidate.candidate_id);
            tx.updated_at = Utc::now().to_rfc3339();
            store.save(&tx)?;
            return Ok(tx);
        }
    };
    let expected_projection = candidate_projection_digest(
        &project,
        &candidate.base_commit_oid,
        &candidate.candidate_commit_oid,
        &candidate.candidate_tree_oid,
    )?;
    let actual_projection = candidate_projection_digest(
        &project,
        &candidate.base_commit_oid,
        &candidate.candidate_commit_oid,
        &result_tree,
    )?;
    if expected_projection != actual_projection {
        bail!("merge.content_binding_mismatch: candidate-controlled entries differ");
    }
    let integration = commit_tree(
        &project,
        &result_tree,
        &[&observed, &candidate.candidate_commit_oid],
        &format!(
            "wg merge candidate {} for {}",
            candidate.candidate_id, candidate.task_id
        ),
    )?;
    publish_ref(&project, &result_ref, &integration)?;
    let lock = project.join(".wg").join("merge-authority.lock");
    let _guard = FileLock::acquire(&lock)?;
    let again = git_text(&project, &["rev-parse", target_ref])?;
    if again != observed {
        bail!("merge.target_moved: target changed before CAS");
    }
    let update = git_output(
        &project,
        &["update-ref", target_ref, &integration, &observed],
    )?;
    if !update.status.success() {
        bail!("merge.target_moved: {}", stderr(&update));
    }
    if project.join(".git").exists() {
        let reset = git_output(&project, &["reset", "--hard", &integration])?;
        if !reset.status.success() {
            bail!(
                "cleanup.failed_preserved: target advanced but checkout sync failed: {}",
                stderr(&reset)
            );
        }
    }
    let result_manifest = manifest_for_tree(&project, &result_tree)?;
    let result_manifest_cid = store.put_object(&result_manifest)?;
    let projection = candidate_projection_digest(
        &project,
        &candidate.base_commit_oid,
        &candidate.candidate_commit_oid,
        &result_tree,
    )?;
    let receipt_body = serde_json::json!({"action_id":action_id,"binding":candidate.binding,"base":candidate.base_commit_oid,"expected":observed,"integration":integration,"tree":result_tree,"manifest":result_manifest_cid,"projection":projection,"target":target_ref});
    let receipt_id = cid_value(&receipt_body)?;
    let receipt = MergeReceipt {
        receipt_id,
        action_id,
        binding: candidate.binding.clone(),
        base_commit_oid: candidate.base_commit_oid.clone(),
        expected_target_commit_oid: observed,
        expected_target_tree_oid: target_tree,
        integration_commit_oid: integration,
        result_tree_oid: result_tree,
        result_manifest_cid,
        candidate_projection_digest: projection,
        target_ref: target_ref.into(),
        ref_cas: true,
        created_at: Utc::now().to_rfc3339(),
    };
    store.put_object(&receipt)?;
    tx.phase = FinalizationPhase::Merged;
    tx.merge_receipt = Some(receipt);
    tx.merge_conflict = None;
    tx.retained_reason = None;
    tx.replay_action = None;
    tx.safe_next_command = format!("wg finalize status {}", candidate.task_id);
    tx.updated_at = Utc::now().to_rfc3339();
    store.save(&tx)?;
    Ok(tx)
}

pub fn reconcile(store: &FinalizationStore, task: &str) -> Result<Option<FinalizationTransaction>> {
    let Some(mut tx) = store.load_task(task)? else {
        return Ok(None);
    };
    if tx.phase == FinalizationPhase::Validating && tx.validation.is_none() {
        let c = tx.candidate.clone().context("candidate missing")?;
        tx.validation = Some(validate_candidate(store, &tx.project_root, &c)?);
        tx.phase = FinalizationPhase::CandidateCheckpointed;
        tx.replay_action = Some(format!(
            "merge:{}:refs/heads/main:{}",
            c.candidate_id, c.base_commit_oid
        ));
        tx.updated_at = Utc::now().to_rfc3339();
        store.save(&tx)?;
    }
    if matches!(
        tx.phase,
        FinalizationPhase::CandidateCheckpointed | FinalizationPhase::MergePending
    ) {
        let c = tx.candidate.clone().context("candidate missing")?;
        return merge_candidate(store, &c).map(Some);
    }
    Ok(Some(tx))
}

fn validate_context(ctx: &FinalizationContext) -> Result<()> {
    if ctx.task_id.is_empty() || ctx.attempt_id.is_empty() || ctx.terminal_reservation_id.is_empty()
    {
        bail!("finalize.stale_terminal_intent: incomplete source authority");
    }
    if ctx.worktree_lease_epoch != ctx.attempt_fence {
        bail!("finalize.lease_epoch_mismatch");
    }
    if !ctx.quiescence.process_group_empty
        || !ctx.quiescence.nonce_pipe_eof
        || ctx.quiescence.process_identity_digest.is_empty()
    {
        bail!("finalize.quiescence_unproven: exact PID/start/nonce/group receipt required");
    }
    if !ctx.worktree_path.join(".git").exists() {
        bail!("retention.source_identity_unknown: leased worktree missing");
    }
    Ok(())
}
fn validate_candidate(
    store: &FinalizationStore,
    project: &Path,
    c: &CandidateDescriptor,
) -> Result<ValidationResult> {
    verify_candidate_at(store, project, c)?;
    let manifest = manifest_for_tree(project, &c.candidate_tree_oid)?;
    let cid = store.put_object(&manifest)?;
    if cid != c.content_manifest_cid {
        bail!("validation.binding_mismatch");
    }
    let request_id =
        cid_bytes(format!("validate:{}:{}", c.candidate_id, c.validation_policy_cid).as_bytes());
    let body = serde_json::json!({"request":request_id,"binding":c.binding,"policy":c.validation_policy_cid,"tree":c.candidate_tree_oid,"manifest":cid,"passed":true});
    let result_id = cid_value(&body)?;
    let result = ValidationResult {
        result_id,
        request_id,
        binding: c.binding.clone(),
        policy_cid: c.validation_policy_cid.clone(),
        materialized_tree_oid: c.candidate_tree_oid.clone(),
        materialized_manifest_cid: cid,
        passed: true,
        validator_identity: "wg-deterministic-readonly-v1".into(),
        created_at: Utc::now().to_rfc3339(),
    };
    store.put_object(&result)?;
    Ok(result)
}
fn verify_candidate_at(
    store: &FinalizationStore,
    project: &Path,
    c: &CandidateDescriptor,
) -> Result<()> {
    let tree = git_text(
        project,
        &["rev-parse", &format!("{}^{{tree}}", c.candidate_commit_oid)],
    )?;
    if tree != c.candidate_tree_oid {
        bail!("candidate.binding_mismatch: commit/tree");
    }
    let m = manifest_for_tree(project, &tree)?;
    if store.put_object(&m)? != c.content_manifest_cid {
        bail!("candidate.binding_mismatch: manifest");
    }
    Ok(())
}
fn snapshot_tree(
    store: &FinalizationStore,
    ctx: &FinalizationContext,
    head: &str,
) -> Result<(String, String)> {
    let index = store
        .root
        .join("tmp")
        .join(format!("index-{}", uuid::Uuid::now_v7()));
    let mut read = git_command(&ctx.worktree_path, &["read-tree", head]);
    read.env("GIT_INDEX_FILE", &index);
    run_ok(read, "read private index")?;
    let mut add = git_command(&ctx.worktree_path, &["add", "-A", "--", "."]);
    add.env("GIT_INDEX_FILE", &index);
    run_ok(add, "snapshot source")?;
    // Exact root lifecycle controls are never candidate source. Managed
    // worktrees contain a `.wg` symlink back to the live graph; capturing it
    // would let a later target checkout replace/delete its own graph. A path
    // tracked by the repository at HEAD remains source and is not excluded.
    for control in [".wg", ".wg-cleanup-pending"] {
        let tracked_in_head = git_output(
            &ctx.worktree_path,
            &["ls-tree", "--name-only", "HEAD", "--", control],
        )
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty());
        if !tracked_in_head {
            let mut rm = git_command(
                &ctx.worktree_path,
                &["update-index", "--force-remove", "--", control],
            );
            rm.env("GIT_INDEX_FILE", &index);
            let _ = rm.output();
        }
    }
    let mut write = git_command(&ctx.worktree_path, &["write-tree"]);
    write.env("GIT_INDEX_FILE", &index);
    let out = write.output()?;
    let _ = fs::remove_file(&index);
    if !out.status.success() {
        bail!("rescue.write_failed: {}", stderr(&out));
    }
    let tree = String::from_utf8(out.stdout)?.trim().to_string();
    let commit = commit_tree(
        &ctx.project_root,
        &tree,
        &[head],
        &format!(
            "wg rescue checkpoint {} generation {} attempt {}",
            ctx.task_id, ctx.generation, ctx.attempt_id
        ),
    )?;
    Ok((tree, commit))
}
fn manifest_for_tree(project: &Path, tree: &str) -> Result<ContentManifest> {
    let out = git_output(project, &["ls-tree", "-r", "-z", "--full-tree", "-l", tree])?;
    if !out.status.success() {
        bail!("manifest tree read failed: {}", stderr(&out));
    }
    let mut entries = Vec::new();
    for rec in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = rec
            .iter()
            .position(|b| *b == b'\t')
            .context("invalid ls-tree")?;
        let meta = String::from_utf8_lossy(&rec[..tab]);
        let mut parts = meta.split_whitespace();
        let mode = parts.next().unwrap_or("").to_string();
        let kind = parts.next().unwrap_or("").to_string();
        let oid = parts.next().unwrap_or("").to_string();
        let size = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let path = String::from_utf8_lossy(&rec[tab + 1..]).to_string();
        let content = if kind == "blob" {
            let o = git_output(project, &["cat-file", "blob", &oid])?;
            if !o.status.success() {
                bail!("manifest blob missing {oid}");
            }
            o.stdout
        } else {
            oid.as_bytes().to_vec()
        };
        entries.push(ManifestEntry {
            path,
            git_mode: mode,
            kind,
            git_object_oid: oid,
            blake3_content_digest: cid_bytes(&content),
            size,
        });
    }
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Ok(ContentManifest {
        schema_version: SCHEMA_VERSION,
        tree_oid: tree.into(),
        entries,
    })
}
fn delta_manifest(project: &Path, base: &str, commit: &str) -> Result<serde_json::Value> {
    let out = git_output(
        project,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-z",
            base,
            commit,
        ],
    )?;
    if !out.status.success() {
        bail!("delta manifest failed: {}", stderr(&out));
    }
    Ok(
        serde_json::json!({"schema_version":1,"base":base,"candidate":commit,"raw_digest":cid_bytes(&out.stdout),"raw_hex":hex::encode(out.stdout)}),
    )
}
fn merge_tree(
    project: &Path,
    base: &str,
    target: &str,
    candidate: &str,
    store: &FinalizationStore,
) -> Result<String> {
    let index = store
        .root
        .join("tmp")
        .join(format!("merge-index-{}", uuid::Uuid::now_v7()));
    let mut cmd = git_command(project, &["read-tree", "-m", base, target, candidate]);
    cmd.env("GIT_INDEX_FILE", &index);
    let out = cmd.output()?;
    if !out.status.success() {
        let _ = fs::remove_file(index);
        bail!("merge.conflict: {}", stderr(&out));
    }
    let mut unmerged = git_command(project, &["ls-files", "-u"]);
    unmerged.env("GIT_INDEX_FILE", &index);
    let u = unmerged.output()?;
    if !u.stdout.is_empty() {
        let _ = fs::remove_file(index);
        bail!("merge.conflict: unmerged index stages");
    }
    let mut write = git_command(project, &["write-tree"]);
    write.env("GIT_INDEX_FILE", &index);
    let out = write.output()?;
    let _ = fs::remove_file(index);
    if !out.status.success() {
        bail!("merge.conflict: {}", stderr(&out));
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn candidate_projection_digest(
    project: &Path,
    base: &str,
    candidate: &str,
    result_tree: &str,
) -> Result<String> {
    let changed = git_output(project, &["diff", "--name-only", "-z", base, candidate])?;
    let mut data = Vec::new();
    for path in changed.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let p = String::from_utf8_lossy(path);
        let o = git_output(project, &["ls-tree", result_tree, "--", &p])?;
        data.extend_from_slice(p.as_bytes());
        data.push(0);
        data.extend_from_slice(&o.stdout);
    }
    Ok(cid_bytes(&data))
}
fn canonical_main_base(root: &Path, head: &str) -> Result<String> {
    for target in ["refs/heads/main", "refs/heads/master"] {
        if let Ok(value) = git_text(root, &["merge-base", head, target]) {
            return Ok(value);
        }
    }
    bail!("candidate.inclusion_ambiguous: no canonical target base")
}
fn commit_tree(root: &Path, tree: &str, parents: &[&str], message: &str) -> Result<String> {
    let mut cmd = git_command(root, &["commit-tree", tree]);
    for p in parents {
        cmd.arg("-p").arg(p);
    }
    cmd.arg("-m").arg(message);
    cmd.env("GIT_AUTHOR_NAME", "WG Finalizer")
        .env("GIT_AUTHOR_EMAIL", "finalizer@worksgood.local")
        .env("GIT_COMMITTER_NAME", "WG Finalizer")
        .env("GIT_COMMITTER_EMAIL", "finalizer@worksgood.local");
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("rescue.write_failed: {}", stderr(&out));
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn publish_ref(root: &Path, name: &str, oid: &str) -> Result<()> {
    if let Ok(existing) = git_text(root, &["rev-parse", "--verify", name]) {
        if existing == oid {
            return Ok(());
        }
        bail!("candidate.version_exists: immutable ref differs");
    }
    let out = git_output(root, &["update-ref", name, oid, &"0".repeat(40)])?;
    if !out.status.success() {
        bail!("rescue.ref_publish_failed: {}", stderr(&out));
    }
    Ok(())
}
fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root).args(args);
    c
}
fn git_output(root: &Path, args: &[&str]) -> Result<Output> {
    Ok(git_command(root, args).output()?)
}
fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let o = git_output(root, args)?;
    if !o.status.success() {
        bail!("git {:?}: {}", args, stderr(&o));
    }
    Ok(String::from_utf8(o.stdout)?.trim().into())
}
fn run_ok(mut c: Command, what: &str) -> Result<()> {
    let o = c.output()?;
    if !o.status.success() {
        bail!("{what}: {}", stderr(&o));
    }
    Ok(())
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).trim().into()
}
fn canonical<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(v)?)
}
fn cid_bytes(v: &[u8]) -> String {
    format!("wgcid:v1:blake3:{}", blake3::hash(v).to_hex())
}
fn cid_value<T: Serialize>(v: &T) -> Result<String> {
    Ok(cid_bytes(&canonical(v)?))
}
fn cid_suffix(cid: &str) -> &str {
    cid.rsplit(':').next().unwrap_or(cid)
}
fn safe_name(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
fn append_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(bytes)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::now_v7()));
    let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    if let Some(p) = path.parent() {
        if let Ok(d) = fs::File::open(p) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}
fn candidate_body(c: &CandidateDescriptor) -> Result<Vec<u8>> {
    let mut v = serde_json::to_value(c)?;
    v.as_object_mut().unwrap().remove("candidate_id");
    v.as_object_mut().unwrap().remove("binding");
    v.as_object_mut().unwrap().remove("immutable_ref");
    canonical(&v)
}
fn verify_derived_id(id: &str, body: Vec<u8>) -> Result<()> {
    if cid_bytes(&body) != id {
        bail!("candidate descriptor CID mismatch")
    }
    Ok(())
}
fn project_from_git_path(_r: &str, store: &Path) -> Result<PathBuf> {
    store
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .context("project root unavailable")
}
struct FileLock {
    file: fs::File,
}
impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let file = OpenOptions::new().create(true).write(true).open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                bail!("merge lock failed");
            }
        }
        Ok(Self { file })
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
