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
use std::io::Write;
use std::path::{Path, PathBuf};

pub const WORKER_CONTROL_PROTOCOL: &str = "worksgood-worker-control-v1";

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

/// Mint an opaque bearer capability.  Only its digest is made durable.
pub fn mint_attempt_capability(
    dir: &Path,
    task_id: &str,
    generation: u64,
    attempt_id: &str,
    fence: u64,
    lease_epoch: u64,
    agent_id: &str,
) -> Result<(String, AttemptCapabilityBinding)> {
    let mut random = [0_u8; 32];
    getrandom::getrandom(&mut random).context("generate worker capability")?;
    let token = format!("wgcap_v1_{}", hex::encode(random));
    let digest = token_digest(&token);
    let binding = AttemptCapabilityBinding {
        protocol: WORKER_CONTROL_PROTOCOL.to_string(),
        graph_id: load_or_create_graph_identity(dir)?,
        task_id: task_id.to_string(),
        generation,
        attempt_id: attempt_id.to_string(),
        fence,
        lease_epoch,
        agent_id: agent_id.to_string(),
        token_sha256: digest.clone(),
        issued_at: Utc::now().to_rfc3339(),
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
    load_registry(dir)?
        .capabilities
        .remove(&digest)
        .ok_or_else(|| anyhow::anyhow!("worker_control.capability_unknown"))
}

/// Persist the request intent before mutation. A daemon crash after this point
/// yields `Pending` on replay, which fails closed instead of duplicating the
/// mutation. Completed replies are replayed byte-for-byte.
pub fn begin_request(
    dir: &Path,
    request_id: &str,
    token: &str,
    operation: WorkerOperationKind,
) -> Result<BeginRequest> {
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        bail!("worker_control.request_id_invalid");
    }
    let digest = token_digest(token);
    let mut registry = load_registry(dir)?;
    if let Some(existing) = registry.requests.get(request_id) {
        if existing.token_sha256 != digest || existing.operation != operation {
            bail!("worker_control.request_id_conflict");
        }
        return Ok(match &existing.state {
            RequestJournalState::Pending => BeginRequest::Pending,
            RequestJournalState::Completed(response) => BeginRequest::Completed(response.clone()),
        });
    }
    registry.requests.insert(
        request_id.to_string(),
        RequestJournalEntry {
            token_sha256: digest,
            operation,
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
    fn status_never_overclaims_environment_omission_as_isolation() {
        // Ambient environment is irrelevant to the attested mode; only a
        // successful sandbox visibility probe may ever produce Enforced.
        let status = filesystem_isolation_status();
        assert_eq!(status.mode, FilesystemIsolationMode::Degraded);
        assert!(!status.enforced);
        assert!(status.reason.contains("same-uid"));
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
