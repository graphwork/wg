//! Attempt-scoped worker control-plane capabilities.
//!
//! A task worker never receives a graph directory.  It receives an opaque
//! bearer token and the daemon endpoint; the daemon resolves that token to the
//! exact lifecycle tuple below before performing a typed operation.  The
//! registry intentionally stores only a SHA-256 digest of the bearer token.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Seek, Write};

use crate::completion_evidence::{AttemptSaveKey, content_cid};
use crate::save_transaction::{
    SaveFact, SavePhase, SaveTransactionKernel, SaveTransactionState, SaveTransitionRequest,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const WORKER_CONTROL_PROTOCOL: &str = "worksgood-worker-control-v2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemIsolationMode {
    Enforced,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilesystemIsolationStatus {
    pub mode: FilesystemIsolationMode,
    pub enforced: bool,
    pub reason: String,
}

/// Report only guarantees this build actually installs. Worktree separation
/// and environment-variable omission are explicitly not filesystem isolation.
/// The sandbox adapter will switch this to `Enforced` only after a child-side
/// mount visibility probe succeeds; until then compatibility mode is loud.
pub fn filesystem_isolation_status() -> FilesystemIsolationStatus {
    FilesystemIsolationStatus {
        mode: FilesystemIsolationMode::Degraded,
        enforced: false,
        reason: "no verified mount/container sandbox adapter installed; capability broker is enforced but same-uid path guessing remains possible".to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptCapabilityBinding {
    pub protocol: String,
    pub graph_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub fence: u64,
    pub lease_epoch: u64,
    pub agent_id: String,
    pub token_sha256: String,
    pub issued_at: String,
    /// Exact immutable source tuple and root selected before launch. Brokered
    /// completion must use these bytes, never reconstruct a path from the
    /// mutable agent registry or daemon-thread environment.
    pub save_source: AttemptSaveKey,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    pub allowed_operations: Vec<WorkerOperationKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationKind {
    Show,
    Context,
    MessageRead,
    MessagePoll,
    MessageSend,
    Log,
    ArtifactList,
    ArtifactAdd,
    ArtifactRemove,
    DependencyArtifactRead,
    Checkpoint,
    Wait,
    DoneHandoff,
    FailHandoff,
    FinishHandoff,
    PiWatchdog,
    Telemetry,
    Heartbeat,
}

impl WorkerOperationKind {
    pub fn default_attempt_operations() -> Vec<Self> {
        vec![
            Self::Show,
            Self::Context,
            Self::MessageRead,
            Self::MessagePoll,
            Self::MessageSend,
            Self::Log,
            Self::ArtifactList,
            Self::ArtifactAdd,
            Self::ArtifactRemove,
            Self::DependencyArtifactRead,
            Self::Checkpoint,
            Self::Wait,
            Self::DoneHandoff,
            Self::FailHandoff,
            Self::FinishHandoff,
            Self::PiWatchdog,
            Self::Telemetry,
            Self::Heartbeat,
        ]
    }

    fn after_terminal_reservation_operations() -> Vec<Self> {
        vec![
            Self::Show,
            Self::Context,
            Self::MessageRead,
            Self::MessagePoll,
            Self::Log,
            Self::ArtifactList,
            Self::DependencyArtifactRead,
            // DoneHandoff is a one-way semantic reservation, but the exact
            // fenced owner must retain authority to drive the already-durable
            // transaction through settle/cleanup. Revoking FinishHandoff here
            // stranded every Prepared transaction in Quiescing and caused the
            // dispatcher to respawn the same completed worker indefinitely.
            Self::FinishHandoff,
            Self::PiWatchdog,
            Self::Telemetry,
            Self::Heartbeat,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum WorkerOperation {
    Show {
        json: bool,
    },
    Context {
        json: bool,
    },
    MessageRead {
        json: bool,
    },
    MessagePoll {
        json: bool,
    },
    MessageSend {
        body: String,
        priority: String,
    },
    Log {
        message: String,
    },
    ArtifactList {
        json: bool,
    },
    ArtifactAdd {
        path: String,
    },
    ArtifactRemove {
        path: String,
    },
    DependencyArtifactRead {
        dependency: String,
        path: String,
    },
    Checkpoint {
        summary: String,
        #[serde(default)]
        files: Vec<String>,
    },
    Wait {
        until: String,
        checkpoint: Option<String>,
    },
    DoneHandoff {
        converged: bool,
        full_smoke: bool,
    },
    FailHandoff {
        reason: String,
        class: Option<String>,
    },
    FinishHandoff {
        action: FinishHandoffAction,
    },
    PiWatchdogBootstrap {
        agent_dir: String,
        pid: u32,
        wrapper_pid: Option<u32>,
    },
    PiWatchdogProcessExit {
        exit_code: i32,
        pid: Option<u32>,
    },
    RecordTelemetry {
        raw_stream: Option<String>,
        exit_code: i32,
        executor: Option<String>,
        route: Option<String>,
    },
    Heartbeat,
}

impl WorkerOperation {
    pub fn kind(&self) -> WorkerOperationKind {
        match self {
            Self::Show { .. } => WorkerOperationKind::Show,
            Self::Context { .. } => WorkerOperationKind::Context,
            Self::MessageRead { .. } => WorkerOperationKind::MessageRead,
            Self::MessagePoll { .. } => WorkerOperationKind::MessagePoll,
            Self::MessageSend { .. } => WorkerOperationKind::MessageSend,
            Self::Log { .. } => WorkerOperationKind::Log,
            Self::ArtifactList { .. } => WorkerOperationKind::ArtifactList,
            Self::ArtifactAdd { .. } => WorkerOperationKind::ArtifactAdd,
            Self::ArtifactRemove { .. } => WorkerOperationKind::ArtifactRemove,
            Self::DependencyArtifactRead { .. } => WorkerOperationKind::DependencyArtifactRead,
            Self::Checkpoint { .. } => WorkerOperationKind::Checkpoint,
            Self::Wait { .. } => WorkerOperationKind::Wait,
            Self::DoneHandoff { .. } => WorkerOperationKind::DoneHandoff,
            Self::FailHandoff { .. } => WorkerOperationKind::FailHandoff,
            Self::FinishHandoff { .. } => WorkerOperationKind::FinishHandoff,
            Self::PiWatchdogBootstrap { .. } | Self::PiWatchdogProcessExit { .. } => {
                WorkerOperationKind::PiWatchdog
            }
            Self::RecordTelemetry { .. } => WorkerOperationKind::Telemetry,
            Self::Heartbeat => WorkerOperationKind::Heartbeat,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishHandoffAction {
    Settle,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRequestEnvelope {
    pub protocol: String,
    pub request_id: String,
    pub capability: String,
    pub operation: WorkerOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAuditEvent {
    pub timestamp: String,
    pub request_id: String,
    pub token_hint: String,
    pub operation: WorkerOperationKind,
    pub outcome: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredWorkerResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestJournalState {
    Pending,
    Completed(StoredWorkerResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestJournalEntry {
    pub token_sha256: String,
    pub operation: WorkerOperationKind,
    /// Content digest of the complete operation, not merely its enum tag.
    /// Reusing an intent key with changed flags/body is a conflict.
    pub operation_cid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_transaction_id: Option<String>,
    pub started_at: String,
    pub state: RequestJournalState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CapabilityRegistry {
    #[serde(default)]
    capabilities: BTreeMap<String, AttemptCapabilityBinding>,
    #[serde(default)]
    requests: BTreeMap<String, RequestJournalEntry>,
}

#[derive(Debug, Clone)]
pub enum BeginRequest {
    Fresh,
    Completed(StoredWorkerResponse),
    Pending,
}

fn service_dir(dir: &Path) -> PathBuf {
    dir.join("service")
}

fn registry_path(dir: &Path) -> PathBuf {
    service_dir(dir).join("worker-capabilities.json")
}

fn graph_identity_path(dir: &Path) -> PathBuf {
    service_dir(dir).join("graph-identity")
}

pub fn audit_path(dir: &Path) -> PathBuf {
    service_dir(dir).join("worker-capability-audit.jsonl")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("worker control path has no parent")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("worker-control"),
        uuid::Uuid::now_v7()
    ));
    {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Ok(parent_file) = fs::File::open(parent) {
        parent_file.sync_all()?;
    }
    Ok(())
}

fn load_registry(dir: &Path) -> Result<CapabilityRegistry> {
    let path = registry_path(dir);
    if !path.exists() {
        return Ok(CapabilityRegistry::default());
    }
    serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parse worker capability registry {}", path.display()))
}

fn save_registry(dir: &Path, registry: &CapabilityRegistry) -> Result<()> {
    atomic_write(&registry_path(dir), &serde_json::to_vec_pretty(registry)?)
}

pub fn load_or_create_graph_identity(dir: &Path) -> Result<String> {
    let path = graph_identity_path(dir);
    if path.exists() {
        let value = fs::read_to_string(&path)?;
        let value = value.trim();
        if value.starts_with("wggraph:v1:") && value.len() > 24 {
            return Ok(value.to_string());
        }
        bail!("worker_control.graph_identity_invalid: {}", path.display());
    }
    let value = format!("wggraph:v1:{}", uuid::Uuid::now_v7());
    atomic_write(&path, format!("{value}\n").as_bytes())?;
    Ok(value)
}

pub fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn digest_text(value: impl AsRef<[u8]>) -> String {
    format!("b3:{}", blake3::hash(value.as_ref()).to_hex())
}

fn worktree_identity(path: &Path) -> Result<(String, String)> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize bound worktree {}", path.display()))?;
    let git_marker = fs::read(canonical.join(".git")).unwrap_or_default();
    let metadata = fs::metadata(&canonical)?;
    #[cfg(unix)]
    let identity_material = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}\0{}\0{}\0{}",
            canonical.display(),
            metadata.dev(),
            metadata.ino(),
            String::from_utf8_lossy(&git_marker)
        )
    };
    #[cfg(not(unix))]
    let identity_material = format!(
        "{}\0{}",
        canonical.display(),
        String::from_utf8_lossy(&git_marker)
    );
    Ok((
        canonical.to_string_lossy().into_owned(),
        digest_text(identity_material),
    ))
}

fn bound_worktree(dir: &Path, agent_id: &str) -> Result<(String, String)> {
    let project = dir
        .parent()
        .context("graph directory has no project root")?;
    let candidate = project.join(".wg-worktrees").join(agent_id);
    match worktree_identity(&candidate) {
        Ok(identity) => Ok(identity),
        Err(_) => {
            // Capability minting also serves non-worktree unit/inline tasks.
            // Bind the selected path now, but terminal handoff will refuse it
            // until that exact root exists and its filesystem identity verifies.
            let path = candidate.to_string_lossy().into_owned();
            Ok((path.clone(), digest_text(format!("missing:{path}"))))
        }
    }
}

pub fn verify_bound_worktree(binding: &AttemptCapabilityBinding) -> Result<PathBuf> {
    let (observed_path, observed_digest) = worktree_identity(Path::new(&binding.worktree_path))?;
    if observed_path != binding.worktree_path
        || observed_digest != binding.save_source.worktree_identity_digest
    {
        bail!("worker_control.worktree_identity_mismatch");
    }
    Ok(PathBuf::from(observed_path))
}

/// Mint an opaque bearer capability. Only its digest is made durable. Legacy
/// callers bind the conventional agent-named path; spawn uses
/// [`mint_attempt_capability_for_worktree`] so retry-in-place can bind the
/// retained checkout whose directory name belongs to the prior dead owner.
pub fn mint_attempt_capability(
    dir: &Path,
    task_id: &str,
    generation: u64,
    attempt_id: &str,
    fence: u64,
    lease_epoch: u64,
    agent_id: &str,
) -> Result<(String, AttemptCapabilityBinding)> {
    mint_attempt_capability_for_worktree(
        dir,
        task_id,
        generation,
        attempt_id,
        fence,
        lease_epoch,
        agent_id,
        None,
    )
}

pub fn mint_attempt_capability_for_worktree(
    dir: &Path,
    task_id: &str,
    generation: u64,
    attempt_id: &str,
    fence: u64,
    lease_epoch: u64,
    agent_id: &str,
    explicit_worktree: Option<&Path>,
) -> Result<(String, AttemptCapabilityBinding)> {
    let mut random = [0_u8; 32];
    getrandom::getrandom(&mut random).context("generate worker capability")?;
    let token = format!("wgcap_v2_{}", hex::encode(random));
    let digest = token_digest(&token);
    let graph_id = load_or_create_graph_identity(dir)?;
    let graph = crate::parser::load_graph(dir.join("graph.jsonl")).ok();
    let task = graph.as_ref().and_then(|graph| graph.get_task(task_id));
    let process_epoch = task.map_or(0, |task| task.lifecycle.pi_process_epoch);
    let route_snapshot_cid = task
        .and_then(|task| task.lifecycle.pi_continuation.as_ref())
        .map(|proof| proof.route_snapshot_digest.clone())
        .unwrap_or_else(|| digest_text(format!("route:{task_id}:{generation}:{attempt_id}")));
    let session_proof_digest = task
        .and_then(|task| task.lifecycle.pi_continuation.as_ref())
        .map(|proof| proof.session_proof_digest.clone())
        .unwrap_or_else(|| digest_text(format!("session:{task_id}:{generation}:{attempt_id}")));
    let (worktree_path, worktree_identity_digest) = match explicit_worktree {
        Some(path) => worktree_identity(path).with_context(|| {
            format!(
                "bind explicit retained worktree {} for {agent_id}",
                path.display()
            )
        })?,
        None => bound_worktree(dir, agent_id)?,
    };
    let save_source = AttemptSaveKey {
        graph_id: graph_id.clone(),
        task_id: task_id.to_string(),
        generation,
        attempt_id: attempt_id.to_string(),
        attempt_fence: fence,
        worktree_lease_epoch: lease_epoch,
        process_epoch,
        wrapper_epoch: process_epoch.max(1),
        route_snapshot_cid,
        session_proof_digest,
        worktree_identity_digest,
    };
    let binding = AttemptCapabilityBinding {
        protocol: WORKER_CONTROL_PROTOCOL.to_string(),
        graph_id,
        task_id: task_id.to_string(),
        generation,
        attempt_id: attempt_id.to_string(),
        fence,
        lease_epoch,
        agent_id: agent_id.to_string(),
        token_sha256: digest.clone(),
        issued_at: Utc::now().to_rfc3339(),
        save_source,
        worktree_path,
        revoked_at: None,
        allowed_operations: WorkerOperationKind::default_attempt_operations(),
    };
    let mut registry = load_registry(dir)?;
    registry.capabilities.insert(digest, binding.clone());
    save_registry(dir, &registry)?;
    Ok((token, binding))
}

pub fn revoke_capability(dir: &Path, token_sha256: &str) -> Result<()> {
    let mut registry = load_registry(dir)?;
    if let Some(binding) = registry.capabilities.get_mut(token_sha256)
        && binding.revoked_at.is_none()
    {
        binding.revoked_at = Some(Utc::now().to_rfc3339());
        save_registry(dir, &registry)?;
    }
    Ok(())
}

pub fn lookup_capability(dir: &Path, token: &str) -> Result<AttemptCapabilityBinding> {
    let digest = token_digest(token);
    let mut capabilities = load_registry(dir)?;
    let mut binding = capabilities
        .capabilities
        .get(&digest)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("worker_control.capability_unknown"))?;

    // Compatibility convergence for capabilities minted before retry-in-place
    // supplied the retained worktree path explicitly. Repair is narrowly
    // fenced: the originally bound path must never have existed, the current
    // graph tuple must still name this exact owner, the locked spawn registry
    // must name one existing alternate path, and no save transaction may have
    // begun under the old identity. No source/owner/observer bytes are edited.
    if !Path::new(&binding.worktree_path).exists() {
        let graph = crate::parser::load_graph(dir.join("graph.jsonl"))?;
        let exact_owner = graph.get_task(&binding.task_id).is_some_and(|task| {
            task.lifecycle.generation == binding.generation
                && task.lifecycle.fence == binding.fence
                && task.assigned.as_deref() == Some(binding.agent_id.as_str())
                && task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_some_and(|attempt| {
                        attempt.id == binding.attempt_id && attempt.fence == binding.fence
                    })
        });
        if exact_owner
            && let Some(path) = crate::service::registry::AgentRegistry::load(dir)?
                .get_agent(&binding.agent_id)
                .and_then(|agent| agent.worktree_path.clone())
        {
            let old_transaction = binding
                .save_source
                .transaction_id()
                .map_err(anyhow::Error::msg)?;
            if load_save_transaction(dir, &old_transaction)?.is_none() {
                let (canonical, identity) = worktree_identity(Path::new(&path))?;
                binding.worktree_path = canonical;
                binding.save_source.worktree_identity_digest = identity;
                capabilities
                    .capabilities
                    .insert(digest.clone(), binding.clone());
                save_registry(dir, &capabilities)?;
            }
        }
    }
    Ok(binding)
}

fn validate_request_id(request_id: &str) -> Result<()> {
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        bail!("worker_control.request_id_invalid");
    }
    Ok(())
}

/// Inspect an already-journaled request without creating a fresh intent.
/// This lets an exact retry recover its completed response after the mutation
/// itself released/fenced the attempt capability (notably `done`). Token and
/// operation must still match the authenticated journal entry byte-for-byte.
pub fn replay_request(
    dir: &Path,
    request_id: &str,
    token: &str,
    operation: &WorkerOperation,
) -> Result<Option<BeginRequest>> {
    validate_request_id(request_id)?;
    let digest = token_digest(token);
    let registry = load_registry(dir)?;
    let Some(existing) = registry.requests.get(request_id) else {
        return Ok(None);
    };
    let operation_cid = content_cid(operation).map_err(anyhow::Error::msg)?;
    if existing.token_sha256 != digest
        || existing.operation != operation.kind()
        || existing.operation_cid != operation_cid
    {
        bail!("worker_control.request_id_conflict");
    }
    Ok(Some(match &existing.state {
        RequestJournalState::Pending => BeginRequest::Pending,
        RequestJournalState::Completed(response) => BeginRequest::Completed(response.clone()),
    }))
}

/// Persist the request intent before mutation. A daemon crash after this point
/// yields `Pending` on replay, which fails closed instead of duplicating the
/// mutation. Completed replies are replayed byte-for-byte.
pub fn begin_request(
    dir: &Path,
    request_id: &str,
    token: &str,
    operation: &WorkerOperation,
) -> Result<BeginRequest> {
    validate_request_id(request_id)?;
    if let Some(existing) = replay_request(dir, request_id, token, operation)? {
        return Ok(existing);
    }
    let digest = token_digest(token);
    let mut registry = load_registry(dir)?;
    registry.requests.insert(
        request_id.to_string(),
        RequestJournalEntry {
            token_sha256: digest,
            operation: operation.kind(),
            operation_cid: content_cid(operation).map_err(anyhow::Error::msg)?,
            save_transaction_id: None,
            started_at: Utc::now().to_rfc3339(),
            state: RequestJournalState::Pending,
        },
    );
    save_registry(dir, &registry)?;
    Ok(BeginRequest::Fresh)
}

pub fn complete_request(
    dir: &Path,
    request_id: &str,
    response: StoredWorkerResponse,
) -> Result<()> {
    let mut registry = load_registry(dir)?;
    let entry = registry
        .requests
        .get_mut(request_id)
        .ok_or_else(|| anyhow::anyhow!("worker_control.request_intent_missing"))?;
    entry.state = RequestJournalState::Completed(response);
    save_registry(dir, &registry)
}

fn completion_root(dir: &Path) -> PathBuf {
    dir.join("completion").join("v2")
}

fn transaction_slot(transaction_id: &str) -> String {
    blake3::hash(transaction_id.as_bytes()).to_hex().to_string()
}

fn transaction_head_path(dir: &Path, transaction_id: &str) -> PathBuf {
    completion_root(dir)
        .join("transactions")
        .join(transaction_slot(transaction_id))
        .join("head.json")
}

fn transaction_journal_path(dir: &Path, transaction_id: &str) -> PathBuf {
    completion_root(dir)
        .join("journal")
        .join(format!("{}.jsonl", transaction_slot(transaction_id)))
}

pub fn store_completion_object<T: Serialize>(dir: &Path, value: &T) -> Result<String> {
    let cid = content_cid(value).map_err(anyhow::Error::msg)?;
    let objects = completion_root(dir).join("objects");
    fs::create_dir_all(&objects)?;
    atomic_write(
        &objects.join(transaction_slot(&cid)),
        &serde_json::to_vec(value)?,
    )?;
    Ok(cid)
}

pub fn load_completion_object<T: serde::de::DeserializeOwned + Serialize>(
    dir: &Path,
    cid: &str,
) -> Result<T> {
    let path = completion_root(dir)
        .join("objects")
        .join(transaction_slot(cid));
    let bytes =
        fs::read(&path).with_context(|| format!("read completion object {}", path.display()))?;
    let value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse completion object {}", path.display()))?;
    let observed = content_cid(&value).map_err(anyhow::Error::msg)?;
    if observed != cid {
        bail!("worker_control.completion_object_cid_mismatch");
    }
    Ok(value)
}

pub fn load_save_transaction(
    dir: &Path,
    transaction_id: &str,
) -> Result<Option<SaveTransactionState>> {
    let journal = transaction_journal_path(dir, transaction_id);
    if journal.exists() {
        let (state, _, _) = read_save_journal(dir, transaction_id)?;
        if let Some(state) = &state {
            // head.json is a rebuildable projection. Repair a missing/stale
            // head after a crash between journal fsync and atomic replacement.
            let head = transaction_head_path(dir, transaction_id);
            let head_matches = fs::read(&head)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SaveTransactionState>(&bytes).ok())
                .is_some_and(|head| head == *state);
            if !head_matches {
                atomic_write(&head, &serde_json::to_vec_pretty(state)?)?;
            }
        }
        return Ok(state);
    }

    // Compatibility for a head written before the journal adapter landed.
    let path = transaction_head_path(dir, transaction_id);
    if !path.exists() {
        return Ok(None);
    }
    let state: SaveTransactionState = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parse SaveTransaction head {}", path.display()))?;
    if state.transaction_id != transaction_id {
        bail!("worker_control.save_transaction_slot_mismatch");
    }
    Ok(Some(state))
}

pub fn list_save_transactions(dir: &Path) -> Result<Vec<SaveTransactionState>> {
    let mut transaction_ids = std::collections::BTreeSet::new();
    let transactions = completion_root(dir).join("transactions");
    if transactions.exists() {
        for entry in fs::read_dir(transactions)? {
            let head = entry?.path().join("head.json");
            if let Ok(bytes) = fs::read(&head)
                && let Ok(state) = serde_json::from_slice::<SaveTransactionState>(&bytes)
            {
                transaction_ids.insert(state.transaction_id);
            }
        }
    }
    // A crash may leave the authoritative journal fsynced without head.json.
    // Read only the transaction id candidate here; load_save_transaction then
    // validates the complete checksum chain and every referenced state object.
    let journals = completion_root(dir).join("journal");
    if journals.exists() {
        for entry in fs::read_dir(journals)? {
            let bytes = fs::read(entry?.path())?;
            let Some(line) = bytes.split_inclusive(|byte| *byte == b'\n').next() else {
                continue;
            };
            if !line.ends_with(b"\n") {
                continue;
            }
            if let Ok(frame) = serde_json::from_slice::<SaveJournalFrame>(&line[..line.len() - 1]) {
                transaction_ids.insert(frame.transaction_id);
            }
        }
    }
    let mut states = Vec::new();
    for transaction_id in transaction_ids {
        if let Some(state) = load_save_transaction(dir, &transaction_id)? {
            states.push(state);
        }
    }
    states.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id));
    Ok(states)
}

pub fn save_transaction_matches_task(
    state: &SaveTransactionState,
    task: &crate::graph::Task,
) -> bool {
    state.source.task_id == task.id
        && state.source.generation == task.lifecycle.generation
        && state.source.attempt_fence == task.lifecycle.fence
        && task
            .lifecycle
            .current_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.id == state.source.attempt_id)
}

pub fn save_transaction_for_task(
    dir: &Path,
    task: &crate::graph::Task,
) -> Result<Option<SaveTransactionState>> {
    Ok(list_save_transactions(dir)?
        .into_iter()
        .filter(|state| save_transaction_matches_task(state, task))
        .max_by_key(|state| state.revision))
}

pub fn save_transaction_bindings(
    dir: &Path,
) -> Result<Vec<(AttemptCapabilityBinding, SaveTransactionState)>> {
    let registry = load_registry(dir)?;
    let mut bound = Vec::new();
    for binding in registry.capabilities.values() {
        let transaction_id = binding
            .save_source
            .transaction_id()
            .map_err(anyhow::Error::msg)?;
        if let Some(state) = load_save_transaction(dir, &transaction_id)? {
            bound.push((binding.clone(), state));
        }
    }
    bound.sort_by(|(left, _), (right, _)| {
        left.save_source
            .task_id
            .cmp(&right.save_source.task_id)
            .then_with(|| {
                left.save_source
                    .generation
                    .cmp(&right.save_source.generation)
            })
            .then_with(|| {
                left.save_source
                    .attempt_id
                    .cmp(&right.save_source.attempt_id)
            })
    });
    bound.dedup_by(|(_, left), (_, right)| left.transaction_id == right.transaction_id);
    Ok(bound)
}

pub fn request_save_transaction(
    dir: &Path,
    request_id: &str,
) -> Result<Option<SaveTransactionState>> {
    let mut registry = load_registry(dir)?;
    if let Some(transaction_id) = registry
        .requests
        .get(request_id)
        .and_then(|entry| entry.save_transaction_id.as_deref())
    {
        return load_save_transaction(dir, transaction_id);
    }

    // Crash cut: the SaveTransaction frame may be durable while the request
    // journal's rebuildable transaction pointer is not. Discover the exact
    // idempotency key from authoritative transaction heads and repair only
    // that cache link; never execute the terminal operation again.
    let discovered = list_save_transactions(dir)?
        .into_iter()
        .find(|state| state.requests.contains_key(request_id));
    if let Some(state) = &discovered
        && let Some(entry) = registry.requests.get_mut(request_id)
    {
        entry.save_transaction_id = Some(state.transaction_id.clone());
        save_registry(dir, &registry)?;
    }
    Ok(discovered)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SaveJournalFrame {
    schema_version: u32,
    transaction_id: String,
    revision: u64,
    prior_phase: SavePhase,
    next_phase: SavePhase,
    action_key: String,
    idempotency_key: String,
    state_cid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_frame_cid: Option<String>,
    committed_at: String,
    frame_cid: String,
}

#[derive(Serialize)]
struct SaveJournalFrameMaterial<'a> {
    schema_version: u32,
    transaction_id: &'a str,
    revision: u64,
    prior_phase: SavePhase,
    next_phase: SavePhase,
    action_key: &'a str,
    idempotency_key: &'a str,
    state_cid: &'a str,
    prior_frame_cid: Option<&'a str>,
    committed_at: &'a str,
}

fn frame_cid(frame: &SaveJournalFrame) -> Result<String> {
    content_cid(&SaveJournalFrameMaterial {
        schema_version: frame.schema_version,
        transaction_id: &frame.transaction_id,
        revision: frame.revision,
        prior_phase: frame.prior_phase,
        next_phase: frame.next_phase,
        action_key: &frame.action_key,
        idempotency_key: &frame.idempotency_key,
        state_cid: &frame.state_cid,
        prior_frame_cid: frame.prior_frame_cid.as_deref(),
        committed_at: &frame.committed_at,
    })
    .map_err(anyhow::Error::msg)
}

/// Return the last checksum-valid state, its frame CID, and the byte boundary
/// through which the journal is valid. A torn/corrupt tail is ignored and
/// truncated before the next append; no later frame can validate across it.
fn read_save_journal(
    dir: &Path,
    transaction_id: &str,
) -> Result<(Option<SaveTransactionState>, Option<String>, u64)> {
    let path = transaction_journal_path(dir, transaction_id);
    if !path.exists() {
        return Ok((None, None, 0));
    }
    let bytes = fs::read(&path)?;
    let mut valid_len = 0_u64;
    let mut prior_frame_cid: Option<String> = None;
    let mut prior_state: Option<SaveTransactionState> = None;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        let Ok(frame) = serde_json::from_slice::<SaveJournalFrame>(&line[..line.len() - 1]) else {
            break;
        };
        if frame.schema_version != 2
            || frame.transaction_id != transaction_id
            || frame.prior_frame_cid != prior_frame_cid
            || frame_cid(&frame)? != frame.frame_cid
        {
            break;
        }
        let object = completion_root(dir)
            .join("objects")
            .join(transaction_slot(&frame.state_cid));
        let Ok(state_bytes) = fs::read(object) else {
            break;
        };
        let Ok(state) = serde_json::from_slice::<SaveTransactionState>(&state_bytes) else {
            break;
        };
        if state.transaction_id != transaction_id
            || state.revision != frame.revision
            || state.phase != frame.next_phase
            || content_cid(&state).map_err(anyhow::Error::msg)? != frame.state_cid
            || prior_state.as_ref().is_some_and(|prior| {
                prior.revision.saturating_add(1) != state.revision
                    || prior.phase != frame.prior_phase
            })
        {
            break;
        }
        valid_len = valid_len.saturating_add(line.len() as u64);
        prior_frame_cid = Some(frame.frame_cid);
        prior_state = Some(state);
    }
    Ok((prior_state, prior_frame_cid, valid_len))
}

fn save_commit_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn commit_save_transition(
    dir: &Path,
    request: SaveTransitionRequest,
) -> Result<SaveTransactionState> {
    let _guard = save_commit_mutex()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let transaction_id = request
        .source
        .transaction_id()
        .map_err(anyhow::Error::msg)?;
    let current = load_save_transaction(dir, &transaction_id)?
        .unwrap_or(SaveTransactionState::new(request.source.clone()).map_err(anyhow::Error::msg)?);
    let prior_phase = current.phase;
    let action_key = request.action_key.clone();
    let idempotency_key = request.idempotency_key.clone();
    let plan = SaveTransactionKernel::transition(&current, request).map_err(anyhow::Error::msg)?;
    if plan.duplicate {
        return Ok(plan.state);
    }

    let objects = completion_root(dir).join("objects");
    fs::create_dir_all(&objects)?;
    let state_cid = content_cid(&plan.state).map_err(anyhow::Error::msg)?;
    atomic_write(
        &objects.join(transaction_slot(&state_cid)),
        &serde_json::to_vec(&plan.state)?,
    )?;
    let journal = transaction_journal_path(dir, &transaction_id);
    fs::create_dir_all(journal.parent().context("journal has no parent")?)?;
    let (_, prior_frame_cid, valid_len) = read_save_journal(dir, &transaction_id)?;
    let mut frame = SaveJournalFrame {
        schema_version: 2,
        transaction_id: transaction_id.clone(),
        revision: plan.state.revision,
        prior_phase,
        next_phase: plan.state.phase,
        action_key,
        idempotency_key,
        state_cid,
        prior_frame_cid,
        committed_at: Utc::now().to_rfc3339(),
        frame_cid: String::new(),
    };
    frame.frame_cid = frame_cid(&frame)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    let mut file = options.open(&journal)?;
    file.set_len(valid_len)?;
    file.seek(std::io::SeekFrom::Start(valid_len))?;
    serde_json::to_writer(&mut file, &frame)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if let Some(parent) = journal.parent()
        && let Ok(parent_file) = fs::File::open(parent)
    {
        parent_file.sync_all()?;
    }
    atomic_write(
        &transaction_head_path(dir, &transaction_id),
        &serde_json::to_vec_pretty(&plan.state)?,
    )?;
    Ok(plan.state)
}

pub fn prepare_done_transaction(
    dir: &Path,
    binding: &AttemptCapabilityBinding,
    request_id: &str,
    operation: &WorkerOperation,
) -> Result<SaveTransactionState> {
    if !matches!(operation, WorkerOperation::DoneHandoff { .. }) {
        bail!("worker_control.save_intent_operation_invalid");
    }
    let transaction_id = binding
        .save_source
        .transaction_id()
        .map_err(anyhow::Error::msg)?;
    verify_bound_worktree(binding)?;
    let operation_cid = store_completion_object(dir, operation)?;
    let state = load_save_transaction(dir, &transaction_id)?.unwrap_or(
        SaveTransactionState::new(binding.save_source.clone()).map_err(anyhow::Error::msg)?,
    );
    let next = commit_save_transition(
        dir,
        SaveTransitionRequest {
            source: binding.save_source.clone(),
            expected_revision: state.revision,
            expected_phase: state.phase,
            next_phase: SavePhase::Prepared,
            idempotency_key: request_id.to_string(),
            action_key: format!("prepare:{request_id}"),
            fact: SaveFact::Evidence {
                cid: operation_cid,
                binding: None,
            },
        },
    )?;
    let mut registry = load_registry(dir)?;
    if let Some(entry) = registry.requests.get_mut(request_id) {
        entry.save_transaction_id = Some(transaction_id);
    }
    if let Some(capability) = registry.capabilities.get_mut(&binding.token_sha256) {
        capability.allowed_operations =
            WorkerOperationKind::after_terminal_reservation_operations();
    }
    save_registry(dir, &registry)?;
    Ok(next)
}

pub fn save_transaction_for_agent(
    dir: &Path,
    agent_id: &str,
) -> Result<Option<SaveTransactionState>> {
    Ok(save_transaction_bindings(dir)?
        .into_iter()
        .filter(|(binding, _)| binding.agent_id == agent_id)
        .map(|(_, state)| state)
        .max_by(|left, right| {
            left.source
                .generation
                .cmp(&right.source.generation)
                .then_with(|| left.revision.cmp(&right.revision))
        }))
}

pub fn append_audit(dir: &Path, event: &WorkerAuditEvent) -> Result<()> {
    fs::create_dir_all(service_dir(dir))?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(audit_path(dir))?;
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

pub fn token_hint(token: &str) -> String {
    token_digest(token).chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_never_persists_bearer_token() {
        let temp = tempfile::tempdir().unwrap();
        let (token, binding) =
            mint_attempt_capability(temp.path(), "task-a", 3, "attempt-3-2", 8, 8, "agent-7")
                .unwrap();
        let bytes = fs::read(registry_path(temp.path())).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&token));
        assert_eq!(lookup_capability(temp.path(), &token).unwrap(), binding);
    }

    #[test]
    fn legacy_capability_rebinds_once_to_registered_retained_worktree() {
        let project = tempfile::tempdir().unwrap();
        let dir = project.path().join(".wg");
        fs::create_dir_all(&dir).unwrap();
        let retained = project.path().join(".wg-worktrees/agent-old");
        fs::create_dir_all(&retained).unwrap();
        fs::write(retained.join(".git"), "gitdir: retained-admin\n").unwrap();
        let row = serde_json::json!({
            "kind": "task",
            "id": "task-a",
            "title": "Task A",
            "status": "in-progress",
            "assigned": "agent-2",
            "lifecycle": {
                "generation": 1,
                "fence": 2,
                "attempt_sequence": 1,
                "current_attempt": {
                    "id": "attempt-1-1",
                    "generation": 1,
                    "fence": 2,
                    "actor_id": "agent-2"
                }
            }
        });
        fs::write(dir.join("graph.jsonl"), format!("{row}\n")).unwrap();
        let mut agents = crate::service::registry::AgentRegistry::new();
        agents.next_agent_id = 2;
        let id = agents.register_agent(std::process::id(), "task-a", "pi", "/tmp/retained-output");
        assert_eq!(id, "agent-2");
        agents.set_worktree_path(&id, &retained);
        agents.save(&dir).unwrap();

        let (token, original) =
            mint_attempt_capability(&dir, "task-a", 1, "attempt-1-1", 2, 2, "agent-2").unwrap();
        assert!(!Path::new(&original.worktree_path).exists());
        let repaired = lookup_capability(&dir, &token).unwrap();
        assert_eq!(
            Path::new(&repaired.worktree_path),
            retained.canonicalize().unwrap()
        );
        assert_eq!(verify_bound_worktree(&repaired).unwrap(), retained);
        assert_eq!(lookup_capability(&dir, &token).unwrap(), repaired);
    }

    #[test]
    fn status_never_overclaims_environment_omission_as_isolation() {
        // Ambient environment is irrelevant to the attested mode; only a
        // successful sandbox visibility probe may ever produce Enforced.
        let status = filesystem_isolation_status();
        assert_eq!(status.mode, FilesystemIsolationMode::Degraded);
        assert!(!status.enforced);
        assert!(status.reason.contains("same-uid"));
    }

    #[test]
    fn transaction_journal_rebuilds_head_and_truncates_torn_tail() {
        let project = tempfile::tempdir().unwrap();
        let dir = project.path().join(".wg");
        fs::create_dir_all(&dir).unwrap();
        let worktree = project.path().join(".wg-worktrees/agent-1");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".git"), "gitdir: test\n").unwrap();
        let row = serde_json::json!({
            "kind": "task",
            "id": "task-a",
            "title": "Task A",
            "status": "in-progress",
            "assigned": "agent-1",
            "lifecycle": {
                "generation": 1,
                "fence": 2,
                "attempt_sequence": 1,
                "current_attempt": {
                    "id": "attempt-1-1",
                    "generation": 1,
                    "fence": 2,
                    "actor_id": "agent-1"
                }
            }
        });
        fs::write(dir.join("graph.jsonl"), format!("{row}\n")).unwrap();
        let (token, binding) =
            mint_attempt_capability(&dir, "task-a", 1, "attempt-1-1", 2, 2, "agent-1").unwrap();
        let operation = WorkerOperation::DoneHandoff {
            converged: false,
            full_smoke: false,
        };
        begin_request(&dir, "intent-1", &token, &operation).unwrap();
        let prepared = prepare_done_transaction(&dir, &binding, "intent-1", &operation).unwrap();
        let reserved = lookup_capability(&dir, &token).unwrap();
        assert!(
            !reserved
                .allowed_operations
                .contains(&WorkerOperationKind::DoneHandoff)
        );
        assert!(
            !reserved
                .allowed_operations
                .contains(&WorkerOperationKind::FailHandoff)
        );
        assert!(
            reserved
                .allowed_operations
                .contains(&WorkerOperationKind::FinishHandoff)
        );
        assert!(
            reserved
                .allowed_operations
                .contains(&WorkerOperationKind::PiWatchdog)
        );
        assert!(
            reserved
                .allowed_operations
                .contains(&WorkerOperationKind::Telemetry)
        );
        let head = transaction_head_path(&dir, &prepared.transaction_id);
        fs::remove_file(&head).unwrap();
        assert_eq!(
            load_save_transaction(&dir, &prepared.transaction_id)
                .unwrap()
                .unwrap(),
            prepared
        );
        let journal = transaction_journal_path(&dir, &prepared.transaction_id);
        let mut file = OpenOptions::new().append(true).open(&journal).unwrap();
        file.write_all(b"{torn").unwrap();
        file.sync_all().unwrap();
        let loaded = load_save_transaction(&dir, &prepared.transaction_id)
            .unwrap()
            .unwrap();
        let next = commit_save_transition(
            &dir,
            SaveTransitionRequest {
                source: loaded.source.clone(),
                expected_revision: loaded.revision,
                expected_phase: loaded.phase,
                next_phase: SavePhase::Quiescing,
                idempotency_key: "quiesce-1".into(),
                action_key: "quiesce-1".into(),
                fact: SaveFact::Evidence {
                    cid: "quiescence-cid".into(),
                    binding: None,
                },
            },
        )
        .unwrap();
        assert_eq!(next.phase, SavePhase::Quiescing);
        let journal_bytes = fs::read(journal).unwrap();
        assert!(!journal_bytes.windows(5).any(|window| window == b"{torn"));
        assert!(journal_bytes.ends_with(b"\n"));
    }

    #[test]
    fn graph_identity_is_stable_and_not_a_checkout_path() {
        let temp = tempfile::tempdir().unwrap();
        let first = load_or_create_graph_identity(temp.path()).unwrap();
        let second = load_or_create_graph_identity(temp.path()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("wggraph:v1:"));
        assert!(!first.contains(temp.path().to_string_lossy().as_ref()));
    }
}
