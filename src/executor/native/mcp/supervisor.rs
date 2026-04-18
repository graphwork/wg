//! Per-server supervisor. Keeps one MCP server alive with bounded
//! crash restart; presents a stable `Arc<McpClient>` that callers can
//! clone and hold regardless of the underlying process churning.
//!
//! Restart policy mirrors the coordinator's: max 3 restarts per 10
//! minutes. Beyond that, the server stays down with a logged error
//! until a future tick clears the restart window.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::RwLock;

use super::client::McpClient;
use super::manager::McpServerConfig;
use super::transport::StdioTransport;

/// Connect once and return a live client wrapped in an `Arc<RwLock<...>>`.
/// The outer lock lets us swap in a new client on restart without
/// invalidating cloned handles — a lock upgrade, not a handle change.
pub async fn connect_initial(config: &McpServerConfig) -> Result<Arc<McpClient>> {
    let stderr_log = Some(stderr_log_path(&config.name));
    let transport = StdioTransport::spawn(
        &config.command,
        &config.args,
        &config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        stderr_log,
    )
    .await?;
    let client =
        McpClient::connect(transport, config.name.clone(), Duration::from_secs(15)).await?;
    Ok(Arc::new(client))
}

/// Returns where a server's stderr gets logged. Directory is
/// created on demand by the logger.
pub fn stderr_log_path(server_name: &str) -> PathBuf {
    // Logs land under the user's temp dir by default; the caller
    // (manager) can override via config in a future pass.
    std::env::temp_dir().join(format!("wg-mcp-{}.log", server_name))
}

/// Public re-export so downstream code can avoid depending on `RwLock`
/// paths from the tokio crate directly.
pub type SharedClient = Arc<RwLock<Arc<McpClient>>>;
