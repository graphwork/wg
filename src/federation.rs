//! WG-Fed peer federation: name/path/wgid resolution + cross-repo dependency status.
//!
//! This whole module is gated behind the NON-DEFAULT `federation` cargo feature
//! (`cargo build --features federation`). The default `wg` binary drops it, along with
//! `crate::identity`, `crate::review`, `crate::providers`, `crate::trust` and their
//! CLI arms (`wg identity`, `wg fed-node`, `wg review`, `wg provider`, `wg pilot`,
//! and the key-based half of `wg peer` / `wg msg --to wgid:`).
//!
//! The agency-store transfer half of the historical `crate::federation` module lives
//! in [`crate::agency::federation`] and stays unconditional.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Re-exported federation.yaml config surface (shared with the unconditional
/// [`crate::agency::federation`] transfer/store plumbing, `wg peer`, and the agency
/// remote registry) so the gated cluster keeps one spelling for it.
pub use crate::agency::federation::{
    FederationConfig, PeerConfig, PeerServiceStatus, Remote, check_peer_service,
    load_federation_config, save_federation_config,
};
use crate::service::is_process_alive;

// ---------------------------------------------------------------------------
// Peer resolution: name → path → .wg dir → socket discovery
// ---------------------------------------------------------------------------

/// Result of resolving a peer reference to a concrete path.
#[derive(Debug, Clone)]
pub struct ResolvedPeer {
    /// The project root (parent of .wg/).
    pub project_path: PathBuf,
    /// The .wg directory.
    pub workgraph_dir: PathBuf,
}

/// Resolve a peer reference string to a concrete path.
///
/// Resolution order (per §2.3 of cross-repo design doc):
/// 1. Named peer in federation.yaml → look up `path`
/// 2. Absolute path or `~/` → filesystem path
/// 3. Relative path → resolve from CWD
pub fn resolve_peer(reference: &str, workgraph_dir: &Path) -> Result<ResolvedPeer, anyhow::Error> {
    let config = load_federation_config(workgraph_dir)?;

    // Check named peers first
    let raw_path = if let Some(peer) = config.peers.get(reference) {
        if peer.path.is_empty() {
            anyhow::bail!(
                "peer '{reference}' is key-based (wgid: {}) and has no filesystem path. \
                 Path resolution is for same-host federation; use wgid endpoint resolution \
                 (cross-graph messaging) for this peer.",
                peer.wgid.as_deref().unwrap_or("<none>")
            );
        }
        peer.path.clone()
    } else {
        reference.to_string()
    };

    // Expand ~/
    let expanded = if let Some(suffix) = raw_path.strip_prefix("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        home.join(suffix)
    } else {
        PathBuf::from(&raw_path)
    };

    // Make absolute
    let project_path = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };

    // Canonicalize if possible
    let project_path = project_path.canonicalize().unwrap_or(project_path);

    let wg_dir = project_path.join(".wg");
    if !wg_dir.is_dir() {
        anyhow::bail!(
            "No .wg directory found at '{}'. Is this a WG project?",
            project_path.display()
        );
    }

    Ok(ResolvedPeer {
        project_path,
        workgraph_dir: wg_dir,
    })
}

// ---------------------------------------------------------------------------
// Key-based endpoint resolution: the ADR-fed-001 §D5 cascade
// ---------------------------------------------------------------------------

/// Which rung of the resolution cascade produced an endpoint set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveSource {
    /// A cached signed endpoint record: a configured peer's `endpoints`, or the
    /// `endpoints` of a locally fetched + verified `IdentityRecord`.
    Cache,
    /// An optional directory hint (a configured directory node confirmed it hosts the
    /// identity). A forged directory can only hint, never override verification.
    Directory,
    /// DHT / Iroh discovery — **deferred past Wave 4** (the wire library is
    /// candidate-agnostic through Wave 4, ADR-fed-002 §OQ1). Reserved.
    Dht,
}

impl std::fmt::Display for ResolveSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ResolveSource::Cache => "cache",
            ResolveSource::Directory => "directory",
            ResolveSource::Dht => "dht",
        };
        f.write_str(s)
    }
}

/// A resolved key-based peer: the normalized `wgid:` and the delivery endpoint(s) to
/// try, in fallback-ladder order (ADR-fed-002 §D1/§D2).
#[derive(Debug, Clone)]
pub struct ResolvedEndpoint {
    pub wgid: String,
    pub endpoints: Vec<String>,
    pub source: ResolveSource,
}

/// Resolve a peer **reference** (a `federation.yaml` peer name, or a raw
/// `wgid:`/`did:key:` address) to delivery endpoints, via the fail-safe cascade
/// (ADR-fed-001 §D5): **cached signed endpoint record → optional directory hint →
/// DHT** — any one step suffices.
///
/// Verification is never delegated to this function: it only finds *where* to deliver
/// (a convenience). Whether a fetched bundle is authentic is always a separate local
/// signature check (`identity::sigchain::verify`), so a forged directory or peer entry
/// cannot impersonate — it can at most send you to the wrong (still self-verifying)
/// box.
pub fn resolve_peer_endpoint(
    reference: &str,
    workgraph_dir: &Path,
) -> Result<ResolvedEndpoint, anyhow::Error> {
    let config = load_federation_config(workgraph_dir)?;

    // Normalize the target wgid: either the reference *is* a wgid/did:key, or it is a
    // peer name whose entry carries a wgid.
    let (target_wgid, named_peer) = if is_wgid_like(reference) {
        (normalize_wgid(reference)?, None)
    } else if let Some(peer) = config.peers.get(reference) {
        let wgid = peer.wgid.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "peer '{reference}' has no wgid: — it is path-based only and cannot be \
                 addressed by key. Add `wgid:` + `endpoints:` to it in federation.yaml, \
                 or pass a wgid: directly."
            )
        })?;
        (normalize_wgid(&wgid)?, Some(peer.clone()))
    } else {
        anyhow::bail!(
            "'{reference}' is neither a known peer nor a wgid:/did:key: address. \
             Known peers: {}",
            if config.peers.is_empty() {
                "(none)".to_string()
            } else {
                config.peers.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        );
    };

    let mut tried: Vec<&str> = Vec::new();

    // ── Stage 1: cache (cached signed endpoint record) ─────────────────────────
    tried.push("cache");
    // 1a. Endpoints on the named peer entry.
    if let Some(peer) = &named_peer {
        let eps = http_endpoints(&peer.endpoints);
        if !eps.is_empty() {
            return Ok(ResolvedEndpoint {
                wgid: target_wgid,
                endpoints: eps,
                source: ResolveSource::Cache,
            });
        }
    }
    // 1b. Endpoints on a peer (by-name miss) whose wgid matches the target.
    for peer in config.peers.values() {
        if peer
            .wgid
            .as_deref()
            .and_then(|w| normalize_wgid(w).ok())
            .as_deref()
            == Some(target_wgid.as_str())
        {
            let eps = http_endpoints(&peer.endpoints);
            if !eps.is_empty() {
                return Ok(ResolvedEndpoint {
                    wgid: target_wgid,
                    endpoints: eps,
                    source: ResolveSource::Cache,
                });
            }
        }
    }
    // 1c. Endpoints carried in a locally fetched + verified IdentityRecord (the
    //     genuine "cached signed endpoint record" — these endpoints were signed).
    if let Some(eps) = cached_record_endpoints(workgraph_dir, &target_wgid) {
        if !eps.is_empty() {
            return Ok(ResolvedEndpoint {
                wgid: target_wgid,
                endpoints: eps,
                source: ResolveSource::Cache,
            });
        }
    }

    // ── Stage 2: optional directory hint ───────────────────────────────────────
    if let Some(directory) = &config.directory {
        tried.push("directory");
        if directory_hosts(directory, &target_wgid) {
            return Ok(ResolvedEndpoint {
                wgid: target_wgid,
                endpoints: vec![directory.trim_end_matches('/').to_string()],
                source: ResolveSource::Directory,
            });
        }
    }

    // ── Stage 3: DHT — deferred past Wave 4 (wire library not yet bound) ────────
    // Reserved for the Iroh/relay discovery leg (ADR-fed-002 §OQ1). Not built here.

    anyhow::bail!(
        "could not resolve delivery endpoints for {target_wgid} (cascade tried: {}). \
         Add `endpoints:` to the peer in federation.yaml, fetch+save its identity \
         (`wg identity fetch <wgid> --save <name>`), or configure a `directory:`.",
        tried.join(" → ")
    )
}

/// True if `s` looks like a key address (`wgid:` or `did:key:`).
fn is_wgid_like(s: &str) -> bool {
    s.starts_with("wgid:") || s.starts_with("did:key:")
}

/// Normalize a `wgid:`/`did:key:` spelling to the canonical `wgid:` the stores key by.
fn normalize_wgid(s: &str) -> Result<String, anyhow::Error> {
    let pubkey = crate::identity::keys::pubkey_from_wgid(s)?;
    Ok(crate::identity::keys::wgid_from_pubkey(&pubkey))
}

/// Keep only http(s) endpoints (the rung implemented in Wave 4), preserving order.
fn http_endpoints(eps: &[String]) -> Vec<String> {
    eps.iter()
        .filter(|e| e.starts_with("http://") || e.starts_with("https://"))
        .map(|e| e.trim_end_matches('/').to_string())
        .collect()
}

/// Look in `<workgraph_dir>/identity/*.json` for a saved bundle whose wgid matches,
/// and return the http(s) `endpoints` from its (signed) `IdentityRecord`.
fn cached_record_endpoints(workgraph_dir: &Path, target_wgid: &str) -> Option<Vec<String>> {
    let dir = workgraph_dir.join("identity");
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let wgid_matches = value
            .get("wgid")
            .and_then(|w| w.as_str())
            .and_then(|w| normalize_wgid(w).ok())
            .map(|w| w == target_wgid)
            .unwrap_or(false);
        if !wgid_matches {
            continue;
        }
        let uris: Vec<String> = value
            .get("record")
            .and_then(|r| r.get("endpoints"))
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|ep| ep.get("uri").and_then(|u| u.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let eps = http_endpoints(&uris);
        if !eps.is_empty() {
            return Some(eps);
        }
    }
    None
}

/// Best-effort probe: does the configured directory node host a head for this wgid?
fn directory_hosts(directory: &str, wgid: &str) -> bool {
    let base = directory.trim_end_matches('/');
    let url = format!(
        "{base}{}/heads/{}",
        crate::identity::transport::API_PREFIX,
        crate::identity::transport::sanitize(wgid)
    );
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()
        .and_then(|c| c.get(&url).send().ok())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Cross-repo dependency resolution
// ---------------------------------------------------------------------------

/// Parse a remote reference string like "peer:task-id" into (peer_name, task_id).
///
/// Defined in the unconditional [`crate::agency::federation`] module (the pure
/// string parse is needed by ungated dep-id validation / rendering) and re-exported
/// here so the gated cluster keeps one spelling for it.
pub use crate::agency::federation::parse_remote_ref;

/// The status of a remote task, resolved via IPC or direct file access.
#[derive(Debug, Clone)]
pub struct RemoteTaskStatus {
    pub task_id: String,
    pub status: crate::graph::Status,
    pub title: Option<String>,
    pub assigned: Option<String>,
    /// How the status was resolved
    pub resolution: RemoteResolution,
}

/// How the remote task status was resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteResolution {
    /// Resolved via IPC to a running peer service
    Ipc,
    /// Resolved by directly reading the peer's graph.jsonl
    DirectFileAccess,
    /// Could not resolve — peer not found or inaccessible
    Unreachable(String),
}

/// Resolve the status of a task in a remote peer WG project.
///
/// Resolution order (per §4.4 of cross-repo design doc):
/// 1. Look up peer path from federation config or parse as path
/// 2. Check if peer service is running
/// 3. If running: query via IPC
/// 4. If not running: load peer's graph.jsonl directly
/// 5. If peer not found: return error
pub fn resolve_remote_task_status(
    peer_name: &str,
    task_id: &str,
    local_workgraph_dir: &Path,
) -> RemoteTaskStatus {
    // Try to resolve the peer
    let resolved = match resolve_peer(peer_name, local_workgraph_dir) {
        Ok(r) => r,
        Err(e) => {
            return RemoteTaskStatus {
                task_id: task_id.to_string(),
                status: crate::graph::Status::Open, // treat as blocking
                title: None,
                assigned: None,
                resolution: RemoteResolution::Unreachable(format!(
                    "Cannot resolve peer '{}': {}",
                    peer_name, e
                )),
            };
        }
    };

    // Check if the peer's service is running
    let service_status = check_peer_service(&resolved.workgraph_dir);

    if service_status.running
        && let Some(socket_path) = &service_status.socket_path
    {
        // Try IPC first
        match query_task_via_ipc(socket_path, task_id) {
            Ok(status) => return status,
            Err(_) => {
                // Fall through to direct file access
            }
        }
    }

    // Fall back to direct graph file read
    let graph_path = resolved.workgraph_dir.join("graph.jsonl");
    if !graph_path.exists() {
        return RemoteTaskStatus {
            task_id: task_id.to_string(),
            status: crate::graph::Status::Open,
            title: None,
            assigned: None,
            resolution: RemoteResolution::Unreachable(format!(
                "No graph.jsonl at peer '{}'",
                peer_name
            )),
        };
    }

    match crate::parser::load_graph(&graph_path) {
        Ok(graph) => match graph.get_task(task_id) {
            Some(task) => RemoteTaskStatus {
                task_id: task.id.clone(),
                status: task.status,
                title: Some(task.title.clone()),
                assigned: task.assigned.clone(),
                resolution: RemoteResolution::DirectFileAccess,
            },
            None => RemoteTaskStatus {
                task_id: task_id.to_string(),
                status: crate::graph::Status::Open,
                title: None,
                assigned: None,
                resolution: RemoteResolution::Unreachable(format!(
                    "Task '{}' not found in peer '{}'",
                    task_id, peer_name
                )),
            },
        },
        Err(e) => RemoteTaskStatus {
            task_id: task_id.to_string(),
            status: crate::graph::Status::Open,
            title: None,
            assigned: None,
            resolution: RemoteResolution::Unreachable(format!(
                "Failed to load peer '{}' graph: {}",
                peer_name, e
            )),
        },
    }
}

/// Query a task's status via IPC to a running peer service.
#[cfg(unix)]
fn query_task_via_ipc(socket_path: &str, task_id: &str) -> Result<RemoteTaskStatus, anyhow::Error> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let request = serde_json::json!({
        "QueryTask": { "task_id": task_id }
    });
    writeln!(stream, "{}", request)?;
    stream.flush()?;

    let reader = BufReader::new(&stream);
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let resp: serde_json::Value = serde_json::from_str(&line)?;
        if resp.get("ok") == Some(&serde_json::Value::Bool(true)) {
            let status_str = resp
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("Open");
            let status = parse_status_string(status_str);
            return Ok(RemoteTaskStatus {
                task_id: task_id.to_string(),
                status,
                title: resp.get("title").and_then(|v| v.as_str()).map(String::from),
                assigned: resp
                    .get("assigned")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                resolution: RemoteResolution::Ipc,
            });
        } else {
            let err_msg = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("IPC error: {}", err_msg);
        }
    }
    anyhow::bail!("No response from peer service")
}

#[cfg(not(unix))]
fn query_task_via_ipc(
    _socket_path: &str,
    _task_id: &str,
) -> Result<RemoteTaskStatus, anyhow::Error> {
    anyhow::bail!("IPC is only supported on Unix systems")
}

/// Parse a status string (from IPC response) into a Status enum.
fn parse_status_string(s: &str) -> crate::graph::Status {
    match s.to_lowercase().as_str() {
        "done" => crate::graph::Status::Done,
        "open" => crate::graph::Status::Open,
        "inprogress" | "in-progress" => crate::graph::Status::InProgress,
        "failed" => crate::graph::Status::Failed,
        "abandoned" => crate::graph::Status::Abandoned,
        "blocked" => crate::graph::Status::Blocked,
        _ => crate::graph::Status::Open,
    }
}

/// Ensure the target store directory structure exists.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Cross-repo dependency tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_remote_ref_valid() {
        let result = parse_remote_ref("workgraph:implement-trace");
        assert_eq!(result, Some(("workgraph", "implement-trace")));
    }

    #[test]
    fn parse_remote_ref_multiple_colons() {
        // Only splits on first colon
        let result = parse_remote_ref("peer:task:with:colons");
        assert_eq!(result, Some(("peer", "task:with:colons")));
    }

    #[test]
    fn parse_remote_ref_no_colon() {
        assert_eq!(parse_remote_ref("local-task-id"), None);
    }

    #[test]
    fn parse_remote_ref_empty_peer() {
        assert_eq!(parse_remote_ref(":task-id"), None);
    }

    #[test]
    fn parse_remote_ref_empty_task() {
        assert_eq!(parse_remote_ref("peer:"), None);
    }

    #[test]
    fn parse_remote_ref_empty_string() {
        assert_eq!(parse_remote_ref(""), None);
    }

    #[test]
    fn parse_status_string_all_variants() {
        assert_eq!(parse_status_string("Done"), crate::graph::Status::Done);
        assert_eq!(parse_status_string("done"), crate::graph::Status::Done);
        assert_eq!(parse_status_string("Open"), crate::graph::Status::Open);
        assert_eq!(
            parse_status_string("InProgress"),
            crate::graph::Status::InProgress
        );
        assert_eq!(
            parse_status_string("in-progress"),
            crate::graph::Status::InProgress
        );
        assert_eq!(parse_status_string("Failed"), crate::graph::Status::Failed);
        assert_eq!(
            parse_status_string("Abandoned"),
            crate::graph::Status::Abandoned
        );
        assert_eq!(
            parse_status_string("Blocked"),
            crate::graph::Status::Blocked
        );
        // Unknown defaults to Open
        assert_eq!(parse_status_string("bogus"), crate::graph::Status::Open);
    }

    #[test]
    fn resolve_remote_task_status_via_direct_file_access() {
        let tmp = TempDir::new().unwrap();

        // Set up local WG project with federation config pointing to a peer
        let local_wg = tmp.path().join("local").join(".wg");
        std::fs::create_dir_all(&local_wg).unwrap();

        // Set up peer WG project with a task
        let peer_project = tmp.path().join("peer-project");
        let peer_wg = peer_project.join(".wg");
        std::fs::create_dir_all(&peer_wg).unwrap();

        // Create a task in the peer's graph
        let mut peer_graph = crate::graph::WorkGraph::new();
        let mut task = crate::graph::Task::default();
        task.id = "remote-task".to_string();
        task.title = "A remote task".to_string();
        task.status = crate::graph::Status::Done;
        peer_graph.add_node(crate::graph::Node::Task(task));
        crate::parser::save_graph(&peer_graph, peer_wg.join("graph.jsonl")).unwrap();

        // Configure federation with the peer
        let config = FederationConfig {
            remotes: BTreeMap::new(),
            peers: {
                let mut m = BTreeMap::new();
                m.insert(
                    "mypeer".to_string(),
                    PeerConfig {
                        path: peer_project.to_str().unwrap().to_string(),
                        description: None,
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        save_federation_config(&local_wg, &config).unwrap();

        // Resolve the remote task
        let result = resolve_remote_task_status("mypeer", "remote-task", &local_wg);
        assert_eq!(result.status, crate::graph::Status::Done);
        assert_eq!(result.title.as_deref(), Some("A remote task"));
        assert_eq!(result.resolution, RemoteResolution::DirectFileAccess);
    }

    #[test]
    fn resolve_remote_task_status_not_found() {
        let tmp = TempDir::new().unwrap();
        let local_wg = tmp.path().join("local").join(".wg");
        std::fs::create_dir_all(&local_wg).unwrap();

        // Set up peer with empty graph
        let peer_project = tmp.path().join("peer-project");
        let peer_wg = peer_project.join(".wg");
        std::fs::create_dir_all(&peer_wg).unwrap();
        let peer_graph = crate::graph::WorkGraph::new();
        crate::parser::save_graph(&peer_graph, peer_wg.join("graph.jsonl")).unwrap();

        let config = FederationConfig {
            remotes: BTreeMap::new(),
            peers: {
                let mut m = BTreeMap::new();
                m.insert(
                    "mypeer".to_string(),
                    PeerConfig {
                        path: peer_project.to_str().unwrap().to_string(),
                        description: None,
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        save_federation_config(&local_wg, &config).unwrap();

        let result = resolve_remote_task_status("mypeer", "nonexistent", &local_wg);
        // Task not found → treated as open (blocking)
        assert_eq!(result.status, crate::graph::Status::Open);
        assert!(
            matches!(result.resolution, RemoteResolution::Unreachable(_)),
            "expected Unreachable, got {:?}",
            result.resolution
        );
    }

    #[test]
    fn resolve_remote_task_status_unknown_peer() {
        let tmp = TempDir::new().unwrap();
        let local_wg = tmp.path().join("local").join(".wg");
        std::fs::create_dir_all(&local_wg).unwrap();

        // No federation config → empty peers
        let result = resolve_remote_task_status("unknown-peer", "some-task", &local_wg);
        assert_eq!(result.status, crate::graph::Status::Open);
        assert!(matches!(
            result.resolution,
            RemoteResolution::Unreachable(_)
        ));
    }
}

#[cfg(test)]
mod endpoint_resolution_tests {
    //! Wave-4 key-based resolution cascade (ADR-fed-001 §D5): cached endpoint record
    //! → directory → DHT, with path-based peers continuing to resolve alongside.
    use super::*;
    use crate::identity::keys::{gen_ed25519, wgid_from_pubkey};
    use tempfile::TempDir;

    fn fresh_wgid() -> String {
        let kp = gen_ed25519().unwrap();
        wgid_from_pubkey(&kp.public)
    }

    fn wg_dir(tmp: &TempDir) -> std::path::PathBuf {
        let d = tmp.path().join(".wg");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cache_hit_resolves_named_key_peer_endpoints() {
        let tmp = TempDir::new().unwrap();
        let dir = wg_dir(&tmp);
        let wgid = fresh_wgid();
        let mut cfg = FederationConfig::default();
        cfg.peers.insert(
            "bob".to_string(),
            PeerConfig {
                wgid: Some(wgid.clone()),
                endpoints: vec!["http://127.0.0.1:9001".into()],
                ..Default::default()
            },
        );
        save_federation_config(&dir, &cfg).unwrap();

        let resolved = resolve_peer_endpoint("bob", &dir).unwrap();
        assert_eq!(resolved.source, ResolveSource::Cache);
        assert_eq!(resolved.wgid, wgid);
        assert_eq!(
            resolved.endpoints,
            vec!["http://127.0.0.1:9001".to_string()]
        );
    }

    #[test]
    fn cache_hit_resolves_by_raw_wgid_across_peers() {
        let tmp = TempDir::new().unwrap();
        let dir = wg_dir(&tmp);
        let wgid = fresh_wgid();
        let mut cfg = FederationConfig::default();
        cfg.peers.insert(
            "named-differently".to_string(),
            PeerConfig {
                wgid: Some(wgid.clone()),
                endpoints: vec!["https://node.example/".into()],
                ..Default::default()
            },
        );
        save_federation_config(&dir, &cfg).unwrap();

        // Address by the raw wgid, not the peer name.
        let resolved = resolve_peer_endpoint(&wgid, &dir).unwrap();
        assert_eq!(resolved.source, ResolveSource::Cache);
        // Trailing slash trimmed.
        assert_eq!(resolved.endpoints, vec!["https://node.example".to_string()]);
    }

    #[test]
    fn path_based_peers_still_resolve_alongside_key_based() {
        // FR-F6: path-based federation keeps working when key-based peers coexist.
        let tmp = TempDir::new().unwrap();
        let dir = wg_dir(&tmp);
        // A real path-based peer project.
        let peer_root = tmp.path().join("peer-proj");
        std::fs::create_dir_all(peer_root.join(".wg")).unwrap();

        let mut cfg = FederationConfig::default();
        cfg.peers.insert(
            "legacy".to_string(),
            PeerConfig {
                path: peer_root.to_str().unwrap().to_string(),
                ..Default::default()
            },
        );
        cfg.peers.insert(
            "modern".to_string(),
            PeerConfig {
                wgid: Some(fresh_wgid()),
                endpoints: vec!["http://127.0.0.1:9100".into()],
                ..Default::default()
            },
        );
        save_federation_config(&dir, &cfg).unwrap();

        // The path-based peer still resolves by path (unchanged behavior).
        let legacy = resolve_peer("legacy", &dir).unwrap();
        assert_eq!(
            legacy.project_path.canonicalize().unwrap(),
            peer_root.canonicalize().unwrap()
        );
        // And the key-based peer resolves by endpoint.
        let modern = resolve_peer_endpoint("modern", &dir).unwrap();
        assert_eq!(modern.source, ResolveSource::Cache);

        // Asking for endpoints on a path-only peer is a clear error (not a panic).
        assert!(resolve_peer_endpoint("legacy", &dir).is_err());
        // Asking for a path on a key-only peer is a clear error too.
        assert!(resolve_peer("modern", &dir).is_err());
    }

    #[test]
    fn unresolvable_reference_reports_cascade_tried() {
        let tmp = TempDir::new().unwrap();
        let dir = wg_dir(&tmp);
        save_federation_config(&dir, &FederationConfig::default()).unwrap();
        // A valid wgid with no configured endpoints / directory → cascade exhausts.
        let wgid = fresh_wgid();
        let err = resolve_peer_endpoint(&wgid, &dir).unwrap_err().to_string();
        assert!(
            err.contains("cache"),
            "error should name the cascade: {err}"
        );
    }
}
