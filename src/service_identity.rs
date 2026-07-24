//! Authenticated, read-only service identity primitives shared by the `wg`
//! daemon and the feature-gated `worksgood` concierge trial.
//!
//! The identity deliberately contains no credential, endpoint URL, or model
//! prompt.  It binds a daemon to one canonical graph, one executable byte
//! image, one protocol version, and one effective merged-config fingerprint.

use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::{Stream, prelude::*};

pub const SERVICE_IDENTITY_PROTOCOL: &str = "worksgood-service-identity-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceIdentity {
    pub canonical_graph: String,
    pub graph_digest: String,
    pub executable: String,
    pub executable_sha256: String,
    pub build_id: String,
    pub protocol: String,
    pub config_fingerprint: String,
    /// Exact project-profile generation selected when the service started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_profile_fingerprint: Option<String>,
}

/// Read-only subset of the daemon state file.  Lifecycle clients deliberately
/// deserialize rather than call `wg service status`: the latter historically
/// repaired stale state, while `worksgood status` and dry-run must never write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedServiceState {
    pub pid: u32,
    pub socket_path: String,
    pub started_at: String,
    #[serde(default)]
    pub pid_start_identity: Option<String>,
    #[serde(default)]
    pub identity: Option<ServiceIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceHealth {
    Down,
    StalePid,
    Unverified,
    Unresponsive,
    /// The live handshake is self-consistent but names another graph or an
    /// invalid executable/build identity. Concierge callers must never signal it.
    Foreign,
    Healthy,
}

/// Authenticated observation used by the concierge reconcile planner.  A
/// healthy result requires four independent agreements: state-file PID birth,
/// exact project socket path, a live socket response, and equal identities in
/// the state file and socket response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceObservation {
    pub health: ServiceHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<PersistedServiceState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_identity: Option<ServiceIdentity>,
    pub detail: String,
}

pub fn service_state_path(dir: &Path) -> PathBuf {
    dir.join("service").join("state.json")
}

pub fn default_socket_path(dir: &Path) -> PathBuf {
    dir.join("service").join("daemon.sock")
}

fn canonical_json(value: &serde_json::Value, out: &mut Vec<u8>) {
    match value {
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(key).unwrap_or_default());
                out.push(b':');
                canonical_json(&map[key], out);
            }
            out.push(b'}');
        }
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                canonical_json(item, out);
            }
            out.push(b']');
        }
        _ => out.extend(serde_json::to_vec(value).unwrap_or_default()),
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn executable_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open executable {}", path.display()))?;
    // `wg` debug images are hundreds of MiB. ring's SHA-256 backend uses the
    // platform's optimized implementation even in a dev build; the pure-Rust
    // sha2 state machine in an unoptimized binary can otherwise delay service
    // readiness beyond ten seconds. The digest and wire spelling stay SHA-256.
    let mut hasher = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0_u8; 8 * 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to hash executable {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finish().as_ref())))
}

pub fn graph_digest(path: &Path) -> Result<String> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize graph {}", path.display()))?;
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        canonical.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = canonical.to_string_lossy().as_bytes().to_vec();
    let mut domain = b"worksgood-service-graph-v1\0".to_vec();
    domain.extend(bytes);
    Ok(sha256_bytes(&domain))
}

/// Fingerprint the complete effective merged config without serializing any
/// values into the handshake.  Canonical JSON makes map order irrelevant.
pub fn config_fingerprint(config: &Config) -> Result<String> {
    let value = serde_json::to_value(config).context("Failed to serialize effective config")?;
    let mut bytes = b"worksgood-service-config-v1\0".to_vec();
    canonical_json(&value, &mut bytes);
    Ok(sha256_bytes(&bytes))
}

pub fn build_id(executable_sha256: &str) -> String {
    let short = executable_sha256
        .strip_prefix("sha256:")
        .unwrap_or(executable_sha256)
        .chars()
        .take(12)
        .collect::<String>();
    format!("{}+{}", env!("CARGO_PKG_VERSION"), short)
}

pub fn selected_profile_identity(dir: &Path) -> Result<(Option<String>, Option<String>)> {
    let association = crate::profile::project::read_association(dir)?;
    Ok(match association {
        Some(association) => (
            Some(association.profile),
            Some(association.profile_fingerprint),
        ),
        None => (None, None),
    })
}

fn identity_shape_error(dir: &Path, identity: &ServiceIdentity) -> Option<String> {
    let graph = match dir.canonicalize() {
        Ok(graph) => graph,
        Err(error) => return Some(format!("cannot authenticate graph identity: {error}")),
    };
    let expected_graph_digest = match graph_digest(&graph) {
        Ok(digest) => digest,
        Err(error) => return Some(format!("cannot authenticate graph digest: {error}")),
    };
    if identity.canonical_graph != graph.display().to_string()
        || identity.graph_digest != expected_graph_digest
    {
        return Some("live service reports a foreign canonical graph identity".to_string());
    }
    if !Path::new(&identity.executable).is_absolute() {
        return Some("live service reports a non-absolute executable identity".to_string());
    }
    let raw_hash = identity
        .executable_sha256
        .strip_prefix("sha256:")
        .unwrap_or_default();
    if raw_hash.len() != 64 || !raw_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some("live service reports a malformed executable build fingerprint".to_string());
    }
    let short = &raw_hash[..12];
    if identity.build_id.rsplit_once('+').map(|(_, value)| value) != Some(short) {
        return Some("live service build id disagrees with its content fingerprint".to_string());
    }
    None
}

pub fn expected_identity(
    dir: &Path,
    executable: &Path,
    config: &Config,
) -> Result<ServiceIdentity> {
    let graph = dir
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize graph {}", dir.display()))?;
    let executable = executable
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize executable {}", executable.display()))?;
    let executable_hash = executable_sha256(&executable)?;
    let (selected_profile, selected_profile_fingerprint) = selected_profile_identity(dir)?;
    Ok(ServiceIdentity {
        canonical_graph: graph.display().to_string(),
        graph_digest: graph_digest(&graph)?,
        executable: executable.display().to_string(),
        executable_sha256: executable_hash.clone(),
        build_id: build_id(&executable_hash),
        protocol: SERVICE_IDENTITY_PROTOCOL.to_string(),
        config_fingerprint: config_fingerprint(config)?,
        selected_profile,
        selected_profile_fingerprint,
    })
}

/// Linux/Android PID birth identity (`/proc/<pid>/stat` field 22).  Other
/// platforms retain the ISO start timestamp and return `None` here.
pub fn pid_start_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The comm field is parenthesized and may contain spaces.  Fields after
        // its final ')' start at field 3, so field 22 is index 19 in this tail.
        let tail = stat.rsplit_once(')')?.1.trim();
        tail.split_whitespace()
            .nth(19)
            .map(|ticks| format!("proc-start:{ticks}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(unix)]
fn socket_name(path: &Path) -> std::io::Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    path.as_os_str()
        .to_os_string()
        .to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
fn socket_name(path: &Path) -> std::io::Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = DefaultHasher::new();
    abs.hash(&mut hasher);
    format!("workgraph-daemon-{:016x}", hasher.finish()).to_ns_name::<GenericNamespaced>()
}

fn socket_status_identity(socket: &Path) -> Result<ServiceIdentity> {
    let mut stream = Stream::connect(socket_name(socket)?)
        .with_context(|| format!("Could not connect to {}", socket.display()))?;
    #[cfg(unix)]
    {
        stream.set_recv_timeout(Some(Duration::from_secs(2)))?;
        stream.set_send_timeout(Some(Duration::from_secs(2)))?;
    }
    writeln!(stream, r#"{{"cmd":"status"}}"#)?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let value: serde_json::Value =
        serde_json::from_str(line.trim()).context("Invalid service status handshake")?;
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        anyhow::bail!(
            "Service rejected status handshake: {}",
            value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
        );
    }
    serde_json::from_value(
        value
            .get("identity")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Service handshake omitted identity"))?,
    )
    .context("Service handshake identity is malformed")
}

/// Observe a service without repairing, removing, signalling, or creating any
/// file.  This is intentionally safe for help, status, plan, and strict
/// dry-run paths.
pub fn observe_service(dir: &Path) -> ServiceObservation {
    let path = service_state_path(dir);
    let state: PersistedServiceState = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(state) => state,
            Err(e) => {
                return ServiceObservation {
                    health: ServiceHealth::Unverified,
                    state: None,
                    handshake_identity: None,
                    detail: format!("Malformed service state: {e}"),
                };
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ServiceObservation {
                health: ServiceHealth::Down,
                state: None,
                handshake_identity: None,
                detail: "No service state file".to_string(),
            };
        }
        Err(e) => {
            return ServiceObservation {
                health: ServiceHealth::Unverified,
                state: None,
                handshake_identity: None,
                detail: format!("Cannot read service state: {e}"),
            };
        }
    };

    let current_birth = pid_start_identity(state.pid);
    if current_birth.is_none()
        || (state.pid_start_identity.is_some() && state.pid_start_identity != current_birth)
    {
        return ServiceObservation {
            health: ServiceHealth::StalePid,
            state: Some(state),
            handshake_identity: None,
            detail: "Recorded PID is not the recorded process birth".to_string(),
        };
    }
    let expected_socket = default_socket_path(dir);
    if Path::new(&state.socket_path) != expected_socket {
        let detail = format!(
            "State socket {} differs from project socket {}",
            Path::new(&state.socket_path).display(),
            expected_socket.display()
        );
        return ServiceObservation {
            health: ServiceHealth::Unverified,
            state: Some(state),
            handshake_identity: None,
            detail,
        };
    }
    let Some(state_identity) = state.identity.clone() else {
        return ServiceObservation {
            health: ServiceHealth::Unverified,
            state: Some(state),
            handshake_identity: None,
            detail: "Legacy daemon state has no authenticated identity".to_string(),
        };
    };
    match socket_status_identity(&expected_socket) {
        Ok(handshake) if handshake == state_identity => {
            if let Some(detail) = identity_shape_error(dir, &handshake) {
                ServiceObservation {
                    health: ServiceHealth::Foreign,
                    state: Some(state),
                    handshake_identity: Some(handshake),
                    detail,
                }
            } else {
                ServiceObservation {
                    health: ServiceHealth::Healthy,
                    state: Some(state),
                    handshake_identity: Some(handshake),
                    detail: "PID birth, socket, graph, and build identity agree".to_string(),
                }
            }
        }
        Ok(handshake) => ServiceObservation {
            health: ServiceHealth::Unverified,
            state: Some(state),
            handshake_identity: Some(handshake),
            detail: "State-file and socket identities disagree".to_string(),
        },
        Err(e) => ServiceObservation {
            health: ServiceHealth::Unresponsive,
            state: Some(state),
            handshake_identity: None,
            detail: e.to_string(),
        },
    }
}

pub fn canonical_absolute(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize {}", path.display()))?;
    if !canonical.is_absolute() {
        anyhow::bail!("Canonical path is not absolute: {}", canonical.display());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn config_fingerprint_is_stable_and_changes_with_route() {
        let mut a = Config::default();
        let before = config_fingerprint(&a).unwrap();
        assert_eq!(before, config_fingerprint(&a).unwrap());
        a.agent.model = "codex:gpt-5.5".to_string();
        assert_ne!(before, config_fingerprint(&a).unwrap());
    }

    #[test]
    fn expected_identity_contains_only_digests_and_absolute_identity() {
        let temp = TempDir::new().unwrap();
        let graph = temp.path().join(".wg");
        std::fs::create_dir(&graph).unwrap();
        let exe = temp.path().join("wg-candidate");
        std::fs::write(&exe, b"candidate").unwrap();
        let identity = expected_identity(&graph, &exe, &Config::default()).unwrap();
        assert!(identity.canonical_graph.starts_with('/'));
        assert!(identity.executable.starts_with('/'));
        assert!(identity.executable_sha256.starts_with("sha256:"));
        assert_eq!(identity.protocol, SERVICE_IDENTITY_PROTOCOL);
    }
}
