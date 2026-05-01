//! Manager: spawns all configured MCP servers at agent-init time,
//! discovers their tools, and returns a list of `McpTool` instances
//! ready to drop into a `ToolRegistry`.
//!
//! The manager owns the live client handles; dropping it shuts
//! down all servers (via `kill_on_drop` on the child processes).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::McpTool;
use super::client::McpClient;
use super::supervisor::connect_initial;

/// Per-server config entry, mirrored in `.wg/config.toml`:
///
/// ```toml
/// [[mcp.servers]]
/// name = "filesystem"
/// command = "npx"
/// args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
/// enabled = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Holds all live MCP server clients for this agent session.
pub struct McpManager {
    clients: HashMap<String, Arc<McpClient>>,
}

impl McpManager {
    /// Spawn every enabled server in `configs`, run the MCP
    /// handshake, and keep the clients alive.
    ///
    /// Per-server failures are logged and skipped — a broken
    /// server doesn't wedge the whole agent. The session proceeds
    /// with the servers that did come up.
    pub async fn start(configs: Vec<McpServerConfig>) -> Result<Self> {
        let mut clients = HashMap::new();
        for cfg in configs {
            if !cfg.enabled {
                continue;
            }
            match connect_initial(&cfg).await {
                Ok(client) => {
                    eprintln!("\x1b[2m[mcp] server {:?} ready\x1b[0m", cfg.name);
                    clients.insert(cfg.name.clone(), client);
                }
                Err(e) => {
                    eprintln!(
                        "\x1b[33m[mcp] server {:?} failed to start: {} — skipping\x1b[0m",
                        cfg.name, e
                    );
                }
            }
        }
        Ok(Self { clients })
    }

    /// Discover tools from every live server and wrap each one as
    /// an `McpTool` ready to register.
    pub async fn discover_tools(&self) -> Vec<McpTool> {
        let mut out = Vec::new();
        for (name, client) in &self.clients {
            match client.list_tools().await {
                Ok(tools) => {
                    for t in tools {
                        out.push(McpTool::new(
                            name.clone(),
                            t.name,
                            t.description,
                            super::schema::sanitize_input_schema(t.input_schema),
                            client.clone(),
                        ));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "\x1b[33m[mcp] tools/list failed on {:?}: {}\x1b[0m",
                        name, e
                    );
                }
            }
        }
        out
    }

    /// Number of servers that successfully came up.
    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Names of all live servers, for display / debugging.
    pub fn server_names(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }
}

/// Convenience: build a manager and discover all its tools in one
/// call. Returns `(manager, tools)` so the caller can hold the
/// manager for lifetime and register the tools into their registry.
pub async fn start_and_discover(
    configs: Vec<McpServerConfig>,
) -> Result<(Arc<McpManager>, Vec<McpTool>)> {
    let manager = McpManager::start(configs)
        .await
        .context("start MCP manager")?;
    let tools = manager.discover_tools().await;
    Ok((Arc::new(manager), tools))
}
