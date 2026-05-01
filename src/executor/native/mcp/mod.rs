//! MCP (Model Context Protocol) client support for WGNEX.
//!
//! Lets WGNEX talk to the ecosystem of MCP servers (filesystem,
//! github, sentry, linear, sequential-thinking, browser, fetch,
//! sqlite, memory, …) without each integration needing a hand-rolled
//! Rust wrapper. Servers declared in `.wg/config.toml` are
//! spawned at agent-init time, their tools discovered via
//! `tools/list`, and each one surfaced into the `ToolRegistry` as an
//! `McpTool` namespaced `<server>__<tool>`. From the agent's
//! perspective, MCP tools are indistinguishable from native ones.
//!
//! Scope of this module (v1):
//! - stdio transport only (SSE / WebSocket deferred)
//! - `initialize`, `tools/list`, `tools/call` (resources + prompts
//!   are a follow-up)
//! - Supervised lifecycle with bounded crash restart
//! - JSON Schema → Anthropic `ToolDefinition` translation
//!
//! Rollback knob: `AgentLoop::with_mcp(None)` or `wg nex --no-mcp`
//! disables MCP entirely.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub mod client;
pub mod manager;
pub mod schema;
pub mod supervisor;
pub mod transport;

pub use client::McpClient;
pub use manager::{McpManager, McpServerConfig};

use super::client::ToolDefinition;
use super::tools::{Tool, ToolOutput};

/// A tool surfaced from an MCP server, adapted to the `Tool` trait
/// so it can sit in the same `ToolRegistry` as native tools.
pub struct McpTool {
    /// Namespaced public name, e.g. `filesystem__read_file`.
    namespaced_name: String,
    /// The original (server-local) name, e.g. `read_file`.
    server_local_name: String,
    description: String,
    input_schema: Value,
    /// Client for the server that owns this tool.
    client: Arc<McpClient>,
    /// Server alias for error messages.
    server_name: String,
}

impl McpTool {
    pub fn new(
        server_name: String,
        server_local_name: String,
        description: String,
        input_schema: Value,
        client: Arc<McpClient>,
    ) -> Self {
        let namespaced_name = format!("{}__{}", server_name, server_local_name);
        Self {
            namespaced_name,
            server_local_name,
            description,
            input_schema,
            client,
            server_name,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.namespaced_name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    async fn execute(&self, input: &Value) -> ToolOutput {
        match self.client.call_tool(&self.server_local_name, input).await {
            Ok(content) => ToolOutput::success(content),
            Err(e) => ToolOutput::error(format!(
                "MCP tool {}/{} failed: {}",
                self.server_name, self.server_local_name, e
            )),
        }
    }

    /// MCP can't tell us whether a tool is read-only. Conservatively
    /// mark as mutating so `filter_read_only` drops them — users
    /// opting into MCP explicitly accept the surface.
    fn is_read_only(&self) -> bool {
        false
    }
}

/// Convenience: create an MCP manager from config and a shutdown
/// signal. Returns None if no servers are enabled — callers should
/// treat that as "MCP not in use."
pub async fn init_from_config(servers: &[McpServerConfig]) -> Result<Option<Arc<McpManager>>> {
    let enabled: Vec<_> = servers.iter().filter(|s| s.enabled).cloned().collect();
    if enabled.is_empty() {
        return Ok(None);
    }
    let manager = McpManager::start(enabled).await?;
    Ok(Some(Arc::new(manager)))
}
