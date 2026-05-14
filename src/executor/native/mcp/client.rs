//! JSON-RPC 2.0 client for a single MCP server.
//!
//! Holds the stdio transport and a pending-request map. A background
//! task reads responses off the transport and routes them to the
//! oneshot channel that's waiting for that request id. Supports
//! concurrent in-flight requests from multiple agent calls.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

use super::transport::StdioTransport;

/// Protocol version we negotiate. Bump as MCP evolves; most servers
/// accept 2024-11-05 and older.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Default per-request timeout. MCP tool calls can be long (web
/// fetches, sequential-thinking runs); 120s gives them room.
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// One discovered tool, as returned by the server's `tools/list`.
#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// An MCP client bound to one server.
pub struct McpClient {
    /// Write side of the transport. Wrapped in a mutex so many
    /// callers can share one client without interleaving their
    /// writes mid-line.
    send_half: Mutex<StdioTransportWriter>,
    /// Next request id to allocate.
    next_id: AtomicU64,
    /// Pending requests waiting for their matching response.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    /// Cached tool list from the last `tools/list` call. Refreshed
    /// on demand.
    server_name: String,
}

/// Holds just the transport's write side + incoming channel so the
/// background reader task can own the transport body.
struct StdioTransportWriter {
    stdin: tokio::process::ChildStdin,
}

impl StdioTransportWriter {
    async fn send_line(&mut self, json: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

impl McpClient {
    /// Spawn a reader task over the transport, perform the
    /// `initialize` + `notifications/initialized` handshake, and
    /// return the live client. Errors if the handshake fails or
    /// the server doesn't respond within `handshake_timeout`.
    pub async fn connect(
        transport: StdioTransport,
        server_name: String,
        handshake_timeout: Duration,
    ) -> Result<Self> {
        let StdioTransport {
            child: _,
            stdin,
            mut incoming,
            _stderr_task,
            _stdout_task,
        } = transport;
        let writer = StdioTransportWriter { stdin };
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Background reader: parse each line, dispatch responses by
        // id. Notifications (no id) are currently ignored; a future
        // iteration could surface server-initiated notifications.
        let pending_reader = pending.clone();
        let server_name_for_reader = server_name.clone();
        tokio::spawn(async move {
            while let Some(line) = incoming.recv().await {
                if line.trim().is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "[mcp:{}] malformed line from server: {} ({})",
                            server_name_for_reader, e, line
                        );
                        continue;
                    }
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    let mut p = pending_reader.lock().await;
                    if let Some(tx) = p.remove(&id) {
                        let _ = tx.send(v);
                    }
                }
                // Notifications (no id) silently dropped for now.
            }
        });

        let client = Self {
            send_half: Mutex::new(writer),
            next_id: AtomicU64::new(1),
            pending,
            server_name,
        };

        // Handshake.
        tokio::time::timeout(handshake_timeout, client.initialize())
            .await
            .context("MCP initialize timed out")??;
        client
            .send_notification("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<Value> {
        self.call_raw(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "WG",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
            Duration::from_secs(30),
        )
        .await
    }

    /// Send a JSON-RPC request and await the matching response.
    pub async fn call_raw(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.pending.lock().await;
            p.insert(id, tx);
        }
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&request).context("serialize MCP request")?;
        {
            let mut w = self.send_half.lock().await;
            w.send_line(&body).await.context("send MCP request line")?;
        }
        let response = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| anyhow!("MCP call {} timed out after {:?}", method, timeout))?
            .map_err(|_| anyhow!("MCP call {} dropped before response", method))?;
        if let Some(err) = response.get("error") {
            bail!("MCP error on {}: {}", method, err);
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("MCP response missing `result` on {}", method))
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let notif = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&notif)?;
        let mut w = self.send_half.lock().await;
        w.send_line(&body).await
    }

    /// Enumerate tools the server exposes. Schema: `{ tools: [...] }`.
    pub async fn list_tools(&self) -> Result<Vec<DiscoveredTool>> {
        let result = self
            .call_raw("tools/list", json!({}), Duration::from_secs(30))
            .await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("tools/list result missing `tools` array"))?;
        let mut out = Vec::with_capacity(tools.len());
        for t in tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("tool entry missing `name`"))?
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            out.push(DiscoveredTool {
                name,
                description,
                input_schema,
            });
        }
        Ok(out)
    }

    /// Invoke a tool. Returns the concatenated text content the
    /// server emitted, or an error if the call failed or the server
    /// flagged `isError: true`.
    pub async fn call_tool(&self, name: &str, arguments: &Value) -> Result<String> {
        let result = self
            .call_raw(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                }),
                DEFAULT_CALL_TIMEOUT,
            )
            .await?;
        // Server may flag an error via `isError: true`.
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let text = extract_content_text(&result);
            bail!("tool call returned error: {}", text);
        }
        Ok(extract_content_text(&result))
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

/// Flatten the MCP `content` array (a list of `{type, text}` /
/// `{type, image, ...}` blocks) into a single text string. Non-text
/// blocks are surfaced as a marker so the agent sees something
/// non-empty.
fn extract_content_text(result: &Value) -> String {
    let Some(blocks) = result.get("content").and_then(|c| c.as_array()) else {
        return serde_json::to_string_pretty(result).unwrap_or_default();
    };
    let mut parts = Vec::new();
    for b in blocks {
        let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "text" => {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    parts.push(t.to_string());
                }
            }
            "image" => {
                parts.push("[image content omitted]".to_string());
            }
            "resource" => {
                let uri = b
                    .get("resource")
                    .and_then(|r| r.get("uri"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                parts.push(format!("[resource: {}]", uri));
            }
            _ => {
                parts.push(format!("[unknown content type: {}]", ty));
            }
        }
    }
    parts.join("\n")
}
